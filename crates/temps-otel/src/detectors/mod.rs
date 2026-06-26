//! Typed, polymorphic detector definitions for metric alert rules.
//!
//! A rule's detector lives in `metric_alert_rules.detection_config` (jsonb). The
//! raw `serde_json::Value` exists ONLY on the sea-orm entity column; every
//! service and DTO layer uses the typed [`DetectionConfig`] here — mirroring the
//! sanctioned `temps_revenue::providers::ProviderConfig` +
//! `revenue_integrations.config` precedent. This satisfies the "never expose
//! untyped `serde_json::Value` on the API surface" rule: the API surface is
//! 100% typed; only the storage column is a `Value`.
//!
//! Forward-compatibility is the whole point: a new detector family
//! (anomaly→EWMA→forecast→outlier→auto-watch) is a **new enum variant + a
//! `validate` arm + an evaluator branch + `openapi-ts` regen** — never a
//! migration. `detection_kind` is a plain `String` column (not a Postgres enum),
//! so a new kind also needs no `ALTER TYPE`.
//!
//! Tagging follows the `ProviderConfig` precedent *verbatim*: internally tagged
//! by `kind`, with NO `#[schema(discriminator)]` (which is a compile error
//! alongside `serde(tag)` in utoipa 5.x). utoipa + hey-api render this as a
//! usable `(StaticParams & { kind: 'static' }) | …` TS discriminated union.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::OtelError;

fn default_deviations() -> f64 {
    3.0
}
fn default_pct_anomalous() -> f64 {
    1.0
}

/// The typed detector definition stored (as jsonb) in
/// `metric_alert_rules.detection_config`.
///
/// Today only [`DetectionConfig::Static`] is evaluable; the other variants are
/// schema-present (so the SDK/UI and storage are already future-shaped) but
/// rejected by [`DetectionConfig::validate`] until their evaluator lands. Each is
/// then enabled code-only, with no schema migration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetectionConfig {
    /// v0 (shipping): static threshold comparison of the aggregated value.
    Static(StaticParams),
    /// Seasonal anomaly band (basic/agile/robust/ewma share this variant — the
    /// algorithm is a field, not a new kind). Creation rejected until evaluated.
    Anomaly(AnomalyParams),
    /// Predict a future threshold breach (capacity planning). Stub.
    Forecast(ForecastParams),
    /// Cross-series population outlier (one host misbehaving vs its peers). Stub.
    Outlier(OutlierParams),
    /// Watchdog-style self-tuning auto-watch (engine picks bounds). Stub.
    AutoWatch(AutoWatchParams),
}

/// Static threshold detector: compare the aggregated `value` against `threshold`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StaticParams {
    /// How `value` is compared against `threshold`.
    pub comparator: Comparator,
    /// The threshold the aggregated value is compared against.
    pub threshold: f64,
}

/// Seasonal anomaly-band detector parameters (stub — not yet evaluated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AnomalyParams {
    /// Baseline model. `robust` is the default (seasonal, stable, flags level
    /// shifts); `ewma`/`agile` adopt level shifts; `basic` is non-seasonal.
    #[serde(default)]
    pub algorithm: AnomalyAlgorithm,
    /// Band width in robust standard deviations (Datadog's `bounds`).
    #[serde(default = "default_deviations")]
    pub deviations: f64,
    /// Which side(s) of the band a deviation must be on to count.
    #[serde(default)]
    pub direction: Direction,
    /// Seasonality model for the baseline.
    #[serde(default)]
    pub seasonality: Seasonality,
    /// Fraction (0..=1) of points in the window that must be anomalous to fire.
    #[serde(default = "default_pct_anomalous")]
    pub pct_anomalous: f64,
    /// How far back to build the baseline. `None` = an evaluator default.
    #[serde(default)]
    pub baseline_lookback_days: Option<i32>,
}

