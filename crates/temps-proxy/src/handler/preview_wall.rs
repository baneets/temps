//! Workspace preview password wall.
//!
//! Renders the HTML login form shown when an unauthenticated user hits a
//! workspace preview host. The cryptographic bits (cookie minting,
//! verification, rate limiting) live in [`crate::preview_auth`]; this module
//! only handles HTML rendering.
//!
//! Login flow (replaces HTTP Basic auth):
//!   1. GET `ws-<sid>-<port>.<preview_domain>/anything` without a valid
//!      `temps_preview_<sid>` cookie → proxy issues a 303 to
//!      `/__temps/preview/login?next=<encoded path>`.
//!   2. GET `/__temps/preview/login` → this form.
//!   3. POST `/__temps/preview/login` with `password` + `next` → proxy
//!      verifies with argon2, mints the cookie (see
//!      [`crate::preview_auth::encode_preview_cookie`]), 303s back to `next`.
//!   4. POST `/__temps/preview/logout` → 303 `/` with an expired cookie.
//!
//! Why not Basic auth: browsers cache Basic credentials unpredictably across
//! subdomains, show native prompts that can't be dismissed, and some HTTP
//! clients refuse to pass them over plain HTTP. Form + cookie is reliable
//! across both http/https and survives subdomain hops (cookie scoped to the
//! parent preview domain).

/// Path that the proxy intercepts to serve the login form and accept
/// credentials. Kept under a `/__temps/` prefix to avoid colliding with any
/// realistic dev-server route.
pub const PREVIEW_LOGIN_PATH: &str = "/__temps/preview/login";

/// Path that clears the preview cookie.
pub const PREVIEW_LOGOUT_PATH: &str = "/__temps/preview/logout";

const PREVIEW_FORM_HTML: &str = include_str!("../../preview_wall/preview_form.html");

/// Render the workspace login form. Delegates to the generic renderer using
/// a `session #<id>` label — kept for the existing call sites.
pub fn generate_preview_form_html(
    session_id: i32,
    port: u16,
    next: &str,
    show_error: bool,
) -> String {
    generate_preview_form_html_labeled(&format!("session #{}", session_id), port, next, show_error)
}

/// Render the login form with an arbitrary display label (e.g. `session #42`
/// for workspaces, `sandbox sbx_abc…` for sandboxes). `next` is the path the
/// user will be redirected to after a successful login — always sanitized by
/// the caller.
pub fn generate_preview_form_html_labeled(
    label: &str,
    port: u16,
    next: &str,
    show_error: bool,
) -> String {
    // The template uses `{{SESSION_ID}}` substituted into `session #{{SESSION_ID}}`.
    // The simplest and safest change that works for both workspaces and
    // sandboxes is to replace the whole `session #{{SESSION_ID}}` phrase
    // with the provided label, falling back to numeric substitution for
    // older templates that don't include the phrase.
    let escaped_label = html_escape(label);
    let with_label = PREVIEW_FORM_HTML.replace("session #{{SESSION_ID}}", &escaped_label);
    with_label
        // Keep the token replacement for any remaining occurrences so older
        // template copies still render (substitutes empty string, since we
        // already swapped the canonical phrase above).
        .replace("{{SESSION_ID}}", "")
        .replace("{{PORT}}", &port.to_string())
        .replace("{{REDIRECT_PATH}}", &html_escape(next))
        .replace(
            "{{ERROR_DISPLAY}}",
            if show_error { "flex" } else { "none" },
        )
        .replace(
            "{{ERROR_INPUT_CLASS}}",
            if show_error { "input-error" } else { "" },
        )
}

/// Build an expired Set-Cookie header for logout — matches the scope of the
/// live cookie so the browser actually drops it. `secure` must match the
/// scheme used when the live cookie was set.
pub fn build_logout_cookie(session_id: i32, preview_domain: &str, secure: bool) -> String {
    build_logout_cookie_raw(
        &format!(
            "{}{}",
            crate::preview_auth::PREVIEW_COOKIE_PREFIX,
            session_id
        ),
        preview_domain,
        secure,
    )
}

/// Build an expired Set-Cookie header for a standalone sandbox logout.
pub fn build_logout_cookie_sandbox(
    public_id_suffix: &str,
    preview_domain: &str,
    secure: bool,
) -> String {
    build_logout_cookie_raw(
        &format!(
            "{}{}",
            crate::preview_auth::PREVIEW_SANDBOX_COOKIE_PREFIX,
            public_id_suffix
        ),
        preview_domain,
        secure,
    )
}

fn build_logout_cookie_raw(cookie_name: &str, preview_domain: &str, secure: bool) -> String {
    let domain = preview_domain.trim_start_matches("*.");
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{cookie_name}=; Domain=.{domain}; Path=/; HttpOnly{secure_attr}; SameSite=Lax; Max-Age=0"
    )
}

/// Sanitize a `next` redirect target to prevent open-redirect abuse. Only
/// allow paths that start with `/` and don't start with `//` (which browsers
/// interpret as a scheme-relative URL to another host).
pub fn sanitize_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") {
        next.to_string()
    } else {
        "/".to_string()
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_substitutes_session_port_and_next() {
        let html = generate_preview_form_html(42, 3000, "/foo/bar", false);
        assert!(html.contains("session #42"));
        assert!(html.contains("port 3000"));
        assert!(html.contains("value=\"/foo/bar\""));
        assert!(html.contains("display: none"));
    }

    #[test]
    fn form_shows_error_state() {
        let html = generate_preview_form_html(1, 8080, "/", true);
        assert!(html.contains("display: flex"));
        assert!(html.contains("input-error"));
    }

    #[test]
    fn form_escapes_next_to_prevent_xss() {
        let html = generate_preview_form_html(1, 3000, "/\"><script>x</script>", false);
        assert!(!html.contains("<script>x</script>"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn sanitize_next_accepts_absolute_path() {
        assert_eq!(sanitize_next("/dashboard"), "/dashboard");
        assert_eq!(sanitize_next("/a?b=c"), "/a?b=c");
    }

    #[test]
    fn sanitize_next_rejects_scheme_relative() {
        assert_eq!(sanitize_next("//evil.example.com"), "/");
    }

    #[test]
    fn sanitize_next_rejects_absolute_url() {
        assert_eq!(sanitize_next("https://evil.example.com"), "/");
        assert_eq!(sanitize_next("javascript:alert(1)"), "/");
    }

    #[test]
    fn sanitize_next_rejects_relative() {
        assert_eq!(sanitize_next("foo"), "/");
        assert_eq!(sanitize_next(""), "/");
    }

    #[test]
    fn logout_cookie_has_max_age_zero_and_domain() {
        let c = build_logout_cookie(5, "*.localho.st", true);
        assert!(c.starts_with("temps_preview_5="));
        assert!(c.contains("Domain=.localho.st"));
        assert!(c.contains("Max-Age=0"));
        assert!(c.contains("; Secure"));
    }

    #[test]
    fn logout_cookie_omits_secure_on_http() {
        let c = build_logout_cookie(5, "localho.st", false);
        assert!(!c.contains("Secure"));
    }
}
