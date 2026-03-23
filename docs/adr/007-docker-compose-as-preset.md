# ADR-007: Docker Compose as a Project Preset (Not a Parallel System)

**Status:** Proposed
**Date:** 2026-03-23
**Author:** David Viejo

## Context

The current compose stacks implementation (`temps-compose` crate) is a **complete parallel system** — separate entity (`compose_stacks`), separate handler (`/stacks/*`), separate executor, separate UI. This creates problems:

- No git push deployments (the project pipeline handles that)
- No preview environments per PR
- No environment variable management (the env system already exists)
- No deployment history or rollbacks
- Separate route/domain management from existing domain system
- Port conflicts between stacks require complex validation and remapping UI
- No multi-node support (compose executor runs locally only)

Users want to deploy public Docker Compose repos (ClickHouse, PostgreSQL clusters, etc.) that they can't modify. All configuration — port mapping, service routing, env overrides — must happen through the Temps UI, not by editing compose files.

## Decision

**Docker Compose becomes a preset (`docker-compose`) in the existing project pipeline.** No parallel system. One project, one pipeline, one deployment model.

### How It Works End-to-End

#### 1. Project Creation

User creates a project with preset `docker-compose`:

```
Project:
  name: "my-clickhouse"
  preset: docker-compose
  repo: github.com/ClickHouse/examples  (or no repo — inline compose)
  directory: ./docker-compose
  compose_path: "docker-compose.yml"    (new field in preset config)
```

Uses existing fields: `repo_owner`, `repo_name`, `main_branch`, `directory`, `git_provider_connection_id`. The `compose_path` lives in `PresetConfig::DockerCompose`.

#### 2. Deployment Pipeline

The existing pipeline has job types: `DownloadRepoJob → BuildImageJob → DeployContainerJob`. For compose, the sequence becomes:

```
DownloadRepoJob (same — clone repo, checkout branch)
    ↓
ComposeDeployJob (NEW — replaces Build + Deploy for compose presets)
    ↓
    1. Read compose file from repo at {directory}/{compose_path}
    2. Merge environment variables from Temps env var system
    3. Create isolated Docker network: temps-{project_id}-{env_id}
    4. Run: docker compose -f <file> -p temps-{project_id}-{env_id} up -d --pull always
    5. Wait for health checks (compose healthcheck or TCP port probe)
    6. Discover running containers and their ports
    7. Insert rows into deployment_containers (one per service)
    8. Register proxy routes for services with exposed ports
```

No Dockerfile generation. No image build. No image push. The compose file IS the deployment artifact.

#### 3. Container Tracking — Multiple Containers per Deployment

Currently `deployment_containers` has one row per deployment. For compose, one deployment creates N containers. Add a `service_name` column:

```sql
ALTER TABLE deployment_containers ADD COLUMN service_name VARCHAR(255);
```

```
deployment_id | container_id | container_name         | service_name | container_port | host_port
42            | abc123       | temps-2-3-web-1        | web          | 8080           | NULL
42            | def456       | temps-2-3-redis-1      | redis        | 6379           | NULL
42            | ghi789       | temps-2-3-postgres-1   | postgres     | 5432           | NULL
```

**Key change:** `host_port` is NULL for compose services. They communicate via Docker network, not host port binding. The proxy connects to the container IP directly on the Docker network. This eliminates the entire port conflict problem.

#### 4. Routing — Domain per Exposed Service

**Auto-generated routes (default):**

Every compose service with `ports` defined gets a subdomain:

```
{service_name}-{env_slug}.{preview_domain}
```

Examples:
```
web-production.preview.temps.dev        → container abc123:8080
adminer-production.preview.temps.dev    → container def456:8080
```

Internal services (redis, postgres — no ports or marked internal) get NO route.

**Custom domain mapping (UI):**

Users configure in project settings which services map to which domains:

```
Service Routes (configured in UI):
┌──────────────────────────────────────────────────────────┐
│ Service    │ Port  │ Domain                │ Status      │
├────────────┼───────┼───────────────────────┼─────────────┤
│ web        │ 8080  │ app.example.com       │ ● routed    │
│ api        │ 3000  │ api.example.com       │ ● routed    │
│ adminer    │ 8080  │ (auto-generated)      │ ● routed    │
│ redis      │ 6379  │ —                     │ ○ internal  │
│ postgres   │ 5432  │ —                     │ ○ internal  │
└──────────────────────────────────────────────────────────┘
```

This config is stored as a new entity `compose_service_routes` (per environment):

```
project_id | environment_id | service_name | target_port | domain        | enabled
2          | 3              | web          | 8080        | app.example.com | true
2          | 3              | api          | 3000        | NULL            | true   (auto-subdomain)
2          | 3              | redis        | 6379        | NULL            | false  (internal)
```

#### 5. Proxy Integration

