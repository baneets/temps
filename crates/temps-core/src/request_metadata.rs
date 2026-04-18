use axum::http::HeaderMap;

#[derive(Clone)]
pub struct RequestMetadata {
    pub ip_address: String,
    pub user_agent: String,
    pub headers: HeaderMap,
    pub visitor_id_cookie: Option<String>,
    pub session_id_cookie: Option<String>,
    /// Full origin including scheme and `:port` suffix when present.
    /// Suitable for constructing user-visible absolute URLs (links, redirects).
    pub base_url: String,
    pub scheme: String, // "http" or "https"
    /// Hostname from the Host header with any `:port` suffix stripped.
    /// Matches the key used by the proxy route table, so handlers can safely
    /// pass this into `CachedPeerTable::get_route(&metadata.host)` without
    /// worrying about non-standard ports (e.g. the :8080 dev proxy).
    pub host: String,
    pub is_secure: bool, // true if HTTPS
}

/// Strip any `:port` suffix from a raw Host header.
///
/// The proxy's route table is keyed on the hostname only, so requests that
/// arrive on non-default ports (dev setups, `localho.st:8080`, etc.) must be
/// normalized before lookup. IPv6 literals are not supported in Host headers
/// without brackets, so naive `split(':')` is sufficient here.
pub fn host_without_port(raw_host: &str) -> &str {
    raw_host.split(':').next().unwrap_or(raw_host)
}

#[cfg(test)]
mod tests {
    use super::host_without_port;

    #[test]
    fn strips_port_when_present() {
        assert_eq!(
            host_without_port("sandbox-test.localho.st:8080"),
            "sandbox-test.localho.st"
        );
    }

    #[test]
    fn passes_through_when_no_port() {
        assert_eq!(host_without_port("example.com"), "example.com");
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(host_without_port(""), "");
    }
}
