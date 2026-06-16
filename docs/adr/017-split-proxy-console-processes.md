---
title: "ADR-017: Split proxy and console into independently-runnable processes"
status: Proposed
date: 2026-06-15
author: David Viejo
---

# ADR-017: Split proxy and console into independently-runnable processes

**Status:** Proposed
**Date:** 2026-06-15
**Author:** David Viejo

## Context

Today `temps serve` is a single binary with two logical halves that share one
address space:

1. **Proxy half** — Pingora listening on `:80`/`:443`, owning the main OS
   thread. `start_proxy_server` blocks at
   `crates/temps-cli/src/commands/serve/mod.rs:445`; Pingora takes the
   main runtime and never returns while the process is alive.
2. **Console half** — Axum API + web SPA + plugins + background workers,
   spawned on a tokio runtime thread via `rt.spawn(async move { start_console_api(...).await })`
   at `serve/mod.rs:404`. The serve comment at `:413-415` already
   acknowledges the separation: _"The console management UI will not be
   available. Proxied traffic to deployed applications is NOT affected."_

This design means that **any restart of the `temps` process — for a version
upgrade, configuration change, plugin reload, or crash recovery — drops all
in-flight TCP connections on port 80/443 for the duration of the process
restart**. On a typical server that is 2–10 seconds of downtime during an
upgrade. For operators running production workloads, that is unacceptable.

The structural separation is already half-done. `temps proxy` is an existing,
recently-maintained subcommand (`crates/temps-cli/src/commands/proxy.rs`,
`ProxyCommand` struct at `proxy.rs:111`, `execute()` at `proxy.rs:138`,
`start_proxy_server()` at `proxy.rs:180`, last touched 2026-05-29 in
`e8371d52 perf(proxy): bind the load balancer before loading routes`). It is
registered in CLI dispatch at `crates/temps-cli/src/lib.rs:49` and `:198`.

The standalone `temps proxy` already wires PG `LISTEN/NOTIFY` route-table
machinery (`proxy.rs:237-259`) and is production-ready except for two
missing pieces: on-demand scale-to-zero support (passed as `None` at
`proxy.rs:293`) and admin-gate wiring (passed as `None` at `proxy.rs:294`).
Those two gaps are the only blockers for a fully-supported split topology.

### Why the split is feasible now

Coordination between the two halves already happens over PostgreSQL, not
shared memory:

- **Route table**: `CachedPeerTable` is an in-process cache refreshed by
  `RouteTableListener` and `ProjectChangeListener` over PG `LISTEN/NOTIFY`.
  Both listeners are already wired in the standalone proxy (`proxy.rs:237-259`).
  Each process maintains its own in-process cache; PG is the shared source of
  truth.
- **In-process broadcast queue**: `BroadcastQueueService` (`temps-queue/src/queue.rs:41-42`,
  `tokio::sync::broadcast`) carries `Job::ForceRouteReload` for the fast
  single-process path. In split mode, this queue is local to each process and
  carries no cross-process signal — the `RouteReloadSubscriber` in the proxy
  process will never receive `ForceRouteReload` events published by the console
  process's deploy pipeline. PG `NOTIFY` is the operative cross-process route
  reload path, exactly as the author documented at `proxy.rs:264-272`:

  > _"NOTE: In this standalone `temps proxy` command the deploy pipeline runs
  > in a separate control-plane process with its own queue, so ForceRouteReload
  > events never reach this subscriber — the PG LISTEN/NOTIFY path
  > (ProjectChangeListener / RouteTableListener above) remains the
  > route-reload mechanism here. The deterministic in-process path only applies
  > to the single-binary `temps serve` mode where the control plane and proxy
  > share one queue."_

  This is not a new discovery — the standalone proxy was designed with split
  mode semantics in mind. The in-process reload was added to avoid NOTIFY
  latency in the monolith; split mode re-accepts that latency for the
  independence benefit.

- **On-demand / scale-to-zero**: `OnDemandManager` wakes a sleeping
  environment from inside the Pingora request hot path
  (`crates/temps-proxy/src/proxy.rs:2535-2599`, `try_acquire_wake_slot`,
  `wake_environment`, `wait_for_route_reload`, bounded re-resolve loop).
  Its wake path is already cross-process aware: `do_wake` publishes
  `Job::ForceRouteReload` in-process and also fires a raw `NOTIFY
  route_table_changes` via `notify_route_change()` (`on_demand.rs:888-898`) to
  reach remote workers. The `wait_for_route_reload` doc comment at
  `on_demand.rs:251-262` states:

  > _"the proxy caller does NOT rely on this signal for correctness — it
  > re-resolves the route in a bounded loop afterwards, so a missed wakeup
  > costs latency, not a failed request."_

  The `ContainerLifecycle` trait (`on_demand.rs:28-37`), which wraps
  `start_container` / `stop_container` / `is_container_healthy`, is the only
  true console dependency that must now be constructed in the proxy process.
  In `temps serve`, this is injected via `ContainerLifecycleAdapter` wrapping
  `DockerRuntime` at `serve/mod.rs:263-281`. The proxy process runs on the
  same node as the containers, so it has the same Docker socket access.

## Decision

Introduce a supported **split topology** alongside the existing all-in-one
default:

- **All-in-one default** (unchanged): `temps serve` — proxy + console in one
  process, full shared-queue fast-path, no new configuration needed.
- **Split proxy**: `temps proxy` (existing command) — Pingora only, route table
  via PG `NOTIFY`, on-demand fully wired (Phase 2 below), admin gate wired
  (Phase 1 below).
- **Split console**: `temps serve --role=console` (new flag, added to existing
  `ServeCommand`) — Axum API + web SPA + plugins + background workers, no
  `:80`/`:443` bind, no on-demand manager in-console, no proxy-log batch writer.

The proxy side reuses the existing `temps proxy` command unchanged except for
the Phase 1/2 gaps described below. A new `--role=proxy` on `serve` is
explicitly rejected (see Rejected Alternatives §iv).

**Coordination contract in split mode:**

All state crossing the process boundary travels through PostgreSQL. The
in-process broadcast queue is an intra-process optimization in the monolith;
in split mode it becomes inert for cross-process signals. The route table is
refreshed in the proxy process solely by PG `NOTIFY`. This implies:

- Route reloads after a new deployment reach the split proxy within PG `NOTIFY`
  propagation time (typically 100–400 ms on local Postgres). This is slower
  than the in-process path (<5 ms) but within acceptable bounds for a console
  restart scenario, and already the operative path on multi-node worker nodes.
- Wake-after-sleep in split mode fires both `Job::ForceRouteReload` (inert
  cross-process) and raw `NOTIFY route_table_changes` (operative); the proxy
  re-resolves in a bounded loop so correctness is preserved.

### 1. Route-table sync in split mode

The proxy's `CachedPeerTable` is kept fresh by two PG-backed listeners already
wired in `temps proxy`:

- `RouteTableListener` at `proxy.rs:237-245`: subscribes to
  `NOTIFY route_table_changes`, triggers `CachedPeerTable::load_routes()`.
