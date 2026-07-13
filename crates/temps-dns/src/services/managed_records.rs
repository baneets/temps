//! Ownership-guarded DNS record management (ADR-031)
//!
//! [`ManagedDnsRecordService`] is the ONLY path other crates should use to
//! create public A/AAAA/CNAME records in user zones. Unlike the raw
//! [`crate::services::DnsRecordService`] (which upserts blindly and is kept
//! for ACME challenge TXT records that temps unambiguously owns), every write
//! here is guarded by the ownership scheme from [`crate::ownership`]:
//!
//! - **Create/update** refuses if a record with the target name/type exists
//!   without a temps ownership marker, or with a marker from a different
//!   temps install. Conflicts surface as typed [`DnsError::RecordConflict`] /
//!   [`DnsError::NotOwnedByInstance`] so the UI can offer import-or-skip.
//! - **Delete** only removes records this install owns.
//! - **Import** is the explicit, user-confirmed adoption path that stamps a
//!   marker onto a pre-existing record.
//!
//! Proxied (Cloudflare orange-cloud) writes additionally pass the Universal
//! SSL depth guardrail — see [`crate::ownership::check_proxied_depth`].
//!
//! Write ordering: the ownership marker TXT is written BEFORE the target
//! record. A crash between the two leaves a harmless orphan marker, never a
//! live unmarked record that a later run would refuse to manage.

use std::sync::Arc;

use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use temps_entities::dns_instance_identity;
use tracing::{info, warn};

use crate::errors::DnsError;
use crate::ownership::{check_proxied_depth, registry_record_name, OwnershipMarker};
use crate::providers::{DnsProvider, DnsRecord, DnsRecordContent, DnsRecordRequest, DnsRecordType};
use crate::services::provider_service::DnsProviderService;

/// What a managed record was created for; stamped into the ownership marker
/// so the provider-side registry shows which project/environment a record
/// belongs to.
#[derive(Debug, Clone, Copy, Default)]
pub struct OwnershipScope {
    pub project_id: Option<i32>,
    pub environment_id: Option<i32>,
}

/// Ownership state of a record at the provider, for the domain UI's
/// per-record status (created / conflict / unmanaged).
#[derive(Debug, Clone)]
pub enum RecordOwnership {
    /// No record with this name/type exists.
    NotFound,
    /// Record exists but carries no temps ownership marker — temps will not
    /// touch it unless the user imports it.
    Unmanaged(DnsRecord),
    /// Record exists and is owned by this temps install.
    Owned(DnsRecord, OwnershipMarker),
    /// Record exists and is owned by a DIFFERENT temps install.
    OwnedByOther(DnsRecord, OwnershipMarker),
}

/// Ownership-guarded record management on top of [`DnsProviderService`].
pub struct ManagedDnsRecordService {
    db: Arc<DatabaseConnection>,
    provider_service: Arc<DnsProviderService>,
    instance_id: tokio::sync::OnceCell<String>,
}

impl ManagedDnsRecordService {
    pub fn new(db: Arc<DatabaseConnection>, provider_service: Arc<DnsProviderService>) -> Self {
        Self {
            db,
            provider_service,
            instance_id: tokio::sync::OnceCell::new(),
        }
    }

    /// Get (or create on first use) this install's ownership instance ID.
    ///
    /// The ID never rotates once created — rotating would orphan every record
    /// this install previously stamped.
    pub async fn instance_id(&self) -> Result<String, DnsError> {
        let id = self
            .instance_id
            .get_or_try_init(|| async {
                if let Some(row) = dns_instance_identity::Entity::find()
                    .one(self.db.as_ref())
                    .await?
                {
                    return Ok::<String, DnsError>(row.instance_id);
                }

                let fresh = uuid::Uuid::new_v4().to_string();
                let row = dns_instance_identity::ActiveModel {
                    id: Set(1),
                    instance_id: Set(fresh),
                    ..Default::default()
                };
                // Two concurrent first writes can race on the single-row PK;
                // whoever loses re-reads the winner's ID instead of failing.
                match row.insert(self.db.as_ref()).await {
                    Ok(created) => Ok(created.instance_id),
                    Err(insert_err) => dns_instance_identity::Entity::find()
                        .one(self.db.as_ref())
                        .await?
                        .map(|row| row.instance_id)
                        .ok_or(DnsError::Database(insert_err)),
                }
            })
            .await?;
        Ok(id.clone())
    }

