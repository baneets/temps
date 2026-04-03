use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Application settings stored in the database
/// All fields have sensible defaults for easy onboarding
#[derive(Debug, Clone, Serialize, ToSchema, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    // Core settings
    pub external_url: Option<String>,
    pub preview_domain: String,

    // Access control
    pub allow_readonly_external_access: bool,

    // Demo mode settings
    pub demo_mode: DemoModeSettings,

    // Screenshot settings
    pub screenshots: ScreenshotSettings,

    // TLS/ACME settings
    pub letsencrypt: LetsEncryptSettings,

    // DNS provider settings
    pub dns_provider: DnsProviderSettings,

    // Security settings
    pub security_headers: SecurityHeadersSettings,
    pub rate_limiting: RateLimitSettings,

    // Docker registry settings
    pub docker_registry: DockerRegistrySettings,

    // System monitoring settings
    pub disk_space_alert: DiskSpaceAlertSettings,

    // Docker container log settings
    pub container_logs: ContainerLogSettings,

    // Multi-node settings
    pub multi_node: MultiNodeSettings,

    // Agent sandbox settings (global defaults)
    pub agent_sandbox: AgentSandboxSettings,
}

/// Docker container log rotation settings
/// Controls the `--log-opt max-size` and `--log-opt max-file` for containers
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ContainerLogSettings {
    /// Maximum size of each log file (e.g., "50m", "100m", "1g")
    /// Docker default is unlimited; we default to "50m" to prevent disk exhaustion
    #[schema(example = "50m")]
    pub max_size: String,
    /// Maximum number of rotated log files to keep (e.g., 3 means up to 3 x max_size total)
    #[schema(example = 3)]
    pub max_file: u32,
    /// Maximum size for external service container logs (postgres, redis, etc.)
    /// Defaults to "20m" since services are typically less verbose than app containers
    #[schema(example = "20m")]
    pub service_max_size: String,
    /// Maximum rotated log files for external service containers
    #[schema(example = 3)]
    pub service_max_file: u32,
}

/// Global agent sandbox settings. Controls whether agent runs are isolated
/// inside Docker containers by default. Individual agents can override this.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct AgentSandboxSettings {
    /// Whether sandbox is enabled globally for all agents by default.
    /// Individual agents can override this with their `sandbox_enabled` field.
    pub enabled: bool,
    /// CPU limit in cores for sandbox containers
    #[schema(example = 2.0)]
    pub cpu_limit: f64,
    /// Memory limit in MB for sandbox containers
    #[schema(example = 2048)]
    pub memory_limit_mb: u64,
}

impl Default for AgentSandboxSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cpu_limit: 2.0,
            memory_limit_mb: 2048,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ScreenshotSettings {
    pub enabled: bool,
    pub provider: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct LetsEncryptSettings {
    pub email: Option<String>,
    pub environment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct DnsProviderSettings {
    pub provider: String,
    pub cloudflare_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct DockerRegistrySettings {
    pub enabled: bool,
    pub registry_url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls_verify: bool,
    pub ca_certificate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct SecurityHeadersSettings {
    pub enabled: bool,
    pub preset: String,
    pub content_security_policy: Option<String>,
    pub x_frame_options: String,
    pub x_content_type_options: String,
    pub x_xss_protection: String,
    pub strict_transport_security: String,
    pub referrer_policy: String,
    pub permissions_policy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct RateLimitSettings {
    pub enabled: bool,
    pub max_requests_per_minute: u32,
    pub max_requests_per_hour: u32,
    pub whitelist_ips: Vec<String>,
    pub blacklist_ips: Vec<String>,
}

/// Disk space alert settings for monitoring disk usage
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct DiskSpaceAlertSettings {
    /// Whether disk space alerts are enabled
    pub enabled: bool,
    /// Threshold percentage (0-100) at which to trigger alerts
    #[schema(minimum = 0, maximum = 100, example = 80)]
    pub threshold_percent: u32,
    /// Interval in seconds between disk space checks
    #[schema(minimum = 60, example = 300)]
    pub check_interval_seconds: u64,
    /// Path to monitor (defaults to data directory)
    pub monitor_path: Option<String>,
}

/// Demo mode settings for allowing unauthenticated access to demo subdomain
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
#[derive(Default)]
pub struct DemoModeSettings {
    /// Whether demo mode is enabled (disabled by default for security)
    pub enabled: bool,
    /// Optional custom domain for demo mode (defaults to demo.<preview_domain>)
    /// If set, this overrides the default demo.preview_domain pattern
    #[schema(example = "demo.example.com")]
    pub domain: Option<String>,
}

/// Multi-node cluster settings
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(default)]
#[derive(Default)]
pub struct MultiNodeSettings {
    /// SHA-256 hash of the join token (never store plaintext)
    pub join_token_hash: Option<String>,
    /// Private/WireGuard IP address of the control plane node.
    /// Used by remote worker nodes to reach services (databases, etc.) running on the control plane.
    /// Set via `--private-address` or `TEMPS_PRIVATE_ADDRESS`.
    pub private_address: Option<String>,
}

const DEFAULT_LOCAL_DOMAIN: &str = "localho.st";
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            external_url: None,
            preview_domain: DEFAULT_LOCAL_DOMAIN.to_string(),
            allow_readonly_external_access: false,
            demo_mode: DemoModeSettings::default(),
            screenshots: ScreenshotSettings::default(),
            letsencrypt: LetsEncryptSettings::default(),
            dns_provider: DnsProviderSettings::default(),
            security_headers: SecurityHeadersSettings::default(),
            rate_limiting: RateLimitSettings::default(),
            docker_registry: DockerRegistrySettings::default(),
            disk_space_alert: DiskSpaceAlertSettings::default(),
            container_logs: ContainerLogSettings::default(),
            multi_node: MultiNodeSettings::default(),
            agent_sandbox: AgentSandboxSettings::default(),
        }
    }
}

impl Default for ContainerLogSettings {
    fn default() -> Self {
        Self {
            max_size: "50m".to_string(),
            max_file: 3,
            service_max_size: "20m".to_string(),
            service_max_file: 3,
        }
    }
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default as requested
            provider: "local".to_string(),
            url: "".to_string(),
        }
    }
}

