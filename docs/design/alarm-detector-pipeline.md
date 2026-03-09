# Alarm Detector Pipeline

## Problem

### 1. Duplicate data fetching

Every monitor fetches data independently. If we add a `HighLatencyEvaluator` and an `ErrorRateEvaluator`, both query `otel_spans`. Two identical DB queries. `ContainerHealthMonitor` calls `get_container_info` then `get_container_stats`, but `get_container_stats` internally calls `get_container_info` again. That's 3 Docker API calls per container when 2 would suffice.

Current cost per 30s cycle for N containers: `1 + 2N` DB queries + `3N` Docker API calls. With 50 containers that's 101 DB queries and 150 Docker calls every 30 seconds.

### 2. No historical container metrics

Container CPU/memory data is **completely ephemeral**. The Docker API gives a point-in-time snapshot, we compare it against a threshold, and throw it away. We can't detect:

- A spike from 20% to 85% CPU in 2 minutes (rate of change)
- Gradual memory leak over hours (drift detection)
- "This container usually sits at 30% CPU, 60% is abnormal for it" (baseline comparison)
- Correlation between a deploy and a resource spike

Meanwhile, OTEL metrics and spans already have TimescaleDB hypertables with continuous aggregates, compression, and retention. Container stats should get the same treatment.

### 3. No shared abstraction

Each monitor is a bespoke struct. Adding a new detector means copying the polling loop, threshold logic, consecutive-check debouncing, and state management. `DiskSpaceMonitor` doesn't even go through `AlarmService`.

## Design: Collect -> Store -> Evaluate

```
                         ┌──────────────────────┐
  Data Sources           │   TimescaleDB        │     Evaluators
  ============           │   (time-series store) │     ==========
                         │                      │
  Docker ──> ContainerCollector ──> container_metrics ──┬──> HighCpuEvaluator
                         │                      │      ├──> HighMemoryEvaluator
                         │                      │      ├──> CpuSpikeEvaluator
                         │                      │      └──> MemoryLeakEvaluator
                         │                      │
  OTEL ───> (already stored) ──> otel_spans ────┬──> HighLatencyEvaluator
                         │                      │      └──> ErrorRateEvaluator
                         │                      │
  OTEL ───> (already stored) ──> otel_metrics ──┬──> AnomalyEvaluator
                         │                      │
  sysinfo ─> DiskCollector ──> (in-memory only) ──> DiskSpaceEvaluator
                         │                      │
                         └──────────────────────┘
                                                       All evaluators
                                                            │
                                                            ▼
                                                      AlarmService
                                                     (fire / resolve)
```

The key change from the previous draft: collectors don't just pass snapshots to evaluators. They **persist data to hypertables** first. Evaluators then query windows of historical data, not just the latest point.

For data that's already persisted (OTEL spans, OTEL metrics, status checks), there's no collector. The evaluator queries the existing table directly.

## New Table: `container_metrics`

Container stats are the only major data source with no persistence. Everything else (spans, metrics, status checks, events) already has a hypertable.

### Migration

```sql
CREATE TABLE IF NOT EXISTS container_metrics (
    id              BIGSERIAL,
    container_id    INTEGER NOT NULL,  -- FK to deployment_containers.id
    deployment_id   INTEGER NOT NULL,  -- denormalized for fast queries
    project_id      INTEGER NOT NULL,  -- denormalized for fast queries
    environment_id  INTEGER NOT NULL,  -- denormalized for fast queries

    -- Resource metrics
    cpu_percent         DOUBLE PRECISION NOT NULL,
    memory_bytes        BIGINT NOT NULL,
    memory_limit_bytes  BIGINT,
    memory_percent      DOUBLE PRECISION,
    network_rx_bytes    BIGINT NOT NULL DEFAULT 0,
    network_tx_bytes    BIGINT NOT NULL DEFAULT 0,

    -- Container state
    restart_count       INTEGER,
    status              VARCHAR(20) NOT NULL,  -- running, exited, dead, etc.

    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (id, recorded_at)
);

-- Convert to hypertable (1-day chunks, same pattern as otel_metrics)
SELECT create_hypertable('container_metrics', 'recorded_at',
    partitioning_column => 'id',
    number_partitions => 4,
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE);

-- Indexes for evaluator queries
CREATE INDEX idx_container_metrics_container_time
    ON container_metrics (container_id, recorded_at DESC);
CREATE INDEX idx_container_metrics_project_time
    ON container_metrics (project_id, recorded_at DESC);
CREATE INDEX idx_container_metrics_deployment_time
    ON container_metrics (deployment_id, recorded_at DESC);

-- Compression: segment by container, order by time
ALTER TABLE container_metrics SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'container_id, project_id',
    timescaledb.compress_orderby = 'recorded_at DESC'
);
SELECT add_compression_policy('container_metrics', INTERVAL '7 days', if_not_exists => TRUE);

-- Retention: drop data older than 30 days
SELECT add_retention_policy('container_metrics', INTERVAL '30 days', if_not_exists => TRUE);
```

