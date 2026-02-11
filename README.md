<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="temps-logo-assets/logo/temps-logo-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="temps-logo-assets/logo/temps-logo-light.png">
  <img alt="Temps" src="temps-logo-assets/logo/temps-logo-dark.png" width="280">
</picture>

**Self-hosted deployment platform with built-in analytics, monitoring, and error tracking.**

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![GitHub Release](https://img.shields.io/github/v/release/gotempsh/temps)](https://github.com/gotempsh/temps/releases)

**[Documentation](https://temps.sh/docs)** • **[Quick Start](https://temps.sh/docs/introduction)** • **[GitHub](https://github.com/gotempsh/temps)**

</div>

---

## Quick Start

```bash
# 1. Start PostgreSQL with TimescaleDB
docker volume create temps-postgres
docker run -d \
  --name temps-postgres \
  -v temps-postgres:/var/lib/postgresql/data \
  -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=temps \
  -e POSTGRES_DB=temps \
  -p 16432:5432 \
  timescale/timescaledb:latest-pg18

# 2. Install Temps
curl -fsSL https://temps.sh/install.sh | sh
source ~/.zshrc  # or ~/.bashrc

# 3. Run Setup (configures database, creates admin user, sets up TLS)
temps setup \
  --database-url "postgresql://postgres:temps@localhost:16432/temps" \
  --admin-email "your-email@example.com" \
  --wildcard-domain "*.yourdomain.com" \
  --github-token "ghp_xxxxxxxxxxxx" \
  --dns-provider "cloudflare" \
  --cloudflare-token "your-cloudflare-api-token"

# 4. Start Temps
temps serve \
  --database-url "postgresql://postgres:temps@localhost:16432/temps" \
  --address 0.0.0.0:80 \
  --tls-address 0.0.0.0:443 \
  --console-address 0.0.0.0:8081

# 5. Open https://temps.yourdomain.com to access the console
```

For detailed setup options, see the [Introduction Guide](https://temps.sh/docs/introduction).

---

## What is Temps?

Deploy **any application** from Git with zero configuration:

- **Frontend**: React, Next.js, Vue, Svelte, Angular
- **Backend**: Node.js, Python, Go, Rust, Ruby, PHP
- **Static Sites**: Hugo, Jekyll, Gatsby
- **Custom**: Anything with a Dockerfile

**Built-in observability** - no extra SaaS subscriptions:
- Analytics & funnels
- Error tracking (Sentry-compatible)
- Session replay
- Uptime monitoring

---

## Documentation

| Topic | Link |
|-------|------|
| **Getting Started** | [Introduction](https://temps.sh/docs/introduction) |
| **Installation** | [Installation Guide](https://temps.sh/docs/installation) |
| **Deploying Apps** | [Deployment Guide](https://temps.sh/docs/quickstart) |
| **CLI Reference** | [CLI Commands](https://temps.sh/docs/reference/cli) |

---

## Contributing

We welcome contributions! See [CLAUDE.md](CLAUDE.md) for development guidelines.

```bash
# Clone and build
git clone https://github.com/gotempsh/temps.git
cd temps
cargo build --release
```

---

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).

---

<div align="center">

**[temps.sh](https://temps.sh)** • **[Documentation](https://temps.sh/docs)** • **[GitHub](https://github.com/gotempsh/temps)**

</div>
