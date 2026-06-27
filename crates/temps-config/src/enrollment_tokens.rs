//! Node enrollment token service (ADR-020 WS-1.1).
//!
//! Mints short-lived, single-use (or bounded-use), optionally node-scoped
//! tokens that authorize a worker to register, and validates+consumes them
//! atomically at registration time. Only the SHA-256 hash is persisted; the
//! plaintext is returned once at mint time and never stored.

use std::sync::Arc;

use rand::Rng;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Statement,
};
use sha2::{Digest, Sha256};
use temps_entities::node_enrollment_tokens;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnrollmentError {
    #[error("Enrollment token is invalid")]
    InvalidToken,
    #[error("Enrollment token has expired")]
    Expired,
    #[error("Enrollment token has been revoked")]
    Revoked,
    #[error("Enrollment token has reached its maximum number of uses")]
    Exhausted,
    #[error("Enrollment token {id} not found")]
    NotFound { id: i32 },
    #[error("Validation error: {message}")]
    Validation { message: String },
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),
}

/// Parameters for minting a new enrollment token.
#[derive(Debug, Clone)]
pub struct MintParams {
    /// Maximum registrations this token may authorize (>= 1).
    pub max_uses: i32,
    /// Time-to-live in seconds (e.g. 3600 for 1h).
    pub ttl_secs: i64,
    /// Optional pin: token only valid to register this node name.
    pub bound_node_name: Option<String>,
    /// Optional pin: scheduling labels the joining node must carry.
    pub bound_labels: Option<serde_json::Value>,
    /// Admin user who minted it (for audit).
    pub created_by_user_id: Option<i32>,
    /// SHA-256 fingerprint of the cluster CA at mint time (out-of-band CP
    /// verification by the joining node — ADR-020 WS-2.2).
    pub ca_fingerprint: Option<String>,
}

pub struct EnrollmentTokenService {
    db: Arc<DatabaseConnection>,
}

impl EnrollmentTokenService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    fn hash(plaintext: &str) -> String {
        hex::encode(Sha256::digest(plaintext.as_bytes()))
    }

    /// Mint a new enrollment token. Returns the plaintext token ONCE (it is
    /// never stored) plus the persisted row.
    pub async fn mint(
        &self,
        params: MintParams,
    ) -> Result<(String, node_enrollment_tokens::Model), EnrollmentError> {
        if params.max_uses < 1 {
            return Err(EnrollmentError::Validation {
                message: "max_uses must be >= 1".into(),
            });
        }
        if params.ttl_secs <= 0 {
            return Err(EnrollmentError::Validation {
                message: "ttl_secs must be > 0".into(),
            });
        }

        // 32 random bytes -> 64 hex chars (256-bit token). Scope the RNG so the
        // `!Send` `ThreadRng` is dropped before any `.await` — otherwise the
        // returned future is not `Send` and can't be used in an axum handler.
        let plaintext = {
            let mut rng = rand::thread_rng();
            let bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
            hex::encode(&bytes)
        };
        let token_hash = Self::hash(&plaintext);

        let now = chrono::Utc::now();
        let model = node_enrollment_tokens::ActiveModel {
            token_hash: Set(token_hash),
            max_uses: Set(params.max_uses),
            used_count: Set(0),
            expires_at: Set(now + chrono::Duration::seconds(params.ttl_secs)),
            bound_node_name: Set(params.bound_node_name),
            bound_labels: Set(params.bound_labels),
            created_by_user_id: Set(params.created_by_user_id),
            revoked_at: Set(None),
            ca_fingerprint: Set(params.ca_fingerprint),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };

        let inserted = model.insert(self.db.as_ref()).await?;
        Ok((plaintext, inserted))
    }

    /// Validate a presented plaintext token and atomically consume one use.
    ///
    /// Returns the token row (with any `bound_node_name`/`bound_labels` pins the
    /// caller must enforce) on success. Consumption is race-safe: the increment
    /// is a single conditional UPDATE, so two concurrent registrations can never
    /// over-consume a single-use token.
    pub async fn validate_and_consume(
        &self,
        plaintext: &str,
    ) -> Result<node_enrollment_tokens::Model, EnrollmentError> {
        let token_hash = Self::hash(plaintext);

        let row = node_enrollment_tokens::Entity::find()
            .filter(node_enrollment_tokens::Column::TokenHash.eq(&token_hash))
            .one(self.db.as_ref())
            .await?
            .ok_or(EnrollmentError::InvalidToken)?;

        if row.revoked_at.is_some() {
            return Err(EnrollmentError::Revoked);
        }
        if row.expires_at < chrono::Utc::now() {
            return Err(EnrollmentError::Expired);
        }
        if row.used_count >= row.max_uses {
            return Err(EnrollmentError::Exhausted);
        }

        // Atomic conditional consume — guards against concurrent over-use.
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE node_enrollment_tokens \
             SET used_count = used_count + 1, updated_at = now() \
             WHERE token_hash = $1 AND revoked_at IS NULL \
               AND expires_at > now() AND used_count < max_uses",
            [token_hash.into()],
        );
        let res = self.db.execute(stmt).await?;
        if res.rows_affected() == 0 {
            // Lost a race or just exhausted between the read and the update.
            return Err(EnrollmentError::Exhausted);
        }

        Ok(row)
    }

    /// List currently-valid (non-revoked, non-expired) tokens, newest first.
    pub async fn list_active(&self) -> Result<Vec<node_enrollment_tokens::Model>, EnrollmentError> {
        let now = chrono::Utc::now();
        Ok(node_enrollment_tokens::Entity::find()
            .filter(node_enrollment_tokens::Column::RevokedAt.is_null())
            .filter(node_enrollment_tokens::Column::ExpiresAt.gt(now))
            .order_by_desc(node_enrollment_tokens::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    /// Revoke a token by id (idempotent-ish: errors only if it doesn't exist).
    pub async fn revoke(&self, id: i32) -> Result<(), EnrollmentError> {
        let row = node_enrollment_tokens::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or(EnrollmentError::NotFound { id })?;
        let mut active: node_enrollment_tokens::ActiveModel = row.into();
        active.revoked_at = Set(Some(chrono::Utc::now()));
        active.updated_at = Set(chrono::Utc::now());
        active.update(self.db.as_ref()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_is_stable_64_hex() {
        let h = EnrollmentTokenService::hash("abc");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, EnrollmentTokenService::hash("abc"));
        assert_ne!(h, EnrollmentTokenService::hash("xyz"));
    }
}
