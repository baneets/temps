//! DNS record ownership markers (ADR-031)
//!
//! Temps writes public A/AAAA/CNAME records into zones it does not own.
//! The one mistake this feature must never make is touching a record temps
//! did not create. Ownership is therefore recorded *at the provider*, next to
//! the record itself, as a companion TXT "registry" record
//! (`_temps-owned.<name>`) whose content is a typed JSON marker. Before any
//! update or delete, the marker is fetched and must parse AND match this
//! install's instance ID; anything else refuses the write.
//!
//! The companion-TXT scheme works uniformly across every provider. Cloudflare
//! additionally has a per-record `comment` field, but the `cloudflare` crate's
//! DNS params don't expose it, so comment stamping is deferred (the TXT
//! registry is used there too).
//!
//! The marker JSON is a compatibility surface once it exists in user zones —
//! it carries a `v` field so the format can evolve. Unknown fields are
//! tolerated on parse so a `v: 2` writer doesn't brick a `v: 1` reader.

use serde::{Deserialize, Serialize};

use crate::errors::DnsError;

/// Current marker format version.
pub const OWNERSHIP_MARKER_VERSION: u32 = 1;

/// Value of `managed_by` in every marker temps writes.
pub const OWNERSHIP_MANAGED_BY: &str = "temps";

/// Label prefix of the companion TXT registry record.
pub const OWNERSHIP_REGISTRY_PREFIX: &str = "_temps-owned";

/// Replacement for the `*` label when building a registry name for a
/// wildcard record (`*` is not a meaningful label to prefix).
const WILDCARD_REPLACEMENT: &str = "wildcard";

/// Ownership marker stored in the companion TXT record.
///
/// `instance` is the install-scoped random ID from
/// [`crate::services::ManagedDnsRecordService`]; two temps installs managing
/// the same zone will refuse to touch each other's records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipMarker {
    /// Always [`OWNERSHIP_MANAGED_BY`]. Anything else fails to parse as ours.
    pub managed_by: String,

    /// Install-scoped random ID of the temps instance that created the record.
    pub instance: String,

    /// Project the record was created for, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i32>,

    /// Environment the record was created for, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<i32>,

    /// Marker format version.
    pub v: u32,
}

impl OwnershipMarker {
    pub fn new(instance: &str, project_id: Option<i32>, environment_id: Option<i32>) -> Self {
        Self {
            managed_by: OWNERSHIP_MANAGED_BY.to_string(),
            instance: instance.to_string(),
            project_id,
            environment_id,
            v: OWNERSHIP_MARKER_VERSION,
        }
    }

    /// Serialize to the TXT record content.
    pub fn to_txt_content(&self) -> Result<String, DnsError> {
        serde_json::to_string(self).map_err(DnsError::Serialization)
    }

    /// Parse a TXT record content as an ownership marker.
    ///
    /// Returns `None` for anything that is not a well-formed temps marker —
    /// unparsable JSON, wrong `managed_by`, missing fields. Callers treat
    /// `None` as "not ours: hands off".
    pub fn parse(content: &str) -> Option<Self> {
        let marker: Self = serde_json::from_str(content.trim()).ok()?;
        if marker.managed_by != OWNERSHIP_MANAGED_BY || marker.instance.is_empty() {
            return None;
        }
        Some(marker)
    }

    /// Whether this marker was written by the given temps instance.
    pub fn is_owned_by(&self, instance: &str) -> bool {
        self.instance == instance
    }
}

/// Name of the companion TXT registry record for a managed record name.
///
/// - `@` / empty (zone apex) → `_temps-owned`
/// - `www` → `_temps-owned.www`
/// - `*-staging` → `_temps-owned.wildcard-staging`
/// - `*.staging` → `_temps-owned.wildcard.staging`
///
/// The wildcard label is replaced because `_temps-owned.*` is not a queryable
/// name; the replacement is deterministic so lookups and writes agree.
pub fn registry_record_name(record_name: &str) -> String {
    if record_name == "@" || record_name.is_empty() {
        return OWNERSHIP_REGISTRY_PREFIX.to_string();
    }
    let sanitized = record_name.replace('*', WILDCARD_REPLACEMENT);
    format!("{}.{}", OWNERSHIP_REGISTRY_PREFIX, sanitized)
}

/// Number of subdomain levels a record name adds below the zone apex.
///
/// `@` → 0, `www` → 1, `*-staging` → 1, `*.staging` → 2, `a.b.c` → 3.
pub fn subdomain_depth(record_name: &str) -> usize {
    if record_name == "@" || record_name.is_empty() {
        return 0;
    }
    record_name.split('.').filter(|l| !l.is_empty()).count()
}