- `ProjectChangeListener` at `proxy.rs:248-259`: subscribes to project-scoped
  change events, triggers partial table updates.

The `RouteReloadSubscriber` (`proxy.rs:274-277`) is also wired but its comment
(`proxy.rs:264-272`, quoted verbatim in Context above) confirms it is inert in
split mode: `ForceRouteReload` never arrives from the console process's queue.

No changes are needed to the route-sync layer for split mode. The PG `NOTIFY`
path is the operative, proven mechanism. **The latency trade-off is explicit
and acceptable:** after a new deployment, the split proxy's route table
refreshes within PG `NOTIFY` propagation time rather than the in-process few
milliseconds. Console restarts do not interrupt route serving; the proxy
continues serving all routes from its in-process cache until the next NOTIFY.

### 2. On-demand wake cross-process

**Current state:** `temps proxy` passes `None` for on-demand at `proxy.rs:293`.
Sleeping environments are therefore invisible to the standalone proxy —
wake-on-request is disabled.

**Required state:** pass `Some(OnDemandManager)` into `setup_proxy_server` in
the standalone proxy, with a `ContainerLifecycle` impl constructed from the
local Docker socket.

The construction path is already established in `serve/mod.rs:238-282`:

```rust
// (illustrative — implementer copies from serve/mod.rs:238-282)
let docker_handle = bollard::Docker::connect_with_defaults()?;
let docker_runtime = temps_deployer::docker::DockerRuntime::new(
    Arc::new(docker_handle), true, "temps".to_string()
);
let adapter = ContainerLifecycleAdapter::new(
    Arc::new(docker_runtime) as Arc<dyn temps_deployer::ContainerDeployer>
);
let on_demand_manager = Arc::new(OnDemandManager::new(
    db.clone(),
    Arc::new(adapter) as Arc<dyn ContainerLifecycle>,
    queue.clone(),
    None, // control-plane-local: NULL node_id containers are local
));
```

`ContainerLifecycleAdapter` (`serve/mod.rs:268`) is defined in
`commands/serve/proxy.rs` and wraps `ContainerDeployer`. The standalone proxy
command must import and construct it identically. This does not require pulling
in the full console plugin set — `bollard::Docker` + `temps_deployer::docker::DockerRuntime`
are the only dependencies, and both are lightweight.

**Driving `notify_route_reloaded` from the PG listener:**

In the monolith, `notify_route_reloaded()` (`on_demand.rs:240`) is called from
the route-table sleeping callback registered at `serve/mod.rs:292-314`. In
split mode the same callback registration must happen in `ProxyCommand::start_proxy_server`,
immediately after `on_demand_manager` is constructed and before
`listener.start_listening()` is called:

```rust
// (illustrative)
route_table.set_on_sleeping_callback(Arc::new(move |entries, on_demand_configs| {
    on_demand_mgr.clear_sleeping_domains();
    for entry in entries { on_demand_mgr.register_sleeping_domain(...); }
    for config in on_demand_configs { on_demand_mgr.register_on_demand_environment(...); }
    on_demand_mgr.notify_route_reloaded(); // drives wait_for_route_reload
}));
on_demand_manager.start_sweep_task(Duration::from_secs(60));
```

When `do_wake` fires `notify_route_change()` (raw PG `NOTIFY`) and the proxy's
`RouteTableListener` triggers a route reload, the sleeping callback fires,
which calls `notify_route_reloaded()`. The wake caller's
`wait_for_route_reload()` then observes the reload signal. The bounded
re-resolve loop (`proxy.rs:2610-2660`) remains the correctness guarantee; the
notify is a latency optimization. This is the same design as the monolith — it
degrades gracefully when the signal is missed.

The idle sweep task (`on_demand.rs:903`, `start_sweep_task`) must also be
started in the proxy process because it issues `stop_container` calls that
require the local Docker socket. The console process must not start a second
sweep task — in split mode the console does not instantiate `OnDemandManager`.

**Schema-skew note:** `OnDemandManager` reads `deployment_containers` and
`environments` tables. During a rolling console upgrade, the proxy may briefly
run against a schema version newer or older than the one it was compiled
against. The on-demand tables have been stable; migrations that touch them must
be backward-compatible with the previous console binary for the duration of the
upgrade window.

### 3. Admin gate wiring

`AdminGateHandle` is a lightweight, periodically-refreshed in-memory snapshot
of DB-backed IP/host allowlist state (`serve/admin_gate_service.rs`). In the
monolith it is constructed at `serve/mod.rs:360-367` and passed to the proxy at
`serve/mod.rs:456`.

The standalone proxy passes `None` (`proxy.rs:294`). For split mode, Phase 1
adds `AdminGateService::new` construction to `ProxyCommand::start_proxy_server`
and threads the handle into `setup_proxy_server`. The service must run its
periodic refresh task in the proxy process's tokio runtime. No console
dependency is required — the service reads its own DB table.

Admin gate construction: `AdminGateService::new(db, admin_allowed_ips, admin_allowed_hosts, trust_forwarded_for)`.
The same env vars (`TEMPS_ADMIN_ALLOWED_IPS`, `TEMPS_ADMIN_ALLOWED_HOSTS`,
`TEMPS_ADMIN_TRUST_FORWARDED_FOR`) must be passed through to the proxy process.

### 4. Background worker ownership

The following table assigns each background loop to one side of the split.
"Console only" tasks rely on the plugin system and full console initialization
and have no coupling to the Pingora runtime. "Proxy only" tasks run in or are
called from the Pingora event loop. "Either" tasks are self-contained and
stateless with respect to the boundary.

| Background task | Owns in split mode | Rationale |
|---|---|---|
| `RouteTableListener` (PG NOTIFY) | Proxy | Route table lives in proxy |
| `ProjectChangeListener` (PG NOTIFY) | Proxy | Route table lives in proxy |
| `RouteReloadSubscriber` (in-process) | Proxy | Inert in split mode; wired for monolith parity |
| Proxy-log batch writer | Proxy | Spawns own thread in `setup_proxy_server` |
| `OnDemandManager::start_sweep_task` | Proxy | Requires local Docker socket |
| Admin gate refresh | Proxy | Gate state owned by proxy |
| TLS/cert renewal scheduler | Console | Needs ACME + domain plugin |
| Disk-space monitor | Console | Management concern |
| Outage detection | Console | Management concern |
| Container health monitor | Console | Uses Docker via agent plugin |
| Metrics scraper | Console | Management concern |
| Alert evaluator | Console | Management concern |
| Backup job processor | Console | Management concern |
| Cron scheduler | Console | Management concern |
| Preview gateway reconciler | Console | Workspace preview is console-side |
| TimescaleDB cagg backfill | Console | One-shot post-migration, not hot-path |

The proxy process runs a minimal OS-thread and async footprint: Pingora workers,
route listeners, proxy-log writer, on-demand sweep, admin gate refresh. No
plugin lifecycle, no Axum listener, no web SPA serving.

### 5. Shared-state inventory

