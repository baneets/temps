# ROLLBACK — Luria self-hosted Temps (Oracle Always Free)

Scope of this deployment: **replay + error tracking + uptime only.** Hosting stays on
Vercel, DB/Auth on Supabase (unchanged). So rollback is low-blast-radius: nothing
serving Luria's app or storing its primary data lives here.

Status: **NOT YET PROVISIONED** (2026-07-23). Fill in the box facts once launched.

| Fact | Value |
|---|---|
| Provider / tier | Oracle Cloud, Always Free (Ampere A1) |
| Instance OCID | _tbd_ |
| Public IP | _tbd_ |
| Home region / AD | _tbd_ |
| SSH key | `~/.ssh/luria_temps_oci` (ed25519, no passphrase) |
| Temps fork | https://github.com/baneets/temps @ v0.0.8 |
| Telemetry | `TEMPS_TELEMETRY=0` (see CHANGES-FROM-UPSTREAM.md §1) |

## What "on" looks like (so rollback is a clean off)
- Replay/analytics/error snippet is injected via the **existing variant-serving
  script**, pointed at `https://<box-domain>` , tagged `client_id` + `variant_id`.
- Nothing else routes through the box. PostHog free tier may still run in parallel for
  heatmaps.

## Rollback = revert to pre-Temps state (revert in this order)
1. **Stop sending data to the box.** Remove the Temps snippet endpoint from the
   variant-serving script config (feature-flag it off). Sessions stop flowing within one
   config TTL. PostHog (if kept) continues uninterrupted → **no analytics gap.**
2. **Confirm no dependency.** Nothing in Vercel/Supabase reads from the box, so there is
   nothing else to unwind. The bandit engine reads Supabase, not the box (until the
   daily pull job is wired — see below).
3. **If the daily bandit-pull job was wired** (later work order): disable that cron; the
   bandit falls back to its existing Supabase signals. Removing the box does not corrupt
   bandit state because writes land in the existing Supabase tables, not a new store.
4. **Decommission the box** (optional, only when sure): `oci compute instance terminate
   --instance-id <OCID> --preserve-boot-volume false`. Free tier, so no billing either
   way, but terminate to free the A1 allotment.
5. **Data note:** replay data captured only lived in the box's Postgres. Losing it loses
   captured sessions, not any Luria primary data. Export first if the sessions matter
   (`pg_dump` the replay DB) before terminating.

## Fallback stack after rollback
Vercel (host) + Supabase (DB/Auth/RLS/Realtime) + PostHog free tier (replay/heatmaps) —
i.e. exactly the locked MVP stack in `02_MVP_BUILD_PLAN.md`. Nothing to restore; it never
left.

## VPS SWAP (moving to a new/bigger box as we scale)

Designed so the box is disposable. Config is in the repo (`docker-compose.override.yml`),
so only the **data** has to move. No IaC for a test box — this is the whole procedure:

1. **Provision** the new VPS (any provider, Ubuntu 22.04/24.04). Open 22/80/443.
2. **Bring it up:** `git clone` the fork, then `sudo bash scripts/luria-bootstrap.sh`.
   Installs Docker, opens 80/443, generates fresh secrets, `docker compose up -d --build`.
   The override (`TEMPS_TELEMETRY=0`, port maps) is auto-merged — identical config, new box.
3. **Move the data** (replay/analytics/error history lives in Postgres):
   - Test-scale (now): `pg_dump` on old → restore on new.
     ```
     # on OLD box
     docker compose exec -T postgres pg_dump -U temps -Fc temps > temps.dump
     # copy temps.dump to NEW box, then
     docker compose exec -T postgres pg_restore -U temps -d temps --clean --if-exists < temps.dump
     ```
   - Scale path (later, not wired yet): Temps' built-in **WAL-G** backups already run in
     the `timescaledb-walg` image. Point them at external object storage (Cloudflare R2 /
     Backblaze B2 free tier) and a swap becomes new-box + WAL-G restore, no manual dump.
     Wire this when the box holds data we can't lose — YAGNI for the test box.
4. **Repoint DNS:** change the `t.luriart.com` A record to the new box IP. Clients/snippet
   never change — they only know the hostname. Wait out TTL.
5. **Verify** on the new box (dashboard + TLS + zero `*.temps.sh` egress), then
   **terminate the old box**. Update the fact table at the top of this doc.

Because the only per-box state is the Postgres volume + DNS, swap cost is ~1 pg_dump +
one A-record edit. That's the point.

## Keep-until rule (from prior work order)
Do not tear down PostHog replay as a fallback until the Oracle box has 2-4 weeks of clean
runtime capturing sessions tagged correctly by `client_id` + `variant_id`.