/// Forecast detector parameters (stub — not yet evaluated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ForecastParams {
    #[serde(default)]
    pub algorithm: ForecastAlgorithm,
    /// How far ahead to project before checking the breach condition.
    pub forecast_horizon_secs: i32,
    #[serde(default = "default_deviations")]
    pub deviations: f64,
    /// Comparator + threshold the *forecast* is checked against.
    pub comparator: Comparator,
    pub threshold: f64,
}

/// Outlier (cross-series population) detector parameters (stub — not evaluated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OutlierParams {
    #[serde(default)]
    pub algorithm: OutlierAlgorithm,
    /// Sensitivity; higher tolerates larger spread before flagging.
    #[serde(default = "default_deviations")]
    pub tolerance: f64,
    /// Label key defining the peer population compared across series (e.g. `host`).
    pub peer_group_key: String,
}

/// Auto-watch (Watchdog-style) detector parameters (stub — not evaluated).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AutoWatchParams {
    /// The engine self-tunes the band; the user supplies only the direction.
    #[serde(default)]
    pub direction: Direction,
}

/// Comparator for static/forecast threshold detectors. Serializes to the
/// keyword forms `gt|gte|lt|lte` (NOT the SQL operators used by
/// `temps-monitoring::compare`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Comparator {
    #[default]
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Comparator {
    /// Whether `value` breaches `threshold` under this comparator.
    pub fn breaches(self, value: f64, threshold: f64) -> bool {
        match self {
            Comparator::Gt => value > threshold,
            Comparator::Gte => value >= threshold,
            Comparator::Lt => value < threshold,
            Comparator::Lte => value <= threshold,
        }
    }

    /// A human-readable operator symbol for alarm messages.
    pub fn symbol(self) -> &'static str {
        match self {
            Comparator::Gt => ">",
            Comparator::Gte => ">=",
            Comparator::Lt => "<",
            Comparator::Lte => "<=",
        }
    }
}

/// Which side(s) of an anomaly band count as a deviation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Both,
    Above,
    Below,
}

/// Seasonality model for an anomaly baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Seasonality {
    #[default]
    None,
    Hourly,
    Daily,
    Weekly,
}

/// Anomaly baseline algorithm. Adding one (e.g. a new robust variant) is a
/// code-only enum addition — no migration, since it lives inside the blob.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyAlgorithm {
    #[default]
    Robust,
    Basic,
    Agile,
    Ewma,
}

/// Forecast model family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForecastAlgorithm {
    #[default]
    Linear,
    Seasonal,
}

/// Outlier detection algorithm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutlierAlgorithm {
    #[default]
    Dbscan,
    ScaledDbscan,
    Mad,
    ScaledMad,
}

