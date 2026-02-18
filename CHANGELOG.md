# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- MCP (Model Context Protocol) server with 210 tools across 30 domain modules (`mcp/`)
- OpenAPI SDK auto-generated via `@hey-api/openapi-ts` for MCP server
- WebSocket support for container runtime logs in MCP server
- 103 integration tests for MCP server
- RustFS service logo and improved service type detection in web UI
- Auto-generate `secret_key` for MinIO service creation
- Analytics seed data utilities (`scripts/seed-data/`)
- Web UI build integration via `build.rs`
- Placeholder dist directory for debug builds
- GitHub Actions release workflow for Linux AMD64
- Release automation script (`scripts/release.sh`)
- Comprehensive development and release documentation

### Changed
- Updated `clippy::ptr_arg` warnings to use `&Path` instead of `&PathBuf`
- Fixed `clippy::only_used_in_recursion` warning in workflow executor
- Rewrote CLAUDE.md with comprehensive error handling, resilience, and testing guidance
- Refined service parameter strategies in `temps-providers`

### Fixed
- Build failures when web UI is skipped in debug mode

### Security
- Addressed security audit findings in `temps-cli` skill: removed `curl|sh` pattern, credential path disclosure, and secret-like example tokens

## [0.1.0] - 2024-10-22

### Added
- Initial project structure
- Core architecture with 30+ workspace crates
- Analytics engine with funnels and session replay
- Error tracking (Sentry-compatible)
- Git provider integrations (GitHub, GitLab)
- Deployment orchestration with Docker
- Reverse proxy with automatic TLS/ACME
- Managed services (PostgreSQL, Redis, S3)
- Status page and uptime monitoring
- Web UI built with React and Rsbuild

[Unreleased]: https://github.com/gotempsh/temps/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gotempsh/temps/releases/tag/v0.1.0
