//! Per-provider configuration and per-event price/product attribution.
//!
//! Two additions, both purely additive:
//!
//!   1. `revenue_integrations.config JSONB NULL` — opaque, typed on the
//!      application side via the `ProviderConfig` tagged enum. Holds
//!      provider-specific settings like Stripe's price/product allowlist
//!      and `metered_mode`. Old rows stay at NULL and the ingestion path
//!      treats that as "accept all events" for backwards compatibility.
//!
//!   2. `revenue_events.price_id TEXT NULL` and `product_id TEXT NULL` —
//!      populated from Stripe subscription items and invoice lines so the
//!      ingestion filter and analytics can key on a specific SKU within a
//!      shared webhook.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE revenue_integrations \
             ADD COLUMN IF NOT EXISTS config JSONB NULL",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE revenue_events \
             ADD COLUMN IF NOT EXISTS price_id TEXT NULL",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE revenue_events \
             ADD COLUMN IF NOT EXISTS product_id TEXT NULL",
        )
        .await?;

        // Lookup for the UI "events for SKU X" filter, narrow & optional.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_revenue_events_price \
             ON revenue_events (project_id, price_id) \
             WHERE price_id IS NOT NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_revenue_events_price")
            .await?;
        db.execute_unprepared("ALTER TABLE revenue_events DROP COLUMN IF EXISTS product_id")
            .await?;
        db.execute_unprepared("ALTER TABLE revenue_events DROP COLUMN IF EXISTS price_id")
            .await?;
        db.execute_unprepared("ALTER TABLE revenue_integrations DROP COLUMN IF EXISTS config")
            .await?;
        Ok(())
    }
}
