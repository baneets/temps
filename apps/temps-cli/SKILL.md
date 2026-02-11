# Temps CLI - Complete Reference

Temps CLI is the command-line interface for the Temps deployment platform. It provides full control over projects, deployments, services, domains, monitoring, and platform configuration.

## Installation

```bash
# Run directly without installing
npx @temps-sdk/cli --version
bunx @temps-sdk/cli --version

# Or install globally
npm install -g @temps-sdk/cli
bun add -g @temps-sdk/cli
```

> **Note:** All examples below use `temps` as the command. If running via `npx`/`bunx`, replace `temps` with `npx @temps-sdk/cli` or `bunx @temps-sdk/cli`.

## Configuration

```bash
# Interactive configuration wizard
temps configure

# Set API URL
temps configure set apiUrl https://your-server.example.com:3000

# Set default output format (table, json, minimal)
temps configure set outputFormat table

# View current configuration
temps configure show

# List all config values
temps configure list

# Reset to defaults
temps configure reset
```

**Config file**: `~/.temps/config.json`
**Credentials file**: `~/.temps/.secrets` (mode 0600)

**Environment variables** (override config):
| Variable | Description |
|---|---|
| `TEMPS_API_URL` | Override API endpoint |
| `TEMPS_TOKEN` | API token (highest priority) |
| `TEMPS_API_TOKEN` | API token (CI/CD) |
| `TEMPS_API_KEY` | API key |
| `NO_COLOR` | Disable colored output |

## Global Options

```
-v, --version    Display version number
--no-color       Disable colored output
--debug          Enable debug output
-h, --help       Display help for command
```

---

## Authentication

### Login

```bash
# Interactive login (prompts for API key)
temps login

# Non-interactive login
temps login --api-key tk_abc123def456

# Login to specific server
temps login --api-key tk_abc123def456 -u https://temps.example.com
```

**Example output:**
```
  Authenticating...
  Logged in as david@example.com (Admin)
  Credentials saved to ~/.temps/.secrets
```

### Logout

```bash
temps logout
```

**Example output:**
```
  Credentials cleared
```

### Who Am I

```bash
temps whoami
temps whoami --json
```

**Example output:**
```
  Current User
  ID        1
  Email     david@example.com
  Name      David
  Role      Admin
```

**JSON output:**
```json
{
  "id": 1,
  "email": "david@example.com",
  "name": "David",
  "role": "admin"
}
```

---

## Projects

**Aliases**: `project`, `p`

### List Projects

```bash
temps projects list
temps projects ls --json
temps projects list --page 2 --per-page 10
```

**Example output:**
```
  Projects (3)
  ┌──────┬──────────────┬────────┬──────────────┬─────────────────────┐
  │ Name │ Slug         │ Preset │ Environments │ Created             │
  ├──────┼──────────────┼────────┼──────────────┼─────────────────────┤
  │ Blog │ blog         │ nextjs │ 2            │ 2025-01-15 10:30:00 │
  │ API  │ api-backend  │ nodejs │ 1            │ 2025-01-14 08:00:00 │
  │ Docs │ docs-site    │ static │ 1            │ 2025-01-12 14:00:00 │
  └──────┴──────────────┴────────┴──────────────┴─────────────────────┘
```

### Create Project

```bash
# Interactive creation
temps projects create

# Non-interactive
temps projects create -n "My App" -d "Description of my app" --repo https://github.com/org/repo
```

### Show Project

```bash
temps projects show -p my-app
temps projects show -p my-app --json
```

**Example output:**
```
  My App
  ID            5
  Slug          my-app
  Description   My application
  Preset        nextjs
  Main Branch   main
  Repository    org/my-app
  Created       1/15/2025, 10:30:00 AM
  Updated       1/20/2025, 3:45:00 PM
```

### Update Project

```bash
# Update name and description
temps projects update -p my-app -n "New Name" -d "New description"

# Non-interactive mode
temps projects update -p my-app -n "New Name" -y
```

### Update Project Settings

```bash
# Update slug and enable attack mode
temps projects settings -p my-app --slug new-slug --attack-mode

# Enable preview environments
temps projects settings -p my-app --preview-envs

# Disable attack mode
temps projects settings -p my-app --no-attack-mode
```

### Update Git Settings

```bash
temps projects git -p my-app --owner myorg --repo myrepo --branch main --preset nextjs
temps projects git -p my-app --directory apps/web --preset nextjs -y
```

### Update Deployment Config

```bash
# Scale replicas and set resource limits
temps projects config -p my-app --replicas 3 --cpu-limit 1 --memory-limit 512

# Enable auto-deploy
temps projects config -p my-app --auto-deploy
```

### Delete Project

```bash
temps projects delete -p my-app
temps projects rm -p my-app -f    # Skip confirmation
```

---

## Deployments

### Deploy from Git

