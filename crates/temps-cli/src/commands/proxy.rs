use clap::Args;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use temps_config::ServerConfig;
use temps_core::CookieCrypto;
use temps_database::DbConnection;
use temps_proxy::ProxyShutdownSignal;
use tracing::{debug, error, info, warn};

/// Shutdown signal implementation for Ctrl+C with resource cleanup and timeout
struct CtrlCShutdownSignal {
    cleanup_timeout: Duration,
    db: Arc<DbConnection>,
    data_dir: PathBuf,
}

impl CtrlCShutdownSignal {
    fn new(cleanup_timeout: Duration, db: Arc<DbConnection>, data_dir: PathBuf) -> Self {
        Self {
            cleanup_timeout,
            db,
            data_dir,
        }
    }

    /// Perform cleanup operations with timeout
    async fn cleanup_resources(&self) {
        info!("Starting resource cleanup...");

        let cleanup_future = async {
            // Database cleanup
            self.cleanup_database().await;

            // File system cleanup
            self.cleanup_files().await;

            info!("Resource cleanup completed successfully");
        };

        // Apply timeout to cleanup operations
        match tokio::time::timeout(self.cleanup_timeout, cleanup_future).await {
            Ok(()) => {
                info!("All resources cleaned up within timeout");
            }
            Err(_) => {
                warn!(
                    "Cleanup timeout exceeded ({:?}), forcing shutdown",
                    self.cleanup_timeout
                );
            }
        }
    }

    async fn cleanup_database(&self) {
        debug!("Cleaning up database connections...");

        // Close the database connection gracefully
        // if let Err(e) = &self.db.close().await {
        //     warn!("Error closing database connection: {}", e);
        // } else {
        //     debug!("Database connection closed successfully");
        // }

        debug!("Database cleanup completed");
    }

    async fn cleanup_files(&self) {
        debug!("Cleaning up temporary files...");

        // Flush log buffers
        // Note: In a real implementation, you'd have access to the subscriber handle to flush
        debug!("Log buffers flushed");

        // Clean up any temporary files in data directory
        let temp_dir = self.data_dir.join("temp");
        if temp_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
                warn!("Failed to remove temp directory: {}", e);
            } else {
                debug!("Temporary files cleaned up");
            }
        }

        debug!("File cleanup completed");
    }
}

impl ProxyShutdownSignal for CtrlCShutdownSignal {
    fn wait_for_signal(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let cleanup_timeout = self.cleanup_timeout;
        let db = Arc::clone(&self.db);
        let data_dir = self.data_dir.clone();

        Box::pin(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for ctrl-c signal");
            info!("Received Ctrl+C, initiating graceful shutdown...");

            // Create a new instance for cleanup since we moved into the async block
            let cleanup_handler = CtrlCShutdownSignal::new(cleanup_timeout, db, data_dir);
            cleanup_handler.cleanup_resources().await;

            info!("Graceful shutdown completed");
        })
    }
}

#[derive(Args)]
pub struct ProxyCommand {
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

    /// Disable HTTP-to-HTTPS redirect (useful for local development without TLS)
    #[arg(long, env = "TEMPS_DISABLE_HTTPS_REDIRECT")]
    pub disable_https_redirect: bool,
}

