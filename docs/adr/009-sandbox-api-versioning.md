# ADR-009: Sandbox API Versioning Policy

**Status:** Accepted
**Date:** 2026-04-15
**Author:** David Viejo

## Context

The standalone sandbox API lives at `/v1/sandbox/*` and is the first HTTP surface we explicitly commit to keeping stable for third-party clients (in particular, anything written against the `@vercel/sandbox` SDK). Everything else in the Temps control plane is shipped as one product — we upgrade the backend and the web UI together, and we can break internal shapes at will. The sandbox API is different: it's the substrate Temps Cloud customers and downstream agent frameworks build against, so breaking it is an externally visible contract violation.

We need to say, in writing, what "stable" actually means and how we communicate the version a given response came from.

## Decision

### 1. URL-segment major versioning

Breaking changes bump the major version in the URL (`/v1/sandbox` → `/v2/sandbox`). `/v1/*` is maintained for at least 12 months after `/v2/*` is generally available. This matches every other pragmatic PaaS (Vercel, Railway, Render) and is the one thing every HTTP client already knows how to handle.

We do **not** use:
- `Accept` header version negotiation (Rails-style `application/vnd.temps.v1+json`) — too easy to get wrong, and the SDKs we target don't set custom Accept headers.
- Query-string versioning (`?api-version=1`) — rewrites caches badly and doesn't survive link sharing.

### 2. What is a breaking change

The following are breaking and require a major bump:

- Removing or renaming a route.
- Removing or renaming a response field (including nested).
- Changing a response field's type (`string` → `number`, scalar → array).
- Tightening a request-body validator in a way that rejects previously-accepted payloads (e.g., making an optional field required).
- Changing the meaning of an enum value.
- Changing the default of an omitted field such that the server behaves differently for existing callers.
- Changing an HTTP status code for a given outcome (e.g., 200 → 201).

The following are **not** breaking:

- Adding a new route.
- Adding an optional field to a request body (we accept it, old clients don't send it).
- Adding a new field to a response (clients must ignore unknown fields — this is a contractual requirement).
- Adding a new enum value **only if** the server never returned it before and we document that new values may appear.
- Loosening a validator (accepting more input than before).
- Fixing a bug where the server returned the wrong status/shape in an error path that no sane client was relying on.

### 3. DTO strictness

Every request DTO under `/v1/sandbox/*` carries `#[serde(deny_unknown_fields)]`. Without this, a typo in a client payload gets silently accepted, and we discover months later that a field we shipped isn't the one clients are actually using. Deny-unknown-fields makes contract drift a 400 at the first request, not a latent compatibility problem. ADR obligation: if you relax this on any DTO, write an ADR explaining why.

### 4. Response header: `X-Sandbox-API-Version`

Every response to a `/v1/sandbox/*` request carries an `X-Sandbox-API-Version` header whose value is the full Temps binary version (e.g., `1.0.0-alpha3+2026-04-15`). This lets operators correlate a client bug report with a specific build, and lets SDK authors fail fast if they hit a backend older than their minimum-supported version.

The header value is **not** a contract — it's diagnostic. The URL segment (`/v1/`) is the contract. We reserve the right to change the format of the header value without warning.

### 5. Deprecation signal

When we ship `/v2/`, `/v1/` responses gain a `Sunset` header ([RFC 8594](https://datatracker.ietf.org/doc/html/rfc8594)) pointing at the removal date, and a `Deprecation` header set to `true`. Clients that honor these headers get automatic advance warning.

### 6. OpenAPI as the source of truth

The machine-readable contract for `/v1/sandbox/*` lives in the **unified** Temps OpenAPI document at `/api-docs/openapi.json` (and is rendered in Swagger UI at `/swagger-ui`). The sandbox paths are tagged `Sandboxes`, so external SDK generators should fetch the unified doc and filter by tag. There is no separate `/v1/sandbox/openapi.json` endpoint — one API surface, one doc, one explorer.

If the OpenAPI document and the Rust DTOs disagree, the Rust DTOs are authoritative and the OpenAPI doc is the bug. The Vercel-compatibility test suite (`tests/vercel_compat.rs`) pins both.

## Consequences

### Positive

- External SDK generators (TypeScript, Go, Python) get a predictable URL shape and a strict schema. No negotiation, no custom media types.
- The `X-Sandbox-API-Version` header gives support triage a zero-click answer to "which build hit you?" — critical when customers run self-hosted deployments pinned to older versions.
- `deny_unknown_fields` turns most shape drift into a 400 the first time it happens, not a silent compat break discovered weeks later.

### Negative

- Major versions are a commitment. Once we ship `/v2/`, we carry `/v1/` for 12 months minimum, even if only one client uses it.
- We can't piggy-back "harmless" field removals in a minor release. Every rename needs a parallel field with a deprecation window.

### Not solved by this ADR

- Cross-version compatibility between `/v1/` and `/v2/` *during* the overlap window. That's a per-change decision — usually handled by the service layer translating v1 → v2 internally, but sometimes by running two plugins side-by-side. We'll revisit when we actually ship `/v2/`.
- Rate limiting, quotas, authentication scopes — those are orthogonal to versioning and have their own ADRs.

## Implementation

- Header injection: a small Axum middleware installed on the sandbox router that appends `X-Sandbox-API-Version` to every response. Source: `crates/temps-sandbox/src/handlers/version_header.rs`.
- The version value comes from the `TEMPS_VERSION` compile-time env var (the same one shown by `temps --version`). No runtime config needed.
- Guardrail test (`version_header_is_present`) asserts the middleware is wired into the router. If someone drops the layer, the test fails.

## References

- [RFC 8594 — The Sunset HTTP Header](https://datatracker.ietf.org/doc/html/rfc8594)
- [Vercel API versioning](https://vercel.com/docs/rest-api) — the model we're mirroring for sandbox-compat clients.
- PR 1.1 — DTO stability guards (deny_unknown_fields).
- PR 1.2 — Sandbox OpenAPI schema merged into unified `/api-docs/openapi.json` via the plugin system (originally shipped at `/v1/sandbox/openapi.json`; separate route removed to keep a single documentation surface).
- PR 1.5 — Vercel compatibility test suite.