```bash
# Interactive deployment
temps deploy my-app

# Specify branch and environment
temps deploy my-app -b feature/new-ui -e staging

# Fully automated
temps deploy -p my-app -b main -e production -y
```

**Example output:**
```
  Deploying my-app
  Branch        main
  Environment   production

  Deployment started (ID: 42)
  Building...
  Pushing image...
  Starting containers...
  Deployment successful!
```

### Deploy Static Files

```bash
# Deploy a directory
temps deploy:static --path ./dist -p my-app

# Deploy an archive
temps deploy:static --path ./build.tar.gz -p my-app -e production -y
```

### Deploy Docker Image

```bash
# Deploy a pre-built image
temps deploy:image --image ghcr.io/org/app:v1.0 -p my-app

# With environment and automation
temps deploy:image --image registry.example.com/app:latest -p my-app -e staging -y
```

### Deploy Local Docker Image

```bash
# Build from Dockerfile and deploy
temps deploy:local-image -p my-app -f Dockerfile -c .

# Deploy an existing local image
temps deploy:local-image --image my-app:latest -p my-app -e production -y

# With build arguments
temps deploy:local-image -p my-app --build-arg NODE_ENV=production --build-arg API_URL=https://api.example.com
```

### List Deployments

```bash
temps deployments list -p my-app
temps deployments ls -p my-app --limit 5 --json
temps deployments list -p my-app --page 2 --per-page 10 --environment-id 1
```

**Example output:**
```
  Deployments (3)
  ┌────┬─────────┬────────────┬────────────┬──────────┬─────────────────────┐
  │ ID │ Branch  │ Env        │ Status     │ Duration │ Created             │
  ├────┼─────────┼────────────┼────────────┼──────────┼─────────────────────┤
  │ 42 │ main    │ production │ ● running  │ 2m 15s   │ 2025-01-20 15:30:00 │
  │ 41 │ develop │ staging    │ ● running  │ 1m 45s   │ 2025-01-20 14:00:00 │
  │ 40 │ main    │ production │ ○ stopped  │ 3m 02s   │ 2025-01-19 10:00:00 │
  └────┴─────────┴────────────┴────────────┴──────────┴─────────────────────┘
```

### Deployment Status

```bash
temps deployments status -p my-app -d 42
temps deployments status -p my-app -d 42 --json
```

### Deployment Lifecycle

```bash
# Rollback to previous deployment
temps deployments rollback -p my-app -e production

# Rollback to specific deployment
temps deployments rollback -p my-app --to 40

# Cancel a running deployment
temps deployments cancel -p 5 -d 42

# Pause/resume
temps deployments pause -p 5 -d 42
temps deployments resume -p 5 -d 42

# Teardown (remove all resources)
temps deployments teardown -p 5 -d 42
```

### Deployment Logs

```bash
# View logs
temps logs -p my-app

# Stream logs in real-time
temps logs -p my-app -f

# Show last 50 lines from staging
temps logs -p my-app -e staging -n 50

# Logs for specific deployment
temps logs -p my-app -d 42 -f
```

---

## Environments

### List Environments

```bash
temps environments list -p my-app
temps environments ls -p my-app --json
```

### Create Environment

```bash
temps environments create -p my-app -n staging
```

### Delete Environment

```bash
temps environments delete -p my-app -n staging
temps environments rm -p my-app -n staging -f
```

### Environment Variables

```bash
# List all variables
temps environments vars list -p my-app -e production
temps environments vars list -p my-app -e production --json

# Get a specific variable
temps environments vars get -p my-app -e production -k DATABASE_URL

# Set a variable
temps environments vars set -p my-app -e production -k API_KEY -v "sk_live_abc123"

# Set a secret variable (masked in UI)
temps environments vars set -p my-app -e production -k SECRET_KEY -v "supersecret" --secret

# Delete a variable
temps environments vars delete -p my-app -e production -k OLD_KEY -y

# Import from .env file
temps environments vars import -p my-app -e production -f .env.production

# Export to file
temps environments vars export -p my-app -e production -f .env.backup
```

### Environment Resources

```bash
temps environments resources -p my-app -e production --json
```

### Scale Environment

```bash
temps environments scale -p my-app -e production --replicas 3
```

### Cron Jobs

```bash
# List cron jobs
temps environments crons list --project-id 5

# Show cron job details
temps environments crons show --id 1 --json

# List cron executions
temps environments crons executions --cron-id 1 --limit 10
```

---

## Services (Databases, Caches, Storage)

**Alias**: `svc`

### List Services

```bash
temps services list
temps services ls --json
```

**Example output:**
```
  External Services (2)
  ┌────┬───────────┬────────────┬─────────────┬──────────┐
  │ ID │ Name      │ Type       │ Version     │ Status   │
  ├────┼───────────┼────────────┼─────────────┼──────────┤
  │ 1  │ main-db   │ PostgreSQL │ 16-alpine   │ ● active │
  │ 2  │ cache     │ Redis      │ 7-alpine    │ ● active │
  └────┴───────────┴────────────┴─────────────┴──────────┘
```