### Continuous aggregates

Pre-computed rollups so evaluators don't scan raw rows:

```sql
-- 1-minute rollup (for real-time alarm evaluation)
CREATE MATERIALIZED VIEW container_metrics_1min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 minute', recorded_at) AS bucket,
    container_id,
    deployment_id,
    project_id,
    environment_id,
    AVG(cpu_percent) AS avg_cpu,
    MAX(cpu_percent) AS max_cpu,
    AVG(memory_percent) AS avg_memory,
    MAX(memory_percent) AS max_memory,
    MAX(memory_bytes) AS max_memory_bytes,
    MAX(restart_count) AS max_restart_count,
    COUNT(*) AS sample_count
FROM container_metrics
GROUP BY bucket, container_id, deployment_id, project_id, environment_id
WITH NO DATA;

SELECT add_continuous_aggregate_policy('container_metrics_1min',
    start_offset => INTERVAL '1 hour',
    end_offset => INTERVAL '1 minute',
    schedule_interval => INTERVAL '1 minute');

-- 1-hour rollup (for trend analysis, dashboards, baseline computation)
CREATE MATERIALIZED VIEW container_metrics_1hr
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', recorded_at) AS bucket,
    container_id,
    deployment_id,
    project_id,
    environment_id,
    AVG(cpu_percent) AS avg_cpu,
    MAX(cpu_percent) AS max_cpu,
    MIN(cpu_percent) AS min_cpu,
    AVG(memory_percent) AS avg_memory,
    MAX(memory_percent) AS max_memory,
    MIN(memory_percent) AS min_memory,
    MAX(memory_bytes) AS max_memory_bytes,
    AVG(network_rx_bytes)::BIGINT AS avg_network_rx,
    AVG(network_tx_bytes)::BIGINT AS avg_network_tx,
    MAX(restart_count) AS max_restart_count,
    COUNT(*) AS sample_count
FROM container_metrics
GROUP BY bucket, container_id, deployment_id, project_id, environment_id
WITH NO DATA;

SELECT add_continuous_aggregate_policy('container_metrics_1hr',
    start_offset => INTERVAL '3 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '10 minutes');
```

### Data volume estimate

At 30s intervals, one container produces ~2,880 rows/day. 50 containers = ~144K rows/day. Each row is small (~150 bytes). Before compression: ~21 MB/day. After zstd compression (typical 10:1 on numeric time-series): ~2 MB/day. With 30-day retention: ~60 MB total. Negligible.

## Core Traits

### DataCollector

Collectors fetch data from external sources and **persist it to TimescaleDB**. Evaluators never see the raw snapshot; they query the stored data.

```rust
/// A collector fetches raw data from an external source and persists it.
/// Runs once per cycle. Its job is ingestion, not analysis.
#[async_trait]
pub trait DataCollector: Send + Sync {
    /// Unique name (e.g. "container", "disk")
    fn name(&self) -> &'static str;

    /// Collect data and persist it. Returns the number of data points ingested.
    async fn collect(&self, ctx: &CollectorContext) -> Result<CollectResult, CollectorError>;
}

pub struct CollectResult {
    /// How many data points were ingested this cycle
    pub points_ingested: u64,
    /// Targets that were observed (for pruning evaluator state)
    pub active_targets: Vec<AlarmTarget>,
}
```

### AlarmEvaluator

Evaluators query stored time-series data and emit signals. They have access to historical windows, not just the current point.

