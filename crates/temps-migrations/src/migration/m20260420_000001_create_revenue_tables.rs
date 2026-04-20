//! Create revenue tracking tables (provider-agnostic).
//!
//! Four tables:
//!   * `revenue_integrations` — one row per (project, provider).
//!     The `webhook_path_token` is a 256-bit unguessable secret used
//!     in the public webhook URL path; signature verification is the
//!     actual authenticity check.
//!   * `revenue_events` — append-only log of normalized events,
//!     idempotent on `(integration_id, provider_event_id)`. Converted
//!     to a TimescaleDB hypertable on `occurred_at` when available.
//!   * `revenue_subscriptions_state` — current state projection of
//!     each subscription (status + MRR).
//!   * `revenue_customers_state` — current state projection of each
//!     customer (first seen / churned).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // ---- revenue_integrations ----------------------------------
        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS revenue_integrations (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                provider VARCHAR(32) NOT NULL,
                webhook_path_token VARCHAR(64) NOT NULL UNIQUE,
                webhook_signing_secret_encrypted TEXT NOT NULL,
                status VARCHAR(16) NOT NULL DEFAULT 'pending',
                last_event_at TIMESTAMPTZ NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_integrations_project \
             ON revenue_integrations (project_id, provider)",
        )
        .await?;

        // At most one non-disabled integration per (project, provider).
        // Users can disconnect and reconnect, but cannot double-subscribe.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_revenue_integrations_active \
             ON revenue_integrations (project_id, provider) \
             WHERE status <> 'disabled'",
        )
        .await?;

        // ---- revenue_events ----------------------------------------
        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS revenue_events (
                id BIGSERIAL NOT NULL,
                project_id INTEGER NOT NULL,
                integration_id INTEGER NOT NULL,
                provider VARCHAR(32) NOT NULL,
                provider_event_id VARCHAR(255) NOT NULL,
                event_type VARCHAR(64) NOT NULL,
                customer_ref VARCHAR(255) NULL,
                subscription_ref VARCHAR(255) NULL,
                subscription_status VARCHAR(32) NULL,
                mrr_minor BIGINT NULL,
                amount_minor BIGINT NULL,
                currency CHAR(3) NULL,
                occurred_at TIMESTAMPTZ NOT NULL,
                payload JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (id, occurred_at)
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_events_project_time \
             ON revenue_events (project_id, occurred_at DESC)",
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_events_project_type_time \
             ON revenue_events (project_id, event_type, occurred_at DESC)",
        )
        .await?;

        // Convert to a hypertable when TimescaleDB is available. This must
        // happen BEFORE any UNIQUE index is created, because TimescaleDB
        // requires every UNIQUE constraint on a hypertable to include the
        // partitioning column (`occurred_at`). We defer the dedup UNIQUE
        // index to after this call, and always include `occurred_at` in it.
        //
        // Any error here (missing extension, table already hypertable with
        // different settings) is intentionally swallowed — but we run it
        // in its own connection-level statement so a failure does not
        // poison the outer migration transaction.
        let _ = db
            .execute_unprepared(
                "DO $$ BEGIN \
                    PERFORM create_hypertable('revenue_events', 'occurred_at', \
                        if_not_exists => TRUE, migrate_data => TRUE); \
                 EXCEPTION WHEN OTHERS THEN NULL; END $$",
            )
            .await;

        // Idempotency key: the same Stripe event can arrive multiple
        // times (retries / manual resend) and must be stored once.
        // `occurred_at` is included so the UNIQUE index is valid on a
        // TimescaleDB hypertable (partitioning column must be part of
        // every UNIQUE constraint). In practice the first two columns
        // alone are sufficient to deduplicate — occurred_at is constant
        // per provider event id.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_revenue_events_dedup \
             ON revenue_events (integration_id, provider_event_id, occurred_at)",
        )
        .await?;

        // ---- revenue_subscriptions_state ---------------------------
        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS revenue_subscriptions_state (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL,
                integration_id INTEGER NOT NULL REFERENCES revenue_integrations(id) ON DELETE CASCADE,
                provider VARCHAR(32) NOT NULL,
                provider_subscription_id VARCHAR(255) NOT NULL,
                customer_ref VARCHAR(255) NULL,
                status VARCHAR(32) NOT NULL,
                mrr_minor BIGINT NOT NULL DEFAULT 0,
                currency CHAR(3) NULL,
                started_at TIMESTAMPTZ NULL,
                canceled_at TIMESTAMPTZ NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE (integration_id, provider_subscription_id)
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_subs_project_status \
             ON revenue_subscriptions_state (project_id, status)",
        )
        .await?;

        // ---- revenue_customers_state -------------------------------
        db.execute_unprepared(
            r#"
            CREATE TABLE IF NOT EXISTS revenue_customers_state (
                id SERIAL PRIMARY KEY,
                project_id INTEGER NOT NULL,
                integration_id INTEGER NOT NULL REFERENCES revenue_integrations(id) ON DELETE CASCADE,
                provider VARCHAR(32) NOT NULL,
                provider_customer_ref VARCHAR(255) NOT NULL,
                first_seen_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                churned_at TIMESTAMPTZ NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE (integration_id, provider_customer_ref)
            )
            "#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_customers_project_seen \
             ON revenue_customers_state (project_id, first_seen_at)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS revenue_customers_state CASCADE")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS revenue_subscriptions_state CASCADE")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS revenue_events CASCADE")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS revenue_integrations CASCADE")
            .await?;
        Ok(())
    }
}