### Create Service

```bash
# Interactive creation
temps services create

# Non-interactive
temps services create -t postgres -n main-db -y
temps services create -t redis -n cache -y
temps services create -t mongodb -n data-store -y
temps services create -t s3 -n files -y

# With custom parameters
temps services create -t postgres -n analytics-db --parameters '{"version":"17-alpine"}' -y
```

**Service types**: `postgres`, `mongodb`, `redis`, `s3`

### Show Service

```bash
temps services show --id 1
temps services show --id 1 --json
```

**Example output:**
```
  main-db
  ID            1
  Type          PostgreSQL
  Version       16-alpine
  Status        ● active
  Connection    postgresql://user:pass@localhost:5432/main
  Created       1/15/2025, 10:30:00 AM
  Updated       1/20/2025, 3:45:00 PM

  Parameters
  max_connections   200
  shared_buffers    256MB
```

### Service Lifecycle

```bash
# Start/stop
temps services start --id 1
temps services stop --id 1

# Update
temps services update --id 1 -n postgres:17-alpine

# Upgrade version
temps services upgrade --id 1 -v postgres:17-alpine

# Remove
temps services remove --id 1
temps services rm --id 1 -f
```

### Import Existing Service

```bash
# Import a running Docker container as a managed service
temps services import -t postgres -n imported-db --container-id my-postgres-container -y
```

### Link/Unlink to Projects

```bash
# Link service to project (injects env vars)
temps services link --id 1 --project-id 5

# Unlink
temps services unlink --id 1 --project-id 5

# View linked projects
temps services projects --id 1

# View injected env vars
temps services env --id 1 --project-id 5

# Get specific env var
temps services env-var --id 1 --project-id 5 --var DATABASE_URL
```

### List Service Types

```bash
temps services types
temps services types --json
```

---

## Git Providers

### List Providers

```bash
temps providers list
temps providers ls --json
```

### Add Provider

```bash
# Interactive
temps providers add

# Non-interactive
temps providers add --type github --name "My GitHub" --token ghp_abc123 -y
temps providers add --type gitlab --name "My GitLab" --token glpat-abc123 -y
```

### Manage Providers

```bash
temps providers show --id 1 --json
temps providers activate --id 1
temps providers deactivate --id 1
temps providers remove --id 1 -f

# Safe delete (checks for dependencies)
temps providers safe-delete --id 1 -y
temps providers deletion-check --id 1 --json
```

### Git Connections

```bash
# Connect git to project
temps providers git connect --project my-app --provider-id 1 --repo org/repo --branch main

# List repos from provider
temps providers git repos --id 1
temps providers git repos --search "my-app" --language typescript --page 1 --per-page 50
temps providers git repos --sort stars --direction desc --owner myorg

# Manage connections
temps providers connections list --json
temps providers connections list --page 1 --per-page 50 --sort account_name --direction asc
temps providers connections show --id 1
temps providers connections sync --id 1
temps providers connections validate --id 1
temps providers connections update-token --id 1 --token ghp_newtoken
temps providers connections activate --id 1
temps providers connections deactivate --id 1
temps providers connections delete --id 1 -y
```

---

## Domains

### List Domains

```bash
temps domains list -p my-app
temps domains ls -p my-app --json
```

### Add Domain

```bash
temps domains add -p my-app -d example.com -y
```

### Verify & Status

```bash
temps domains verify -p my-app -d example.com
temps domains status -p my-app -d example.com --json
temps domains ssl -p my-app -d example.com --json
```

### Remove Domain

```bash
temps domains remove -p my-app -d example.com -f
```

### Certificate Orders

```bash
# List certificate orders
temps domains orders list --json

# Create order
temps domains orders create --domain-id 1 --challenge-type http-01

# Show order details
temps domains orders show --order-id 1 --json

# Finalize (verify challenge and issue certificate)
temps domains orders finalize --domain-id 1

# Cancel order
temps domains orders cancel --order-id 1 -y
```

### DNS Challenges

```bash
# Get DNS challenge record to add
temps domains dns-challenge --domain-id 1 --json

# Debug HTTP challenge accessibility
temps domains http-debug --domain example.com --token abc123 --expected xyz789
```

---

## Custom Domains

**Alias**: `cdom`

```bash
# List custom domains
temps custom-domains list --project-id 5 --json

# Create with environment targeting
temps custom-domains create --project-id 5 -d app.example.com --environment-id 1 -y

# Create redirect domain
temps custom-domains create --project-id 5 -d old.example.com --redirect-to https://new.example.com --status-code 301 -y

# Show details
temps custom-domains show --project-id 5 --domain-id 1 --json

# Update
temps custom-domains update --project-id 5 --domain-id 1 --branch feature/v2

# Link certificate
temps custom-domains link-cert --project-id 5 --domain-id 1 --certificate-id 3

# Remove
temps custom-domains remove --project-id 5 --domain-id 1 -f
```