    /// Create or update a managed record, enforcing ownership and proxy
    /// guardrails. `domain` may be any FQDN under a managed zone; the record
    /// `request.name` is relative to that zone.
    pub async fn set_managed_record(
        &self,
        domain: &str,
        request: DnsRecordRequest,
        scope: OwnershipScope,
    ) -> Result<DnsRecord, DnsError> {
        let (provider_model, managed) = self
            .provider_service
            .find_provider_for_domain(domain)
            .await?
            .ok_or_else(|| DnsError::DomainNotManaged(domain.to_string()))?;
        let provider = self
            .provider_service
            .create_provider_instance(&provider_model)?;
        let zone = managed.domain.as_str();

        if request.proxied {
            if !provider.capabilities().proxy {
                return Err(DnsError::ProxyNotSupportedByProvider {
                    provider: provider_model.name.clone(),
                });
            }
            check_proxied_depth(zone, &request.name)?;
        }

        let instance = self.instance_id().await?;
        let marker = OwnershipMarker::new(&instance, scope.project_id, scope.environment_id);

        let record =
            Self::guarded_set(provider.as_ref(), zone, request, &marker, &instance).await?;
        info!(
            "Set managed {} record '{}' in zone {} via provider {} (proxied: {})",
            record.content.record_type(),
            record.name,
            zone,
            provider_model.name,
            record.proxied
        );
        Ok(record)
    }

    /// Delete a managed record. Refuses unless this install owns it.
    pub async fn remove_managed_record(
        &self,
        domain: &str,
        name: &str,
        record_type: DnsRecordType,
    ) -> Result<(), DnsError> {
        let (provider_model, managed) = self
            .provider_service
            .find_provider_for_domain(domain)
            .await?
            .ok_or_else(|| DnsError::DomainNotManaged(domain.to_string()))?;
        let provider = self
            .provider_service
            .create_provider_instance(&provider_model)?;
        let zone = managed.domain.as_str();

        let instance = self.instance_id().await?;
        Self::guarded_remove(provider.as_ref(), zone, name, record_type, &instance).await?;
        info!(
            "Removed managed {} record '{}' in zone {} via provider {}",
            record_type, name, zone, provider_model.name
        );
        Ok(())
    }

    /// Explicitly adopt a pre-existing record into temps management by
    /// stamping an ownership marker onto it. This is the user-confirmed
    /// "import" arm of the conflict flow — never called automatically.
    pub async fn import_record(
        &self,
        domain: &str,
        name: &str,
        record_type: DnsRecordType,
        scope: OwnershipScope,
    ) -> Result<OwnershipMarker, DnsError> {
        let (provider_model, managed) = self
            .provider_service
            .find_provider_for_domain(domain)
            .await?
            .ok_or_else(|| DnsError::DomainNotManaged(domain.to_string()))?;
        let provider = self
            .provider_service
            .create_provider_instance(&provider_model)?;
        let zone = managed.domain.as_str();

        let instance = self.instance_id().await?;
        let marker =
            Self::guarded_import(provider.as_ref(), zone, name, record_type, &instance, scope)
                .await?;
        info!(
            "Imported {} record '{}' in zone {} into temps management",
            record_type, name, zone
        );
        Ok(marker)
    }

    /// Ownership state of a record, for the domain UI.
    pub async fn record_ownership(
        &self,
        domain: &str,
        name: &str,
        record_type: DnsRecordType,
    ) -> Result<RecordOwnership, DnsError> {
        let (provider_model, managed) = self
            .provider_service
            .find_provider_for_domain(domain)
            .await?
            .ok_or_else(|| DnsError::DomainNotManaged(domain.to_string()))?;
        let provider = self
            .provider_service
            .create_provider_instance(&provider_model)?;
        let instance = self.instance_id().await?;

        Self::ownership_of(
            provider.as_ref(),
            &managed.domain,
            name,
            record_type,
            &instance,
        )
        .await
    }

