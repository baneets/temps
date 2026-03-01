pub mod console;
mod proxy;
mod shutdown;

use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

pub use console::start_console_api;
pub use proxy::start_proxy_server;

#[derive(Args)]
pub struct ServeCommand {
    /// Address to bind the server to
    #[arg(long, default_value = "127.0.0.1:3000", env = "TEMPS_ADDRESS")]
    pub address: String,

    /// TLS address to bind the server to
    #[arg(long, env = "TEMPS_TLS_ADDRESS")]
    pub tls_address: Option<String>,

    /// Database connection URL
    #[arg(long, env = "TEMPS_DATABASE_URL")]
    pub database_url: String,

    /// Data directory for storing configuration and runtime files
    #[arg(long, env = "TEMPS_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Console/Admin address (defaults to random port on localhost)
    #[arg(long, env = "TEMPS_CONSOLE_ADDRESS")]
    pub console_address: Option<String>,

    /// Screenshot provider to use: "local" (headless Chrome), "remote", or "noop" (disabled)
    /// Use "noop" on servers without Chrome installed to skip screenshot functionality
    #[arg(long, env = "TEMPS_SCREENSHOT_PROVIDER", value_parser = ["local", "remote", "noop", "disabled", "none"])]
    pub screenshot_provider: Option<String>,

    /// Enable demo mode for unauthenticated access at demo.<preview_domain>
    /// WARNING: This allows anyone to access the application without authentication
    #[arg(long, env = "TEMPS_DEMO_MODE")]
    pub demo_mode: bool,

    /// Custom domain for demo mode (overrides default demo.<preview_domain>)
    /// Only used when --demo-mode is enabled
    #[arg(long, env = "TEMPS_DEMO_DOMAIN")]
    pub demo_domain: Option<String>,

    /// Additional template YAML files to load (can be specified multiple times)
    /// Templates are merged with the bundled defaults; validation errors will prevent startup
    #[arg(long = "templates", env = "TEMPS_ADDITIONAL_TEMPLATES")]
    pub additional_templates: Vec<PathBuf>,

    /// Disable HTTP-to-HTTPS redirect (useful for local development without TLS)
    #[arg(long, env = "TEMPS_DISABLE_HTTPS_REDIRECT")]
    pub disable_https_redirect: bool,
}