---

## DNS Management

```bash
# List DNS records
temps dns list --json

# Add record
temps dns add --type A --name app --content 1.2.3.4 --ttl 3600 -y

# Show record
temps dns show --id 1 --json

# Test DNS resolution
temps dns test --name app.example.com --type A --json

# List zones
temps dns zones --json

# Remove record
temps dns remove --id 1 -f
```

---

## DNS Providers

**Alias**: `dnsp`

```bash
# List DNS providers
temps dns-providers list --json

# Create Cloudflare provider
temps dns-providers create -n "Cloudflare" -t cloudflare --api-token cf_abc123 -y

# Create Route53 provider
temps dns-providers create -n "AWS" -t route53 --access-key-id AKIA... --secret-access-key secret --region us-east-1 -y

# Test provider connection
temps dns-providers test --id 1

# List provider zones
temps dns-providers zones --id 1 --json

# Manage domains
temps dns-providers domains list --id 1 --json
temps dns-providers domains add --id 1 -d example.com --auto-manage
temps dns-providers domains verify --provider-id 1 -d example.com
temps dns-providers domains remove --provider-id 1 -d example.com -f

# DNS lookup
temps dns-providers lookup -d example.com --json
```

**Provider types**: `cloudflare`, `namecheap`, `route53`, `digitalocean`, `gcp`, `azure`, `manual`

---

## Notifications

### Notification Providers

```bash
# List providers
temps notifications list --json

# Add Slack provider
temps notifications add --type slack --name "Alerts" --webhook-url https://hooks.slack.com/... --channel "#alerts" -y

# Add Email provider
temps notifications add --type email --name "Email Alerts" --smtp-host smtp.gmail.com --smtp-port 587 --smtp-user user@gmail.com --smtp-pass apppassword --from alerts@example.com --to team@example.com -y

# Add Webhook provider
temps notifications add --type webhook --name "Custom Hook" --url https://example.com/webhook --secret mysecret -y

# Show/manage providers
temps notifications show --id 1 --json
temps notifications enable --id 1
temps notifications disable --id 1
temps notifications update --id 1 --name "New Name"
temps notifications test --id 1
temps notifications remove --id 1 -f
```

### Notification Preferences

**Alias**: `notif-prefs`

```bash
# Show current preferences
temps notification-preferences show --json

# Update preferences
temps notification-preferences update -k email_enabled -v true
temps notification-preferences update -k deployment_failures_enabled -v true
temps notification-preferences update -k ssl_days_before_expiration -v 30
temps notification-preferences update -k minimum_severity -v warning

# Reset to defaults
temps notification-preferences reset -y
```

**Available preference keys:**
- **Boolean**: `email_enabled`, `slack_enabled`, `weekly_digest_enabled`, `batch_similar_notifications`, `deployment_failures_enabled`, `build_errors_enabled`, `runtime_errors_enabled`, `ssl_expiration_enabled`, `domain_expiration_enabled`, `dns_changes_enabled`, `backup_failures_enabled`, `backup_successes_enabled`, `route_downtime_enabled`, `load_balancer_issues_enabled`, `s3_connection_issues_enabled`, `retention_policy_violations_enabled`
- **Numbers**: `error_threshold`, `error_time_window`, `ssl_days_before_expiration`
- **Strings**: `minimum_severity`, `digest_send_time`, `digest_send_day`

---

## Monitoring

### Monitors

```bash
# List monitors
temps monitors list --project-id 5 --json

# Create HTTP monitor
temps monitors create --project-id 5 -n "API Health" -t http -i 60 -y

# Create TCP monitor
temps monitors create --project-id 5 -n "DB Connection" -t tcp -i 300 -y

# Show details
temps monitors show --id 1 --json

# Current status
temps monitors status --id 1 --json

# Uptime history
temps monitors history --id 1 --days 30 --json

# Remove
temps monitors remove --id 1 -f
```

**Monitor types**: `http`, `tcp`, `ping`
**Intervals**: `60`, `300`, `600`, `900`, `1800` seconds

### Incidents

**Alias**: `incident`

```bash
# List incidents
temps incidents list --project-id 5 --status investigating --json
temps incidents list --project-id 5 --page 1 --page-size 20 --environment-id 1

# Create incident
temps incidents create --project-id 5 -t "API Degradation" -d "High response times" -s major -y

# Show incident
temps incidents show --id 1 --json

# Update status
temps incidents update-status --id 1 -s monitoring -m "Fix deployed, monitoring"
temps incidents update-status --id 1 -s resolved -m "Issue resolved"

# List updates
temps incidents updates --id 1 --json

# Bucketed incidents (time series)
temps incidents bucketed --project-id 5 -i hourly --json
```

**Severities**: `critical`, `major`, `minor`
**Statuses**: `investigating`, `identified`, `monitoring`, `resolved`

---

## Containers

**Alias**: `cts`

