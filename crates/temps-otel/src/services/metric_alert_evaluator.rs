//! Background evaluator for first-class metric alert rules.
//!
//! On a fixed interval (~30s) the evaluator scans every enabled rule, queries the
//! latest aggregated bucket for the rule's metric over its window via the same
//! `OtelService::query_metrics` path the explorer uses, compares the value against
//! the threshold, and tracks an `ok <-> firing` state machine. A breach only
//! transitions `ok -> firing` after it has persisted for `for_duration_secs`.
//!
//! Firing/resolving reuses `temps_monitoring::AlarmService` (the same path the
//! monitoring evaluator uses): it inserts the alarm row, enforces a cooldown,
//! fans out the notification to Slack/webhook/email, and emits a `Job::AlarmFired`.
//! We do NOT build a new notifier.
//!
//! Resilience: a query failure for one rule logs and continues; the loop never
//! panics. `for_duration` is approximated with an in-memory `breach_start` map
//! (lost on restart, matching the monitoring evaluator's approach), but
//! `last_state`/`last_value`/`last_evaluated_at` are persisted so the UI badge
//! survives restarts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use temps_monitoring::{AlarmService, AlarmSeverity, AlarmType, FireAlarmRequest};

use crate::detectors::DetectionConfig;
use crate::services::metric_alert_service::MetricAlertService;
use crate::services::OtelService;
use crate::types::{MetricAggregation, MetricBucket, MetricQuery};
use temps_entities::metric_alert_rules::Model as AlertRule;

/// How often the evaluator scans enabled rules.
const EVAL_INTERVAL_SECS: u64 = 30;

/// The state transition the evaluator should apply for a single rule this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertTransition {
    /// Not breaching, was already ok: nothing to do.
    StayOk,
    /// Newly breaching (or still breaching) but `for_duration` not yet elapsed.
    StartBreach,
    /// Breach has persisted long enough: transition ok -> firing.
    FireNow,
    /// Already firing and still breaching: nothing to do.
    StayFiring,
    /// Was firing, now recovered: transition firing -> ok.
    Resolve,
}

/// Pure state-machine function (no DB, no I/O) — unit-testable.
///
/// `prev_state` is the persisted `last_state` (`ok|firing|unknown`).
/// `breaching` is the result of [`compare`]. `breach_elapsed_secs` is how long the
/// current breach has persisted (0 if not breaching). Once the breach has
/// persisted for `>= for_duration_secs`, `ok -> firing` fires.
pub fn evaluate_transition(
    prev_state: &str,
    breaching: bool,
    breach_elapsed_secs: u64,
    for_duration_secs: u64,
) -> AlertTransition {
    let was_firing = prev_state == "firing";
    if breaching {
        if was_firing {
            AlertTransition::StayFiring
        } else if breach_elapsed_secs >= for_duration_secs {
            AlertTransition::FireNow
        } else {
            AlertTransition::StartBreach
        }
    } else if was_firing {
        AlertTransition::Resolve
    } else {
        AlertTransition::StayOk
    }
}

/// The value to evaluate for a rule against its threshold.
///
/// For percentile aggregations on a histogram metric, the quantile is computed
/// from the explicit bucket layout (`histogram_summary`) — the server's scalar
/// `value` for a percentile is the quantile of the synthetic per-point means,
/// not the true distribution. All other cases use the server-computed `value`.
fn value_for_rule(latest: &MetricBucket, aggregation: MetricAggregation) -> f64 {
    if let MetricAggregation::Quantile(q) = aggregation {
        if let Some(hs) = &latest.histogram_summary {
            if !hs.bounds.is_empty() {
                return histogram_quantile(&hs.bounds, &hs.bucket_counts, q);
            }
        }
    }
    latest.value
}

