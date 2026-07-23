# CHANGES-FROM-UPSTREAM

Fork of [gotempsh/temps](https://github.com/gotempsh/temps) for Luria. Upstream is
MIT / Apache-2.0 dual-licensed; both license files (`LICENSE`, `LICENSE-MIT`) and
`NOTICE` are kept intact. This file logs every deviation from upstream and the audit
behind them.

Fork: https://github.com/baneets/temps · Upstream pinned at: v0.0.8 (first audit
2026-07-23).

---

## 1. Audit: telemetry / phone-home

**Finding:** Temps ships anonymous product telemetry, **ON by default**.

- Client crate: `crates/temps-telemetry` (`service.rs`, `lib.rs`, `plugin.rs`).
- Default endpoint: `https://telemetry.temps.sh/v1/events`
  (`DEFAULT_TELEMETRY_ENDPOINT`, `service.rs:25`).
- Payload: anonymous instance id (`ANONYMOUS_ID_FILE`) + lifecycle/deployment events
  (`ServiceCreated`, `ServiceClusterCreated`, `PgMajorUpgradeCompleted`,
  `ErrorTrackingFirstError`, `analytics_first_event_received`, …). Fire-and-forget,
  time-bounded POST.
- Receiver in repo: `telemetry-api/` (Bun/Postgres ingest service temps.sh runs). Not
  something our instance runs; it's the other end of the pipe.
- Opt-out (documented, upstream): `TEMPS_TELEMETRY=0` (also `false|off|no|disabled`).
  Endpoint overridable via `TEMPS_TELEMETRY_ENDPOINT`. Wired through a
  `TelemetryReporter` trait with a `NoopTelemetryReporter` fallback.
- All other `gotempsh/*` string hits in the tree are Docker base-image names
  (`gotempsh/postgres-walg`, `gotempsh/redis-walg`, `gotempsh/postgres-ha`), pulled
  from Docker Hub for managed services — not phone-home.

**Neutralization (chosen approach — env, not code fork):**

- Set `TEMPS_TELEMETRY=0` in the deploy env / `docker-compose.yml`. One line, reversible,
  survives upstream merges (no source divergence to reconcile).
- Belt-and-suspenders (optional, only if we want it dead even on env misconfig):
  patch `enabled_from_env()` default to `false` and/or set
  `TEMPS_TELEMETRY_ENDPOINT` to a blackhole. Not yet applied.

**Verification (REQUIRED before trusting, per work order — via network, not code):**
- [ ] Run instance with `TEMPS_TELEMETRY=0`; capture egress (e.g. `tcpdump`/proxy logs);
      confirm zero connections to `telemetry.temps.sh` / any `*.temps.sh`.
- [ ] Status: NOT YET RUN (no instance provisioned).

## 2. Feature / gap audit vs Luria's needs

Confirmed from source (v0.0.8), not marketing:

| Capability | Present? | Evidence |
|---|---|---|
| Session replay (rrweb) | ✅ | `crates/temps-analytics-session-replay` |
| Analytics + funnels + visitor tracking | ✅ | same crate + `telemetry-api` events |
| **Per-project (multi-tenant) isolation** | ✅ native | replay crate scoped by `project_id`; cross-tenant guards (`project_scope_guard!`, `project_access_guard!`); project resolved from originating **host** via `route_table.get_route(&metadata.host)` |
| Sentry-compatible error tracking | ✅ | `crates/temps-error-tracking/src/sentry/` |
| Uptime / monitoring | ✅ | `crates/temps-monitoring` |
| Managed Postgres / Redis / S3(MinIO) / Mongo | ✅ | `crates/temps-providers` |
| Git push → deploy, auto-TLS (Pingora) | ✅ | deployer + proxy crates |
| **Heatmaps** | ❌ **GAP** | zero `heatmap` hits in any crate/web. Not built. |
| Autocapture (PostHog-style) | ❌ | not present; snippet is explicit events |
| Global edge assignment | ❌ n/a | single-VPS, single-region (Pingora), not a global edge network |

**Multi-tenant mapping for Luria:** one Temps project per client, resolved by host =
Supabase `sites.shop_domain`. `client_id` isolation is native (no shared-stream tag
needed). `variant_id` rides as event/session metadata for query-by-variant.

**Heatmap gap:** Temps has none. Options: keep PostHog free tier for heatmaps only, or
build simple scroll/click aggregation from the events we already emit (doc 06
contemplates this). Do not pay for a heatmap product.

## 3. Upstream security-fix pull runbook

Fork tracks upstream loosely; pull security fixes periodically.

```
# one-time
git remote add upstream https://github.com/gotempsh/temps.git

# periodic (e.g. monthly, or on a tagged security release)
git fetch upstream --tags
git log --oneline HEAD..upstream/main        # review what changed
git merge upstream/main                       # or cherry-pick security commits only
# re-run the telemetry egress verification (§1) after any merge — upstream could
# re-enable or move the telemetry endpoint. TEMPS_TELEMETRY=0 stays in our env.
cargo build --release && <run acceptance-tests/>
```

Because our only change is env-level (`TEMPS_TELEMETRY=0`), merges should be conflict-free
unless we later add the optional code-level opt-out in §1.