```rust
/// An evaluator queries stored metrics and produces alarm signals.
/// Can look at historical windows to detect trends, spikes, and anomalies.
///
/// Evaluators are stateless. The pipeline handles debouncing
/// (consecutive-check counters) and cooldown (AlarmService).
#[async_trait]
pub trait AlarmEvaluator: Send + Sync {
    /// Unique name (e.g. "high_cpu", "high_latency")
    fn name(&self) -> &'static str;

    /// Which alarm type this evaluator fires
    fn alarm_type(&self) -> AlarmType;

    /// Evaluate stored data and emit signals.
    /// The EvaluationWindow tells the evaluator what time range to consider.
    async fn evaluate(
        &self,
        ctx: &EvaluatorContext,
        config: &EvaluatorConfig,
    ) -> Vec<EvaluatorSignal>;
}
```

### Supporting Types

```rust
/// Identifies a specific alarm target
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct AlarmTarget {
    pub project_id: i32,
    pub environment_id: i32,
    pub deployment_id: i32,
    pub container_id: Option<i32>,
    /// Disambiguator for same-type alarms on different resources
    /// e.g. service_name for latency alarms, disk path for disk alarms
    pub resource_key: Option<String>,
}

/// What an evaluator returns per target
pub enum EvaluatorSignal {
    /// Target is breaching threshold right now
    Firing {
        target: AlarmTarget,
        severity: AlarmSeverity,
        title: String,
        message: String,
        metadata: serde_json::Value,
    },
    /// Target is back to normal (auto-resolve)
    Ok {
        target: AlarmTarget,
    },
}

/// Context for evaluators: DB access + time window
pub struct EvaluatorContext {
    pub db: Arc<DatabaseConnection>,
    pub now: DateTime<Utc>,
    /// How far back to look for the current evaluation (e.g. 5 minutes)
    pub evaluation_window: Duration,
    /// How far back to look for baseline comparison (e.g. 7 days)
    pub baseline_window: Duration,
}

/// Context for collectors
pub struct CollectorContext {
    pub db: Arc<DatabaseConnection>,
    pub deployer: Arc<dyn ContainerDeployer>,
    pub now: DateTime<Utc>,
}
```

## Pipeline Orchestrator

```rust
pub struct AlarmPipeline {
    collectors: Vec<Box<dyn DataCollector>>,
    evaluators: Vec<Box<dyn AlarmEvaluator>>,
    alarm_service: Arc<AlarmService>,
    config: PipelineConfig,

    /// Consecutive-check counters: (evaluator_name, target) -> count
    /// Only fires alarm after N consecutive Firing signals.
    consecutive_counters: RwLock<HashMap<(String, AlarmTarget), u32>>,
}
```

### One cycle

```
1. COLLECT: Run all collectors in parallel
     ContainerCollector:
       - Fetch Docker stats for all active containers
       - INSERT INTO container_metrics (batch)
       - Return active targets list
     (DiskCollector etc.)

2. EVALUATE: Run all evaluators in parallel
     Each evaluator queries the DB directly:
       - HighCpuEvaluator: SELECT FROM container_metrics_1min
         WHERE container_id IN (...) AND bucket > now() - '5 min'
       - HighLatencyEvaluator: SELECT FROM otel_spans
         WHERE start_time > now() - '5 min' GROUP BY service_name
       - CpuSpikeEvaluator: compare last 2 min avg vs last 1 hour avg
     Each returns Vec<EvaluatorSignal>

3. PROCESS signals through debounce logic:
     Firing -> increment consecutive counter
       if counter >= config.consecutive_checks_required:
         call alarm_service.fire_alarm(...)
         reset counter
     Ok -> reset counter
       if target had a firing alarm:
         call alarm_service.resolve_alarms_by_type(...)

4. PRUNE counters for targets that didn't appear in any signal
```

### Why collectors and evaluators are separate phases

Collectors must finish before evaluators run. If the `ContainerCollector` writes rows at 14:30:00 and the `HighCpuEvaluator` queries `container_metrics_1min` for the last 5 minutes, it needs those rows to exist. Sequential: collect first, then evaluate.

Within each phase, everything runs in parallel:

