# CLI Smoke Test Guide

Test all CLI commands against a running Temps server.

## Setup

```bash
cd apps/temps-cli

# Create alias for convenience
alias tcli="bun run src/index.ts"

# Point to your local server
export TEMPS_API_URL=<your_url>

# Authenticate (pick one)
export TEMPS_API_TOKEN=<api_key>
# or
tcli login --api-key <your-api-key>

# Verify
tcli whoami
tcli configure show
```

## Smoke Test Script

Run all read-only `list` commands to verify each module works:

```bash
#!/usr/bin/env bash
set -euo pipefail

# Note: aliases don't expand in scripts, so we use the full command here
CLI="bun run src/index.ts"
PASS=0
FAIL=0
ERRORS=""

run() {
  local label="$1"
  shift
  printf "  %-45s" "$label"
  if output=$($CLI "$@" --json 2>&1); then
    echo "PASS"
    ((PASS++))
  else
    echo "FAIL"
    ERRORS="$ERRORS\n  $label: $output"
    ((FAIL++))
  fi
}

run_no_json() {
  local label="$1"
  shift
  printf "  %-45s" "$label"
  if output=$($CLI "$@" 2>&1); then
    echo "PASS"
    ((PASS++))
  else
    echo "FAIL"
    ERRORS="$ERRORS\n  $label: $output"
    ((FAIL++))
  fi
}

echo ""
echo "=== Temps CLI Smoke Tests ==="
echo "Server: ${TEMPS_API_URL:-http://localhost:3000}"
echo ""

# --- Auth & Config ---
echo "-- Auth & Config --"
run "whoami"                          whoami
run "configure show"                  configure show

# --- Core (pre-existing) ---
echo ""
echo "-- Core Commands --"
run "projects list"                   projects list
run "domains list"                    domains list
run "environments list"               environments list
run "services list"                   services list
run "providers list"                  providers list
run "monitors list"                   monitors list
run "webhooks list"                   webhooks list
run "notifications list"              notifications list
run "backups schedules list"          backups schedules list
run "containers list"                 containers list
run "apikeys list"                    apikeys list
run "users list"                      users list
run "settings show"                   settings show
run "tokens list"                     tokens list

# --- Phase 1: Updated commands ---
echo ""
echo "-- Phase 1: Updated Commands --"
run "domains orders list"             domains orders list
run "backups sources list"            backups sources list
run "providers connections list"      providers connections list

# --- Phase 2: New commands (no project-id) ---
echo ""
echo "-- Phase 2: New Commands (global) --"
run "dns-provider list"               dns-provider list
run "ip-access list"                  ip-access list
run "audit list"                      audit list
run "proxy-logs list"                 proxy-logs list
run "email-domains list"              email-domains list
run "email-providers list"            email-providers list
run "dns list"                        dns list

# --- Phase 2: New commands (project-scoped) ---
# Replace 1 with a valid project ID
PID=1
echo ""
echo "-- Phase 2: New Commands (project-scoped, project=$PID) --"
run "errors list"                     errors list --project-id $PID
run "scans list"                      scans list --project-id $PID
run "custom-domains list"             custom-domains list --project-id $PID
run "incidents list"                  incidents list --project-id $PID

# --- Phase 3: New commands (global) ---
echo ""
echo "-- Phase 3: New Commands (global) --"
run "emails list"                     emails list
run "lb list"                         load-balancer list
run "imports sources"                 imports sources
run "templates list"                  templates list
run "platform info"                   platform info
run "presets list"                    presets list
run "notif-prefs show"                notification-preferences show

# --- Phase 3: New commands (project-scoped) ---
echo ""
echo "-- Phase 3: New Commands (project-scoped, project=$PID) --"
run "dsn list"                        dsn list --project-id $PID
run "funnels list"                    funnels list --project-id $PID

# --- Phase 3: Placeholder commands ---
echo ""
echo "-- Phase 3: Placeholder Commands --"
run_no_json "kv status"               kv status --project-id $PID
run_no_json "blob status"             blob status --project-id $PID

# --- Summary ---
echo ""
echo "==============================="
echo "  PASS: $PASS"
echo "  FAIL: $FAIL"
echo "  TOTAL: $((PASS + FAIL))"
echo "==============================="

if [ -n "$ERRORS" ]; then
  echo ""
  echo "Failed commands:"
  echo -e "$ERRORS"
fi

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
```

## Running the Tests

```bash
# Make sure your server is running on localhost:8081
export TEMPS_API_URL=http://localhost:8081/api
export TEMPS_API_TOKEN=<your-api-key>

# Copy the script above to a file and run it
bash test-smoke.sh

# Or run individual commands manually
tcli projects list --json
tcli errors list --project-id 1 --json
```

## Testing Write Operations

These modify data. Run with caution.

```bash
PID=1  # your project ID

# Create + delete a webhook
tcli webhooks create --project-id $PID \
  --url "https://httpbin.org/post" --events "deployment.created" -y
# note the ID, then:
tcli webhooks remove --project-id $PID --webhook-id <id> -f -y

# Validate an email
tcli emails validate --email test@example.com --json

# Check if an IP is blocked
tcli ip-access check --ip 8.8.8.8 --json

# Trigger a vulnerability scan
tcli scans trigger --project-id $PID --environment-id 1

# Lookup DNS A records
tcli dns-provider lookup --domain example.com --json

# Get platform info
tcli platform public-ip
tcli platform private-ip
tcli platform access --json
```

## Customizing the Project ID

Many commands require `--project-id`. Find valid IDs with:

```bash
tcli projects list --json | head -20
```

Then replace `PID=1` in the smoke test script with an actual project ID from your server.

## Expected Behavior

- **PASS**: Command returns valid JSON (or output) with exit code 0
- **FAIL**: Command returns non-zero exit code (auth error, 404, server error, etc.)
- **KV/Blob**: These are placeholder commands that show "coming soon" — they pass if they run without crashing

## Troubleshooting

| Issue | Fix |
|-------|-----|
| `Authentication required` | Run `temps login --api-key <key>` or set `TEMPS_API_TOKEN` |
| `Connection refused` | Verify server is running on the configured port |
| `404 Not Found` | The endpoint may not exist on your server version |
| `403 Forbidden` | Your API key may lack permissions for that operation |
| `Invalid project ID` | Use `projects list` to find valid IDs |