```bash
# List containers
temps containers list -p 5 -e 1 --json

# Show container details
temps containers show -p 5 -e 1 -c abc123 --json

# Start/stop/restart
temps containers start -p 5 -e 1 -c abc123
temps containers stop -p 5 -e 1 -c abc123
temps containers restart -p 5 -e 1 -c abc123

# Force stop
temps containers stop -p 5 -e 1 -c abc123 -f

# Live metrics (auto-refresh)
temps containers metrics -p 5 -e 1 -c abc123 -w -i 2
temps containers metrics -p 5 -e 1 -c abc123 --json
```

---

## Runtime Logs

**Alias**: `rtlogs`

```bash
# Stream container runtime logs
temps runtime-logs -p my-app

# Follow mode with specific environment
temps runtime-logs -p my-app -e staging -f

# Show specific container
temps runtime-logs -p my-app --container web-1 --tail 200

# JSON output
temps runtime-logs -p my-app --json
```

---

## Backups

### Backup Sources

```bash
# List sources
temps backups sources list --json

# Create source
temps backups sources create -n "Main DB" --source-type postgres --connection-string "postgresql://..." -y

# Show source
temps backups sources show --id 1 --json

# Update source
temps backups sources update --id 1 -n "Primary DB"

# List backups for source
temps backups sources backups --id 1 --json

# Trigger manual backup
temps backups sources run --id 1

# Remove source
temps backups sources remove --id 1 -f
```

### Backup Schedules

```bash
# List schedules
temps backups schedules list --json

# Create schedule
temps backups schedules create --source-id 1 --cron "0 2 * * *" --retention-count 7 --storage-backend local -y

# Show schedule
temps backups schedules show --id 1 --json

# Enable/disable
temps backups schedules enable --id 1
temps backups schedules disable --id 1

# Delete schedule
temps backups schedules delete --id 1 -f
```

### Backups

```bash
# List all backups
temps backups list --json

# Show backup details
temps backups show --id 1 --json

# Run a service backup
temps backups run-service --service-id 1 --json
```

---

## Security Scanning

**Alias**: `scan`

```bash
# List project scans
temps scans list --project-id 5 --json
temps scans list --project-id 5 --page 2 --page-size 10

# Trigger scan
temps scans trigger --project-id 5 --environment-id 1

# Latest scan
temps scans latest --project-id 5 --json

# Scans per environment
temps scans environments --project-id 5 --json

# Show scan details
temps scans show --id 1 --json

# List vulnerabilities
temps scans vulnerabilities --id 1 --json
temps scans vulns --id 1 --severity CRITICAL --json

# Scan by deployment
temps scans by-deployment --deployment-id 42 --json

# Remove scan
temps scans remove --id 1 -f
```

**Severity filter**: `CRITICAL`, `HIGH`, `MEDIUM`, `LOW`

---

## Error Tracking

**Alias**: `error`

```bash
# List error groups
temps errors list --project-id 5 --json
temps errors list --project-id 5 --status unresolved --page 1 --page-size 20
temps errors list --project-id 5 --environment-id 1 --start-date 2025-01-01 --end-date 2025-01-31
temps errors list --project-id 5 --sort-by total_count --sort-order desc

# Show error group
temps errors show --project-id 5 --group-id abc123 --json

# Update error group status
temps errors update --project-id 5 --group-id abc123 --status resolved

# List events for error group
temps errors events --project-id 5 --group-id abc123 --json

# Show single event
temps errors event --project-id 5 --group-id abc123 --event-id evt456 --json

# Statistics
temps errors stats --project-id 5 --json

# Timeline
temps errors timeline --project-id 5 --days 7 --bucket 1h --json

# Dashboard
temps errors dashboard --project-id 5 --days 7 --compare --json
```

**Statuses**: `unresolved`, `resolved`, `ignored`

---

## Webhooks

**Alias**: `hooks`

```bash
# List webhooks
temps webhooks list --project-id 5 --json

# List deliveries with limit
temps webhooks deliveries list --project-id 5 --webhook-id 1 --limit 100 --json

# Create webhook
temps webhooks create --project-id 5 -u https://example.com/webhook -e "deployment.success,deployment.failed" -s mysecret -y

# Show webhook
temps webhooks show --project-id 5 --webhook-id 1 --json

# Update webhook
temps webhooks update --project-id 5 --webhook-id 1 -u https://new-endpoint.com/webhook

# Enable/disable
temps webhooks enable --project-id 5 --webhook-id 1
temps webhooks disable --project-id 5 --webhook-id 1

# List available event types
temps webhooks events --json

# View deliveries
temps webhooks deliveries list --project-id 5 --webhook-id 1 --json
temps webhooks deliveries show --project-id 5 --webhook-id 1 --delivery-id 1 --json

# Retry failed delivery
temps webhooks deliveries retry --project-id 5 --webhook-id 1 --delivery-id 1

# Remove webhook
temps webhooks remove --project-id 5 --webhook-id 1 -f
```