```rust
async fn run_cycle(&self) {
    let collector_ctx = self.build_collector_context();

    // Phase 1: Collect (parallel)
    let collect_results: Vec<_> = futures::future::join_all(
        self.collectors.iter().map(|c| {
            let ctx = &collector_ctx;
            async move { (c.name(), c.collect(ctx).await) }
        })
    ).await;

    for (name, result) in &collect_results {
        match result {
            Ok(r) => debug!("Collector '{}': {} points", name, r.points_ingested),
            Err(e) => error!("Collector '{}' failed: {}", name, e),
        }
    }

    // Phase 2: Evaluate (parallel)
    let evaluator_ctx = self.build_evaluator_context();
    let eval_results: Vec<_> = futures::future::join_all(
        self.evaluators.iter().map(|e| {
            let ctx = &evaluator_ctx;
            let config = &self.config.evaluator_config;
            async move { (e.name(), e.alarm_type(), e.evaluate(ctx, config).await) }
        })
    ).await;

    // Phase 3: Process signals (sequential, touches shared state)
    for (name, alarm_type, signals) in eval_results {
        self.process_signals(&name, alarm_type, signals).await;
    }
}
```

## What Each Evaluator Can Do Now

With historical data, evaluators go beyond simple threshold checks:

### Threshold check (what we have today)
> "CPU is above 90% right now"

```sql
SELECT container_id, avg_cpu
FROM container_metrics_1min
WHERE bucket > now() - interval '1 minute'
  AND container_id = $1
ORDER BY bucket DESC LIMIT 1
```

### Rate of change (spike detection)
> "CPU jumped from 20% to 85% in the last 2 minutes"

```sql
SELECT
    container_id,
    first(avg_cpu, bucket) AS cpu_2min_ago,
    last(avg_cpu, bucket) AS cpu_now,
    last(avg_cpu, bucket) - first(avg_cpu, bucket) AS delta
FROM container_metrics_1min
WHERE bucket > now() - interval '2 minutes'
  AND container_id = $1
GROUP BY container_id
HAVING last(avg_cpu, bucket) - first(avg_cpu, bucket) > 40  -- 40pp jump
```

### Baseline deviation (anomaly for this specific container)
> "This container usually runs at 30% CPU. 65% is 2 stddev above its normal."

```sql
WITH baseline AS (
    SELECT
        container_id,
        AVG(avg_cpu) AS mean_cpu,
        STDDEV(avg_cpu) AS stddev_cpu
    FROM container_metrics_1hr
    WHERE bucket > now() - interval '7 days'
      AND container_id = $1
      AND EXTRACT(DOW FROM bucket) = EXTRACT(DOW FROM now())  -- same day of week
    GROUP BY container_id
),
current AS (
    SELECT container_id, AVG(avg_cpu) AS current_cpu
    FROM container_metrics_1min
    WHERE bucket > now() - interval '5 minutes'
      AND container_id = $1
    GROUP BY container_id
)
SELECT
    c.container_id,
    c.current_cpu,
    b.mean_cpu,
    b.stddev_cpu,
    (c.current_cpu - b.mean_cpu) / NULLIF(b.stddev_cpu, 0) AS z_score
FROM current c JOIN baseline b USING (container_id)
WHERE (c.current_cpu - b.mean_cpu) / NULLIF(b.stddev_cpu, 0) > 3.0
```

### Drift detection (memory leak)
> "Memory has been climbing steadily for the last 4 hours at 0.5% per hour"

```sql
SELECT
    container_id,
    regr_slope(avg_memory, EXTRACT(EPOCH FROM bucket)) AS slope,
    regr_r2(avg_memory, EXTRACT(EPOCH FROM bucket)) AS r_squared
FROM container_metrics_1hr
WHERE bucket > now() - interval '4 hours'
  AND container_id = $1
GROUP BY container_id
HAVING regr_r2(avg_memory, EXTRACT(EPOCH FROM bucket)) > 0.8  -- strong linear trend
   AND regr_slope(avg_memory, EXTRACT(EPOCH FROM bucket)) > 0  -- increasing
```

### Deploy correlation
> "CPU spiked 3 minutes after deployment deploy-abc went live"