The `UpstreamResolver` currently resolves: `domain → deployment → container[0]`.

For compose, it becomes: `domain → deployment → container WHERE service_name = X`.

The proxy already supports this — `deployment_containers` has `container_id` and `container_port`. The resolver just needs to match on the route's service_name to pick the right container from the deployment.

**Docker networking:** Compose services run on an isolated Docker network (`temps-{project_id}-{env_id}`). The Temps proxy container (Pingora) needs to be connected to this network to reach the containers. This is done by `docker network connect` after compose up.

#### 6. Environment Variables

The existing env var system works unchanged:
- Project-level vars → injected as `environment:` in all services
- Environment-level vars → override for specific environments
- `include_in_preview` flag → controls preview env injection

Compose `.env` files from the repo are read and imported as env vars on first deploy, then managed through the Temps UI going forward.

#### 7. Preview Environments

Works exactly like other presets:
- Git push to `feature-x` → create preview environment
- `docker compose -p temps-{project_id}-{preview_env_id} up -d`
- Each preview gets its own Docker network and containers
- Routes: `web-feature-x.preview.temps.dev`
- On PR merge: `docker compose down` + remove containers + delete env

#### 8. Rollbacks

The deployment history tracks which compose file content was deployed (stored in deployment metadata). Rollback = re-deploy with the previous deployment's compose content.

#### 9. Multi-Node

For now, compose deployments run on the node where the project is assigned (same as single-container deployments). Future: could split services across nodes, but that's a separate decision.

### What Changes

| Component | Change | Effort |
|-----------|--------|--------|
| `temps-entities/preset.rs` | Add `DockerCompose` variant + `DockerComposeConfig` | Small |
| `temps-presets` | Add `DockerComposePreset` struct implementing `Preset` trait | Small |
| `temps-entities/deployment_containers.rs` | Add `service_name: Option<String>` column | Small |
| `temps-deployer` | New `ComposeDeploymentExecutor` (compose up/down/ps) | Medium |
| `temps-deployments` | New `ComposeDeployJob` job type in pipeline | Medium |
| `temps-entities` | New `compose_service_routes` entity for per-service routing config | Small |
| `temps-proxy` | Extend `UpstreamResolver` to route by service_name | Small |
| `temps-deployments` handler | Endpoint to configure service routes per environment | Small |
| Web UI | Compose service routes config in project settings | Medium |
| Web UI | Multi-container view in deployment detail | Medium |
| Migration | Add `service_name` to `deployment_containers`, create `compose_service_routes` table | Small |

### What Gets Deleted (After Migration)

| Component | Why |
|-----------|-----|
| `compose_stacks` entity | Replaced by projects with docker-compose preset |
| `compose_stack_routes` entity | Replaced by `compose_service_routes` |
| `ComposeExecutor` in temps-compose | Replaced by `ComposeDeploymentExecutor` in temps-deployer |
| `ComposeService` in temps-compose | Logic moves into deployment pipeline |
| `port_validator` module | Port conflicts eliminated by Docker networking |
| `/stacks/*` API endpoints | Projects API handles everything |
| Web UI stacks section | Projects UI with compose-specific views |

### Implementation Order

1. **Entity + Migration**: Add `DockerCompose` preset, `service_name` column, `compose_service_routes` table
2. **Preset**: `DockerComposePreset` in temps-presets
3. **Deployer**: `ComposeDeploymentExecutor` — compose pull/up/down/ps
4. **Pipeline Job**: `ComposeDeployJob` — integrates executor into deployment pipeline
5. **Proxy**: Extend upstream resolver for service-name routing
6. **UI**: Project creation with compose preset, service routes config, multi-container deployment view
7. **Cleanup**: Remove temps-compose parallel system

## Consequences

### Positive
- Compose stacks get git push deploys, preview environments, rollbacks, audit logging for free
- No port conflicts — Docker networking handles isolation
- Single UI for all project types
- Route configuration via UI, not compose file modification
- Multi-node deployment path is clear (same as other presets)

### Negative
- Larger refactor than iterating on the current parallel system
- Existing compose stacks need migration to projects
- Compose-specific features (multiple containers, service discovery) add complexity to the deployment_containers model

### Risks
- Docker compose commands are slower than single container operations
- Pingora proxy needs Docker network access to reach compose containers (requires `docker network connect`)
- Some compose files use features (build context, depends_on conditions) that may need special handling

## Alternatives Considered

### A. Keep the parallel system and iterate
Rejected because: doubles every feature (env vars, preview envs, domains, audit logs, rollbacks) and creates permanent maintenance burden.

### B. Compose as a wrapper Dockerfile (docker-in-docker)
Rejected because: requires elevated privileges, hides container state, makes routing to individual services impossible.

### C. Extract main service only, ignore other compose services
Rejected because: defeats the purpose — users want the full stack (app + db + cache) deployed together.
