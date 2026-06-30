//! Service for AI pending-action lifecycle management (propose → confirm/reject).
//!
//! The AI NEVER executes a mutation. When the model proposes a write, this
//! service inserts a `proposed` row in `ai_pending_actions`. A human later
//! calls the confirm endpoint, which is the ONLY place execution runs. The
//! confirming user's `AuthContext` is used — never anything the model supplied.

use std::sync::Arc;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use tracing::error;

use temps_auth::context::AuthContext;
use temps_auth::permissions::Permission;
use temps_core::{AuditContext, AuditLogger};
use temps_entities::ai_pending_actions;
use temps_ai_api_tools::{ApiCallScope, WriteApiToolsHandle};

use crate::audit::{AiActionConfirmedAudit, AiActionRejectedAudit};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the pending-action service.
#[derive(Debug, thiserror::Error)]
pub enum PendingActionError {
    #[error("pending action '{public_id}' not found")]
    NotFound { public_id: String },

    #[error("pending action '{public_id}' has status '{status}', expected 'proposed'")]
    InvalidState { public_id: String, status: String },

    #[error("permission '{permission}' is required to confirm this action")]
    PermissionDenied { permission: String },

    #[error("write action feature is not available (write caller not yet wired)")]
    Unavailable,

    #[error("execution of '{operation_id}' failed: {reason}")]
    Execution { operation_id: String, reason: String },

