<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="web/public/logo/temps-logo-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="web/public/logo/temps-logo-light.png">
  <img alt="Temps" src="web/public/logo/temps-logo-dark.png" width="280">
</picture>

### The open-source, self-hosted deployment platform.
### Deploy, observe, and scale -- from a single binary.

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/gotempsh/temps)](https://github.com/gotempsh/temps/releases)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![GitHub Stars](https://img.shields.io/github/stars/gotempsh/temps?style=social)](https://github.com/gotempsh/temps)

[Website](https://temps.sh) | [Documentation](https://temps.sh/docs) | [Quick Start](https://temps.sh/docs/introduction) | [GitHub](https://github.com/gotempsh/temps)

</div>

---

![Temps Dashboard](assets/screenshots/dashboard.png)

Stop paying for 5 different SaaS tools. Temps replaces your deployment platform, analytics, error tracking, session replay, and uptime monitoring -- all self-hosted, all in one binary.

---

## Features

### Deploy anything

Push to Git. Temps builds and deploys. Framework auto-detection handles the rest -- Next.js, Vite, Go, Python, Rust, Java, .NET, NestJS, or any custom Dockerfile. Every push creates a deployment with build logs, rollback support, and zero-downtime updates.

![Deployments](assets/screenshots/deployments.png)

### Built-in observability

Web analytics with funnels and visitor tracking. Session replay powered by rrweb. Error tracking with a Sentry-compatible SDK. Speed insights. Uptime monitoring. All of it ships inside Temps -- no external services, no extra billing, no data leaving your infrastructure.

![Analytics](assets/screenshots/analytics.png)

### Production infrastructure

Temps runs on Cloudflare's Pingora reverse proxy -- the same technology that handles trillions of requests at Cloudflare. Automatic TLS certificates via Let's Encrypt (HTTP-01 and DNS-01 challenges), custom domains, and load balancing are built in and configured through the UI.

![Domain Management](assets/screenshots/domains.png)

### Managed services

Provision PostgreSQL, Redis, S3-compatible storage (MinIO), and MongoDB alongside your applications. Temps manages the lifecycle -- creation, backups, connection strings, and teardown -- so your apps and their data live in the same place.

### Full request visibility

Every HTTP request that flows through the proxy is logged with method, path, status code, response time, upstream target, and routing metadata. Filter, search, and drill into traffic patterns without bolting on a separate logging stack.

![Proxy Logs](assets/screenshots/proxy-logs.png)

### Monitoring and alerts

Configure monitors for deployment failures, build errors, runtime crashes, domain and certificate expiry, and backup health. Get notified through your preferred channels before problems reach your users.

![Monitoring](assets/screenshots/monitoring-detail.png)

---

## Quick Start

```bash
# Install
curl -fsSL https://temps.sh/deploy.sh | sh

# Setup (interactive -- configures database, admin user, TLS, DNS)
temps setup

# Start
temps serve
```

<details>
<summary>Or use Docker Compose</summary>

```yaml
version: "3.8"

services:
  postgres:
    image: timescale/timescaledb:latest-pg18
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: temps
      POSTGRES_DB: temps
    volumes:
      - temps-postgres:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 5s
      timeout: 5s
      retries: 5

  redis:
    image: redis:7-alpine
    volumes:
      - temps-redis:/data

  temps:
    image: ghcr.io/gotempsh/temps:latest
    ports:
      - "80:80"
      - "443:443"
      - "8081:8081"
    environment:
      TEMPS_DATABASE_URL: postgresql://postgres:temps@postgres:5432/temps
      TEMPS_ADDRESS: 0.0.0.0:80
      TEMPS_TLS_ADDRESS: 0.0.0.0:443
      TEMPS_CONSOLE_ADDRESS: 0.0.0.0:8081
      REDIS_URL: redis://redis:6379
    volumes:
      - temps-data:/app/data
      - /var/run/docker.sock:/var/run/docker.sock
    depends_on:
      postgres:
        condition: service_healthy

volumes:
  temps-postgres:
  temps-redis:
  temps-data:
```

```bash
docker-compose up -d
```

</details>

For detailed setup options, see the [Installation Guide](https://temps.sh/docs/installation).

---

## What Temps replaces

| What you get | Instead of paying for |
|---|---|
| Git deployments + preview URLs | Vercel / Netlify / Railway ($20+/mo) |
| Web analytics + funnels | PostHog / Plausible ($0-450/mo) |
| Session replay | PostHog / FullStory ($0-2000/mo) |
| Error tracking | Sentry ($26+/mo) |
| Uptime monitoring | Better Uptime / Pingdom ($20+/mo) |
| Managed Postgres/Redis/S3 | AWS RDS / ElastiCache ($50+/mo) |
| Request logs + proxy | Cloudflare ($0-200/mo) |
| **Total with Temps** | **$0 (self-hosted)** |

---

## Tech Stack

- **Backend:** Rust, Axum, Sea-ORM, Pingora (Cloudflare's proxy engine), Bollard (Docker API)
- **Frontend:** React 19, TypeScript, Tailwind CSS, shadcn/ui
- **Database:** PostgreSQL + TimescaleDB
- **Architecture:** 30+ workspace crates, three-layer service architecture

---

## SDKs

| Package | Description |
|---|---|
| [`@temps-sdk/react-analytics`](https://www.npmjs.com/package/@temps-sdk/react-analytics) | React analytics, session replay, error tracking |
| [`@temps-sdk/kv`](https://www.npmjs.com/package/@temps-sdk/kv) | Key-value store |
| [`@temps-sdk/blob`](https://www.npmjs.com/package/@temps-sdk/blob) | Blob storage |
| [`@temps-sdk/node-sdk`](https://www.npmjs.com/package/@temps-sdk/node-sdk) | Blob storage |

---

## Contributing

We welcome contributions. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

```bash
git clone https://github.com/gotempsh/temps.git
cd temps
cargo build --release
```

---

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE).

---

<div align="center">

[temps.sh](https://temps.sh) | [Documentation](https://temps.sh/docs) | [GitHub](https://github.com/gotempsh/temps)

</div>
