# v0.0.6 Release Testing Checklist

## Core Deployment

- [x] git push deploys successfully (build + container start)
- [x] Preview environments create on PR branches
- [x] Deployment promotion between environments works
- [x] Rollback to previous deployment works

## Multi-Node / Cluster

- [x] temps join connects a worker node (direct or relay mode)
- [x] Deployments schedule on worker nodes (LeastLoaded)
- [x] Node drain migrates containers off a node
- [x] Node health check marks stale nodes offline
- [x] Cross-node routing works (proxy routes to worker containers)

## Password Protection

- [x] Set password on environment -> password wall appears immediately
- [x] Correct password -> cookie set, passes through
- [x] Change password -> old cookie invalidated
- [x] Remove password (empty string) -> wall disappears immediately

## On-Demand Scale-to-Zero

- [x] Environment goes to sleep after idle timeout
- [x] Incoming request wakes environment automatically
- [x] Wake/sleep API endpoints work

## Environment Protection

- [ ] Protected environments block direct deploys
- [ ] Branch restrictions enforced

## AI Gateway / GenAI Tracing

- [x] AI provider keys CRUD works
- [x] Proxy routes AI requests through gateway
- [x] OTel gen_ai spans show in traces UI
- [x] Token usage analytics display correctly

## Analytics

- [x] Funnel cards show step pipeline with conversions
- [x] Funnel edit page loads correctly
- [x] Property drill-downs (country -> region -> city, etc.)
- [x] Channels, Devices, Languages, OS charts render

## External Services

- [x] Create standalone PostgreSQL/Redis/MongoDB/S3
- [x] Service cluster init (PostgreSQL HA) — replication and failover working
- [ ] Backup/restore works (sidecar pg_dump)
- [ ] Remote service creation on worker nodes

## Proxy Features

- [x] Accept: text/markdown returns markdown conversion
- [x] Security headers applied from project settings
- [x] IP access control works
- [x] CAPTCHA challenge works

## Other

- [x] OTel ingest (traces/metrics/logs) via OTLP
- [x] External plugins load and route correctly
- [x] MCP server responds to tool calls
- [x] Encrypted env vars decrypt properly in containers
- [x] Cron jobs fire with CRON_SECRET header
- [x] Domain management pagination + search
- [x] Mobile responsiveness on environment settings

## Release Mechanics

- [ ] scripts/release.sh bumps version, tags, pushes
- [ ] GitHub Actions builds Linux/macOS binaries + Docker image
- [ ] Migration runs cleanly on fresh and existing databases
