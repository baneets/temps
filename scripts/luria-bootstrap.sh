#!/usr/bin/env bash
# luria-bootstrap.sh — bring a FRESH Ubuntu box up as Luria's Temps host.
#
# Provider-agnostic on purpose: nothing Oracle-specific. Swapping VPS = run this
# on the new box, restore the DB (see ROLLBACK.md "VPS SWAP"), repoint DNS. This
# is the whole portability story — no Terraform/Ansible for a test box (YAGNI).
#
# Idempotent: safe to re-run. Run as a sudo-capable user on Ubuntu 22.04/24.04.
#   curl the repo down or `git clone`, then:  sudo bash scripts/luria-bootstrap.sh
set -euo pipefail

REPO_URL="${REPO_URL:-https://github.com/baneets/temps.git}"
APP_DIR="${APP_DIR:-/opt/luria-temps}"
ADMIN_EMAIL="${TEMPS_ADMIN_EMAIL:-ops@luriart.com}"

log() { printf '\n\033[1;36m[luria]\033[0m %s\n' "$*"; }

# 1. Docker (official convenience script — this is Docker's, NOT temps.sh's).
if ! command -v docker >/dev/null 2>&1; then
  log "installing Docker"
  curl -fsSL https://get.docker.com | sh
fi
# compose plugin ships with modern Docker; verify.
docker compose version >/dev/null 2>&1 || { echo "docker compose plugin missing" >&2; exit 1; }

# 2. Oracle/Ubuntu images ship iptables that REJECT everything but 22 even after
#    the cloud security list opens a port. Open 80/443 in netfilter, idempotently.
if command -v iptables >/dev/null 2>&1; then
  log "opening 80/443 in netfilter (idempotent)"
  iptables -C INPUT -p tcp -m multiport --dports 80,443 -j ACCEPT 2>/dev/null \
    || iptables -I INPUT -p tcp -m multiport --dports 80,443 -j ACCEPT
  command -v netfilter-persistent >/dev/null 2>&1 && netfilter-persistent save || true
fi

# 3. Source tree (clone once, else pull — build-from-source, no install script).
if [ ! -d "$APP_DIR/.git" ]; then
  log "cloning fork -> $APP_DIR"
  git clone "$REPO_URL" "$APP_DIR"
else
  log "updating fork in $APP_DIR"
  git -C "$APP_DIR" pull --ff-only
fi
cd "$APP_DIR"

# 4. Secrets: generate once, never overwrite (so re-runs don't rotate creds).
if [ ! -f .env ]; then
  log "generating .env (POSTGRES/REDIS passwords, DOCKER_GID)"
  DOCKER_GID="$(stat -c '%g' /var/run/docker.sock)"
  install -m 600 /dev/null .env
  {
    echo "POSTGRES_PASSWORD=$(openssl rand -hex 32)"
    echo "REDIS_PASSWORD=$(openssl rand -hex 32)"
    echo "DOCKER_GID=${DOCKER_GID}"
    echo "TEMPS_ADMIN_EMAIL=${ADMIN_EMAIL}"
    echo "TEMPS_ADMIN_PASSWORD_FILE=./secrets/admin_password"
  } >> .env
fi
if [ ! -f secrets/admin_password ]; then
  log "generating initial admin password -> secrets/admin_password (printed once below)"
  mkdir -p secrets && chmod 700 secrets
  openssl rand -base64 24 | tr -d '\n' > secrets/admin_password
  chmod 444 secrets/admin_password
  echo "ADMIN PASSWORD (save to your vault now): $(cat secrets/admin_password)"
fi

# 5. Up. --build compiles the temps binary from source per the repo Dockerfile.
#    The override file (TEMPS_TELEMETRY=0, ports) is auto-merged.
log "docker compose up -d --build (first build compiles Rust — slow on first run)"
docker compose up -d --build

log "done. Verify:  docker compose ps  &&  curl -fsS http://localhost:3000/health"
log "Telemetry kill check: the override sets TEMPS_TELEMETRY=0 — confirm with egress capture (ROLLBACK.md / audit)."