| Shared state | How shared | Notes |
|---|---|---|
| `EncryptionService` | Duplicated (stateless, keyed from env) | Each process constructs from `TEMPS_ENCRYPTION_KEY` |
| `CookieCrypto` | Duplicated (stateless, keyed from env) | Each process constructs from `TEMPS_AUTH_SECRET` |
| `ServerConfig` | Duplicated (loaded from env at startup) | Must be identical; schema: env vars |
| PG connection pool | Independent pools, same DB | Each process holds its own `Arc<DbConnection>` |
| `CachedPeerTable` | Duplicated cache; PG is source of truth | Kept in sync via PG NOTIFY |
| `BroadcastQueueService` | NOT shared — each process has own instance | Cross-process signals via PG NOTIFY only |
| `OnDemandManager` state (`last_activity`, `wake_states`, etc.) | NOT shared — proxy process only | Console does not instantiate it in split mode |
| `AdminGateHandle` | NOT shared — proxy process only | Console constructs `AdminGateService` for its admin API, proxy for gate enforcement |

### 6. `serve --role=console` flag

Add `--role=<all|console>` to `ServeCommand` (default `all`, existing behavior).
When `--role=console`:

- Do NOT bind `:80`/`:443`. The `start_proxy_server` call at `serve/mod.rs:445`
  is skipped entirely.
- Do NOT construct `OnDemandManager`. Scale-to-zero management (sleep/wake
  decisions, idle sweep) belong to the proxy process.
- Do NOT start the `RouteReloadSubscriber`. The console does not serve routes.
- Do NOT spawn the proxy-log batch writer thread.
- Do NOT call `temps_agents::preview_gateway::spawn_reconcile` (it targets
  proxy-facing Docker network). Alternatively, this could remain in the console
  if the preview gateway reconciler is decoupled from the route-serving path —
  that is an implementation-time decision.
- DO start all plugin-lifecycle services (cert renewal, backups, monitoring,
  cron, etc.).
- DO bind the console address (`TEMPS_CONSOLE_ADDRESS`) as today.
- DO continue to publish `Job::ForceRouteReload` and `NOTIFY route_table_changes`
  after deployments — the console's deploy pipeline and the proxy's PG listener
  are the only cross-process coordination points.
- DO `temps migrate` must run before `temps serve --role=console` starts
  (unchanged from today — migrations run separately or at startup).

The `--role` flag on `ProxyCommand` is NOT added. The existing `temps proxy` is
the proxy role; no new flag is needed there.

### 7. Ops: systemd units, upgrade sequence, and `temps upgrade`

**Systemd units (split mode):**

```ini
# /etc/systemd/system/temps-proxy.service
[Unit]
Description=Temps Proxy (Pingora, port 80/443)
After=network.target

[Service]
ExecStart=/usr/local/bin/temps proxy \
  --address 0.0.0.0:80 \
  --tls-address 0.0.0.0:443 \
  --database-url ${TEMPS_DATABASE_URL} \
  --console-address 127.0.0.1:3001
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/temps-console.service
[Unit]
Description=Temps Console (Axum API + dashboard)
After=network.target temps-proxy.service

[Service]
ExecStart=/usr/local/bin/temps serve --role=console \
  --console-address 0.0.0.0:3001 \
  --database-url ${TEMPS_DATABASE_URL}
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
```

**Zero-downtime console upgrade sequence:**

1. Download the new binary to `/usr/local/bin/temps-new`.
2. Run `temps-new migrate` (or confirm `temps serve --role=console` auto-migrates;
   migrations must be backward-compatible with the running proxy binary during
   the upgrade window — see schema-skew risk in Consequences).
3. `systemctl stop temps-console` — dashboard and new-deploy API become
   unavailable. Production traffic on `:80`/`:443` continues uninterrupted
   because the proxy unit is running independently.
4. Replace `/usr/local/bin/temps` with `temps-new`.
5. `systemctl start temps-console` — console starts against new binary, same DB.
   Proxy's route table is already current via PG NOTIFY (routes did not change
   during the upgrade window, and any NOTIFY fired during the window will be
   re-applied when the proxy's listener re-subscribes after network hiccup).
6. Proxy continues serving without interruption. Operators optionally restart
   the proxy unit separately on a maintenance window to pick up any proxy-binary
   changes.

The proxy restart (step 6) is optional and decoupled from the console upgrade.
A console upgrade never requires a proxy restart unless the new binary ships
proxy-specific changes (Pingora config, proxy-log schema, etc.).

**`temps upgrade` implications:**

The existing `temps upgrade` command (`crates/temps-cli/src/commands/upgrade.rs`)
downloads and replaces the binary, then restarts the process. In split mode, it
must:

- Detect split mode (check for a `TEMPS_ROLE` env var, or presence of the
  `temps-proxy.service` unit, or an explicit `--split` flag).
- In split mode: upgrade and restart only the console unit; log a notice that
  the proxy unit requires a separate, scheduled restart.
- In all-in-one mode: unchanged.

A `--split` flag on `temps upgrade` (e.g. `temps upgrade --split`) is the
simplest approach; deploy scripts set it when they provision split units.

**Deploy-script integration:**

`scripts/deploy.sh` should grow a `--topology=split` option that provisions
two systemd units instead of one. Until that lands, operators can follow the
manual steps above. The existing `--mode=local|quick|testing|advanced` options
are orthogonal to the split topology and remain unchanged.

## Rejected alternatives

### i. Keep the monolith and accept console-restart downtime

Pros: no new complexity; upgrade is one `systemctl restart temps`.

Cons: `:80`/`:443` drops for 2–10 seconds on every version upgrade or
config change. Unacceptable for production. This is the exact problem the ADR
is solving.

### ii. `serve --role=proxy` instead of reusing `temps proxy`

Folding the proxy role into a `--role=proxy` flag on `ServeCommand` was
considered. It would give a single binary entry point for both roles. But:

- `temps proxy` already exists, is maintained, and serves exactly this purpose.
  Duplicating its startup path inside `ServeCommand` increases surface area and
  divergence risk.
- `temps proxy` has its own clean set of CLI flags and env vars. A `--role=proxy`
  on serve would either duplicate them (confusing) or inherit serve's broader
  flag set (misleading — e.g. `--role=proxy --console-address` would be
  meaningless noise).

The user has explicitly decided: proxy side = existing `temps proxy`. This ADR
adds only `serve --role=console` on the console side.

### iii. Replace the broadcast queue with a PG-backed durable job queue

A PG-backed job queue (e.g. `pgmq`, or a `job_events` table with a worker
poll loop) would let `ForceRouteReload` cross process boundaries without
`NOTIFY`. Pros: reliable delivery, replayable, inspectable.

Cons: substantially heavier than PG `NOTIFY`; introduces a polling delay or a
second `LISTEN` subscription; the existing `NOTIFY route_table_changes`
mechanism already crosses process boundaries reliably (it is the operative path
for multi-node worker nodes today). Adding a durable queue is a larger
architectural change that solves a reliability problem we do not currently have.
PG `NOTIFY` with the bounded re-resolve correctness guarantee is sufficient.

### iv. Graceful Pingora hot-reload / `SO_REUSEPORT` socket handoff within one binary