```sql
WITH recent_deploys AS (
    SELECT id, project_id, environment_id, ready_at
    FROM deployments
    WHERE ready_at > now() - interval '30 minutes'
),
post_deploy_metrics AS (
    SELECT
        m.container_id,
        d.id AS deployment_id,
        AVG(m.avg_cpu) AS post_deploy_cpu
    FROM container_metrics_1min m
    JOIN recent_deploys d ON m.deployment_id = d.id
    WHERE m.bucket BETWEEN d.ready_at AND d.ready_at + interval '10 minutes'
    GROUP BY m.container_id, d.id
),
pre_deploy_metrics AS (
    SELECT
        m.container_id,
        d.id AS deployment_id,
        AVG(m.avg_cpu) AS pre_deploy_cpu
    FROM container_metrics_1min m
    JOIN recent_deploys d ON m.deployment_id != d.id
      AND m.project_id = d.project_id
    WHERE m.bucket BETWEEN d.ready_at - interval '30 minutes' AND d.ready_at
    GROUP BY m.container_id, d.id
)
SELECT
    post.container_id,
    post.deployment_id,
    pre.pre_deploy_cpu,
    post.post_deploy_cpu,
    post.post_deploy_cpu - pre.pre_deploy_cpu AS cpu_delta
FROM post_deploy_metrics post
JOIN pre_deploy_metrics pre USING (container_id, deployment_id)
WHERE post.post_deploy_cpu - pre.pre_deploy_cpu > 30
```

## Concrete Collectors

### ContainerCollector

The only collector that needs to persist new data. OTEL data is already written by the OTEL ingestion pipeline. Disk data is cheap enough to not persist.

```rust
pub struct ContainerCollector {
    deployer: Arc<dyn ContainerDeployer>,
}

#[async_trait]
impl DataCollector for ContainerCollector {
    fn name(&self) -> &'static str { "container" }

    async fn collect(&self, ctx: &CollectorContext) -> Result<CollectResult, CollectorError> {
        // 1 DB query: get all active containers
        let containers = deployment_containers::Entity::find()
            .filter(deployment_containers::Column::DeletedAt.is_null())
            .all(ctx.db.as_ref())
            .await?;

        // Parallel Docker fetch (10 concurrent)
        let data: Vec<_> = futures::stream::iter(&containers)
            .map(|container| async {
                let deployment = deployments::Entity::find_by_id(container.deployment_id)
                    .one(ctx.db.as_ref()).await.ok()??;
                let info = self.deployer.get_container_info(&container.container_id).await.ok()?;
                let stats = self.deployer.get_container_stats(&container.container_id).await.ok()?;
                Some((container, deployment, info, stats))
            })
            .buffer_unordered(10)
            .filter_map(|x| async { x })
            .collect()
            .await;

        // Batch INSERT into container_metrics
        // Use a single multi-row INSERT, not N individual inserts
        let sql = build_batch_insert(&data, ctx.now);
        ctx.db.execute(sql).await?;

        let targets = data.iter().map(|(c, d, _, _)| AlarmTarget {
            project_id: d.project_id,
            environment_id: d.environment_id,
            deployment_id: d.id,
            container_id: Some(c.id),
            resource_key: None,
        }).collect();

        Ok(CollectResult {
            points_ingested: data.len() as u64,
            active_targets: targets,
        })
    }
}
```

### DiskCollector

Disk metrics are cheap (local syscall) and not worth persisting to TimescaleDB. The evaluator gets the snapshot directly via `CycleData`, not via DB.

Exception to the "evaluators query the DB" rule. Some data sources are ephemeral and that's fine.

```rust
pub struct DiskSnapshot {
    pub disks: Vec<DiskData>,
}

pub struct DiskData {
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}
```

### No collector needed for OTEL data

OTEL spans and metrics are already ingested by `temps-otel`'s gRPC/HTTP receivers and stored in `otel_spans` / `otel_metrics` hypertables. The evaluators just query those tables directly. No collector needed.

## Concrete Evaluators

### HighCpuEvaluator (threshold)

```rust
#[async_trait]
impl AlarmEvaluator for HighCpuEvaluator {
    fn name(&self) -> &'static str { "high_cpu" }
    fn alarm_type(&self) -> AlarmType { AlarmType::HighCpu }

    async fn evaluate(&self, ctx: &EvaluatorContext, config: &EvaluatorConfig) -> Vec<EvaluatorSignal> {
        // Query the 1-minute continuous aggregate for the evaluation window
        let rows = query_container_metrics_1min(
            ctx.db.as_ref(),
            ctx.now - ctx.evaluation_window,
            ctx.now,
        ).await;

        let mut signals = Vec::new();
        for row in rows {
            let target = AlarmTarget { /* from row */ };
            if row.avg_cpu > config.cpu_threshold_percent {
                signals.push(EvaluatorSignal::Firing { target, /* ... */ });
            } else {
                signals.push(EvaluatorSignal::Ok { target });
            }
        }
        signals
    }
}
```

