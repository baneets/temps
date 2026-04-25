//! Opaque public IDs for workspace sessions. Format: `wss_` + 16 hex chars.
//!
//! Embedded in preview hostnames (`ws-<hex>-<port>.<domain>`) so users can't
//! enumerate sessions by walking the integer primary key. API routes and
//! foreign keys still use `id: i32` — this is purely a display identifier
//! for URLs that escape the authenticated control plane.

use rand::RngCore;
use std::fmt::Write;

pub const PUBLIC_ID_PREFIX: &str = "wss_";

/// Generate a new opaque public workspace session ID.
pub fn generate() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(PUBLIC_ID_PREFIX.len() + 16);
    out.push_str(PUBLIC_ID_PREFIX);
    for b in bytes {
        // `write!` into a String is infallible.
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// Strip the `wss_` prefix, returning the DNS-safe hex label used in
/// preview hostnames. Falls back to the input when no prefix is present
/// (defensive — backfilled rows always carry the prefix).
pub fn hex_label(public_id: &str) -> &str {
    public_id
        .strip_prefix(PUBLIC_ID_PREFIX)
        .unwrap_or(public_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_has_prefix_and_length() {
        let id = generate();
        assert!(id.starts_with(PUBLIC_ID_PREFIX));
        let rest = id.strip_prefix(PUBLIC_ID_PREFIX).unwrap();
        assert_eq!(rest.len(), 16);
        assert!(rest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_is_unique() {
        assert_ne!(generate(), generate());
    }

    #[test]
    fn hex_label_strips_prefix() {
        assert_eq!(hex_label("wss_abcdef0123456789"), "abcdef0123456789");
    }

    #[test]
    fn hex_label_passthrough_when_no_prefix() {
        assert_eq!(hex_label("abcdef0123456789"), "abcdef0123456789");
    }
}
