//! Preview gateway authentication for Pingora.
//!
//! When a request hits a hostname matching `ws-<session_id>-<port>.<preview_domain>`,
//! the proxy looks up the workspace session, checks for a valid preview cookie,
//! and (on success) forwards the request to the local preview gateway at
//! `127.0.0.1:8090`.
//!
//! Unauthenticated requests are redirected to a form-based login page at
//! `/__temps/preview/login` (handled in [`crate::handler::preview_wall`] and
//! `proxy.rs`). HTTP Basic auth is **not** supported — browsers cache Basic
//! credentials unpredictably across subdomains and some clients refuse to
//! send them over plain HTTP. Cookie + form is the only supported flow.
//!
//! Design notes:
//! - The preview gateway itself is a dumb TCP-level reverse proxy bound to
//!   loopback. All authentication happens here in Pingora so the gateway never
//!   needs to talk to the database.
//! - Failures are rate-limited per (client_ip, session_id) using an in-memory
//!   sliding window. This is best-effort and resets on proxy restart.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::PasswordHash;
use argon2::{Argon2, PasswordVerifier};
use dashmap::DashMap;
use sea_orm::EntityTrait;
use temps_core::CookieCrypto;
use temps_database::DbConnection;
use temps_entities::workspace_sessions;
use tracing::{debug, warn};

/// Cookie name template — one cookie per session (`temps_preview_<sid>`)
/// scoped to all ports of that session via the parent preview domain.
pub const PREVIEW_COOKIE_PREFIX: &str = "temps_preview_";

/// How long a preview session cookie is valid before the user is asked to
/// re-enter the password. Rotating the password invalidates cookies
/// immediately regardless of this TTL.
pub const PREVIEW_COOKIE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The local TCP address where the preview gateway listens. Pingora forwards
/// authenticated preview requests to this peer.
pub const PREVIEW_GATEWAY_PEER: &str = "127.0.0.1:8090";

/// Maximum number of failed auth attempts allowed per (client_ip, session_id)
/// inside [`RATE_LIMIT_WINDOW`] before the proxy starts rejecting with 429.
const MAX_FAILURES: u32 = 10;

/// Sliding window for rate limiting failed auth attempts.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Parsed preview hostname components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewHost {
    pub session_id: i32,
    pub port: u16,
}

/// Parse a hostname against `ws-<session_id>-<port>.<preview_domain>`.
///
/// `preview_domain` may start with `*.` (wildcard form) — the leading `*.` is
/// stripped before comparison. The session id must be a positive `i32` and the
/// port must be a non-zero `u16`.
pub fn parse_preview_host(host: &str, preview_domain: &str) -> Option<PreviewHost> {
    let domain = preview_domain.trim_start_matches("*.");
    let host_no_port = host.split(':').next()?.to_ascii_lowercase();
    let suffix = format!(".{}", domain.to_ascii_lowercase());
    let label = host_no_port.strip_suffix(&suffix)?;

    // label must be `ws-<sid>-<port>`
    let rest = label.strip_prefix("ws-")?;
    let (sid_str, port_str) = rest.rsplit_once('-')?;

    let session_id: i32 = sid_str.parse().ok()?;
    if session_id <= 0 {
        return None;
    }
    let port: u16 = port_str.parse().ok()?;
    if port == 0 {
        return None;
    }

    Some(PreviewHost { session_id, port })
}

/// Outcome of preview auth processing.
#[derive(Debug)]
pub enum PreviewAuthOutcome {
    /// Auth succeeded — forward the request to [`PREVIEW_GATEWAY_PEER`].
    Allow { host: PreviewHost },
    /// No valid cookie — reply with 303 redirect to the login form.
    LoginRequired { host: PreviewHost },
    /// Too many failed attempts — reply with 429.
    RateLimited { host: PreviewHost },
    /// Session does not exist or has no preview password configured.
    NotFound { host: PreviewHost },
}

/// SHA-256 of the full argon2 PHC hash, truncated to 16 hex chars. Folded
/// into the cookie payload so rotating the password (which changes the
/// argon2 hash) immediately invalidates every live cookie for that session.
///
/// Previous implementation took `&hash[..12]`, which is always the literal
/// prefix `$argon2id$v=` for every argon2id hash and therefore could not
/// distinguish two different passwords. Rotating the password did not
/// revoke existing cookies. Using a digest of the full hash fixes that.
fn hash_fingerprint(hash: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(hash.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8]) // 16 hex chars
}

/// Encode a fresh preview cookie value: `sid|fingerprint|expires_unix`,
/// then encrypted+authenticated by `CookieCrypto` (AES-256-GCM).
pub fn encode_preview_cookie(
    crypto: &CookieCrypto,
    session_id: i32,
    password_hash: &str,
    now: SystemTime,
) -> Option<String> {
    let exp = now
        .checked_add(PREVIEW_COOKIE_TTL)?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    let payload = format!("{}|{}|{}", session_id, hash_fingerprint(password_hash), exp);
    crypto.encrypt(&payload).ok()
}