### CpuSpikeEvaluator (rate of change)

Detects sudden jumps by comparing the last 2 minutes against the previous 10 minutes.

```rust
#[async_trait]
impl AlarmEvaluator for CpuSpikeEvaluator {
    fn name(&self) -> &'static str { "cpu_spike" }
    fn alarm_type(&self) -> AlarmType { AlarmType::HighCpu }

    async fn evaluate(&self, ctx: &EvaluatorContext, config: &EvaluatorConfig) -> Vec<EvaluatorSignal> {
        // Compare recent avg vs previous window avg
        let sql = r#"
            SELECT sub.container_id, sub.project_id, sub.environment_id,
                   sub.deployment_id, sub.recent_cpu, sub.baseline_cpu,
                   sub.recent_cpu - sub.baseline_cpu AS delta
            FROM (
                SELECT
                    container_id, project_id, environment_id, deployment_id,
                    AVG(CASE WHEN bucket > $1 THEN avg_cpu END) AS recent_cpu,
                    AVG(CASE WHEN bucket <= $1 AND bucket > $2 THEN avg_cpu END) AS baseline_cpu
                FROM container_metrics_1min
                WHERE bucket > $2
                GROUP BY container_id, project_id, environment_id, deployment_id
            ) sub
            WHERE sub.recent_cpu - sub.baseline_cpu > $3
        "#;
        // $1 = now - 2min, $2 = now - 12min, $3 = spike_threshold (e.g. 40pp)
        // ...
    }
}
```

### MemoryLeakEvaluator (drift detection)

Uses linear regression over the hourly aggregate to detect steady memory growth.

```rust
#[async_trait]
impl AlarmEvaluator for MemoryLeakEvaluator {
    fn name(&self) -> &'static str { "memory_leak" }
    fn alarm_type(&self) -> AlarmType { AlarmType::HighMemory }

    async fn evaluate(&self, ctx: &EvaluatorContext, config: &EvaluatorConfig) -> Vec<EvaluatorSignal> {
        let sql = r#"
            SELECT sub.container_id, sub.project_id, sub.environment_id,
                   sub.deployment_id, sub.slope, sub.r_squared, sub.latest_memory
            FROM (
                SELECT
                    container_id, project_id, environment_id, deployment_id,
                    regr_slope(avg_memory, EXTRACT(EPOCH FROM bucket)) AS slope,
                    regr_r2(avg_memory, EXTRACT(EPOCH FROM bucket)) AS r_squared,
                    last(avg_memory, bucket) AS latest_memory
                FROM container_metrics_1hr
                WHERE bucket > $1
                GROUP BY container_id, project_id, environment_id, deployment_id
            ) sub
            WHERE sub.r_squared > 0.8 AND sub.slope > 0
        "#;
        // $1 = now - 6 hours
        // slope > 0 means increasing, r_squared > 0.8 means it's a real trend
        // ...
    }
}
```

### HighLatencyEvaluator (OTEL spans, no collector needed)

Queries `otel_spans` directly. This table already exists as a hypertable.

```rust
#[async_trait]
impl AlarmEvaluator for HighLatencyEvaluator {
    fn name(&self) -> &'static str { "high_latency" }
    fn alarm_type(&self) -> AlarmType { AlarmType::HighResponseTime }

    async fn evaluate(&self, ctx: &EvaluatorContext, config: &EvaluatorConfig) -> Vec<EvaluatorSignal> {
        let sql = r#"
            SELECT sub.project_id, sub.service_name, sub.total_spans,
                   sub.error_spans, sub.p95_latency, sub.avg_latency
            FROM (
                SELECT
                    project_id,
                    service_name,
                    COUNT(*) AS total_spans,
                    COUNT(*) FILTER (WHERE status_code = 'ERROR') AS error_spans,
                    percentile_cont(0.95) WITHIN GROUP (ORDER BY duration_ms) AS p95_latency,
                    AVG(duration_ms) AS avg_latency
                FROM otel_spans
                WHERE start_time > $1
                  AND kind = 'SERVER'
                GROUP BY project_id, service_name
                HAVING COUNT(*) >= 10
            ) sub
        "#;
        // $1 = now - evaluation_window
        // ...
    }
}
```