    // ------------------------------------------------------------------
    // Guarded core — associated functions over `&dyn DnsProvider` so the
    // safety logic is unit-testable with an in-memory provider, independent
    // of the database and real provider APIs.
    // ------------------------------------------------------------------

    /// Fetch and parse the ownership marker TXT for a record name, if any.
    async fn fetch_marker(
        provider: &dyn DnsProvider,
        zone: &str,
        record_name: &str,
    ) -> Result<Option<OwnershipMarker>, DnsError> {
        let registry_name = registry_record_name(record_name);
        let txt = provider
            .get_record(zone, &registry_name, DnsRecordType::TXT)
            .await?;
        Ok(txt.and_then(|record| match &record.content {
            DnsRecordContent::TXT { content } => OwnershipMarker::parse(content),
            _ => None,
        }))
    }

    async fn ownership_of(
        provider: &dyn DnsProvider,
        zone: &str,
        name: &str,
        record_type: DnsRecordType,
        instance: &str,
    ) -> Result<RecordOwnership, DnsError> {
        let existing = provider.get_record(zone, name, record_type).await?;
        let Some(record) = existing else {
            return Ok(RecordOwnership::NotFound);
        };
        match Self::fetch_marker(provider, zone, name).await? {
            None => Ok(RecordOwnership::Unmanaged(record)),
            Some(marker) if marker.is_owned_by(instance) => {
                Ok(RecordOwnership::Owned(record, marker))
            }
            Some(marker) => Ok(RecordOwnership::OwnedByOther(record, marker)),
        }
    }

    async fn guarded_set(
        provider: &dyn DnsProvider,
        zone: &str,
        request: DnsRecordRequest,
        marker: &OwnershipMarker,
        instance: &str,
    ) -> Result<DnsRecord, DnsError> {
        let record_type = request.content.record_type();
        let existed = match Self::ownership_of(provider, zone, &request.name, record_type, instance)
            .await?
        {
            RecordOwnership::NotFound => false,
            RecordOwnership::Owned(_, _) => true,
            RecordOwnership::Unmanaged(_) => {
                return Err(DnsError::RecordConflict {
                    domain: zone.to_string(),
                    name: request.name.clone(),
                    record_type: record_type.to_string(),
                    reason: "an existing record with this name is not managed by temps".to_string(),
                });
            }
            RecordOwnership::OwnedByOther(_, marker) => {
                return Err(DnsError::NotOwnedByInstance {
                    domain: zone.to_string(),
                    name: request.name.clone(),
                    record_type: record_type.to_string(),
                    owner_instance: marker.instance,
                });
            }
        };

        // Marker first: a crash after this point leaves an orphan TXT (noise),
        // never a live unmarked record (a permanent conflict against ourselves).
        let registry_request = DnsRecordRequest {
            name: registry_record_name(&request.name),
            content: DnsRecordContent::TXT {
                content: marker.to_txt_content()?,
            },
            ttl: request.ttl,
            proxied: false,
        };
        provider.set_record(zone, registry_request).await?;

        match provider.set_record(zone, request.clone()).await {
            Ok(record) => Ok(record),
            Err(e) => {
                // Creating the target failed. If nothing existed before, the
                // fresh marker is pure junk — clean it up best-effort.
                if !existed {
                    let registry_name = registry_record_name(&request.name);
                    if let Err(cleanup_err) = provider
                        .remove_record(zone, &registry_name, DnsRecordType::TXT)
                        .await
                    {
                        warn!(
                            "Failed to clean up ownership marker '{}' in zone {} after record create failed: {}",
                            registry_name, zone, cleanup_err
                        );
                    }
                }
                Err(e)
            }
        }
    }