---

## API Keys

**Alias**: `keys`

```bash
# List API keys
temps apikeys list --json

# Create API key
temps apikeys create -n "CI/CD Key" -r developer -e 90 -y

# Create with specific permissions
temps apikeys create -n "Deploy Only" -r developer -p "deployments:create,deployments:read" -e 30 -y

# Show key details
temps apikeys show --id 1 --json

# Activate/deactivate
temps apikeys activate --id 1
temps apikeys deactivate --id 1

# List available permissions
temps apikeys permissions --json

# Remove
temps apikeys remove --id 1 -f
```

**Roles**: `admin`, `developer`, `viewer`, `readonly`
**Expiry**: `7`, `30`, `90`, `365` days

---

## Deployment Tokens

**Alias**: `token`

```bash
# List tokens
temps tokens list -p my-app --json

# Create token
temps tokens create -p my-app -n "Analytics Token" --permissions "analytics:read,events:write" -e 90 -y

# Show token
temps tokens show -p my-app --id 1 --json

# Delete token
temps tokens delete -p my-app --id 1 -f

# List available permissions
temps tokens permissions --json
```

**Permissions**: `*`, `visitors:enrich`, `emails:send`, `analytics:read`, `events:write`, `errors:read`
**Expiry**: `7`, `30`, `90`, `365`, `never`

---

## Users

```bash
# List users
temps users list --json

# Create user
temps users create --email user@example.com --name "New User" --password "secure123" --role developer -y

# Show current user
temps users me --json

# Change user role
temps users role --id 2 --role admin

# Remove user (soft delete)
temps users remove --id 2 -f

# Restore deleted user
temps users restore --id 2
```

---

## DSN (Data Source Names)

```bash
# List DSNs
temps dsn list --project-id 5 --json

# Create DSN
temps dsn create --project-id 5 -n "Production DSN" --environment-id 1 -y

# Get or create DSN (idempotent)
temps dsn get-or-create --project-id 5 --environment-id 1 --json

# Regenerate DSN key
temps dsn regenerate --project-id 5 --dsn-id 1 -f

# Revoke DSN
temps dsn revoke --project-id 5 --dsn-id 1 -f
```

---

## Analytics Funnels

**Alias**: `funnel`

```bash
# List funnels
temps funnels list --project-id 5 --json

# Create funnel
temps funnels create --project-id 5 -n "Signup Funnel" \
  -s '[{"event_name":"page_view","filters":{"path":"/signup"}},{"event_name":"form_submit"},{"event_name":"signup_complete"}]' -y

# Update funnel
temps funnels update --project-id 5 --funnel-id 1 -n "Updated Funnel"

# View funnel metrics
temps funnels metrics --project-id 5 --funnel-id 1 --json

# Preview metrics (without saving)
temps funnels preview --project-id 5 \
  -s '[{"event_name":"page_view"},{"event_name":"signup"}]' --json

# Remove funnel
temps funnels remove --project-id 5 --funnel-id 1 -f
```

---

## Email

### Email Providers

**Alias**: `eprov`

```bash
# List email providers
temps email-providers list --json

# Create SES provider
temps email-providers create -n "AWS SES" -t ses --access-key-id AKIA... --secret-access-key secret --region us-east-1 -y

# Create Scaleway provider
temps email-providers create -n "Scaleway" -t scaleway --api-key scw_abc --project-id proj123 --region fr-par -y

# Test provider
temps email-providers test --id 1 --from noreply@example.com

# Remove
temps email-providers remove --id 1 -f
```

### Email Domains

**Alias**: `edom`

```bash
# List domains
temps email-domains list --json

# Create email domain
temps email-domains create -d example.com --provider-id 1 -y

# Show domain
temps email-domains show --id 1 --json

# Get DNS records to configure
temps email-domains dns-records --id 1 --json

# Auto-setup DNS records
temps email-domains setup-dns --id 1 --dns-provider-id 2

# Verify domain
temps email-domains verify --id 1

# Remove
temps email-domains remove --id 1 -f
```

### Emails

**Alias**: `email`

```bash
# List sent emails
temps emails list --json
temps emails list --page 1 --page-size 20 --status delivered
temps emails list --domain-id 1 --project-id 5 --from-address noreply@example.com

# Send email
temps emails send --to user@example.com --subject "Hello" --body "Welcome!" --from noreply@example.com -y

# Show email details
temps emails show --id 1 --json

# Email statistics
temps emails stats --json

# Validate email address
temps emails validate --email user@example.com --json
```

---

## IP Access Control

**Alias**: `ipa`

```bash
# List rules
temps ip-access list --json

# Allow an IP
temps ip-access create --ip 203.0.113.0/24 --action allow --description "Office network" -y

# Block an IP
temps ip-access create --ip 198.51.100.5 --action deny --description "Suspicious traffic" -y

# Check if IP is blocked
temps ip-access check --ip 198.51.100.5 --json

# Update rule
temps ip-access update --id 1 --description "Updated description"

# Remove rule
temps ip-access remove --id 1 -f
```