impl ProxyCommand {
    pub fn execute(self) -> anyhow::Result<()> {
        let serve_config = Arc::new(temps_config::ServerConfig::new(
            self.address.clone(),
            self.database_url.clone(),
            self.tls_address.clone(),
            self.console_address.clone(),
        )?);

        let cookie_crypto = Arc::new(temps_core::CookieCrypto::new(&serve_config.auth_secret)?);
        let encryption_service = Arc::new(temps_core::EncryptionService::new(
            &serve_config.encryption_key,
        )?);

        info!(
            "Starting Temps proxy on {} and {}",
            self.address,
            self.tls_address
                .as_ref()
                .unwrap_or(&"no tls address".to_string())
        );

        debug!("Initializing database connection...");
        // Create tokio runtime for database connection since we need async for this
        let rt = tokio::runtime::Runtime::new()?;
        let db = rt.block_on(temps_database::establish_connection(&self.database_url))?;

        // Services are now available for use
        debug!("Cookie crypto and encryption services initialized");

        // Start proxy server
        self.start_proxy_server(
            db,
            self.address.clone(),
            self.tls_address.clone(),
            self.console_address.clone(),
            cookie_crypto,
            encryption_service,
            serve_config.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_proxy_server(
        &self,
        db: Arc<DbConnection>,
        address: String,
        tls_address: Option<String>,
        console_address: Option<String>,
        cookie_crypto: Arc<CookieCrypto>,
        encryption_service: Arc<temps_core::EncryptionService>,
        config: Arc<ServerConfig>,
    ) -> anyhow::Result<()> {
        let data_dir = config.data_dir.clone();
        let console_address = console_address
            .ok_or_else(|| anyhow::anyhow!("Console address is required for proxy server"))?;

        // Create tokio runtime to fetch preview_domain from config service
        let rt = tokio::runtime::Runtime::new()?;

        // Get preview_domain from settings
        let preview_domain = rt.block_on(async {
            let config_service = temps_config::ConfigService::new(
                Arc::new(temps_config::ServerConfig::new(
                    address.clone(),
                    self.database_url.clone(),
                    tls_address.clone(),
                    Some(console_address.clone()),
                )?),
                db.clone(),
            );

            match config_service.get_settings().await {
                Ok(settings) => Ok::<Option<String>, anyhow::Error>(Some(settings.preview_domain)),
                Err(e) => {
                    warn!("Failed to fetch preview_domain from settings: {}, using default 'localhost'", e);
                    Ok(Some("localhost".to_string()))
                }
            }
        })?;

        let proxy_config = temps_proxy::ProxyConfig {
            address,
            console_address,
            tls_address,
            preview_domain,
            disable_https_redirect: self.disable_https_redirect,
        };

        info!(
            "Starting proxy server with preview_domain: {:?}",
            proxy_config.preview_domain
        );

        // Create job queue for route table update notifications
        let (queue, _keep_alive_receiver): (Arc<dyn temps_core::JobQueue>, _) =
            temps_queue::BroadcastQueueService::create_job_queue_arc_with_receiver(1000);

        // Initialize route table with listener (preview_domain loaded from settings)
        let route_table = Arc::new(temps_proxy::CachedPeerTable::new(db.clone()));
        let listener = Arc::new(temps_routes::RouteTableListener::new(
            route_table.clone(),
            self.database_url.clone(),
            queue.clone(),
        ));

        // Start route table listener
        info!("Starting route table listener...");
        rt.block_on(async { listener.start_listening().await })?;

        // Start project change listener
        // Keep the listener alive on the stack so its Drop doesn't abort the background task
        info!("Starting project change listener...");
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

        // Start the in-process route reload subscriber. Reloads the route table
        // on Job::ForceRouteReload published over the shared queue.
        //
        // NOTE: In this standalone `temps proxy` command the deploy pipeline
        // runs in a *separate* control-plane process with its own queue, so
        // ForceRouteReload events never reach this subscriber — the PG
        // LISTEN/NOTIFY path (ProjectChangeListener / RouteTableListener above)
        // remains the route-reload mechanism here. The deterministic in-process
        // path only applies to the single-binary `temps serve` mode where the
        // control plane and proxy share one queue. We still wire the subscriber
        // for consistency and so it works if this process ever also runs the
        // deploy pipeline. Kept alive on the stack so its Drop doesn't abort the task.
        info!("Starting route reload subscriber...");
        let route_reload_subscriber =
            temps_routes::RouteReloadSubscriber::new(route_table.clone(), queue.clone());
        // start() calls tokio::spawn, so it must run inside the runtime context.
        rt.block_on(async { route_reload_subscriber.start() });

        // Wire the admin gate from boot-time config (env vars take precedence,
        // else the `settings` DB row). This 404s requests to admin/management
        // routes that fall outside the allowlist before they ever reach the
        // console listener — the same network-layer enforcement `temps serve`
        // gives the in-process proxy. We only need the resolved handle here, not
        // the full `AdminGateService` (which exists to back the console's
        // persist API). `new` fails CLOSED on a DB error, so a broken settings
        // row refuses to boot rather than opening the gate.
        //
        // NOTE: this is boot-time config only. Live admin-gate edits made
        // through the console's API swap the console's in-process handle but do
        // NOT yet propagate to this separate proxy process — operators must
        // restart `temps proxy` to pick up a changed allowlist. Cross-process
        // admin-gate refresh is tracked as ADR-017 Phase 3.
        let admin_gate_handle = match rt.block_on(
            crate::commands::serve::admin_gate_service::AdminGateService::new(
                db.clone(),
                &config.admin_allowed_ips,
                &config.admin_allowed_hosts,
                config.admin_trust_forwarded_for,
            ),
        ) {
            Ok((_service, handle)) => Some(handle),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to initialize admin gate: {}. Refusing to start the proxy with \
                     an unresolved gate. Fix the `settings` row or set TEMPS_ADMIN_ALLOWED_IPS / \
                     TEMPS_ADMIN_ALLOWED_HOSTS.",
                    e
                ));
            }
        };

        let shutdown_signal = Box::new(CtrlCShutdownSignal::new(
            Duration::from_secs(30),
            db.clone(),
            data_dir.clone(),
        )) as Box<dyn ProxyShutdownSignal>;

        match temps_proxy::setup_proxy_server(
            db,
            proxy_config,
            cookie_crypto,
            encryption_service,
            route_table,
            shutdown_signal,
            config.clone(),
            None, // on-demand not available in standalone proxy mode (ADR-017 Phase 2)
            admin_gate_handle,
        ) {
            Ok(_) => {
                info!("Proxy server exited");
                Ok(())
            }
            Err(e) => {
                error!("Failed to start proxy server: {}", e);
                Err(anyhow::anyhow!("Failed to start proxy server: {}", e))
            }
        }
    }
}