Pingora supports graceful upgrade via Unix socket passing (`pingora::server::Server::run_forever`
with the `upgrade` flag). The new binary binds a new Pingora server that
inherits the listening sockets from the old one via `SCM_RIGHTS`, draining the
old server while the new one accepts new connections. This keeps one binary and
eliminates the proxy restart downtime.

Pros: no two processes to operate; no PG-NOTIFY latency trade-off.

Cons:
- Requires orchestration from the outside (a watchdog / init integration to
  signal the old process and track the new one).
- The console and proxy remain coupled: a console crash still brings down the
  proxy binary. A console bug that panics the tokio runtime can corrupt the
  Pingora state if they share process memory.
- The socket-handoff protocol requires the old and new binary to speak the
  same Pingora upgrade wire format — a breaking change in Pingora's upgrade
  path would require a coordinated binary version bump.
- Process split gives stronger blast-radius isolation: a console OOM or panic
  cannot affect the proxy's in-flight connections.

Process split is simpler to operate, stronger in isolation, and is the
natural extension of the half-built `temps proxy` command. The Pingora
hot-reload approach is a valid alternative for a future "zero-downtime proxy
binary upgrade" feature and is not mutually exclusive with this ADR.

## Consequences

### Positive

- **Zero-downtime console upgrades**: upgrading the dashboard/API/plugin layer
  no longer drops in-flight TCP connections on `:80`/`:443`.
- **Blast-radius isolation**: a console panic, OOM, or stuck migration does not
  affect the proxy's ability to serve production traffic. The `temps serve`
  comment at `serve/mod.rs:413-415` documents this intent; the split makes it
  a hard process boundary.
- **Proxy is already feature-complete** for this split except for two well-scoped
  gaps (on-demand and admin gate). The implementation risk is low.
- **All-in-one `temps serve` is unchanged**: single-box operators see no new
  complexity.

### Negative

- **PG-NOTIFY latency on the split proxy**: route reloads after a new
  deployment take 100–400 ms to reach the proxy (vs. <5 ms in the monolith).
  This is the explicit, known trade-off for the split, and is already the
  operative path for multi-node workers. First requests to a newly-deployed
  environment in split mode may hit the console's SPA catch-all for up to ~500 ms.
- **On-demand wake slightly slower in split mode**: the in-process
  `ForceRouteReload` after a wake is inert cross-process; the proxy observes
  the route reload only via PG `NOTIFY` + the sleeping callback. The correctness
  guarantee (bounded re-resolve loop at `proxy.rs:2610-2660`) remains in place;
  only latency is affected.
- **Two systemd units to operate**: operators must manage proxy + console
  lifecycle separately. `temps upgrade` must be extended (see §7).
- **Schema-skew risk during upgrade window**: between `systemctl stop
  temps-console` and `systemctl start temps-console` with the new binary, the
  proxy runs against a DB that may have been migrated by the new binary's
  startup. Migrations that touch tables the proxy reads
  (`deployment_containers`, `environments`, `domains`, `deployments`,
  `proxy_logs`, `on_demand_*`) must be backward-compatible with the N-1 proxy
  binary. This is a new operational constraint that did not exist in the monolith.
  The mitigation is: run `temps migrate` before stopping the old console, and
  ensure all migrations in a release are backward-compatible with the previous
  proxy binary (additive-only column additions, no renames or type changes
  until the proxy has also been upgraded).
- **`ContainerLifecycle` construction in proxy**: the standalone proxy must
  import `temps_deployer::docker::DockerRuntime`. This is a compile-time
  dependency the proxy didn't previously have. Acceptable — it is already an
  indirect dependency via the `temps-deployer` crate in the workspace.

### Risks

- **Docker unavailability in the proxy process** (Phase 2): if Docker is down
  when the proxy starts, `OnDemandManager` is not constructed, and scale-to-zero
  is silently disabled — exactly the behavior already documented in
  `serve/mod.rs:250-257`. The risk is pre-existing; the proxy should log a
  clearly visible warning. This is not a regression.
- **NOTIFY delivery gaps during Postgres restarts**: if PG is restarted,
  `LISTEN` subscriptions are lost. Both `RouteTableListener` and
  `ProjectChangeListener` must re-subscribe on reconnect (this is existing
  behavior; verify it handles reconnect correctly before shipping Phase 1 in
  production).
- **Operator error**: operators may upgrade only the console and never upgrade
  the proxy, leading to long-lived schema-skew. The `temps doctor` command
  should detect and warn when `proxy_binary_version != console_binary_version`
  (stored as a DB row on startup).

## Phased implementation plan

### Phase 1 — Low-risk groundwork (no behavior change in monolith)

Target files:

- `crates/temps-cli/src/commands/proxy.rs`: wire `AdminGateService::new` and
  pass the resulting handle into `setup_proxy_server` (replacing `None` at
  `proxy.rs:294`). Import admin gate modules from `serve/admin_gate_service.rs`
  (extract to a shared module if needed, or re-export from `temps-cli`).
- `crates/temps-cli/src/commands/serve/mod.rs`: add `--role=<all|console>`
  flag to `ServeCommand`. When `role == console`, skip the `start_proxy_server`
  call at `serve/mod.rs:445`, skip `on_demand_manager` construction at
  `serve/mod.rs:261-282`, and skip `spawn_reconcile` at `serve/mod.rs:353`.
  All other console initialization (plugins, background workers, Axum bind)
  runs unchanged.
- `crates/temps-cli/src/lib.rs`: no changes needed — `Proxy` and `Serve` are
  already separate dispatch arms at `:198-197`.
- `scripts/` (optional): add `--topology=split` to `deploy.sh` to emit the two
  systemd unit files.

No behavior change when `--role=all` (the default).

### Phase 2 — On-demand cross-process (scale-to-zero in split mode)

Target files:

- `crates/temps-cli/src/commands/proxy.rs`: construct `DockerRuntime`,
  `ContainerLifecycleAdapter`, and `OnDemandManager` (mirroring
  `serve/mod.rs:238-282`). Register the sleeping callback on `route_table`
  (mirroring `serve/mod.rs:290-318`). Call `on_demand_manager.start_sweep_task`.
  Pass `Some(on_demand_manager)` to `setup_proxy_server` (replacing `None` at
  `proxy.rs:293`).
- `crates/temps-cli/src/commands/serve/proxy.rs` (or a new shared module):
  extract `ContainerLifecycleAdapter` into a location importable from both
  `commands/serve/proxy.rs` and `commands/proxy.rs` (e.g. move to
  `crates/temps-proxy/src/on_demand_adapter.rs` or expose from `temps-deployer`).
- Verify: a sleeping environment wakes correctly when only `temps proxy` is
  running (console offline). The bounded re-resolve loop at `proxy.rs:2610-2660`
  is the correctness path; confirm `notify_route_reloaded()` is called by the
  sleeping callback in the standalone proxy as described in §2.