### ErrorRateEvaluator

Same query source as HighLatencyEvaluator but checks `error_spans::float / total_spans > threshold`. Could even share the same SQL query result if both evaluators consume a shared `SpanAggSnapshot`. This is an optimization we can add later; for v1, each evaluator runs its own query. The queries are cheap (they hit the same TimescaleDB chunks).

## Configuration

```rust
pub struct PipelineConfig {
    /// How often the pipeline runs (seconds). Default: 30
    pub cycle_interval_secs: u64,

    /// How far back evaluators look for current state. Default: 5 min
    pub evaluation_window: Duration,

    /// How far back evaluators look for baselines. Default: 7 days
    pub baseline_window: Duration,

    /// Evaluator-specific thresholds
    pub evaluator_config: EvaluatorConfig,
}

pub struct EvaluatorConfig {
    /// Consecutive breaches required before firing. Default: 3
    pub consecutive_checks_required: u32,

    // Container thresholds
    pub cpu_threshold_percent: f64,       // default: 90
    pub memory_threshold_percent: f64,    // default: 90
    pub cpu_spike_delta: f64,             // default: 40 (percentage points)
    pub memory_leak_r_squared: f64,       // default: 0.8

    // OTEL thresholds
    pub p95_latency_threshold_ms: f64,    // default: 5000
    pub error_rate_threshold: f64,        // default: 0.05 (5%)

    // Anomaly detection
    pub anomaly_z_score_threshold: f64,   // default: 3.0
    pub anomaly_sustained_minutes: u32,   // default: 8

    // Disk
    pub disk_threshold_percent: f64,      // default: 80
}
```

These come from `AppSettings` (global config in DB). Later, per-project overrides via an `alarm_rules` table.

## Shared Query Layer (Optional Optimization)

Multiple evaluators querying the same table in the same time window is fine for v1. TimescaleDB caches recently accessed chunks in memory. But if we want to avoid even that, we can introduce a shared query snapshot:

```rust
/// Pre-fetched data that multiple evaluators share.
/// Populated once per cycle, before evaluators run.
pub struct SharedQueryCache {
    /// Latest container metrics (from container_metrics_1min)
    pub container_agg: Vec<ContainerMetricRow>,
    /// Latest span aggregation (from otel_spans)
    pub span_agg: Vec<SpanAggRow>,
}
```

The pipeline fetches these once, then passes them to evaluators via `EvaluatorContext`. This is purely an optimization. The trait interface doesn't change.

## Event-Driven Alarms (Separate from Pipeline)

Some alarms react to events, not periodic data. These don't fit the collector/evaluator pipeline.

```rust
pub struct JobAlarmBridge {
    alarm_service: Arc<AlarmService>,
}
```

| Job | Action |
|---|---|
| `DeploymentFailed` | `fire_alarm(DeploymentFailed, Critical)` |
| `DeploymentSucceeded` | `resolve_alarms_by_type(DeploymentFailed)` |
| `VulnerabilityScanCompleted` (high severity) | `fire_alarm(SecurityVulnerability, Critical)` |

The `OutageDetectionService` stays as-is. It's event-driven (listens to `StatusCheckCompleted` jobs) and already bridges to `AlarmService`.

## Streaming Stats (Future Optimization)

Bollard supports `docker.stats(id, StatsOptions { stream: true })` returning a persistent `Stream<Item = Stats>` at ~1/second.

Instead of 2N Docker API calls per cycle, keep N long-lived streams writing to an in-memory ring buffer. The `ContainerCollector` reads from the buffer and batch-inserts to `container_metrics`.

```
  Docker daemon
       │
       ├── stream "abc" ──> RingBuffer<Stats> ──┐
       ├── stream "def" ──> RingBuffer<Stats> ──┤
       └── stream "ghi" ──> RingBuffer<Stats> ──┤
                                                 │
                            ContainerCollector ◄──┘
                            (reads buffers, writes to DB)
```

Benefits:
- 0 Docker API calls per evaluation cycle
- Higher resolution data (1s vs 30s) for spike detection
- `ContainerCollector` just drains the buffer and batch-inserts

Not needed for v1. The polling approach works fine up to ~100 containers.

## New Alarm Types

