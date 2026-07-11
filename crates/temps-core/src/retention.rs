//! Extension point for per-project ClickHouse retention resolution.
//!
//! OSS registers [`FixedRetentionResolver`] at startup, which always returns
//! the ClickHouse table default. A plugin (e.g. one implementing per-project
//! data retention policies) can supply an alternative implementation — callers
//! pass an `Arc<dyn RetentionResolver>` received at construction time and the
//! plugin overrides the default by registering its implementation via the
//! service registry before the storage backends are wired up.
//!
//! `resolve` is called on the ingest path (once per row in a batch, not once
//! per HTTP request) and must be synchronous and lock-free on the read path.
//! Any expensive lookup (Postgres, external service) must be driven by a
//! background refresh task in the implementing type; the result must be cached
//! so that individual `resolve` calls do no I/O.

/// Which ClickHouse table a retention-days resolution is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetentionTable {
    /// OTel spans table (`spans`). Default TTL: 90 days.
    Spans,
    /// Proxy / request-log table (`proxy_logs`). Default TTL: 30 days.
    ProxyLogs,
}

impl RetentionTable {
    /// The table-level TTL constant in days.
    ///
    /// Matches the `DEFAULT` in the `ADD COLUMN … retention_days` migration
    /// and the prior hardcoded `INTERVAL` in the original DDL. Used by
    /// [`FixedRetentionResolver`] and as a fallback for rows that have no
    /// project context (e.g. unrouted proxy requests).
    pub fn default_days(self) -> u16 {
        match self {
            Self::Spans => 90,
            Self::ProxyLogs => 30,
        }
    }
}

/// Extension point for resolving the effective `retention_days` to stamp onto
/// an ingested ClickHouse row.
///
/// OSS registers [`FixedRetentionResolver`] at startup, which always returns
/// [`RetentionTable::default_days`] regardless of `project_id`. A plugin (e.g.
/// one implementing per-project data retention policies) registers an
/// implementation via the service registry — `context.register_service(resolver)`
/// — only when appropriate (e.g. gated by its own licensing check). Storage
/// backends receive the resolver at construction time as
/// `Arc<dyn RetentionResolver>`, so a plugin-free binary uses the fixed
/// default unconditionally and the ClickHouse rows self-expire at the
/// table-level TTL without any configuration.
pub trait RetentionResolver: Send + Sync {
    /// Return the effective `retention_days` for `project_id` in `table`.
    ///
    /// The returned value is written into the `retention_days` column of each
    /// new row, where it drives the per-row TTL expression
    /// `toDateTime(<time_col>) + toIntervalDay(retention_days)`.
    ///
    /// Implementations must be synchronous and must not perform I/O — see the
    /// module-level note.
    fn resolve(&self, project_id: i32, table: RetentionTable) -> u16;
}

/// Default [`RetentionResolver`] that returns the ClickHouse table-level
/// default for every project.
///
/// Registered at startup when no overriding implementation is present.
/// Callers with no project context (e.g. unrouted proxy requests where
/// `project_id` is `NULL`) should use [`RetentionTable::default_days`]
/// directly rather than passing a fabricated project ID to this resolver.
pub struct FixedRetentionResolver;

impl RetentionResolver for FixedRetentionResolver {
    fn resolve(&self, _project_id: i32, table: RetentionTable) -> u16 {
        table.default_days()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_resolver_returns_table_defaults() {
        let r = FixedRetentionResolver;
        assert_eq!(r.resolve(1, RetentionTable::Spans), 90);
        assert_eq!(r.resolve(1, RetentionTable::ProxyLogs), 30);
        // project_id is ignored
        assert_eq!(r.resolve(99999, RetentionTable::Spans), 90);
        assert_eq!(r.resolve(0, RetentionTable::ProxyLogs), 30);
    }

    #[test]
    fn retention_table_default_days() {
        assert_eq!(RetentionTable::Spans.default_days(), 90);
        assert_eq!(RetentionTable::ProxyLogs.default_days(), 30);
    }
}