**Status: implemented.** `temps proxy` now constructs the `OnDemandManager` from
the local Docker socket (best-effort: on-demand is disabled if Docker is
unavailable), registers the sleeping callback via the extracted
`build_on_demand_sleeping_callback` / `register_on_demand_sleeping_callback`
helpers before the route-table listener's first load, starts the idle sweep, and
passes `Some(on_demand_manager)` to `setup_proxy_server`. Rather than relocating
`ContainerLifecycleAdapter` to a new crate, the `commands::serve::proxy` module
was made `pub(crate)` so the standalone command reuses the existing adapter.
security-auditor APPROVE (faithful parity with `temps serve`; no new
request-path surface).

**Operator note (Docker trust boundary):** in the split topology `temps proxy`
requires — and is trusted with — access to the local Docker socket
(root-equivalent on most hosts), because the containers it wakes/sleeps run on
its host. This is the same capability the single-binary `temps serve` already
grants its in-process proxy, but operators running a dedicated proxy process
should not expose that socket more broadly than expected. `local_node_id` is
`None` (control-plane host), so the proxy only manages `node_id=NULL` containers
and never touches a remote worker's containers; running `temps proxy` on a
worker host would silently skip that worker's own deployments (a functional gap,
not a cross-node action) — a self-node-id resolution is deferred hardening.

### Phase 3 — Ops/upgrade integration (shipped)

**Status: implemented**, scoped down from the original plan per the automation
boundary below. What shipped:

- **Version-skew detector.** The console records its own binary version on
  startup in `AppSettings.console_version` (`crates/temps-core/src/app_settings.rs`),
  written via `ConfigService::update_setting_field` in `serve/mod.rs` for **both**
  `--role=all` and `--role=console`. The standalone `temps proxy` reads it on
  startup (`crates/temps-cli/src/commands/proxy.rs`) and logs WARN on mismatch,
  INFO on match, DEBUG when absent — via a pure, total `compare_versions(proxy,
  console) -> SkewStatus` helper (never panics on garbage/absent, never blocks
  startup). `console_version` is self-recorded state: it is intentionally absent
  from `AppSettingsResponse` and the PATCH path so an operator cannot overwrite it.
- **`temps upgrade --split`.** Opt-in flag that, after the binary swap, **prints**
  the split-topology console-restart steps (restart the operator-run console,
  confirm via `curl /readyz`). The default `temps upgrade` output is unchanged.
  It does **not** run `systemctl`, does **not** restart/manage any process, and
  does **not** poll any health endpoint — see the automation boundary below.

Deliberately **not** done here (deferred / out of scope): emitting systemd units,
a `deploy.sh --topology=split` mode, and a self-hosting-guide doc section. systemd
is owned by the operator's `deploy.sh` (which lives in the deploy tooling, not this
repo), not by the `temps` binary.

### Automation boundary

The split topology has a deliberate split of responsibility over who restarts what:

| When | What | Owner / mechanism |
|---|---|---|
| **Install time** | systemd unit setup | The operator's `deploy.sh` — automated at install (operator runs the installer). Not the `temps` binary. |
| **Runtime** | the **proxy** (`temps proxy`, binds :80/:443) | A **systemd-managed, always-on** service with `Restart=on-failure`. This is the piece that must never blink. |
| **Runtime** | the **console** (`temps serve --role=console`) | **Operator-run and operator-restarted.** Intentionally **not** auto-managed by `temps` — it is whatever the operator runs it as (a manual process, a custom unit, a supervised job). |
| **Upgrade time** | console restart | The **operator's manual action.** `temps upgrade --split` only **prints** the steps; it never executes them. |