    async fn guarded_remove(
        provider: &dyn DnsProvider,
        zone: &str,
        name: &str,
        record_type: DnsRecordType,
        instance: &str,
    ) -> Result<(), DnsError> {
        match Self::ownership_of(provider, zone, name, record_type, instance).await? {
            RecordOwnership::NotFound => {
                // Record already gone; clean up a stray marker of ours if the
                // registry still has one so it doesn't accumulate.
                if let Some(marker) = Self::fetch_marker(provider, zone, name).await? {
                    if marker.is_owned_by(instance) {
                        provider
                            .remove_record(zone, &registry_record_name(name), DnsRecordType::TXT)
                            .await?;
                    }
                }
                Ok(())
            }
            RecordOwnership::Owned(_, _) => {
                provider.remove_record(zone, name, record_type).await?;
                provider
                    .remove_record(zone, &registry_record_name(name), DnsRecordType::TXT)
                    .await?;
                Ok(())
            }
            RecordOwnership::Unmanaged(_) => Err(DnsError::RecordConflict {
                domain: zone.to_string(),
                name: name.to_string(),
                record_type: record_type.to_string(),
                reason: "the record is not managed by temps, so temps will not delete it"
                    .to_string(),
            }),
            RecordOwnership::OwnedByOther(_, marker) => Err(DnsError::NotOwnedByInstance {
                domain: zone.to_string(),
                name: name.to_string(),
                record_type: record_type.to_string(),
                owner_instance: marker.instance,
            }),
        }
    }

    async fn guarded_import(
        provider: &dyn DnsProvider,
        zone: &str,
        name: &str,
        record_type: DnsRecordType,
        instance: &str,
        scope: OwnershipScope,
    ) -> Result<OwnershipMarker, DnsError> {
        match Self::ownership_of(provider, zone, name, record_type, instance).await? {
            RecordOwnership::NotFound => Err(DnsError::RecordNotFound(format!(
                "{} record '{}' in zone {} does not exist, so it cannot be imported",
                record_type, name, zone
            ))),
            RecordOwnership::Owned(_, marker) => Ok(marker), // already ours — idempotent
            RecordOwnership::OwnedByOther(_, marker) => Err(DnsError::NotOwnedByInstance {
                domain: zone.to_string(),
                name: name.to_string(),
                record_type: record_type.to_string(),
                owner_instance: marker.instance,
            }),
            RecordOwnership::Unmanaged(_) => {
                let marker = OwnershipMarker::new(instance, scope.project_id, scope.environment_id);
                let registry_request = DnsRecordRequest {
                    name: registry_record_name(name),
                    content: DnsRecordContent::TXT {
                        content: marker.to_txt_content()?,
                    },
                    ttl: None,
                    proxied: false,
                };
                provider.set_record(zone, registry_request).await?;
                Ok(marker)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{DnsProviderCapabilities, DnsProviderType, DnsZone};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory provider: records keyed by (name, type). Panics are fine in
    /// tests; production paths never touch this.
    struct MockProvider {
        records: Mutex<HashMap<(String, String), DnsRecord>>,
        fail_target_writes: bool,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                records: Mutex::new(HashMap::new()),
                fail_target_writes: false,
            }
        }

        fn with_record(self, name: &str, content: DnsRecordContent) -> Self {
            let record_type = content.record_type().to_string();
            self.records.lock().unwrap().insert(
                (name.to_string(), record_type.clone()),
                DnsRecord {
                    id: Some(format!("{}-{}", name, record_type)),
                    zone: "example.com".to_string(),
                    name: name.to_string(),
                    fqdn: format!("{}.example.com", name),
                    content,
                    ttl: 300,
                    proxied: false,
                    metadata: HashMap::new(),
                },
            );
            self
        }

        fn has_record(&self, name: &str, record_type: DnsRecordType) -> bool {
            self.records
                .lock()
                .unwrap()
                .contains_key(&(name.to_string(), record_type.to_string()))
        }
    }

    #[async_trait]
    impl DnsProvider for MockProvider {
        fn provider_type(&self) -> DnsProviderType {
            DnsProviderType::Manual
        }

        fn capabilities(&self) -> DnsProviderCapabilities {
            DnsProviderCapabilities {
                a_record: true,
                cname_record: true,
                txt_record: true,
                proxy: true,
                ..Default::default()
            }
        }

        async fn test_connection(&self) -> Result<bool, DnsError> {
            Ok(true)
        }

        async fn list_zones(&self) -> Result<Vec<DnsZone>, DnsError> {
            Ok(vec![])
        }

        async fn get_zone(&self, _domain: &str) -> Result<Option<DnsZone>, DnsError> {
            Ok(None)
        }

        async fn list_records(&self, _domain: &str) -> Result<Vec<DnsRecord>, DnsError> {
            Ok(self.records.lock().unwrap().values().cloned().collect())
        }

        async fn get_record(
            &self,
            _domain: &str,
            name: &str,
            record_type: DnsRecordType,
        ) -> Result<Option<DnsRecord>, DnsError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .get(&(name.to_string(), record_type.to_string()))
                .cloned())
        }