    #[error("database error: {0}")]
    Database(#[from] sea_orm::DbErr),
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Manages the lifecycle of AI-proposed write actions (propose → confirm/reject).
pub struct PendingActionService {
    db: Arc<DatabaseConnection>,
    write_handle: Arc<WriteApiToolsHandle>,
    audit_service: Arc<dyn AuditLogger>,
}

impl PendingActionService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        write_handle: Arc<WriteApiToolsHandle>,
        audit_service: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            db,
            write_handle,
            audit_service,
        }
    }

    /// Emit an audit entry best-effort: a failure must never block the caller.
    async fn audit(&self, op: &dyn temps_core::AuditOperation) {
        if let Err(e) = self.audit_service.create_audit_log(op).await {
            error!("Failed to write pending-action audit log: {e}");
        }
    }

    /// Insert a new `proposed` pending action row.
    ///
    /// `public_id` is generated the same way as [`ai_conversations`]: a UUID
    /// v4 formatted as a 32-hex-char simple (no-hyphen) string.
    pub async fn create(
        &self,
        conversation_id: i64,
        project_id: i32,
        prepared: &temps_ai_api_tools::PreparedWrite,
        required_permission: Option<String>,
        created_by: Option<i32>,
    ) -> Result<ai_pending_actions::Model, PendingActionError> {
        let public_id = uuid::Uuid::new_v4().simple().to_string();
        let now = Utc::now();
        let model = ai_pending_actions::ActiveModel {
            public_id: Set(public_id),
            conversation_id: Set(conversation_id),
            message_id: Set(None),
            project_id: Set(project_id),
            operation_id: Set(prepared.operation_id.clone()),
            method: Set(prepared.method.clone()),
            summary: Set(prepared.summary.clone()),
            params: Set(prepared.params.clone()),
            required_permission: Set(required_permission),
            status: Set("proposed".to_string()),
            result: Set(None),
            error: Set(None),
            created_by: Set(created_by),
            confirmed_by: Set(None),
            created_at: Set(now),
            confirmed_at: Set(None),
            executed_at: Set(None),
            ..Default::default()
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(model)
    }

    /// Best-effort: set `message_id` on a batch of pending-action rows.
    ///
    /// This links actions created during a turn to the persisted assistant
    /// message. Failure is logged but never propagated — the turn is already
    /// committed.
    pub async fn link_message(
        &self,
        action_ids: &[i64],
        message_id: i64,
    ) -> Result<(), PendingActionError> {
        if action_ids.is_empty() {
            return Ok(());
        }
        ai_pending_actions::Entity::update_many()
            .col_expr(
                ai_pending_actions::Column::MessageId,
                sea_orm::sea_query::Expr::value(message_id),
            )
            .filter(ai_pending_actions::Column::Id.is_in(action_ids.to_vec()))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    /// Get a single pending action by project-scoped `public_id`.
    ///
    /// 404 when not found OR when the `project_id` does not match (scoping guard).
    pub async fn get(
        &self,
        project_id: i32,
        public_id: &str,
    ) -> Result<ai_pending_actions::Model, PendingActionError> {
        ai_pending_actions::Entity::find()
            .filter(ai_pending_actions::Column::PublicId.eq(public_id))
            .filter(ai_pending_actions::Column::ProjectId.eq(project_id))
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| PendingActionError::NotFound {
                public_id: public_id.to_string(),
            })
    }

    /// All pending actions for a conversation, most-recently-created first.
    pub async fn list_for_conversation(
        &self,
        conversation_id: i64,
    ) -> Result<Vec<ai_pending_actions::Model>, PendingActionError> {
        Ok(ai_pending_actions::Entity::find()
            .filter(ai_pending_actions::Column::ConversationId.eq(conversation_id))
            .order_by_desc(ai_pending_actions::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    /// Confirm a proposed action: validate, claim atomically, execute, persist
    /// outcome (success or failure). Returns the updated model in all cases —
    /// a failed execution is surfaced as `status = "failed"` on the model, not
    /// as an Err, so the caller can surface status to the UI.
    pub async fn confirm(
        &self,
        project_id: i32,
        public_id: &str,
        auth: &AuthContext,
        confirmed_by: Option<i32>,
    ) -> Result<ai_pending_actions::Model, PendingActionError> {
        // 1. Load + project-scope check.
        let action = self.get(project_id, public_id).await?;

        // 2. Status pre-check.
        if action.status != "proposed" {
            return Err(PendingActionError::InvalidState {
                public_id: public_id.to_string(),
                status: action.status.clone(),
            });
        }

        // 3. Advisory permission check using the caller's auth.
        if let Some(ref perm_str) = action.required_permission {
            if let Some(perm) = Permission::from_str(perm_str) {
                if !auth.has_permission(&perm) {
                    return Err(PendingActionError::PermissionDenied {
                        permission: perm_str.clone(),
                    });
                }
            }
            // Unknown permission string → pass (router's permission_guard! is the
            // real boundary at execute time).
        }

        // 4. Atomic claim: flip to "executing" only if still "proposed".
        let now = Utc::now();
        let rows_affected = ai_pending_actions::Entity::update_many()
            .col_expr(
                ai_pending_actions::Column::Status,
                sea_orm::sea_query::Expr::value("executing"),
            )
            .filter(ai_pending_actions::Column::Id.eq(action.id))
            .filter(ai_pending_actions::Column::Status.eq("proposed"))
            .exec(self.db.as_ref())
            .await?
            .rows_affected;

        if rows_affected == 0 {
            // Lost a race or already handled — treat as invalid state.
            return Err(PendingActionError::InvalidState {
                public_id: public_id.to_string(),
                status: "already handled (concurrent confirm/reject)".to_string(),
            });
        }

        // 5. Get write caller.
        let caller = match self.write_handle.get() {
            Some(c) => c,
            None => {
                // Mark failed — we already claimed the row.
                let _ = self.set_failed(&action, "Write caller not available (startup incomplete).").await;
                return Err(PendingActionError::Unavailable);
            }
        };

        // 6. Execute with the CONFIRMING user's auth, scoped to the action's project.
        let scope = ApiCallScope {
            auth: auth.clone(),
            project_ids: vec![project_id],
        };
        let exec_result = caller
            .execute_write(&action.operation_id, action.params.clone(), &scope)
            .await;

        // 7. Persist outcome regardless of success/failure.
        let updated = match exec_result {
            Ok(resp) => {
                ai_pending_actions::ActiveModel {
                    id: Set(action.id),
                    status: Set("executed".to_string()),
                    result: Set(Some(resp.body)),
                    confirmed_by: Set(confirmed_by),
                    confirmed_at: Set(Some(now)),
                    executed_at: Set(Some(Utc::now())),
                    ..Default::default()
                }
                .update(self.db.as_ref())
                .await?
            }
            Err(e) => {
                ai_pending_actions::ActiveModel {
                    id: Set(action.id),
                    status: Set("failed".to_string()),
                    error: Set(Some(e.to_string())),
                    confirmed_by: Set(confirmed_by),
                    confirmed_at: Set(Some(now)),
                    ..Default::default()
                }
                .update(self.db.as_ref())
                .await?
            }
        };

        // 8. Audit best-effort.
        self.audit(&AiActionConfirmedAudit {
            context: AuditContext {
                user_id: auth.user_id(),
                ip_address: None,
                user_agent: String::new(),
            },
            project_id,
            action_id: updated.public_id.clone(),
            operation_id: updated.operation_id.clone(),
            status: updated.status.clone(),
        })
        .await;

        Ok(updated)
    }

    /// Reject a proposed action (no execution — simply mark rejected).
    pub async fn reject(
        &self,
        project_id: i32,
        public_id: &str,
        auth: &AuthContext,
        rejected_by: Option<i32>,
    ) -> Result<ai_pending_actions::Model, PendingActionError> {
        let action = self.get(project_id, public_id).await?;

        if action.status != "proposed" {
            return Err(PendingActionError::InvalidState {
                public_id: public_id.to_string(),
                status: action.status.clone(),
            });
        }

        let now = Utc::now();
        let updated = ai_pending_actions::ActiveModel {
            id: Set(action.id),
            status: Set("rejected".to_string()),
            confirmed_by: Set(rejected_by),
            confirmed_at: Set(Some(now)),
            ..Default::default()
        }
        .update(self.db.as_ref())
        .await?;

        self.audit(&AiActionRejectedAudit {
            context: AuditContext {
                user_id: auth.user_id(),
                ip_address: None,
                user_agent: String::new(),
            },
            project_id,
            action_id: updated.public_id.clone(),
            operation_id: updated.operation_id.clone(),
        })
        .await;

        Ok(updated)
    }

    /// Helper: mark a row as failed (best-effort, used in the confirm error path).
    async fn set_failed(
        &self,
        action: &ai_pending_actions::Model,
        reason: &str,
    ) -> Result<(), PendingActionError> {
        ai_pending_actions::ActiveModel {
            id: Set(action.id),
            status: Set("failed".to_string()),
            error: Set(Some(reason.to_string())),
            confirmed_at: Set(Some(Utc::now())),
            ..Default::default()
        }
        .update(self.db.as_ref())
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use temps_ai_api_tools::PreparedWrite;

    fn make_prepared() -> PreparedWrite {
        PreparedWrite {
            operation_id: "redeploy_deployment".to_string(),
            method: "POST".to_string(),
            path: "/projects/{project_id}/deployments/redeploy".to_string(),
            summary: "POST /projects/{project_id}/deployments/redeploy — redeploy_deployment"
                .to_string(),
            params: serde_json::json!({"deployment_id": 42}),
        }
    }

    fn make_proposed(id: i64, public_id: &str, project_id: i32) -> ai_pending_actions::Model {
        let now = Utc::now();
        ai_pending_actions::Model {
            id,
            public_id: public_id.to_string(),
            conversation_id: 1,
            message_id: None,
            project_id,
            operation_id: "redeploy_deployment".to_string(),
            method: "POST".to_string(),
            summary: "POST ... — redeploy_deployment".to_string(),
            params: serde_json::json!({}),
            required_permission: None,
            status: "proposed".to_string(),
            result: None,
            error: None,
            created_by: Some(1),
            confirmed_by: None,
            created_at: now,
            confirmed_at: None,
            executed_at: None,
        }
    }

    struct NoOpAudit;

    #[async_trait::async_trait]
    impl AuditLogger for NoOpAudit {
        async fn create_audit_log(
            &self,
            _op: &dyn temps_core::AuditOperation,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn noop_write_handle() -> Arc<WriteApiToolsHandle> {
        Arc::new(WriteApiToolsHandle::new())
    }

    fn noop_audit() -> Arc<dyn AuditLogger> {
        Arc::new(NoOpAudit)
    }

    // create inserts a proposed row.
    #[tokio::test]
    async fn test_create_inserts_proposed_row() {
        let row = make_proposed(1, "abc", 7);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![row.clone()]])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());
        let prepared = make_prepared();
        let result = svc.create(1, 7, &prepared, None, Some(1)).await;
        assert!(result.is_ok(), "create should succeed: {:?}", result.err());
        let model = result.unwrap();
        assert_eq!(model.status, "proposed");
        assert_eq!(model.project_id, 7);
    }

    // get: not-found returns NotFound.
    #[tokio::test]
    async fn test_get_not_found_returns_error() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![Vec::<ai_pending_actions::Model>::new()])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());
        let err = svc
            .get(7, "nonexistent")
            .await
            .expect_err("should be not found");
        assert!(
            matches!(err, PendingActionError::NotFound { ref public_id } if public_id == "nonexistent"),
            "unexpected error: {err:?}"
        );
    }

    // confirm on non-proposed → InvalidState.
    #[tokio::test]
    async fn test_confirm_non_proposed_returns_invalid_state() {
        let mut row = make_proposed(1, "abc", 7);
        row.status = "executed".to_string();
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![row]])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());

        let user = temps_entities::users::Model {
            id: 1,
            name: "t".to_string(),
            email: "t@t.com".to_string(),
            password_hash: None,
            email_verified: true,
            email_verification_token: None,
            email_verification_expires: None,
            password_reset_token: None,
            password_reset_expires: None,
            deleted_at: None,
            mfa_secret: None,
            mfa_enabled: false,
            mfa_recovery_codes: None,
            oidc_subject: None,
            oidc_provider_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let auth = AuthContext::new_session(user, temps_auth::permissions::Role::Admin);
        let err = svc
            .confirm(7, "abc", &auth, Some(1))
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, PendingActionError::InvalidState { .. }),
            "unexpected: {err:?}"
        );
    }

    // reject: proposed → rejected.
    #[tokio::test]
    async fn test_reject_transitions_to_rejected() {
        let proposed = make_proposed(1, "abc", 7);
        let rejected = {
            let mut m = proposed.clone();
            m.status = "rejected".to_string();
            m
        };
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // get() returns proposed
            .append_query_results(vec![vec![proposed]])
            // update() returns rejected
            .append_query_results(vec![vec![rejected]])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());

        let user = temps_entities::users::Model {
            id: 1,
            name: "t".to_string(),
            email: "t@t.com".to_string(),
            password_hash: None,
            email_verified: true,
            email_verification_token: None,
            email_verification_expires: None,
            password_reset_token: None,
            password_reset_expires: None,
            deleted_at: None,
            mfa_secret: None,
            mfa_enabled: false,
            mfa_recovery_codes: None,
            oidc_subject: None,
            oidc_provider_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let auth = AuthContext::new_session(user, temps_auth::permissions::Role::Admin);
        let result = svc.reject(7, "abc", &auth, Some(1)).await;
        assert!(result.is_ok(), "reject should succeed: {:?}", result.err());
        let m = result.unwrap();
        assert_eq!(m.status, "rejected");
    }

    // reject on non-proposed → InvalidState.
    #[tokio::test]
    async fn test_reject_non_proposed_returns_invalid_state() {
        let mut row = make_proposed(1, "abc", 7);
        row.status = "rejected".to_string();
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![row]])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());

        let user = temps_entities::users::Model {
            id: 1,
            name: "t".to_string(),
            email: "t@t.com".to_string(),
            password_hash: None,
            email_verified: true,
            email_verification_token: None,
            email_verification_expires: None,
            password_reset_token: None,
            password_reset_expires: None,
            deleted_at: None,
            mfa_secret: None,
            mfa_enabled: false,
            mfa_recovery_codes: None,
            oidc_subject: None,
            oidc_provider_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let auth = AuthContext::new_session(user, temps_auth::permissions::Role::Admin);
        let err = svc
            .reject(7, "abc", &auth, Some(1))
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, PendingActionError::InvalidState { .. }),
            "unexpected: {err:?}"
        );
    }

    // list_for_conversation: returns rows ordered by created_at DESC.
    #[tokio::test]
    async fn test_list_for_conversation_returns_rows() {
        let r1 = make_proposed(1, "p1", 7);
        let r2 = make_proposed(2, "p2", 7);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results(vec![vec![r1, r2]])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());
        let rows = svc.list_for_conversation(1).await.expect("should succeed");
        assert_eq!(rows.len(), 2);
    }

    // link_message: no-op on empty slice.
    #[tokio::test]
    async fn test_link_message_empty_slice_is_noop() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());
        let result = svc.link_message(&[], 42).await;
        assert!(result.is_ok());
    }

    // link_message: updates rows.
    #[tokio::test]
    async fn test_link_message_updates_rows() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 2,
            }])
            .into_connection();
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());
        let result = svc.link_message(&[1, 2], 99).await;
        assert!(result.is_ok(), "link_message should succeed: {:?}", result.err());
    }

    // confirm when write handle is empty → Unavailable (after claiming "executing").
    // We simulate: get returns proposed, atomic claim returns 1 row affected, then
    // handle is None → Unavailable. The "set_failed" update also needs a mock result.
    #[tokio::test]
    async fn test_confirm_unavailable_write_handle() {
        let proposed = make_proposed(1, "abc", 7);
        let failed_model = {
            let mut m = proposed.clone();
            m.status = "failed".to_string();
            m
        };
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // get()
            .append_query_results(vec![vec![proposed]])
            // atomic claim update_many
            .append_exec_results(vec![MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            // set_failed update
            .append_query_results(vec![vec![failed_model]])
            .into_connection();
        // Empty write handle (not wired)
        let svc = PendingActionService::new(Arc::new(db), noop_write_handle(), noop_audit());

        let user = temps_entities::users::Model {
            id: 1,
            name: "t".to_string(),
            email: "t@t.com".to_string(),
            password_hash: None,
            email_verified: true,
            email_verification_token: None,
            email_verification_expires: None,
            password_reset_token: None,
            password_reset_expires: None,
            deleted_at: None,
            mfa_secret: None,
            mfa_enabled: false,
            mfa_recovery_codes: None,
            oidc_subject: None,
            oidc_provider_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let auth = AuthContext::new_session(user, temps_auth::permissions::Role::Admin);
        let err = svc
            .confirm(7, "abc", &auth, Some(1))
            .await
            .expect_err("should fail with Unavailable");
        assert!(
            matches!(err, PendingActionError::Unavailable),
            "unexpected: {err:?}"
        );
    }
}