---

## Load Balancer

**Alias**: `lb`

```bash
# List routes
temps load-balancer list --json

# Create route
temps load-balancer create -d app.example.com -t http://localhost:8080 -y

# Show route
temps load-balancer show -d app.example.com --json

# Update route
temps load-balancer update -d app.example.com -t http://localhost:9090

# Remove route
temps load-balancer remove -d app.example.com -f
```

---

## Audit Logs

```bash
# List audit logs
temps audit list --limit 50 --json

# With pagination and filters
temps audit list --limit 20 --offset 40
temps audit list --operation-type PROJECT_CREATED --user-id 1
temps audit list --from 2025-01-01T00:00:00Z --to 2025-01-31T23:59:59Z

# Show audit log entry
temps audit show --id 1 --json
```

**Example output:**
```
  Audit Logs (50)
  ┌────┬────────────────────┬───────────────────┬──────────────┬──────────────┬─────────────────────┐
  │ ID │ Operation          │ User              │ IP           │ Location     │ Date                │
  ├────┼────────────────────┼───────────────────┼──────────────┼──────────────┼─────────────────────┤
  │ 42 │ PROJECT_CREATED    │ david@example.com │ 203.0.113.1  │ Madrid, ES   │ 2025-01-20 15:30:00 │
  │ 41 │ DEPLOYMENT_STARTED │ david@example.com │ 203.0.113.1  │ Madrid, ES   │ 2025-01-20 15:28:00 │
  └────┴────────────────────┴───────────────────┴──────────────┴──────────────┴─────────────────────┘
```

---

## Proxy Logs

**Alias**: `plogs`

```bash
# List proxy logs
temps proxy-logs list --limit 20 --json

# With pagination and filters
temps proxy-logs list --page 2 --limit 50
temps proxy-logs list --project-id 5 --environment-id 1
temps proxy-logs list --method POST --status-code 500
temps proxy-logs list --host app.example.com --path /api/users
temps proxy-logs list --start-date 2025-01-20T00:00:00Z --end-date 2025-01-21T00:00:00Z
temps proxy-logs list --sort-by response_time_ms --sort-order desc
temps proxy-logs list --is-bot --json
temps proxy-logs list --has-error --json

# Show log details
temps proxy-logs show --id 1 --json

# Get log by request ID
temps proxy-logs by-request --request-id req_abc123 --json

# Request statistics
temps proxy-logs stats --json

# Today's statistics
temps proxy-logs today --json
```

---

## Platform Information

**Alias**: `plat`

```bash
# Platform info (OS, architecture)
temps platform info --json

# Access/networking info
temps platform access --json

# Public IP
temps platform public-ip

# Private IP
temps platform private-ip
```

**Example output (`platform access --json`):**
```json
{
  "access_mode": "public",
  "public_ip": "203.0.113.50",
  "private_ip": "10.0.1.5",
  "can_create_domains": true,
  "domain_creation_error": null
}
```

---

## Settings (Platform)

```bash
# Show platform settings
temps settings show --json

# Update settings
temps settings update --preview-domain example.com

# Set external URL
temps settings set-external-url --url https://app.example.com

# Set preview domain
temps settings set-preview-domain --domain preview.example.com
```

---

## Presets & Templates

```bash
# List build presets
temps presets list --json
temps presets list --type server
temps presets list --type static

# Show preset details
temps presets show nextjs --json

# List deployment templates
temps templates list --json
temps templates list --type server
```

---

## Imports

```bash
# List import sources
temps imports sources --json

# Discover workloads
temps imports discover -s docker --json

# Create import plan
temps imports plan -s docker -w my-container

# Execute import
temps imports execute -s docker -w my-container -y

# Check import status
temps imports status --session-id sess_abc123 --json
```

**Workflow**: `sources` -> `discover` -> `plan` -> `execute` -> `status`

---

## Documentation Generation

```bash
# Generate markdown docs
temps docs

# Generate MDX docs
temps docs -f mdx

# Generate JSON docs
temps docs -f json

# Write to file
temps docs -f markdown -o docs/cli-reference.md
```

---

## Temps Cloud

Temps Cloud (`temps.sh`) is a managed hosting service separate from self-hosted Temps. Cloud commands use their own authentication (`cloudApiKey`) and do not interfere with self-hosted credentials.

**Environment variables:**
| Variable | Description |
|---|---|
| `TEMPS_CLOUD_URL` | Override cloud API endpoint (default: `https://temps.sh`) |
| `TEMPS_CLOUD_TOKEN` | Cloud API token (highest priority) |
| `TEMPS_CLOUD_API_KEY` | Cloud API key |

### Cloud Authentication

```bash
# Login via device authorization flow (opens browser)
temps cloud login

# Show current cloud account
temps cloud whoami

# Logout from Temps Cloud
temps cloud logout
```