        async fn create_record(
            &self,
            domain: &str,
            request: DnsRecordRequest,
        ) -> Result<DnsRecord, DnsError> {
            let record_type = request.content.record_type();
            if self.fail_target_writes && record_type != DnsRecordType::TXT {
                return Err(DnsError::ApiError("simulated write failure".to_string()));
            }
            let record = DnsRecord {
                id: Some(format!("{}-{}", request.name, record_type)),
                zone: domain.to_string(),
                name: request.name.clone(),
                fqdn: format!("{}.{}", request.name, domain),
                content: request.content,
                ttl: request.ttl.unwrap_or(300),
                proxied: request.proxied,
                metadata: HashMap::new(),
            };
            self.records.lock().unwrap().insert(
                (record.name.clone(), record_type.to_string()),
                record.clone(),
            );
            Ok(record)
        }

        async fn update_record(
            &self,
            domain: &str,
            _record_id: &str,
            request: DnsRecordRequest,
        ) -> Result<DnsRecord, DnsError> {
            self.create_record(domain, request).await
        }

        async fn delete_record(&self, _domain: &str, record_id: &str) -> Result<(), DnsError> {
            self.records
                .lock()
                .unwrap()
                .retain(|_, r| r.id.as_deref() != Some(record_id));
            Ok(())
        }
    }

    const INSTANCE: &str = "test-instance";

    fn a_request(name: &str, proxied: bool) -> DnsRecordRequest {
        DnsRecordRequest {
            name: name.to_string(),
            content: DnsRecordContent::A {
                address: "192.0.2.10".to_string(),
            },
            ttl: Some(300),
            proxied,
        }
    }

    fn marker() -> OwnershipMarker {
        OwnershipMarker::new(INSTANCE, Some(1), Some(2))
    }

    #[tokio::test]
    async fn set_creates_record_and_ownership_marker() {
        let provider = MockProvider::new();
        let record = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap();

        assert_eq!(record.name, "app");
        assert!(provider.has_record("app", DnsRecordType::A));
        assert!(provider.has_record("_temps-owned.app", DnsRecordType::TXT));
    }

    #[tokio::test]
    async fn set_refuses_to_overwrite_unmanaged_record() {
        // The core ADR-031 invariant: an existing record without a marker is
        // untouchable, whatever its content.
        let provider = MockProvider::new().with_record(
            "app",
            DnsRecordContent::A {
                address: "203.0.113.1".to_string(),
            },
        );

        let err = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, DnsError::RecordConflict { .. }));
        // Original record untouched
        let existing = provider
            .get_record("example.com", "app", DnsRecordType::A)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(existing.content.to_value_string(), "203.0.113.1");
    }

    #[tokio::test]
    async fn set_refuses_record_owned_by_other_instance() {
        let foreign = OwnershipMarker::new("other-install", None, None);
        let provider = MockProvider::new()
            .with_record(
                "app",
                DnsRecordContent::A {
                    address: "203.0.113.1".to_string(),
                },
            )
            .with_record(
                "_temps-owned.app",
                DnsRecordContent::TXT {
                    content: foreign.to_txt_content().unwrap(),
                },
            );

        let err = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap_err();

        match err {
            DnsError::NotOwnedByInstance { owner_instance, .. } => {
                assert_eq!(owner_instance, "other-install");
            }
            other => panic!("expected NotOwnedByInstance, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn set_updates_record_owned_by_this_instance() {
        let provider = MockProvider::new()
            .with_record(
                "app",
                DnsRecordContent::A {
                    address: "203.0.113.1".to_string(),
                },
            )
            .with_record(
                "_temps-owned.app",
                DnsRecordContent::TXT {
                    content: marker().to_txt_content().unwrap(),
                },
            );

        let record = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap();

        assert_eq!(record.content.to_value_string(), "192.0.2.10");
    }

    #[tokio::test]
    async fn failed_create_cleans_up_fresh_marker() {
        let mut provider = MockProvider::new();
        provider.fail_target_writes = true;

        let err = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, DnsError::ApiError(_)));
        // No orphan marker left behind for a record that was never created.
        assert!(!provider.has_record("_temps-owned.app", DnsRecordType::TXT));
    }

    #[tokio::test]
    async fn remove_refuses_unmanaged_record() {
        let provider = MockProvider::new().with_record(
            "app",
            DnsRecordContent::A {
                address: "203.0.113.1".to_string(),
            },
        );

        let err = ManagedDnsRecordService::guarded_remove(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, DnsError::RecordConflict { .. }));
        assert!(provider.has_record("app", DnsRecordType::A));
    }

    #[tokio::test]
    async fn remove_deletes_owned_record_and_marker() {
        let provider = MockProvider::new()
            .with_record(
                "app",
                DnsRecordContent::A {
                    address: "192.0.2.10".to_string(),
                },
            )
            .with_record(
                "_temps-owned.app",
                DnsRecordContent::TXT {
                    content: marker().to_txt_content().unwrap(),
                },
            );

        ManagedDnsRecordService::guarded_remove(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
        )
        .await
        .unwrap();

        assert!(!provider.has_record("app", DnsRecordType::A));
        assert!(!provider.has_record("_temps-owned.app", DnsRecordType::TXT));
    }

    #[tokio::test]
    async fn remove_of_missing_record_is_ok_and_cleans_stray_marker() {
        let provider = MockProvider::new().with_record(
            "_temps-owned.app",
            DnsRecordContent::TXT {
                content: marker().to_txt_content().unwrap(),
            },
        );

        ManagedDnsRecordService::guarded_remove(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
        )
        .await
        .unwrap();

        assert!(!provider.has_record("_temps-owned.app", DnsRecordType::TXT));
    }

    #[tokio::test]
    async fn import_stamps_marker_on_unmanaged_record() {
        let provider = MockProvider::new().with_record(
            "app",
            DnsRecordContent::A {
                address: "203.0.113.1".to_string(),
            },
        );

        let imported = ManagedDnsRecordService::guarded_import(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
            OwnershipScope {
                project_id: Some(9),
                environment_id: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(imported.project_id, Some(9));
        assert!(provider.has_record("_temps-owned.app", DnsRecordType::TXT));

        // After import, set is allowed.
        let record = ManagedDnsRecordService::guarded_set(
            &provider,
            "example.com",
            a_request("app", false),
            &marker(),
            INSTANCE,
        )
        .await
        .unwrap();
        assert_eq!(record.content.to_value_string(), "192.0.2.10");
    }

    #[tokio::test]
    async fn import_refuses_missing_and_foreign_records() {
        let provider = MockProvider::new();
        let err = ManagedDnsRecordService::guarded_import(
            &provider,
            "example.com",
            "ghost",
            DnsRecordType::A,
            INSTANCE,
            OwnershipScope::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DnsError::RecordNotFound(_)));

        let foreign = OwnershipMarker::new("other-install", None, None);
        let provider = MockProvider::new()
            .with_record(
                "app",
                DnsRecordContent::A {
                    address: "203.0.113.1".to_string(),
                },
            )
            .with_record(
                "_temps-owned.app",
                DnsRecordContent::TXT {
                    content: foreign.to_txt_content().unwrap(),
                },
            );
        let err = ManagedDnsRecordService::guarded_import(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
            OwnershipScope::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DnsError::NotOwnedByInstance { .. }));
    }

    #[tokio::test]
    async fn user_txt_record_at_registry_name_does_not_grant_ownership() {
        // A user TXT that happens to live at `_temps-owned.app` but isn't a
        // valid marker must read as Unmanaged, not Owned.
        let provider = MockProvider::new()
            .with_record(
                "app",
                DnsRecordContent::A {
                    address: "203.0.113.1".to_string(),
                },
            )
            .with_record(
                "_temps-owned.app",
                DnsRecordContent::TXT {
                    content: "v=spf1 -all".to_string(),
                },
            );

        let ownership = ManagedDnsRecordService::ownership_of(
            &provider,
            "example.com",
            "app",
            DnsRecordType::A,
            INSTANCE,
        )
        .await
        .unwrap();
        assert!(matches!(ownership, RecordOwnership::Unmanaged(_)));
    }
}