/// Validate a previously issued preview cookie. Returns true iff the cookie
/// decrypts cleanly, names the same `session_id`, was minted against the
/// current password hash (so rotation revokes), and has not expired.
pub fn verify_preview_cookie(
    crypto: &CookieCrypto,
    cookie_value: &str,
    session_id: i32,
    password_hash: &str,
    now: SystemTime,
) -> bool {
    let Ok(plain) = crypto.decrypt(cookie_value) else {
        return false;
    };
    let parts: Vec<&str> = plain.splitn(3, '|').collect();
    if parts.len() != 3 {
        return false;
    }
    let Ok(sid) = parts[0].parse::<i32>() else {
        return false;
    };
    if sid != session_id {
        return false;
    }
    if parts[1] != hash_fingerprint(password_hash) {
        return false;
    }
    let Ok(exp) = parts[2].parse::<u64>() else {
        return false;
    };
    let Ok(now_secs) = now.duration_since(UNIX_EPOCH) else {
        return false;
    };
    now_secs.as_secs() <= exp
}

/// Pull a single cookie value out of a `Cookie:` header by name.
pub fn extract_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(v);
            }
        }
    }
    None
}

/// Build the full `Set-Cookie` header value for a preview cookie. Scoped to
/// the parent preview domain so it covers every `ws-<sid>-<port>.<domain>`
/// host belonging to the session.
pub fn build_set_cookie(
    session_id: i32,
    cookie_value: &str,
    preview_domain: &str,
    secure: bool,
) -> String {
    // `Secure` is only emitted when the request came in over TLS. Browsers
    // silently drop `Secure` cookies sent over plain HTTP, which would
    // completely break preview auth for self-hosted setups running without
    // TLS (e.g. `http://host.docker.internal:8080`).
    let domain = preview_domain.trim_start_matches("*.");
    let secure_attr = if secure { "; Secure" } else { "" };
    let ttl = PREVIEW_COOKIE_TTL.as_secs();
    format!(
        "{PREVIEW_COOKIE_PREFIX}{session_id}={cookie_value}; Domain=.{domain}; Path=/; HttpOnly{secure_attr}; SameSite=Lax; Max-Age={ttl}"
    )
}

#[derive(Debug, Default)]
struct FailureState {
    count: u32,
    window_start: Option<Instant>,
}

/// In-memory rate limiter for preview auth failures.
#[derive(Debug, Default)]
pub struct PreviewAuthLimiter {
    failures: DashMap<(IpAddr, i32), FailureState>,
}

impl PreviewAuthLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the (ip, session_id) pair is currently rate-limited.
    pub fn is_blocked(&self, ip: IpAddr, session_id: i32) -> bool {
        let entry = self.failures.get(&(ip, session_id));
        let Some(state) = entry else { return false };
        let Some(start) = state.window_start else {
            return false;
        };
        if start.elapsed() > RATE_LIMIT_WINDOW {
            return false;
        }
        state.count >= MAX_FAILURES
    }

    pub fn record_failure(&self, ip: IpAddr, session_id: i32) {
        let mut entry = self.failures.entry((ip, session_id)).or_default();
        let now = Instant::now();
        match entry.window_start {
            Some(start) if start.elapsed() <= RATE_LIMIT_WINDOW => {
                entry.count = entry.count.saturating_add(1);
            }
            _ => {
                entry.window_start = Some(now);
                entry.count = 1;
            }
        }
    }

    pub fn record_success(&self, ip: IpAddr, session_id: i32) {
        self.failures.remove(&(ip, session_id));
    }
}

/// Verify a plaintext password against an argon2 PHC hash. Public so the
/// login POST handler in `proxy.rs` can reuse the exact same verification
/// path as the cookie-only gate.
pub fn verify_argon2(plaintext: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        warn!("preview-auth: stored password hash is malformed");
        return false;
    };
    Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok()
}

/// Result of looking up a workspace session's preview password hash.
#[derive(Debug)]
pub enum PreviewSessionLookup {
    /// Session exists and has a password configured.
    Found { password_hash: String },
    /// Session doesn't exist, or has no password configured.
    NotFound,
}

/// Load the argon2 password hash for a workspace session. Used by both the
/// cookie gate and the login POST handler — factored out so both paths hit
/// the DB identically.
pub async fn lookup_preview_session(
    db: &Arc<DbConnection>,
    session_id: i32,
) -> PreviewSessionLookup {
    match workspace_sessions::Entity::find_by_id(session_id)
        .one(db.as_ref())
        .await
    {
        Ok(Some(row)) => match row.preview_password_hash {
            Some(hash) => PreviewSessionLookup::Found {
                password_hash: hash,
            },
            None => {
                debug!(
                    session_id,
                    "preview-auth: session has no preview password configured"
                );
                PreviewSessionLookup::NotFound
            }
        },
        Ok(None) => PreviewSessionLookup::NotFound,
        Err(e) => {
            warn!(
                session_id,
                error = %e,
                "preview-auth: failed to load workspace session"
            );
            PreviewSessionLookup::NotFound
        }
    }
}