/// Quantile from an explicit-bucket histogram via linear interpolation within
/// the bucket crossing the target rank (same method as the frontend). `bounds`
/// are ascending upper bounds; `counts` has length `bounds.len() + 1`.
fn histogram_quantile(bounds: &[f64], counts: &[u64], q: f64) -> f64 {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let target = q.clamp(0.0, 1.0) * total as f64;
    let mut cumulative: u64 = 0;
    for (i, &c) in counts.iter().enumerate() {
        let next = cumulative + c;
        if next as f64 >= target && c > 0 {
            let lower = if i == 0 { 0.0 } else { bounds[i - 1] };
            let upper = if i < bounds.len() {
                bounds[i]
            } else {
                bounds.last().copied().unwrap_or(lower)
            };
            let within = (target - cumulative as f64) / c as f64;
            return lower + (upper - lower) * within;
        }
        cumulative = next;
    }
    bounds.last().copied().unwrap_or(0.0)
}

/// Map a rule severity string to an `AlarmSeverity`.
fn map_severity(severity: &str) -> AlarmSeverity {
    match severity {
        "critical" => AlarmSeverity::Critical,
        "info" => AlarmSeverity::Info,
        _ => AlarmSeverity::Warning,
    }
}

/// Background task that evaluates enabled metric alert rules and fires/resolves
/// notifications via the reused alarm system.
pub struct MetricAlertEvaluator {
    alert_service: Arc<MetricAlertService>,
    otel_service: Arc<OtelService>,
    alarm_service: Arc<AlarmService>,
    /// rule_id -> when the current breach started (for `for_duration` tracking).
    breach_start: Arc<RwLock<HashMap<i32, Instant>>>,
    /// rule_id -> alarm_id, for resolving the matching alarm on recovery.
    firing: Arc<RwLock<HashMap<i32, i32>>>,
}

