# v0.0.7 Release Checklist

## 1. Commit pending changes

- [ ] Commit all 27 modified files with proper conventional commit message
- [ ] Files span: `temps-deployer`, `temps-deployments`, `temps-entities`, `temps-projects`, `temps-routes`, `temps-git`, `temps-presets`, and `web/`

## 2. Backend verification

- [ ] `cargo check --lib` — no errors
- [ ] `cargo test --lib` — all tests pass
- [ ] `cargo build --bin temps` — debug binary builds
- [ ] `cargo build --release --bin temps` — release binary builds

## 3. Regenerate API client

- [ ] Start server with new binary
- [ ] `cd web && bun run openapi-ts` — regenerate types from updated OpenAPI spec
- [ ] Verify generated `types.gen.ts` includes `service_url`, `service_name`, `public_ports`, `ComposePublicPort`
- [ ] Remove any manual type additions (they'll be replaced by generated ones)

## 4. Frontend verification

- [ ] `cd web && bun run build` — builds without errors
- [ ] No leftover `console.log` debug statements

## 5. Docker Compose features

- [ ] Create project from public repo with docker-compose preset
- [ ] Deploy succeeds (DownloadRepo → DeployCompose → MarkComplete)
- [ ] Containers page shows all compose services (clickhouse, keeper, etc.)
- [ ] Container detail loads (no "Container not found" error)
- [ ] Container logs stream correctly
- [ ] Port override: compose override with remapped ports → no port conflict
- [ ] Volume preservation: redeploy keeps volumes, delete removes them
- [ ] Compose files with `build:` directives → images built from repo

## 6. Public ports

- [ ] No public ports configured → no proxy routes for compose services (private by default)
- [ ] Configure public ports in Git Settings → proxy route created
- [ ] Public port URL resolves and routes to correct container:port
- [ ] DNS labels stay under 63 chars (truncation works)
- [ ] Autocomplete suggestions shown from compose override content

## 7. Git Settings / Preset selection

- [ ] Edit Settings → docker-compose preset is pre-selected (not full catalog flash)
- [ ] Compose-specific fields visible (compose path, compose override, public ports)
- [ ] Public ports shown in read-only view as badges
- [ ] Compose override persists after save and reload
- [ ] Public repo preset detection works (detectPublicPresets API)
- [ ] Branch listing works for repos with >30 branches (pagination)

## 8. Container management

- [ ] Container start/stop/restart actions work
- [ ] Container exec (if enabled) works
- [ ] Per-service URL shown in container list and detail (for public ports only)
- [ ] Full container IDs stored for new deployments (no short ID mismatch)

## 9. Sidebar / Navigation

- [ ] Request Logs appears under Observability submenu
- [ ] Request Logs link (`/request-logs`) renders the page (not empty)
- [ ] All other sidebar links still work

## 10. Regression testing

- [ ] Regular (non-compose) project deploy still works
- [ ] Static site deploy still works
- [ ] Analytics / session replay pages load
- [ ] Environment variables injection works
- [ ] Custom domains resolve correctly
- [ ] Container logs WebSocket streaming works
- [ ] Error tracking page loads
- [ ] Uptime monitors page loads

## 11. Release artifacts

- [ ] Update `CHANGELOG.md` with v0.0.7 entries
- [ ] Tag `v0.0.7`
- [ ] Build release binary: `cargo build --release --bin temps`
- [ ] Update `deploy.sh` / `worker.sh` / `install.sh` if version is hardcoded
- [ ] Push tag + create GitHub release

## 12. Post-release

- [ ] Deploy to Temps Cloud (Hetzner) and verify
- [ ] Smoke test: create project, deploy, check containers, check analytics
- [ ] Update docs if needed

---

## Key changes in v0.0.7

### Added
- Docker Compose as a deployment preset (ADR-007) — deploy multi-container apps via git-push pipeline
- Public ports model — explicit control over which compose service ports are proxied publicly (private by default)
- Compose override — remap ports, volumes, commands without modifying the original compose file
- Per-service proxy routing — each public compose service gets its own subdomain
- Container exec and persistent terminal — WebSocket-based xterm.js shell (opt-in per project)
- Per-service URLs in container list and detail views
- Public ports configuration UI with autocomplete from compose file
- Request Logs in project sidebar under Observability
- Project slug truncation (40 char max) for DNS-safe subdomains

### Fixed
- Docker Compose port override conflict — ports now stripped from base file when override defines them
- Container detail "not found" — prefix matching for short vs full container IDs
- Full container ID resolution — compose deployments now store 64-char IDs from Docker inspect
- Main deployment route round-robin — only routes to first compose service, not all
- DNS label truncation — service URLs and route labels capped at 63 chars
- FrameworkSelector flash — no longer shows full catalog while preset detection loads
- Branch listing pagination — public repos with >30 branches now list all
- Compose fields visibility — handles both `docker-compose` and `dockercompose` slug variants
- Preset pre-selection — injects current preset+path into detected list if not found