impl ServeCommand {
    pub fn execute(self) -> anyhow::Result<()> {
        // Set screenshot provider from CLI flag (takes precedence over env var)
        // This allows: temps serve --screenshot-provider=noop
        if let Some(ref provider) = self.screenshot_provider {
            std::env::set_var("TEMPS_SCREENSHOT_PROVIDER", provider);
            debug!("Screenshot provider set to '{}' from CLI flag", provider);
        }

        let serve_config = Arc::new(temps_config::ServerConfig::new(
            self.address.clone(),
            self.database_url.clone(),
            self.tls_address.clone(),
            self.console_address.clone(),
        )?);
        let encryption_service = Arc::new(temps_core::EncryptionService::new(
            &serve_config.encryption_key,
        )?);

        let cookie_crypto = Arc::new(temps_core::CookieCrypto::new(&serve_config.auth_secret)?);

        debug!("Initializing database connection...");
        // Create tokio runtime for database connection since we need async for this
        let rt = tokio::runtime::Runtime::new()?;
        let db = rt.block_on(temps_database::establish_connection(&self.database_url))?;

        // Update demo mode settings from CLI flags
        if self.demo_mode {
            info!("⚠️  Demo mode ENABLED - unauthenticated access allowed");
            let db_for_settings = db.clone();
            let demo_domain = self.demo_domain.clone();
            let serve_config_for_demo = serve_config.clone();
            rt.block_on(async move {
                let config_service =
                    temps_config::ConfigService::new(serve_config_for_demo, db_for_settings);
                if let Err(e) = config_service
                    .update_setting_field(|settings| {
                        settings.demo_mode.enabled = true;
                        settings.demo_mode.domain = demo_domain;
                    })
                    .await
                {
                    tracing::error!("Failed to update demo mode settings: {}", e);
                }
            });
            if let Some(ref domain) = self.demo_domain {
                info!("Demo mode domain: {}", domain);
            } else {
                info!("Demo mode using default domain: demo.<preview_domain>");
            }
        }

        info!(
            "Starting Temps server on {} and {}",
            self.address,
            self.tls_address
                .as_ref()
                .unwrap_or(&"no tls address".to_string())
        );

        // Services are now available for use
        debug!("Cookie crypto and encryption services initialized");

        // Create the shared job queue FIRST — it is used by route table listeners
        // (to publish RouteTableUpdated) and by the console API (for all other jobs).
        let (queue, _keep_alive_receiver): (Arc<dyn temps_core::JobQueue>, _) =
            temps_queue::BroadcastQueueService::create_job_queue_arc_with_receiver(1000);

        // Create shared route table instance (used by both console API and proxy)
        let route_table = Arc::new(temps_proxy::CachedPeerTable::new(db.clone()));
        let route_table_listener = Arc::new(temps_routes::RouteTableListener::new(
            route_table.clone(),
            self.database_url.clone(),
            queue.clone(),
        ));

        let rt = tokio::runtime::Runtime::new()?;
        // Start the route table listener (block_on to ensure initial load completes)
        let route_table_listener_clone = route_table_listener.clone();
        rt.block_on(async move {
            if let Err(e) = route_table_listener_clone.start_listening().await {
                tracing::error!("Route table listener failed: {}", e);
            }
        });

        // Start the project change listener
        // Keep the listener alive on the stack so its Drop doesn't abort the background task
        let project_listener = temps_routes::ProjectChangeListener::new(
            self.database_url.clone(),
            route_table.clone(),
            queue.clone(),
        );
        rt.block_on(async {
            if let Err(e) = project_listener.start_listening().await {
                tracing::error!("Project change listener failed: {}", e);
            }
        });

        // Start console API server in background (non-blocking).
        // The proxy does NOT wait for the console to be ready. This ensures that
        // deployed applications remain reachable even if console initialization
        // fails (e.g. Docker check, GeoIP validation, plugin init). Console API
        // requests will get connection-refused until the console finishes starting,
        // but that is far better than all proxied traffic being down.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

        let params = console::ConsoleApiParams {
            db: db.clone(),
            config: serve_config.clone(),
            cookie_crypto: cookie_crypto.clone(),
            encryption_service: encryption_service.clone(),
            route_table: route_table.clone(),
            queue: queue.clone(),
            ready_signal: Some(ready_tx),
            additional_templates: self.additional_templates.clone(),
        };
        rt.spawn(async move {
            match start_console_api(params).await {
                Ok(()) => {
                    info!("Console API server exited normally");
                }
                Err(e) => {
                    tracing::error!("❌ Console API failed to start: {}", e);
                    tracing::error!("Error details: {:?}", e);
                    tracing::error!(
                        "The console management UI will not be available. \
                         Proxied traffic to deployed applications is NOT affected."
                    );
                }
            }
        });

        // Monitor console readiness in a background thread so we can log it,
        // but do NOT block proxy startup on it.
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("Failed to create runtime for console monitor: {}", e);
                    return;
                }
            };
            match rt.block_on(ready_rx) {
                Ok(()) => {
                    info!("✅ Console API is ready");
                }
                Err(_) => {
                    tracing::error!(
                        "❌ Console API failed to become ready — check error logs above"
                    );
                }
            }
        });

        info!("Starting proxy server (console API initializing in background)...");

        // Start proxy server (this will block until shutdown)
        start_proxy_server(
            db,
            self.address.clone(),
            self.tls_address.clone(),
            cookie_crypto.clone(),
            encryption_service.clone(),
            self.database_url.clone(),
            route_table,
            serve_config.clone(),
            self.disable_https_redirect,
        )
    }
}