impl MetricAlertEvaluator {
    pub fn new(
        alert_service: Arc<MetricAlertService>,
        otel_service: Arc<OtelService>,
        alarm_service: Arc<AlarmService>,
    ) -> Self {
        Self {
            alert_service,
            otel_service,
            alarm_service,
            breach_start: Arc::new(RwLock::new(HashMap::new())),
            firing: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Run the evaluator loop forever. Skips the immediate first tick.
    pub async fn run(&self) {
        info!("Starting OTel metric alert evaluator");
        let mut interval = tokio::time::interval(Duration::from_secs(EVAL_INTERVAL_SECS));
        interval.tick().await; // discard the immediate first tick
        loop {
            interval.tick().await;
            if let Err(e) = self.run_cycle().await {
                error!(error = %e, "OTel metric alert evaluation cycle failed");
            }
        }
    }

    /// One evaluation cycle: load enabled rules, evaluate each independently.
    async fn run_cycle(&self) -> Result<(), crate::error::OtelError> {
        let rules = self.alert_service.list_enabled().await?;
        debug!(rule_count = rules.len(), "Evaluating metric alert rules");
        for rule in rules {
            let rule_id = rule.id;
            // A single rule's failure must never abort the loop.
            if let Err(e) = self.evaluate_rule(rule).await {
                error!(rule_id, error = %e, "Metric alert rule evaluation failed (continuing)");
            }
        }
        Ok(())
    }

    /// Evaluate one rule: query its latest aggregated value, compare, run the
    /// state machine, fire/resolve as needed, and persist the observed state.
    async fn evaluate_rule(&self, rule: AlertRule) -> Result<(), crate::error::OtelError> {
        let now = Utc::now();
        let window = chrono::Duration::seconds(rule.window_secs.max(1) as i64);
        let aggregation = MetricAggregation::parse(&rule.aggregation);
        let query = MetricQuery {
            project_id: rule.project_id,
            metric_name: Some(rule.metric_name.clone()),
            start_time: Some(now - window),
            end_time: Some(now),
            bucket_interval: Some(format!("{}s", rule.window_secs.max(1))),
            // The window may straddle two epoch-aligned buckets; fetch both (the
            // query returns them ASC) and take the latest below. A limit of 1
            // would return the OLDEST bucket, evaluating stale data.
            limit: Some(2),
            aggregation,
            ..Default::default()
        };

        let buckets = self.otel_service.query_metrics(query).await?;

        // No data in the window: PRESERVE the current state and any open alarm.
        // Do NOT flip a firing rule to `unknown` (which would orphan its alarm),
        // do NOT clobber last_value, and do NOT reset the breach timer — just
        // record that we evaluated.
        let Some(latest) = buckets.last() else {
            if let Err(e) = self
                .alert_service
                .persist_evaluation(rule.id, &rule.last_state, rule.last_value, now)
                .await
            {
                warn!(rule_id = rule.id, error = %e, "Failed to persist no-data evaluation");
            }
            return Ok(());
        };

        // For histogram percentile rules, compute the quantile from the bucket
        // layout — the server's scalar `value` is the percentile of synthetic
        // per-point means, not the true distribution (same reason the explorer
        // recomputes percentiles client-side).
        let value = value_for_rule(latest, aggregation);

        // Decode the typed detector. A corrupt blob fails this rule's cycle (the
        // outer loop logs and continues) rather than evaluating it incorrectly.
        let config = DetectionConfig::from_value(&rule.detection_config)?;
        let breaching = match &config {
            DetectionConfig::Static(p) => p.comparator.breaches(value, p.threshold),
            // Non-static detectors are typed/schema-present but not yet evaluable
            // (creation is rejected by the service). Defensive guard for any rule
            // that predates support: preserve state and skip without firing.
            other => {
                debug!(
                    rule_id = rule.id,
                    kind = other.kind_str(),
                    "Skipping rule: detector kind not yet evaluable"
                );
                if let Err(e) = self
                    .alert_service
                    .persist_evaluation(rule.id, &rule.last_state, rule.last_value, now)
                    .await
                {
                    warn!(rule_id = rule.id, error = %e, "Failed to persist skipped evaluation");
                }
                return Ok(());
            }
        };

        // Track breach duration (in-memory, approximated like temps-monitoring).
        let breach_elapsed_secs = if breaching {
            let mut starts = self.breach_start.write().await;
            let start = starts.entry(rule.id).or_insert_with(Instant::now);
            start.elapsed().as_secs()
        } else {
            0
        };

        let transition = evaluate_transition(
            &rule.last_state,
            breaching,
            breach_elapsed_secs,
            rule.for_duration_secs.max(0) as u64,
        );

        let next_state = match transition {
            AlertTransition::StayOk | AlertTransition::StartBreach => "ok",
            AlertTransition::StayFiring => "firing",
            AlertTransition::FireNow => {
                self.fire(&rule, value, &config).await;
                "firing"
            }
            AlertTransition::Resolve => {
                self.resolve(&rule).await;
                self.clear_breach(rule.id).await;
                "ok"
            }
        };

        if let Err(e) = self
            .alert_service
            .persist_evaluation(rule.id, next_state, Some(value), now)
            .await
        {
            warn!(rule_id = rule.id, error = %e, "Failed to persist rule evaluation");
        }
        Ok(())
    }

    /// Fire an alarm via the reused alarm system and remember the alarm id.
    async fn fire(&self, rule: &AlertRule, value: f64, config: &DetectionConfig) {
        // The condition string + threshold come from the typed detector. Only
        // static rules fire today; the fallback keeps the match exhaustive.
        let (comparator, threshold) = match config {
            DetectionConfig::Static(p) => (p.comparator.symbol(), p.threshold),
            _ => ("?", f64::NAN),
        };
        let request = FireAlarmRequest {
            project_id: rule.project_id,
            environment_id: None,
            deployment_id: None,
            container_id: None,
            service_id: None,
            alarm_type: AlarmType::DeploymentMetricThreshold,
            severity: map_severity(&rule.severity),
            title: format!("Metric threshold breached: {}", rule.name),
            message: format!(
                "{} {} is {:.3} (threshold {} {:.3})",
                rule.metric_name, rule.aggregation, value, comparator, threshold
            ),
            metadata: Some(json!({
                "rule_id": rule.id,
                "metric_name": rule.metric_name,
                "aggregation": rule.aggregation,
                "value": value,
                "threshold": threshold,
                "comparator": comparator,
                "detection_kind": config.kind_str(),
                "window_secs": rule.window_secs,
                "for_duration_secs": rule.for_duration_secs,
                "source": "otel_metric_alert",
            })),
        };
        match self.alarm_service.fire_alarm(request).await {
            Ok(Some(alarm_id)) => {
                info!(rule_id = rule.id, alarm_id, "OTel metric alert fired");
                self.firing.write().await.insert(rule.id, alarm_id);
            }
            Ok(None) => {
                // Suppressed by cooldown — still mark firing so we resolve later.
                debug!(
                    rule_id = rule.id,
                    "OTel metric alert fire suppressed by cooldown"
                );
            }
            Err(e) => {
                error!(rule_id = rule.id, error = %e, "Failed to fire OTel metric alert");
            }
        }
    }

    /// Resolve the alarm previously fired for this rule, if any.
    async fn resolve(&self, rule: &AlertRule) {
        let alarm_id = self.firing.write().await.remove(&rule.id);
        if let Some(alarm_id) = alarm_id {
            if let Err(e) = self
                .alarm_service
                .resolve_alarm(alarm_id, rule.project_id)
                .await
            {
                error!(
                    rule_id = rule.id,
                    alarm_id,
                    error = %e,
                    "Failed to resolve OTel metric alert"
                );
            } else {
                info!(rule_id = rule.id, alarm_id, "OTel metric alert resolved");
            }
        }
    }

    /// Drop any in-flight breach timer for a rule.
    async fn clear_breach(&self, rule_id: i32) {
        self.breach_start.write().await.remove(&rule_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transition_ok_to_firing_requires_for_duration() {
        // Breaching but not long enough yet -> StartBreach (stays effectively ok).
        let t = evaluate_transition("ok", true, 30, 120);
        assert_eq!(t, AlertTransition::StartBreach);

        // Breach has persisted past for_duration -> FireNow.
        let t = evaluate_transition("ok", true, 120, 120);
        assert_eq!(t, AlertTransition::FireNow);

        let t = evaluate_transition("ok", true, 200, 120);
        assert_eq!(t, AlertTransition::FireNow);
    }

    #[test]
    fn test_transition_firing_to_ok() {
        // Was firing, no longer breaching -> Resolve.
        let t = evaluate_transition("firing", false, 0, 120);
        assert_eq!(t, AlertTransition::Resolve);
    }

    #[test]
    fn test_transition_stay_states() {
        // Not breaching, was ok -> StayOk.
        assert_eq!(
            evaluate_transition("ok", false, 0, 120),
            AlertTransition::StayOk
        );
        // Breaching, already firing -> StayFiring (no re-fire).
        assert_eq!(
            evaluate_transition("firing", true, 999, 120),
            AlertTransition::StayFiring
        );
        // Unknown previous state behaves like ok.
        assert_eq!(
            evaluate_transition("unknown", false, 0, 120),
            AlertTransition::StayOk
        );
        assert_eq!(
            evaluate_transition("unknown", true, 5, 120),
            AlertTransition::StartBreach
        );
    }

    #[test]
    fn test_map_severity() {
        assert!(matches!(map_severity("critical"), AlarmSeverity::Critical));
        assert!(matches!(map_severity("info"), AlarmSeverity::Info));
        assert!(matches!(map_severity("warning"), AlarmSeverity::Warning));
        assert!(matches!(
            map_severity("anything-else"),
            AlarmSeverity::Warning
        ));
    }

    #[test]
    fn test_histogram_quantile() {
        // bounds [10,50,100], counts [2,3,4,1] (total 10, cum 2/5/9/10).
        let bounds = [10.0, 50.0, 100.0];
        let counts = [2u64, 3, 4, 1];
        assert!((histogram_quantile(&bounds, &counts, 0.5) - 50.0).abs() < 1e-9);
        assert!((histogram_quantile(&bounds, &counts, 0.9) - 100.0).abs() < 1e-9);
        // mid-bucket interpolation: bounds [10,20], counts [1,2,1] -> p50 = 15.
        assert!((histogram_quantile(&[10.0, 20.0], &[1, 2, 1], 0.5) - 15.0).abs() < 1e-9);
        // empty histogram -> 0.
        assert_eq!(histogram_quantile(&bounds, &[0, 0, 0, 0], 0.95), 0.0);
    }
}