The principle (CLAUDE.md: *let the user configure and control their setup — show
status, give instructions, don't do things silently on their behalf*): `temps`
records the information needed to make a safe call (the version-skew warning) and
prints the guidance, but never silently restarts, manages, or health-checks a
process on the operator's behalf. The always-on proxy is the only systemd-managed
half; upgrading the console is a deliberate operator action.

## References

- `crates/temps-cli/src/commands/proxy.rs` — existing `ProxyCommand` implementation
- `crates/temps-cli/src/commands/serve/mod.rs` — `ServeCommand`, monolith startup
- `crates/temps-proxy/src/proxy.rs:2535-2599` — on-demand wake hot path
- `crates/temps-proxy/src/proxy.rs:463` — `with_on_demand_manager`
- `crates/temps-proxy/src/on_demand.rs:28-37` — `ContainerLifecycle` trait
- `crates/temps-proxy/src/on_demand.rs:113-124` — queue and `route_reloaded` Notify fields
- `crates/temps-proxy/src/on_demand.rs:240` — `notify_route_reloaded()`
- `crates/temps-proxy/src/on_demand.rs:251-262` — `wait_for_route_reload` lost-wakeup semantics
- `crates/temps-proxy/src/on_demand.rs:860-886` — `publish_route_reload` + `notify_route_change`
- `crates/temps-proxy/src/on_demand.rs:903` — `start_sweep_task`
- `crates/temps-queue/src/queue.rs:41-42` — `BroadcastQueueService` (tokio broadcast, not cross-process)
- `crates/temps-cli/src/lib.rs:49,198` — `Proxy` dispatch
- Project memory: `project_route_reload_inprocess`, `project_on_demand_wake_not_synchronous`

---

## Production-Readiness Plan (Single-Node, Tier B)

### Scope and "zero-downtime" contract

Tier B targets a single-node deployment (one host running both `temps-proxy` and
`temps-console` as independent systemd units) carrying real production traffic.
The zero-downtime guarantee is **scoped to console upgrades only**: when
`temps upgrade` replaces and restarts `temps-console`, the proxy continues
serving `:80`/`:443` without interruption and the console is not declared ready
until its `/readyz` endpoint confirms plugin init and DB reachability. Proxy
upgrades still cause a brief `:80`/`:443` blip — this is the explicit trade-off
of the split topology and is documented below under scope exclusions. The
all-in-one `temps serve` mode is untouched throughout.

---

### Workstream 1 — Core split (ADR Phases 1 and 2, already specified)

This plan builds on top of the phased implementation plan above; those items are
not re-specified here. For tracking purposes, their combined estimate is included
in the roll-up table.

**Prerequisite for all downstream workstreams.** The items in §Phases 1-2 must
land before any Tier B item below can be shipped to production.

| Item | File targets | Effort | Risk |
|---|---|---|---|
| Phase 1: admin gate wiring in `temps proxy` | `proxy.rs:294` | 0.5 d | Low |
| Phase 1: `serve --role=console` flag | `serve/mod.rs:445` | 0.5 d | Low |
| Phase 2: `OnDemandManager` in standalone proxy | `proxy.rs:293`, `serve/mod.rs:238-282` | 1.0 d | Med |
| Phase 2: sleeping callback wired in proxy | `proxy.rs` (new), mirrors `serve/mod.rs:290-318` | 0.5 d | Med |

**Phase 1+2 subtotal: ~2.5 d**

---

### Workstream 2 — Health and readiness endpoints

**Problem.** The console's only readiness signal today is a oneshot channel
sent at `console.rs:1697` (dual-listener path) and `console.rs:1726`
(single-listener path) — both fire immediately after `TcpListener::bind`
succeeds and before `axum::serve` starts, and critically before plugin two-phase
init completes. "Ready" currently means "port open," not "plugins initialized
and DB reachable." A zero-downtime upgrade gate that polls the port only catches
bind success, not service readiness, which means the proxy could begin routing
console API calls to a console that is still warming up and returning 500s.

**Required.**

**2-A. Liveness route `GET /healthz`** on the console. Returns 200 with a
minimal JSON body (`{"status":"alive"}`) immediately if the process is alive.
The proxy's `/healthz` (Pingora side) is simpler — an always-200 Pingora
service route, needed for systemd `ExecStartPost` and future LB probes.

```rust
// Illustrative — console router registration
.route("/healthz", get(|| async { Json(serde_json::json!({"status":"alive"})) }))
```

File targets: `crates/temps-cli/src/commands/serve/console.rs` (router
construction), `crates/temps-proxy/src/server.rs` (Pingora health service).

**2-B. Readiness route `GET /readyz`** on the console. Returns 200 only after
both conditions hold: (1) all plugins have completed their two-phase init (the
`init_all` phase of `TempsPlugin`), and (2) a lightweight DB liveness probe
succeeds (e.g. `SELECT 1`). Returns 503 with a JSON body describing which
condition is not yet met during warmup.

The readiness state must be tracked via a shared `Arc<AtomicBool>` (or an
`Arc<tokio::sync::watch::Sender<bool>>`) set to `true` by the plugin
orchestrator after all `init` calls complete. The `/readyz` handler reads
this flag and the DB probe at request time. The ready signal at
`console.rs:1697`/`1726` should be left in place for the internal binary
coordination (proxy ↔ console startup ordering in the monolith) but the
upgrade health-gate must use `/readyz`, not the signal.

```rust
// Illustrative — shared readiness state threaded through AppState
pub struct AppState {
    pub is_ready: Arc<AtomicBool>,
    // ...
}

// readyz handler
async fn readyz(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.is_ready.load(Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status":"warming_up","reason":"plugins_initializing"}))).into_response();
    }
    match state.db.execute(Statement::from_string(DatabaseBackend::Postgres, "SELECT 1".to_string())).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status":"ready"}))).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE,
                   Json(json!({"status":"degraded","reason":e.to_string()}))).into_response(),
    }
}
```

File targets: `crates/temps-cli/src/commands/serve/console.rs` (router +
`AppState`), plugin orchestrator (wherever `init_all` is called — add the flag
flip there).

**Acceptance criteria:** `curl -s http://localhost:3001/readyz` returns 503
during plugin init warmup, then 200 once plugins are live; `curl -s
http://localhost:80/_temps/healthz` (or equivalent Pingora-side path) always
returns 200 while the proxy process is alive.

| Item | File targets | Effort | Risk |
|---|---|---|---|
| 2-A: `/healthz` on console + Pingora side | `console.rs`, `server.rs` | 0.5 d | Low |
| 2-B: `/readyz` with plugin-init flag + DB probe | `console.rs`, AppState, plugin orchestrator | 1.0 d | Med |

**Workstream 2 subtotal: ~1.5 d**

---

### Workstream 3 — `temps upgrade` split-aware orchestration

**Problem.** `upgrade.rs:run_oss` (`:181`) downloads the binary, checksums it,
calls `replace_binary` at `:320`, then prints `"sudo systemctl restart temps"`
at `:466` and exits. It does not restart anything, does not detect split mode,
and has no health-gate. `run_ee` (`:339`) follows the same pattern. The existing
`update_systemd_license_env` at `:1010-1055` edits `/etc/systemd/system/temps.service`
and calls `systemctl daemon-reload` — this is the pattern to extend, not
replace.

**Required.**

**3-A. Split-mode detection.** Detect split topology by checking (in order of
preference): (1) presence of `/etc/systemd/system/temps-console.service` on
disk; (2) env var `TEMPS_ROLE=console`; (3) explicit `--split` flag on `temps
upgrade`. In split mode, the upgrade targets `temps-console` only. Emit a
prominent notice that `temps-proxy.service` was NOT restarted and should be
restarted on a scheduled maintenance window.

**3-B. Console restart + readiness poll.** After `replace_binary`:
1. `systemctl restart temps-console` (or `stop` + `start` for rollback safety).
2. Poll `GET http://<console_address>/readyz` with 200ms intervals, 60-second
   timeout. The console address is read from the systemd unit's `ExecStart`
   (`--console-address` flag) or from the `TEMPS_CONSOLE_ADDRESS` env var.
3. On 200 within timeout: print success message and exit 0.
4. On timeout or repeated 503: print a clear error, tell the operator the old
   binary is no longer on disk (the replace was already atomic), provide the
   rollback command (`temps upgrade --version <prev>` or manual binary swap),
   and exit 1.

```rust
// Illustrative — health poll with timeout
async fn poll_readyz(addr: &str, timeout: Duration) -> anyhow::Result<()> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let deadline = Instant::now() + timeout;
    let url = format!("http://{}/readyz", addr);
    loop {
        if Instant::now() > deadline {
            anyhow::bail!("Console did not become ready within {:?}. \
                           Check logs: journalctl -u temps-console -n 50", timeout);
        }
        match client.get(&url).send().await {
            Ok(r) if r.status() == 200 => return Ok(()),
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}
```

**Acceptance criteria:** `temps upgrade` in split mode restarts only
`temps-console`, polls `/readyz`, prints a success line when it reaches 200,
and exits 1 with a rollback hint on timeout.

| Item | File targets | Effort | Risk |
|---|---|---|---|
| 3-A: split detection + notice | `upgrade.rs` (~`:181`, `:339`) | 0.5 d | Low |
| 3-B: `systemctl restart` + `/readyz` poll + rollback message | `upgrade.rs` | 1.0 d | Med |

**Workstream 3 subtotal: ~1.5 d**

---

### Workstream 4 — Stable console address and systemd units

**Problem 4-A: random port.** `ServerConfig` defaults `console_address` to a
random available port via `get_random_console_address()` at
`temps-config/src/service.rs:136` (implementation at `:281`). The proxy
reverse-proxies all `/api` and `/_temps` traffic to `console_address` per
`temps-proxy/src/services.rs:79`. In split mode, the proxy process starts with
a configured address pointing at the console; if the console binds a different
random port on restart, the proxy 502s on every console-proxied request until
its config is reloaded. A stable, explicitly-configured address is required.

**4-A. Stable address enforcement.** In split mode (`--role=console`), if
`console_address` resolved to a random port (i.e. neither `TEMPS_CONSOLE_ADDRESS`
env var nor `--console-address` flag was supplied), the console must fail at
startup with a clear error:

```
ERROR: split mode requires an explicit console address.
Set TEMPS_CONSOLE_ADDRESS (e.g. 127.0.0.1:3001) in the environment
or pass --console-address. Random port assignment is not supported in split mode.
```

Both the proxy's `--console-address` argument and the console's
`TEMPS_CONSOLE_ADDRESS` must agree. The deploy script (Workstream 4-B below)
enforces this at provisioning time.

File targets: `crates/temps-config/src/service.rs:136` (add split-mode guard),
`crates/temps-cli/src/commands/serve/mod.rs` (read role flag, pass to config
validation).

**4-B. Systemd unit files and `deploy.sh --topology=split`.** The two unit
stubs in ADR §7 are correct in shape but live only in the ADR prose. Tier B
requires them to be emitted by `scripts/deploy.sh` so operators get them via
the standard install path. Add `--topology=split` to `deploy.sh` alongside the
existing `--mode` and `--channel` options (per project memory:
`project_deploy_sh_onboarding_modes`). When `--topology=split` is set, the
script emits `temps-proxy.service` and `temps-console.service` instead of the
single `temps.service`. Key requirements beyond the ADR stub:

- `temps-console.service` must include `After=postgresql.service` (or the
  TimescaleDB container service) in addition to `network.target`.
- `temps-proxy.service` must include `After=network.target` only — it must NOT
  block on `temps-console` coming up; the proxy is designed to serve cached
  routes while the console is offline.
- Both units use `Restart=on-failure`, `RestartSec=5`, `RestartMaxDelaySec=30`.
- `temps-console.service` must carry `Environment=TEMPS_CONSOLE_ADDRESS=127.0.0.1:3001`
  (or operator-chosen value) so the address is stable across restarts.
- `temps-proxy.service` must carry `--console-address 127.0.0.1:3001` as an
  `ExecStart` argument that matches the console's env.

File targets: `scripts/deploy.sh` (new `--topology` branch).

**Acceptance criteria:** `deploy.sh --topology=split` emits two unit files,
`systemctl daemon-reload && systemctl enable --now temps-proxy temps-console`
brings both up, and `systemctl status` shows both active. Console startup fails
with a descriptive error if `TEMPS_CONSOLE_ADDRESS` is absent and role is
`console`.

| Item | File targets | Effort | Risk |
|---|---|---|---|
| 4-A: stable address guard in split mode | `service.rs:136`, `serve/mod.rs` | 0.5 d | Low |
| 4-B: `deploy.sh --topology=split` + two unit files | `scripts/deploy.sh` | 1.0 d | Low |

**Workstream 4 subtotal: ~1.5 d**

---

### Workstream 5 — Schema-skew safety

**Problem.** During a zero-downtime console upgrade, between
`systemctl stop temps-console` and the new console finishing its migrations and
becoming ready, the proxy is running against a DB schema version it was not
compiled against. The proxy reads `deployment_containers`, `environments`,
`domains`, `deployments`, `proxy_logs`, `on_demand_configs`, and related tables
(ADR Consequences §negative, schema-skew note in §2 on-demand). A migration
that renames or drops a column the proxy reads will silently corrupt proxied
traffic.

**This workstream is primarily discipline and tooling, not a large code change.**

**5-A. Migration-compatibility rule (process).** Establish and document: any
migration in a release that touches a proxy-read table must be additive-only
(new nullable columns, new tables, new indexes). Column renames, type changes,
and drops for proxy-read columns must be deferred to the release *after* the
proxy has also been upgraded. Document the list of proxy-read tables in
`crates/temps-proxy/README.md` or a `docs/operations/schema-skew.md` file so
reviewers can check. This is an ongoing process discipline, not a one-time cost.
Effort below is for writing the doc and adding a comment block above relevant
migration files.

**5-B. Version-skew detector.** On console startup, write the console binary
version to a stable row in the DB (e.g. a `settings` row with
`key = 'console_version'`, or a new `system_versions` table with columns
`component TEXT PRIMARY KEY, version TEXT, updated_at TIMESTAMPTZ`). On proxy
startup, read this row and log a structured WARNING if the versions differ:

```
WARN component_skew: proxy_version="v0.1.1" console_version="v0.1.0" \
     message="running ahead of console; proxy-read tables must be backward-compatible"
```

The `temps doctor` command (`crates/temps-cli/src/commands/doctor.rs` or
equivalent) should emit a human-readable check that surfaces this skew:

```
[WARN] Version skew detected
       Proxy:   v0.1.1
       Console: v0.1.0
       Action: upgrade the proxy to v0.1.1 during the next maintenance window.
```

The supported skew window is: proxy may run N (current) while console is N-1
(previous minor), or console may run N while proxy is N-1. Skew of more than
one minor version is unsupported and should be flagged as ERROR in `temps
doctor`.

File targets: `crates/temps-cli/src/commands/serve/console.rs` (write version
row on startup), `crates/temps-cli/src/commands/proxy.rs` (read + warn on
skew), `crates/temps-cli/src/commands/doctor.rs` (skew check), new migration
for `system_versions` table.

**Acceptance criteria:** After a console upgrade with proxy not yet upgraded,
proxy logs show a structured `component_skew` WARN line on startup; `temps
doctor` prints the skew check with action text; the system continues to function
(warning only, not a hard failure).

| Item | File targets | Effort | Risk |
|---|---|---|---|
| 5-A: proxy-read table doc + migration review rule | `docs/operations/schema-skew.md` | 0.25 d | Low |
| 5-B: version-skew detector (write on console start, read on proxy start, `temps doctor`) | `console.rs`, `proxy.rs`, `doctor.rs`, new migration | 1.0 d | Low |

**Workstream 5 subtotal: ~1.25 d**

---

### Workstream 6 — Failure-test matrix

The ADR claims graceful degradation across several failure scenarios.
Production-ready means each claim is demonstrated against a real binary on a
real host, not just asserted. The table below is the required test pass; results
must be documented in `docs/operations/split-topology-test-report.md` before the
topology is declared Tier B production-ready.

#### Failure-test matrix

| Scenario | Test procedure | Expected behavior | Pass criteria |
|---|---|---|---|
| **S1: console killed mid-deploy** | Start a deploy, then `kill -9 $(pidof temps)` targeting the console PID only | Proxy continues serving `:80`/`:443`; in-flight proxied requests complete; new-deploy API returns 503 or connection-refused | No `:80`/`:443` error seen by curl during kill; proxy access log shows no 5xx for app traffic; `temps deploy status` shows the deploy as failed/incomplete |
| **S2: console restart (happy upgrade path)** | `systemctl restart temps-console` while proxy is running | No `:80`/`:443` blip; dashboard returns 503 during warmup then 200 once `/readyz` is green; proxy routes unchanged | `curl -s http://app.example.com/` returns 200 throughout; `curl -s http://console/readyz` transitions 503→200; total console-down window < 30 s |
| **S3: bad new console version (crashing binary)** | Replace console binary with a build that panics at startup; `systemctl restart temps-console` | Proxy keeps serving all app traffic; `temps upgrade` reports health-poll timeout; rollback message is printed | App traffic uninterrupted throughout; `temps upgrade` exits 1 with rollback instructions; `systemctl status temps-console` shows `failed` |
| **S4: on-demand wake while console offline** | Stop console (`systemctl stop temps-console`); request a URL that routes to a sleeping on-demand environment | Proxy wakes the environment via Docker + PG NOTIFY; environment becomes reachable; no 502 after the bounded re-resolve loop completes | HTTP request eventually returns 200 (may take up to 10 s for cold start); no permanent 502; proxy access log shows successful forward after re-resolve |
| **S5: proxy restart (acknowledged blip)** | `systemctl restart temps-proxy` while console is running and apps are serving traffic | Brief `:80`/`:443` outage for the duration of the restart (~2-5 s); console dashboard remains reachable via its own port during the blip | Proxy restart completes in < 10 s; after restart, app traffic resumes; blip duration measured and documented |
| **S6: PG NOTIFY gap (Postgres restart)** | Restart the PostgreSQL service while both processes are running; then trigger a new deployment | Both proxy and console reconnect to PG; proxy re-subscribes to LISTEN channels; route table refreshes after deployment | After PG restart, `temps deploy` succeeds; proxy routes to new deployment within 60 s of deploy completion; no stale routes remain |
| **S7: version skew warning** | Upgrade console binary without upgrading proxy binary | Proxy startup log contains structured `component_skew` WARN; `temps doctor` shows skew | `journalctl -u temps-proxy | grep component_skew` returns a line; `temps doctor` exits with a non-zero code or prints WARN |

Each scenario requires a documented pass/fail result with the binary version
tested, the host spec, and the timestamp.

| Item | File targets | Effort | Risk |
|---|---|---|---|
| Execute S1-S7 on a real host (cpx22 Hetzner or equivalent) and document results | `docs/operations/split-topology-test-report.md` | 1.5 d | Med |

**Workstream 6 subtotal: ~1.5 d**

---

### Workstream 7 — Observability verification under split

Both processes must emit structured logs and any future metrics under their own
systemd unit identifiers so `journalctl -u temps-proxy` and `journalctl -u
temps-console` give independent, parseable streams. This is largely a
verification exercise, not new code, but two specific items require changes.

**7-A. Verify proxy-log writer in standalone proxy.** The proxy-log batch writer
is spawned as a background thread in `setup_proxy_server` at
`temps-proxy/src/server.rs:238-246`. Confirm it is live in `temps proxy`
standalone mode (it should be — `setup_proxy_server` is called from both
`serve/mod.rs` and `proxy.rs`). No code change expected; verification only.

**7-B. Confirm console background workers are console-only.** The background
tasks listed in ADR §4 (cert renewal, monitoring, backups, cron, etc.) are all
spawned inside `start_console_api` or the plugin `init` phase. Confirm that none
of them attempt to reach the proxy's internal ports or the Pingora runtime. The
preview gateway reconciler (`spawn_reconcile`, skipped in `--role=console` per
ADR §6) is the one item that touches Docker networking from the console side;
confirm the `--role=console` skip is correct and does not leave orphaned Docker
network state.

**7-C. Log correlation.** Proxy and console logs will be in separate journals.
Ensure request IDs (if implemented) or at least deploy IDs and project IDs are
present in both process's log output for key operations (e.g., a deploy that
triggers a route reload should produce a log line in the console with the
deploy ID, and a corresponding route-reload log line in the proxy). No new
instrumentation required if already present; document what exists.

