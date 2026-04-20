//! Analytics service: read-only projections for the dashboard.
//!
//! Everything here is project-scoped — `project_id` is always the first
//! predicate so a tenant can never read another tenant's numbers.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, QuerySelect, Statement,
};
use serde::Serialize;
use thiserror::Error;

use temps_entities::{revenue_customers_state, revenue_events, revenue_subscriptions_state};

#[derive(Debug, Error)]
pub enum AnalyticsError {
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),
    #[error("Invalid bucket size: {0}")]
    InvalidBucket(String),
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum Bucket {
    Day,
    Week,
    Month,
}

impl Bucket {
    fn pg_interval(&self) -> &'static str {
        match self {
            Bucket::Day => "1 day",
            Bucket::Week => "1 week",
            Bucket::Month => "1 month",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AnalyticsError> {
        match value {
            "day" | "daily" => Ok(Bucket::Day),
            "week" | "weekly" => Ok(Bucket::Week),
            "month" | "monthly" => Ok(Bucket::Month),
            other => Err(AnalyticsError::InvalidBucket(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSummary {
    pub currency: String,
    pub current_mrr_minor: i64,
    pub current_arr_minor: i64,
    pub active_subscriptions: i64,
    pub active_customers: i64,
    pub churned_last_30d: i64,
    pub arpu_minor: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MrrBucket {
    pub bucket: DateTime<Utc>,
    pub mrr_minor: i64,
    pub charge_total_minor: i64,
    pub refund_total_minor: i64,
    pub charge_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomerMovement {
    pub bucket: DateTime<Utc>,
    pub new_customers: i64,
    pub churned_customers: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentEvent {
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub customer_ref: Option<String>,
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
    pub mrr_minor: Option<i64>,
}

pub struct RevenueAnalyticsService {
    db: Arc<DatabaseConnection>,
}

impl RevenueAnalyticsService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    pub async fn summary(
        &self,
        project_id: i32,
        currency: &str,
    ) -> Result<MetricsSummary, AnalyticsError> {
        let currency = currency.to_lowercase();

        let subs = revenue_subscriptions_state::Entity::find()
            .filter(revenue_subscriptions_state::Column::ProjectId.eq(project_id))
            .filter(revenue_subscriptions_state::Column::Currency.eq(&currency))
            .all(self.db.as_ref())
            .await?;

        let mut mrr: i64 = 0;
        let mut active_subs: i64 = 0;
        for s in &subs {
            if matches!(s.status.as_str(), "active" | "trialing" | "past_due") {
                mrr += s.mrr_minor;
                active_subs += 1;
            }
        }

        // Customers with at least one active sub in this currency.
        let customers = revenue_customers_state::Entity::find()
            .filter(revenue_customers_state::Column::ProjectId.eq(project_id))
            .filter(revenue_customers_state::Column::ChurnedAt.is_null())
            .all(self.db.as_ref())
            .await?;
        let active_customers = customers.len() as i64;

        // Last-30d churn (customers with churned_at in the window)
        let thirty_days_ago = Utc::now() - chrono::Duration::days(30);
        let churned_last_30d = revenue_customers_state::Entity::find()
            .filter(revenue_customers_state::Column::ProjectId.eq(project_id))
            .filter(revenue_customers_state::Column::ChurnedAt.gte(thirty_days_ago))
            .all(self.db.as_ref())
            .await?
            .len() as i64;

        let arpu = if active_customers > 0 {
            mrr / active_customers
        } else {
            0
        };

        Ok(MetricsSummary {
            currency,
            current_mrr_minor: mrr,
            current_arr_minor: mrr.saturating_mul(12),
            active_subscriptions: active_subs,
            active_customers,
            churned_last_30d,
            arpu_minor: arpu,
        })
    }

    /// Time-bucketed MRR + charge/refund totals.
    ///
    /// We bucket from the raw `revenue_events` table rather than the
    /// continuous aggregate so vanilla Postgres installs work too. On
    /// TimescaleDB the planner rewrites time_bucket to the hypertable
    /// chunks automatically.
    pub async fn mrr_timeseries(
        &self,
        project_id: i32,
        currency: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: Bucket,
    ) -> Result<Vec<MrrBucket>, AnalyticsError> {
        #[derive(FromQueryResult)]
        struct Row {
            bucket: DateTime<Utc>,
            mrr_minor: Option<i64>,
            charge_total_minor: Option<i64>,
            refund_total_minor: Option<i64>,
            charge_count: Option<i64>,
        }

        // MRR stacks two independent streams so metered/tiered/hybrid
        // subscriptions show up correctly:
        //
        //   1. `subscription.*` events — flat-fee subs whose MRR lives in
        //      the subscription object itself. `filter_events` zeroes the
        //      mrr_minor for metered subs when `metered_mode =
        //      derive_from_invoices`, so there's no double counting.
        //
        //   2. `mrr.realized` events — synthetic per-invoice-line events
        //      with `occurred_at = period.start`. The mrr_minor on these
        //      is the line's realized MRR (`amount / period_days * 30`).
        //      Summed into the same bucket as their period_start, they
        //      paint the invoice-backed MRR curve.
        //
        // Both streams carry mrr_minor in the same currency; summing them
        // together is safe because the `filter_events` step guarantees
        // no row contributes to both streams for the same subscription.
        let sql = format!(
            r#"
            SELECT
                date_trunc('{granularity}', occurred_at) AS bucket,
                SUM(mrr_minor) FILTER (
                    WHERE event_type IN (
                        'subscription.created',
                        'subscription.updated',
                        'subscription.canceled',
                        'mrr.realized'
                    )
                )::bigint AS mrr_minor,
                SUM(amount_minor) FILTER (WHERE event_type = 'charge.succeeded')::bigint AS charge_total_minor,
                SUM(amount_minor) FILTER (WHERE event_type = 'charge.refunded')::bigint AS refund_total_minor,
                COUNT(*) FILTER (WHERE event_type = 'charge.succeeded')::bigint AS charge_count
            FROM revenue_events
            WHERE project_id = $1
              AND currency = $2
              AND occurred_at >= $3
              AND occurred_at <= $4
            GROUP BY bucket
            ORDER BY bucket ASC
            "#,
            granularity = match bucket {
                Bucket::Day => "day",
                Bucket::Week => "week",
                Bucket::Month => "month",
            }
        );
        // interval usage lives in Bucket::pg_interval for the continuous
        // aggregate; date_trunc is what we use against raw events.
        let _ = bucket.pg_interval();

        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            [
                project_id.into(),
                currency.to_lowercase().into(),
                from.into(),
                to.into(),
            ],
        );

        let rows = Row::find_by_statement(stmt).all(self.db.as_ref()).await?;
        Ok(rows
            .into_iter()
            .map(|r| MrrBucket {
                bucket: r.bucket,
                mrr_minor: r.mrr_minor.unwrap_or(0),
                charge_total_minor: r.charge_total_minor.unwrap_or(0),
                refund_total_minor: r.refund_total_minor.unwrap_or(0),
                charge_count: r.charge_count.unwrap_or(0),
            })
            .collect())
    }

    pub async fn customer_movement(
        &self,
        project_id: i32,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: Bucket,
    ) -> Result<Vec<CustomerMovement>, AnalyticsError> {
        #[derive(FromQueryResult)]
        struct Row {
            bucket: DateTime<Utc>,
            new_customers: Option<i64>,
            churned_customers: Option<i64>,
        }

        let granularity = match bucket {
            Bucket::Day => "day",
            Bucket::Week => "week",
            Bucket::Month => "month",
        };

        let sql = format!(
            r#"
            WITH events AS (
                SELECT date_trunc('{granularity}', first_seen_at) AS bucket, 1 AS n, 0 AS c
                FROM revenue_customers_state
                WHERE project_id = $1 AND first_seen_at BETWEEN $2 AND $3
                UNION ALL
                SELECT date_trunc('{granularity}', churned_at) AS bucket, 0, 1
                FROM revenue_customers_state
                WHERE project_id = $1 AND churned_at IS NOT NULL
                  AND churned_at BETWEEN $2 AND $3
            )
            SELECT bucket,
                   SUM(n)::bigint AS new_customers,
                   SUM(c)::bigint AS churned_customers
            FROM events
            GROUP BY bucket
            ORDER BY bucket ASC
            "#
        );

        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            [project_id.into(), from.into(), to.into()],
        );

        let rows = Row::find_by_statement(stmt).all(self.db.as_ref()).await?;
        Ok(rows
            .into_iter()
            .map(|r| CustomerMovement {
                bucket: r.bucket,
                new_customers: r.new_customers.unwrap_or(0),
                churned_customers: r.churned_customers.unwrap_or(0),
            })
            .collect())
    }

    pub async fn recent_events(
        &self,
        project_id: i32,
        limit: u64,
    ) -> Result<Vec<RecentEvent>, AnalyticsError> {
        let rows = revenue_events::Entity::find()
            .filter(revenue_events::Column::ProjectId.eq(project_id))
            .order_by_desc(revenue_events::Column::OccurredAt)
            .limit(limit.clamp(1, 200))
            .all(self.db.as_ref())
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| RecentEvent {
                occurred_at: r.occurred_at,
                event_type: r.event_type,
                customer_ref: r.customer_ref,
                amount_minor: r.amount_minor,
                currency: r.currency,
                mrr_minor: r.mrr_minor,
            })
            .collect())
    }
}