```rust
pub enum AlarmType {
    // Existing
    ContainerRestart,
    ContainerOomKilled,
    HighResponseTime,
    Outage,
    HighCpu,
    HighMemory,
    DeploymentFailed,
    HealthCheckFailed,

    // New
    HighErrorRate,         // OTEL span error ratio above threshold
    MetricAnomaly,         // Z-score anomaly on any OTEL metric
    DiskSpace,             // Disk usage above threshold
    SecurityVulnerability, // Critical vuln scan finding
    CpuSpike,              // Sudden CPU jump (rate of change)
    MemoryLeak,            // Steady memory growth (drift detection)
}
```

## File Layout

```
temps/crates/temps-monitoring/src/
  pipeline/
    mod.rs               // pub mod collector, evaluator, orchestrator
    collector.rs         // DataCollector trait, CollectorContext, CollectorError, CollectResult
    evaluator.rs         // AlarmEvaluator trait, EvaluatorSignal, AlarmTarget, EvaluatorContext
    orchestrator.rs      // AlarmPipeline (cycle loop, debounce, auto-resolve)
    config.rs            // PipelineConfig, EvaluatorConfig

  collectors/
    mod.rs
    container.rs         // ContainerCollector (Docker -> container_metrics)
    disk.rs              // DiskCollector (sysinfo -> in-memory snapshot)

  evaluators/
    mod.rs
    high_cpu.rs          // Threshold check on container_metrics_1min
    high_memory.rs       // Threshold check on container_metrics_1min
    cpu_spike.rs         // Rate-of-change on container_metrics_1min
    memory_leak.rs       // Linear regression on container_metrics_1hr
    container_restart.rs // Restart count delta on container_metrics
    container_exit.rs    // Status check on container_metrics
    high_latency.rs      // P95 on otel_spans
    error_rate.rs        // Error ratio on otel_spans
    anomaly.rs           // Z-score on otel_metrics (migrate from temps-otel)
    disk_space.rs        // Threshold on DiskSnapshot

  job_alarm_bridge.rs    // Event-driven: DeploymentFailed -> alarm

  // Existing (kept during migration, removed after)
  alarm_service.rs       // Stays forever
  container_health.rs    // Removed after Phase 2
  disk_space.rs          // Removed after Phase 4
  outage.rs              // Stays forever (event-driven)
```

## Migration Path

### Phase 1: container_metrics table + migration
- Create `container_metrics` hypertable, continuous aggregates, compression, retention
- Sea-ORM entity for `container_metrics`

### Phase 2: Traits + Pipeline Orchestrator
- Define `DataCollector`, `AlarmEvaluator`, `EvaluatorSignal`, `AlarmTarget`
- Build `AlarmPipeline` with cycle loop, consecutive-check debouncing, auto-resolve
- Keep `ContainerHealthMonitor` running alongside (don't break anything)

### Phase 3: ContainerCollector + Container Evaluators
- Build `ContainerCollector` (parallel Docker fetch, batch INSERT)
- Build `HighCpuEvaluator`, `HighMemoryEvaluator`, `ContainerRestartEvaluator`, `ContainerExitEvaluator`
- Wire into `AlarmPipeline`
- Verify parity with `ContainerHealthMonitor`, then remove it

### Phase 4: Advanced Container Evaluators
- `CpuSpikeEvaluator` (rate of change, needs ~10 min of data in container_metrics)
- `MemoryLeakEvaluator` (drift detection, needs ~6 hours of data in container_metrics_1hr)

### Phase 5: OTEL Evaluators
- `HighLatencyEvaluator`, `ErrorRateEvaluator` (query otel_spans directly)
- `AnomalyEvaluator` (migrate Z-score logic from temps-otel, query otel_metrics)

### Phase 6: Cleanup + Event-Driven
- Migrate `DiskSpaceMonitor` to `DiskCollector` + `DiskSpaceEvaluator`
- Build `JobAlarmBridge` for deployment failure alarms
- Remove old standalone monitors

### Phase 7: Streaming Stats (optional)
- Persistent bollard streams with in-memory ring buffers
- `ContainerCollector` reads from buffers instead of calling Docker

### Phase 8: Per-Project Rules (v2)
- `alarm_rules` table with per-project threshold overrides
- API endpoints for CRUD
- Pipeline reads per-project config