impl DetectionConfig {
    /// The `detection_kind` column value mirroring this variant's serde tag.
    pub fn kind_str(&self) -> &'static str {
        match self {
            DetectionConfig::Static(_) => "static",
            DetectionConfig::Anomaly(_) => "anomaly",
            DetectionConfig::Forecast(_) => "forecast",
            DetectionConfig::Outlier(_) => "outlier",
            DetectionConfig::AutoWatch(_) => "auto_watch",
        }
    }

    /// An infallible fallback used when a *stored* blob fails to deserialize.
    /// Only reachable on DB corruption — every write is validated and round-
    /// tripped through this type, so a persisted blob is always well-formed.
    pub fn default_static() -> Self {
        DetectionConfig::Static(StaticParams {
            comparator: Comparator::Gt,
            threshold: 0.0,
        })
    }

    /// Deserialize from the stored jsonb value (typed error on malformed input).
    pub fn from_value(value: &serde_json::Value) -> Result<Self, OtelError> {
        serde_json::from_value(value.clone()).map_err(|e| OtelError::Validation {
            message: format!("invalid detection_config: {e}"),
        })
    }

    /// Serialize to a jsonb value for persistence.
    pub fn to_value(&self) -> Result<serde_json::Value, OtelError> {
        serde_json::to_value(self).map_err(|e| OtelError::Validation {
            message: format!("failed to serialize detection_config: {e}"),
        })
    }

    /// Validate the detector's invariants. Currently only `static` rules are
    /// evaluable; the other (typed, schema-present) kinds are rejected at
    /// create/update time until their evaluator lands — enabling each later is
    /// code-only, never a migration.
    pub fn validate(&self) -> Result<(), OtelError> {
        match self {
            DetectionConfig::Static(p) => {
                if !p.threshold.is_finite() {
                    return Err(OtelError::Validation {
                        message: "threshold must be a finite number".to_string(),
                    });
                }
                Ok(())
            }
            other => Err(OtelError::Validation {
                message: format!("detector kind '{}' is not yet supported", other.kind_str()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_round_trip() {
        let cfg = DetectionConfig::Static(StaticParams {
            comparator: Comparator::Gte,
            threshold: 500.0,
        });
        let v = cfg.to_value().unwrap();
        // Internally tagged: the discriminator + the variant's fields are flat.
        assert_eq!(v["kind"], "static");
        assert_eq!(v["comparator"], "gte");
        assert_eq!(v["threshold"], 500.0);
        let back = DetectionConfig::from_value(&v).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.kind_str(), "static");
    }

    #[test]
    fn test_anomaly_round_trip_with_defaults() {
        // A minimal anomaly blob (only the required-by-shape fields) fills the
        // rest from serde defaults — proving additive fields are forward-compat.
        let v = serde_json::json!({ "kind": "anomaly" });
        let cfg = DetectionConfig::from_value(&v).unwrap();
        match &cfg {
            DetectionConfig::Anomaly(p) => {
                assert_eq!(p.algorithm, AnomalyAlgorithm::Robust);
                assert_eq!(p.deviations, 3.0);
                assert_eq!(p.direction, Direction::Both);
                assert_eq!(p.seasonality, Seasonality::None);
                assert_eq!(p.pct_anomalous, 1.0);
                assert_eq!(p.baseline_lookback_days, None);
            }
            _ => panic!("expected anomaly"),
        }
        assert_eq!(cfg.kind_str(), "anomaly");
    }

    #[test]
    fn test_validate_static_ok() {
        let cfg = DetectionConfig::Static(StaticParams {
            comparator: Comparator::Lt,
            threshold: 1.0,
        });
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_static_rejects_non_finite_threshold() {
        let cfg = DetectionConfig::Static(StaticParams {
            comparator: Comparator::Gt,
            threshold: f64::INFINITY,
        });
        assert!(matches!(cfg.validate(), Err(OtelError::Validation { .. })));
    }

    #[test]
    fn test_validate_rejects_unsupported_kinds() {
        // The non-static kinds are typed (schema-present) but not yet evaluable.
        let anomaly =
            DetectionConfig::from_value(&serde_json::json!({ "kind": "anomaly" })).unwrap();
        assert!(matches!(
            anomaly.validate(),
            Err(OtelError::Validation { .. })
        ));
        let auto = DetectionConfig::AutoWatch(AutoWatchParams::default());
        assert!(matches!(auto.validate(), Err(OtelError::Validation { .. })));
    }

    #[test]
    fn test_comparator_breaches() {
        assert!(Comparator::Gt.breaches(600.0, 500.0));
        assert!(!Comparator::Gt.breaches(500.0, 500.0));
        assert!(Comparator::Gte.breaches(500.0, 500.0));
        assert!(Comparator::Lt.breaches(400.0, 500.0));
        assert!(!Comparator::Lt.breaches(500.0, 500.0));
        assert!(Comparator::Lte.breaches(500.0, 500.0));
    }

    #[test]
    fn test_unknown_kind_hard_fails() {
        // An unknown top-level kind must NOT silently degrade — it fails to parse.
        let v = serde_json::json!({ "kind": "telepathy", "threshold": 1.0 });
        assert!(DetectionConfig::from_value(&v).is_err());
    }

    #[test]
    fn test_default_static_fallback() {
        let cfg = DetectionConfig::default_static();
        assert_eq!(cfg.kind_str(), "static");
        assert!(cfg.validate().is_ok());
    }
}