**Acceptance criteria:** After running the failure-test matrix (Workstream 6),
both `journalctl -u temps-proxy -o json` and `journalctl -u temps-console -o
json` produce valid JSONL with no interleaved output; proxy-log rows appear in
the DB for app traffic that arrived while the console was offline (S1, S4).

| Item | File targets | Effort | Risk |
|---|---|---|---|
| 7-A: verify proxy-log writer in standalone mode | `server.rs:238-246` (read-only) | 0.25 d | Low |
| 7-B: confirm console-only background workers + reconciler skip | `serve/mod.rs`, plugin init | 0.25 d | Low |
| 7-C: log correlation audit | structured log output (read-only) | 0.25 d | Low |

**Workstream 7 subtotal: ~0.75 d**

---

### Roll-up

| Workstream | Description | Effort (dev-days) | Risk |
|---|---|---|---|
| WS 1 | Core split (Phases 1 + 2 from ADR) | 2.5 | Med |
| WS 2 | Health + readiness endpoints (`/healthz`, `/readyz`) | 1.5 | Med |
| WS 3 | `temps upgrade` split-aware orchestration | 1.5 | Med |
| WS 4 | Stable console address + systemd unit files + `deploy.sh --topology=split` | 1.5 | Low |
| WS 5 | Schema-skew safety (doc + detector) | 1.25 | Low |
| WS 6 | Failure-test matrix (7 scenarios, real host) | 1.5 | Med |
| WS 7 | Observability verification | 0.75 | Low |
| **Total** | | **~10.5 d** | |