/// Run the preview auth check for a parsed preview host (cookie-only).
///
/// Order of operations:
/// 1. Rate-limit gate.
/// 2. Load the session row from the DB.
/// 3. If a valid `temps_preview_<sid>` cookie is present → Allow.
/// 4. Otherwise → LoginRequired (caller issues a 303 to the login form).
///
/// Note: this does NOT record a rate-limit failure for missing cookies —
/// only the login POST records failures (via [`PreviewAuthLimiter::record_failure`])
/// so GETs without a cookie don't lock users out after a browser refresh.
pub async fn check_preview_auth(
    db: &Arc<DbConnection>,
    crypto: &CookieCrypto,
    limiter: &PreviewAuthLimiter,
    host: PreviewHost,
    client_ip: IpAddr,
    cookie_header: Option<&str>,
) -> PreviewAuthOutcome {
    if limiter.is_blocked(client_ip, host.session_id) {
        return PreviewAuthOutcome::RateLimited { host };
    }

    let stored_hash = match lookup_preview_session(db, host.session_id).await {
        PreviewSessionLookup::Found { password_hash } => password_hash,
        PreviewSessionLookup::NotFound => return PreviewAuthOutcome::NotFound { host },
    };

    let cookie_name = format!("{}{}", PREVIEW_COOKIE_PREFIX, host.session_id);
    if let Some(header) = cookie_header {
        if let Some(value) = extract_cookie(header, &cookie_name) {
            if verify_preview_cookie(
                crypto,
                value,
                host.session_id,
                &stored_hash,
                SystemTime::now(),
            ) {
                limiter.record_success(client_ip, host.session_id);
                return PreviewAuthOutcome::Allow { host };
            }
        }
    }

    PreviewAuthOutcome::LoginRequired { host }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_fingerprint_differs_for_different_hashes() {
        // Two argon2id hashes share the literal `$argon2id$v=` prefix, so
        // any prefix-based fingerprint would collide. The SHA-256-derived
        // fingerprint must actually distinguish them — otherwise password
        // rotation silently fails to revoke existing cookies.
        let hash_a = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let hash_b = "$argon2id$v=19$m=19456,t=2,p=1$BBBBBBBBBBBBBBBBBBBBBB$BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        assert_ne!(hash_fingerprint(hash_a), hash_fingerprint(hash_b));
        // Deterministic on repeated calls.
        assert_eq!(hash_fingerprint(hash_a), hash_fingerprint(hash_a));
        // Exactly 16 hex chars.
        assert_eq!(hash_fingerprint(hash_a).len(), 16);
    }

    #[test]
    fn parse_preview_host_basic() {
        let h = parse_preview_host("ws-14-3000.localho.st", "localho.st").unwrap();
        assert_eq!(h.session_id, 14);
        assert_eq!(h.port, 3000);
    }

    #[test]
    fn parse_preview_host_strips_wildcard_prefix() {
        let h =
            parse_preview_host("ws-7-8080.preview.example.com", "*.preview.example.com").unwrap();
        assert_eq!(h.session_id, 7);
        assert_eq!(h.port, 8080);
    }

    #[test]
    fn parse_preview_host_strips_request_port() {
        let h = parse_preview_host("ws-1-3000.localho.st:8443", "localho.st").unwrap();
        assert_eq!(h.session_id, 1);
        assert_eq!(h.port, 3000);
    }

    #[test]
    fn parse_preview_host_rejects_wrong_domain() {
        assert!(parse_preview_host("ws-1-3000.example.org", "localho.st").is_none());
    }

    #[test]
    fn parse_preview_host_rejects_missing_prefix() {
        assert!(parse_preview_host("foo-1-3000.localho.st", "localho.st").is_none());
    }

    #[test]
    fn parse_preview_host_rejects_zero_port() {
        assert!(parse_preview_host("ws-1-0.localho.st", "localho.st").is_none());
    }

    #[test]
    fn parse_preview_host_rejects_non_positive_session() {
        assert!(parse_preview_host("ws-0-3000.localho.st", "localho.st").is_none());
        assert!(parse_preview_host("ws--1-3000.localho.st", "localho.st").is_none());
    }

    #[test]
    fn rate_limiter_trips_after_max_failures() {
        let limiter = PreviewAuthLimiter::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(!limiter.is_blocked(ip, 1));
            limiter.record_failure(ip, 1);
        }
        assert!(limiter.is_blocked(ip, 1));
    }

    #[test]
    fn rate_limiter_resets_on_success() {
        let limiter = PreviewAuthLimiter::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            limiter.record_failure(ip, 2);
        }
        assert!(limiter.is_blocked(ip, 2));
        limiter.record_success(ip, 2);
        assert!(!limiter.is_blocked(ip, 2));
    }
}
