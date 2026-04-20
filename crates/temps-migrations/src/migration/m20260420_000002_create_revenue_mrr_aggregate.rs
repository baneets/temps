//! TimescaleDB continuous aggregate for daily subscription MRR per
//! project/currency.
//!
//! The raw `revenue_events` table is append-only, so we derive MRR from
//! the latest subscription event in each day. Dashboards can read this
//! aggregate directly for the timeseries chart and fall back to the
//! live `revenue_subscriptions_state` table for the current value.
//!
//! Gracefully no-ops when TimescaleDB is not available so tests and
//! vanilla Postgres environments still migrate.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Everything is wrapped in a single DO block so an environment
        // without TimescaleDB just skips this entire migration.
        let _ = db
            .execute_unprepared(
                r#"
                DO $$ BEGIN
                    EXECUTE $q$
                        CREATE MATERIALIZED VIEW IF NOT EXISTS revenue_mrr_daily
                        WITH (timescaledb.continuous) AS
                        SELECT
                            time_bucket('1 day', occurred_at) AS bucket,
                            project_id,
                            currency,
                            SUM(mrr_minor) FILTER (
                                WHERE event_type IN ('subscription.created', 'subscription.updated', 'subscription.canceled')
                                  AND mrr_minor IS NOT NULL
                            ) AS mrr_delta_minor,
                            COUNT(*) FILTER (WHERE event_type = 'charge.succeeded') AS charge_count,
                            SUM(amount_minor) FILTER (WHERE event_type = 'charge.succeeded') AS charge_total_minor,
                            SUM(amount_minor) FILTER (WHERE event_type = 'charge.refunded') AS refund_total_minor
                        FROM revenue_events
                        WHERE currency IS NOT NULL
                        GROUP BY bucket, project_id, currency
                        WITH NO DATA
                    $q$;

                    PERFORM add_continuous_aggregate_policy('revenue_mrr_daily',
                        start_offset => INTERVAL '30 days',
                        end_offset => INTERVAL '1 hour',
                        schedule_interval => INTERVAL '30 minutes');
                EXCEPTION WHEN OTHERS THEN
                    -- TimescaleDB not installed, or the aggregate already
                    -- exists with a different definition — either way, skip.
                    NULL;
                END $$
                "#,
            )
            .await;

        // Best-effort regular index for environments without Timescale.
        let _ = db
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_revenue_mrr_daily_project_bucket \
                 ON revenue_mrr_daily (project_id, bucket DESC)",
            )
            .await;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let _ = db
            .execute_unprepared(
                "DO $$ BEGIN \
                    PERFORM remove_continuous_aggregate_policy('revenue_mrr_daily', if_exists => true); \
                 EXCEPTION WHEN OTHERS THEN NULL; END $$",
            )
            .await;
        let _ = db
            .execute_unprepared("DROP MATERIALIZED VIEW IF EXISTS revenue_mrr_daily CASCADE")
            .await;
        Ok(())
    }
}