impl Default for LetsEncryptSettings {
    fn default() -> Self {
        Self {
            email: None,
            environment: "production".to_string(),
        }
    }
}

impl Default for DnsProviderSettings {
    fn default() -> Self {
        Self {
            provider: "manual".to_string(),
            cloudflare_api_key: None,
        }
    }
}

impl Default for DockerRegistrySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            registry_url: None,
            username: None,
            password: None,
            tls_verify: true,
            ca_certificate: None,
        }
    }
}

impl Default for SecurityHeadersSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: "moderate".to_string(),
            content_security_policy: Some(
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self'; frame-ancestors 'self'".to_string()
            ),
            x_frame_options: "SAMEORIGIN".to_string(),
            x_content_type_options: "nosniff".to_string(),
            x_xss_protection: "1; mode=block".to_string(),
            strict_transport_security: "max-age=31536000; includeSubDomains".to_string(),
            referrer_policy: "strict-origin-when-cross-origin".to_string(),
            permissions_policy: Some("geolocation=(), microphone=(), camera=()".to_string()),
        }
    }
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for initial setup
            max_requests_per_minute: 60,
            max_requests_per_hour: 1000,
            whitelist_ips: vec![],
            blacklist_ips: vec![],
        }
    }
}

impl Default for DiskSpaceAlertSettings {
    fn default() -> Self {
        Self {
            enabled: true,               // Enabled by default
            threshold_percent: 80,       // Alert at 80% usage
            check_interval_seconds: 300, // Check every 5 minutes
            monitor_path: None,          // Use data directory by default
        }
    }
}

impl SecurityHeadersSettings {
    /// Strict preset for maximum security
    pub fn strict() -> Self {
        Self {
            enabled: true,
            preset: "strict".to_string(),
            content_security_policy: Some(
                "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'".to_string()
            ),
            x_frame_options: "DENY".to_string(),
            x_content_type_options: "nosniff".to_string(),
            x_xss_protection: "1; mode=block".to_string(),
            strict_transport_security: "max-age=63072000; includeSubDomains; preload".to_string(),
            referrer_policy: "no-referrer".to_string(),
            permissions_policy: Some("geolocation=(), microphone=(), camera=(), payment=(), usb=()".to_string()),
        }
    }

    /// Permissive preset for development/compatibility
    pub fn permissive() -> Self {
        Self {
            enabled: true,
            preset: "permissive".to_string(),
            content_security_policy: Some(
                "default-src *; script-src * 'unsafe-inline' 'unsafe-eval'; style-src * 'unsafe-inline'; img-src * data:; font-src * data:".to_string()
            ),
            x_frame_options: "SAMEORIGIN".to_string(),
            x_content_type_options: "nosniff".to_string(),
            x_xss_protection: "1; mode=block".to_string(),
            strict_transport_security: "max-age=31536000".to_string(),
            referrer_policy: "no-referrer-when-downgrade".to_string(),
            permissions_policy: None,
        }
    }

    /// Disabled preset (no security headers)
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            preset: "disabled".to_string(),
            content_security_policy: None,
            x_frame_options: String::new(),
            x_content_type_options: String::new(),
            x_xss_protection: String::new(),
            strict_transport_security: String::new(),
            referrer_policy: String::new(),
            permissions_policy: None,
        }
    }
}

impl AppSettings {
    /// Create settings from JSON value, using defaults for missing fields
    pub fn from_json(value: serde_json::Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }

    /// Convert settings to JSON value
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}