**Example output (`cloud whoami`):**
```
  Temps Cloud Account
  ────────────────────────
  ID:        42
  Name:      David
  Username:  david
  Email:     david@example.com
  Plan:      pro
```

### Cloud VPS

Manage cloud VPS instances. Public endpoints (images, locations, types) work without authentication.

#### List VPS Instances

```bash
temps cloud vps list
temps cloud vps list --json
```

**Example output:**
```
  VPS Instances (2)
  ──────────────────────────────────────────────────────────────
  ID           │ Hostname        │ Status    │ IPv4          │ Type  │ Price
  ─────────────┼─────────────────┼───────────┼───────────────┼───────┼────────
  abc12def     │ vps-abc12def    │ ● active  │ 49.12.100.50  │ cx22  │ €4.51/mo
  xyz34ghi     │ vps-xyz34ghi    │ ● error   │ pending       │ cx32  │ €7.49/mo
```

#### Create VPS Instance

```bash
# Interactive wizard (image → location → server type)
temps cloud vps create

# Non-interactive
temps cloud vps create --image ubuntu-22.04 --location fsn1 --type cx22
temps cloud vps create --image ubuntu-22.04 --location fsn1 --type cx22 --json
```

#### Show VPS Details

```bash
temps cloud vps show abc12def
temps cloud vps show abc12def --json
```

Shows instance details, server specs, and provisioning logs.

#### Destroy VPS Instance

```bash
# With confirmation prompt
temps cloud vps destroy abc12def
```

#### Retry Failed Provisioning

```bash
temps cloud vps retry abc12def
```

#### Show VPS Credentials

```bash
temps cloud vps credentials abc12def
temps cloud vps credentials abc12def --json
```

Shows web panel URL, username, and password.

#### List Available OS Images (No Auth Required)

```bash
temps cloud vps images
temps cloud vps images --json
```

#### List Available Locations (No Auth Required)

```bash
temps cloud vps locations
temps cloud vps locations --json
```

#### List Server Types with Pricing (No Auth Required)

```bash
temps cloud vps types
temps cloud vps types --location fsn1
temps cloud vps types --json
```

**Example output:**
```
  Server Types for fsn1 (4)
  ─────────────────────────────────────────────────────────────────
  ID    │ Name         │ vCPU │ Memory (GB) │ Disk (GB) │ Price     │ Available
  ──────┼──────────────┼──────┼─────────────┼───────────┼───────────┼──────────
  cx22  │ CX22         │    2 │           4 │        40 │ €4.51/mo  │ yes
  cx32  │ CX32         │    4 │           8 │        80 │ €7.49/mo  │ yes
  cx42  │ CX42         │    8 │          16 │       160 │ €14.99/mo │ yes
  cx52  │ CX52         │   16 │          32 │       320 │ €29.99/mo │ no
```

---

## Common Patterns

### Automation / CI/CD

All write commands support `-y/--yes` to skip interactive prompts:

```bash
# Full CI/CD pipeline
export TEMPS_TOKEN=tk_abc123
export TEMPS_API_URL=https://temps.example.com

temps deploy my-app -b main -e production -y
temps environments vars set -p my-app -e production -k VERSION -v "1.2.3"
temps scans trigger --project-id 5 --environment-id 1
```

### JSON Output

Every list/show command supports `--json` for scripting:

```bash
# Get project ID from slug
temps projects show -p my-app --json | jq '.id'

# List running services
temps services list --json | jq '.[] | select(.status == "running")'

# Check deployment status
temps deployments status -p my-app -d 42 --json | jq '.status'
```

### Command Aliases

| Full Command | Short Alias |
|---|---|
| `temps projects` | `temps p` |
| `temps services` | `temps svc` |
| `temps containers` | `temps cts` |
| `temps deployments` | `temps deploys` |
| `temps runtime-logs` | `temps rtlogs` |
| `temps webhooks` | `temps hooks` |
| `temps proxy-logs` | `temps plogs` |
| `temps apikeys` | `temps keys` |
| `temps tokens` | `temps token` |
| `temps custom-domains` | `temps cdom` |
| `temps dns-providers` | `temps dnsp` |
| `temps email-domains` | `temps edom` |
| `temps email-providers` | `temps eprov` |
| `temps ip-access` | `temps ipa` |
| `temps load-balancer` | `temps lb` |
| `temps platform` | `temps plat` |
| `temps scans` | `temps scan` |
| `temps errors` | `temps error` |
| `temps funnels` | `temps funnel` |
| `temps incidents` | `temps incident` |
| `temps emails` | `temps email` |
| `temps templates` | `temps tpl` |
| `temps presets` | `temps preset` |
| `temps notification-preferences` | `temps notif-prefs` |

| `temps cloud vps` | — |

Within commands, common subcommand aliases: `list` -> `ls`, `create` -> `add`/`new`, `remove` -> `rm`, `show` -> `get`.