/// Guardrail for Cloudflare's Universal SSL depth limit (ADR-031 §3).
///
/// Cloudflare's free/pro certificates only cover ONE subdomain level below
/// the apex. A *proxied* record at depth ≥ 2 (`a.b.example.com`,
/// `*.foo.example.com`) passes DNS but fails TLS at the edge with an opaque
/// 526/525 unless the user pays for Advanced Certificate Manager. Detect it
/// at write time and refuse with an actionable message instead.
///
/// Only applies to proxied records — unproxied deep records are fine.
pub fn check_proxied_depth(zone: &str, record_name: &str) -> Result<(), DnsError> {
    let depth = subdomain_depth(record_name);
    if depth < 2 {
        return Ok(());
    }
    let flat_suggestion = record_name.replace('.', "-");
    Err(DnsError::ProxiedDepthUnsupported {
        fqdn: format!("{}.{}", record_name, zone),
        levels: depth,
        flat_suggestion: format!("{}.{}", flat_suggestion, zone),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips_through_txt_content() {
        let marker = OwnershipMarker::new("inst-abc123", Some(7), Some(42));
        let content = marker.to_txt_content().unwrap();
        let parsed = OwnershipMarker::parse(&content).unwrap();
        assert_eq!(parsed, marker);
        assert_eq!(parsed.v, OWNERSHIP_MARKER_VERSION);
    }

    #[test]
    fn marker_without_scope_omits_ids_in_json() {
        let marker = OwnershipMarker::new("inst-abc123", None, None);
        let content = marker.to_txt_content().unwrap();
        assert!(!content.contains("project_id"));
        assert!(!content.contains("environment_id"));
        assert_eq!(OwnershipMarker::parse(&content).unwrap(), marker);
    }

    #[test]
    fn parse_rejects_non_marker_content() {
        // Existing user TXT records must never parse as ours.
        assert!(OwnershipMarker::parse("v=spf1 -all").is_none());
        assert!(OwnershipMarker::parse("").is_none());
        assert!(OwnershipMarker::parse("{\"foo\": 1}").is_none());
    }

    #[test]
    fn parse_rejects_wrong_managed_by() {
        let content = r#"{"managed_by":"other-tool","instance":"x","v":1}"#;
        assert!(OwnershipMarker::parse(content).is_none());
    }

    #[test]
    fn parse_rejects_empty_instance() {
        let content = r#"{"managed_by":"temps","instance":"","v":1}"#;
        assert!(OwnershipMarker::parse(content).is_none());
    }

    #[test]
    fn parse_tolerates_unknown_fields_from_future_versions() {
        let content = r#"{"managed_by":"temps","instance":"x","v":2,"new_field":"y"}"#;
        let marker = OwnershipMarker::parse(content).unwrap();
        assert_eq!(marker.v, 2);
    }

    #[test]
    fn ownership_is_instance_scoped() {
        let marker = OwnershipMarker::new("inst-a", None, None);
        assert!(marker.is_owned_by("inst-a"));
        assert!(!marker.is_owned_by("inst-b"));
    }

    #[test]
    fn registry_name_for_apex_and_subdomains() {
        assert_eq!(registry_record_name("@"), "_temps-owned");
        assert_eq!(registry_record_name(""), "_temps-owned");
        assert_eq!(registry_record_name("www"), "_temps-owned.www");
        assert_eq!(
            registry_record_name("*-staging"),
            "_temps-owned.wildcard-staging"
        );
        assert_eq!(
            registry_record_name("*.staging"),
            "_temps-owned.wildcard.staging"
        );
    }

    #[test]
    fn subdomain_depth_counts_labels() {
        assert_eq!(subdomain_depth("@"), 0);
        assert_eq!(subdomain_depth(""), 0);
        assert_eq!(subdomain_depth("www"), 1);
        assert_eq!(subdomain_depth("*-staging"), 1);
        assert_eq!(subdomain_depth("*.staging"), 2);
        assert_eq!(subdomain_depth("a.b.c"), 3);
    }

    #[test]
    fn proxied_depth_guardrail_allows_single_level() {
        assert!(check_proxied_depth("example.com", "@").is_ok());
        assert!(check_proxied_depth("example.com", "www").is_ok());
        assert!(check_proxied_depth("example.com", "*-staging").is_ok());
    }

    #[test]
    fn proxied_depth_guardrail_rejects_two_levels_with_flat_suggestion() {
        let err = check_proxied_depth("example.com", "*.staging").unwrap_err();
        match err {
            DnsError::ProxiedDepthUnsupported {
                fqdn,
                levels,
                flat_suggestion,
            } => {
                assert_eq!(fqdn, "*.staging.example.com");
                assert_eq!(levels, 2);
                assert_eq!(flat_suggestion, "*-staging.example.com");
            }
            other => panic!("expected ProxiedDepthUnsupported, got {:?}", other),
        }
    }
}