At 5 productive days/week this is **~2 weeks of engineering time**, matching the
Tier B estimate. The estimate assumes one engineer and does not include review
cycles, which add 20-30% buffer.

**Critical path:** WS 1 (core split) is the strict prerequisite for WS 3, WS 4,
WS 6, and WS 7. WS 2 (`/readyz`) is the strict prerequisite for WS 3 (the
upgrade health-gate polls `/readyz`). Therefore the critical path is:

```
WS 1 (2.5 d) → WS 2 (1.5 d) → WS 3 (1.5 d) → WS 6 (1.5 d) → WS 7 (0.75 d)
                    ↓
                WS 4 (1.5 d)   [can run parallel to WS 3]
                WS 5 (1.25 d)  [can run parallel to WS 3 or WS 4]
```

Critical-path length: **7.25 d** (WS 1 + 2 + 3 + 6 + 7 serially, assuming WS 4
and 5 complete in parallel).

**Deliberately out of scope for Tier B:**

- Multi-node split (separate hosts for proxy and console) — requires
  authenticated cross-node `/readyz` polling and network security review.
- Console HA / active-passive replica — requires distributed lock for
  background workers (cert renewal, cron, backups must not run on both consoles
  simultaneously).
- Proxy binary hot-reload via `SO_REUSEPORT` / Pingora graceful upgrade — deferred
  to a future Tier C (see Rejected Alternatives §iv).
- Automated rollback (binary swap on health-gate failure) — too risky to
  automate without operator confirmation; Tier B requires operator-executed
  rollback with clear instructions.

---

### Definition of Done for Tier B production

An operator or reviewer can sign off when all of the following are true:

- [ ] `temps proxy` passes S4 (on-demand wake with console offline) without a 502.
- [ ] `GET /readyz` on the console returns 503 during plugin init and 200 after.
- [ ] `temps upgrade` in split mode restarts only `temps-console`, polls `/readyz`,
  and exits 0 on success / 1 with rollback instructions on timeout.
- [ ] `deploy.sh --topology=split` emits two systemd unit files that bring both
  processes up via `systemctl enable --now`.
- [ ] Console startup fails with a descriptive error if `TEMPS_CONSOLE_ADDRESS`
  is unset in split mode.
- [ ] All 7 failure-test scenarios (S1-S7) pass on a real host with results
  documented in `docs/operations/split-topology-test-report.md`.
- [ ] `temps doctor` shows a WARN with action text when proxy and console binary
  versions differ.
- [ ] Both process journals produce valid structured JSONL under their own unit
  names; proxy-log rows appear in DB for traffic that arrived while the console
  was offline.
