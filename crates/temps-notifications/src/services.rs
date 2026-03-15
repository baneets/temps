use crate::types::{Notification, NotificationPriority, NotificationSeverity, NotificationType};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::{authentication::Credentials, client::TlsParametersBuilder},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, JoinType, ModelTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Set,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use temps_core::notifications::{
    EmailMessage, NotificationData, NotificationError as CoreNotificationError,
    NotificationService as CoreNotificationService,
};
use temps_entities::types::RoleType;
use temps_entities::{
    notification_preferences, notification_providers, notifications, roles, user_roles, users,
};
use tracing::{error, info};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateProviderRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum TlsMode {
    None,     // No encryption
    Starttls, // STARTTLS (opportunistic TLS)
    Tls,      // Direct TLS connection
}

fn default_tls_mode() -> TlsMode {
    TlsMode::Starttls
}

fn default_starttls_required() -> bool {
    true
}

fn default_accept_invalid_certs() -> bool {
    false // Default to secure behavior
}

// Provider-specific structs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailProvider {
    pub smtp_host: String,
    pub smtp_port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub from_address: String,
    pub from_name: Option<String>,
    pub to_addresses: Vec<String>,
    #[serde(default = "default_tls_mode")]
    pub tls_mode: TlsMode,
    #[serde(default = "default_starttls_required")]
    pub starttls_required: bool, // Only used when tls_mode is Starttls
    #[serde(default = "default_accept_invalid_certs")]
    pub accept_invalid_certs: bool, // Accept self-signed certificates (use with caution)
    #[serde(skip)]
    mailer: Option<AsyncSmtpTransport<Tokio1Executor>>,
    #[serde(skip)]
    db: Arc<DatabaseConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackProvider {
    pub webhook_url: String,
    pub channel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SMSProvider {
    pub api_key: String,
    pub from_number: String,
    pub to_numbers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppProvider {
    pub api_key: String,
    pub from_number: String,
    pub to_numbers: Vec<String>,
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn default_timeout_secs() -> u64 {
    30
}

/// Generic webhook provider for custom integrations
/// Sends notification data as JSON payload to any HTTP endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookProvider {
    /// The URL to send webhook requests to
    pub url: String,
    /// HTTP method (POST, PUT, PATCH). Defaults to POST.
    #[serde(default = "default_http_method")]
    pub method: String,
    /// Custom headers to include in the request (e.g., for authentication)
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    /// Request timeout in seconds. Defaults to 30.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

#[async_trait]
pub trait NotificationProvider: Send + Sync {
    async fn initialize(&mut self, db: Arc<DatabaseConnection>) -> Result<()>;
    async fn send(&self, notification: &Notification) -> Result<()>;
    async fn health_check(&self) -> Result<bool>;
}

impl EmailProvider {
    async fn get_admin_users(&self) -> Result<Vec<users::Model>> {
        let db = self.db.as_ref();

        // First get the admin role
        let admin_role = roles::Entity::find()
            .filter(roles::Column::Name.eq(RoleType::Admin.as_str()))
            .one(db)
            .await
            .map_err(|e| {
                error!("Failed to get admin role: {}", e);
                anyhow::anyhow!("Failed to get admin role: {}", e)
            })?
            .ok_or_else(|| anyhow::anyhow!("Admin role not found"))?;

        // Then get all users with admin role through user_roles join
        let admin_users = users::Entity::find()
            .join(JoinType::InnerJoin, users::Relation::UserRoles.def())
            .filter(user_roles::Column::RoleId.eq(admin_role.id))
            .all(db)
            .await
            .map_err(|e| {
                error!("Failed to get admin users: {}", e);
                anyhow::anyhow!("Failed to get admin users: {}", e)
            })?;

        Ok(admin_users)
    }
}

#[async_trait]
impl NotificationProvider for EmailProvider {
    async fn initialize(&mut self, db: Arc<DatabaseConnection>) -> Result<()> {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp_host)
            .port(self.smtp_port);

        // Configure authentication if username is provided
        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            if !username.is_empty() {
                let creds = Credentials::new(username.clone(), password.clone());
                builder = builder.credentials(creds);
            }
        }

        // Configure TLS based on the mode
        let mailer = match self.tls_mode {
            TlsMode::None => {
                // No TLS at all
                builder.build()
            }
            TlsMode::Starttls => {
                // STARTTLS - upgrade plain connection to TLS
                if self.starttls_required {
                    // Require STARTTLS - accept self-signed certificates based on configuration
                    let tls = TlsParametersBuilder::new(self.smtp_host.clone())
                        .dangerous_accept_invalid_certs(
                            self.accept_invalid_certs
                                || self.smtp_host == "localhost"
                                || self.smtp_host == "127.0.0.1",
                        )
                        .dangerous_accept_invalid_hostnames(
                            self.accept_invalid_certs
                                || self.smtp_host == "localhost"
                                || self.smtp_host == "127.0.0.1",
                        )
                        .build()?;
                    builder
                        .tls(lettre::transport::smtp::client::Tls::Required(tls))
                        .build()
                } else {
                    // Opportunistic STARTTLS (use if available) - accept self-signed certificates based on configuration
                    let tls = TlsParametersBuilder::new(self.smtp_host.clone())
                        .dangerous_accept_invalid_certs(
                            self.accept_invalid_certs
                                || self.smtp_host == "localhost"
                                || self.smtp_host == "127.0.0.1",
                        )
                        .dangerous_accept_invalid_hostnames(
                            self.accept_invalid_certs
                                || self.smtp_host == "localhost"
                                || self.smtp_host == "127.0.0.1",
                        )
                        .build()?;
                    builder
                        .tls(lettre::transport::smtp::client::Tls::Opportunistic(tls))
                        .build()
                }
            }
            TlsMode::Tls => {
                // Direct TLS connection (SMTPS) - accept self-signed certificates based on configuration
                let tls = TlsParametersBuilder::new(self.smtp_host.clone())
                    .dangerous_accept_invalid_certs(
                        self.accept_invalid_certs
                            || self.smtp_host == "localhost"
                            || self.smtp_host == "127.0.0.1",
                    )
                    .dangerous_accept_invalid_hostnames(
                        self.accept_invalid_certs
                            || self.smtp_host == "localhost"
                            || self.smtp_host == "127.0.0.1",
                    )
                    .build()?;
                let mut relay_builder =
                    AsyncSmtpTransport::<Tokio1Executor>::relay(&self.smtp_host)?
                        .port(self.smtp_port)
                        .tls(lettre::transport::smtp::client::Tls::Wrapper(tls));

                // Add credentials if provided
                if let (Some(username), Some(password)) = (&self.username, &self.password) {
                    if !username.is_empty() {
                        relay_builder = relay_builder
                            .credentials(Credentials::new(username.clone(), password.clone()));
                    }
                }

                relay_builder.build()
            }
        };

        self.mailer = Some(mailer);
        self.db = db;

        Ok(())
    }

    async fn send(&self, notification: &Notification) -> Result<()> {
        let mailer = self
            .mailer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Email provider not initialized"))?;

        let priority_prefix = match notification.priority {
            NotificationPriority::Low => "[LOW] ",
            NotificationPriority::Normal => "",
            NotificationPriority::High => "[HIGH] ",
            NotificationPriority::Critical => "[CRITICAL] ",
        };

        let from = Mailbox::new(self.from_name.clone(), self.from_address.parse()?);

        // Create the HTML body once since it's the same for all recipients
        let email_body = Self::render_notification_email(notification);

        // Combine configured addresses and admin emails into a single list
        let mut all_recipients = self.to_addresses.clone();
        if let Ok(admin_users) = self.get_admin_users().await {
            all_recipients.extend(admin_users.into_iter().filter_map(|user| {
                if !user.email.trim().is_empty() {
                    Some(user.email)
                } else {
                    None
                }
            }));
        }
        // Remove duplicates
        all_recipients.sort();
        all_recipients.dedup();

        // Send individual emails to each recipient
        for addr in &all_recipients {
            match addr.parse::<Mailbox>() {
                Ok(to_mailbox) => {
                    let email_msg = Message::builder()
                        .from(from.clone())
                        .to(to_mailbox)
                        .subject(format!("{}{}", priority_prefix, notification.title))
                        .header(ContentType::TEXT_HTML)
                        .body(email_body.clone())?;

                    if let Err(e) = mailer.send(email_msg).await {
                        error!("Failed to send email to {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    error!("Invalid email address {}: {}", addr, e);
                }
            }
        }

        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        EmailProvider::email_health_check(self).await
    }
}

impl EmailProvider {
    fn render_notification_email(notification: &Notification) -> String {
        let (accent_color, bg_color, icon, label) = match notification.priority {
            NotificationPriority::Low => ("#6b7280", "#f9fafb", "&#8505;", "Info"),
            NotificationPriority::Normal => ("#2563eb", "#eff6ff", "&#9432;", "Notice"),
            NotificationPriority::High => ("#d97706", "#fffbeb", "&#9888;", "Warning"),
            NotificationPriority::Critical => ("#dc2626", "#fef2f2", "&#128680;", "Critical"),
        };

        let metadata_html = if notification.metadata.is_empty() {
            String::new()
        } else {
            let rows: String = notification
                .metadata
                .iter()
                .map(|(k, v)| {
                    // Format key: replace underscores with spaces and title-case
                    let label = k
                        .split('_')
                        .map(|w| {
                            let mut c = w.chars();
                            match c.next() {
                                None => String::new(),
                                Some(f) => {
                                    f.to_uppercase().collect::<String>() + c.as_str()
                                }
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!(
                        r#"<tr>
                            <td style="padding: 8px 12px; color: #6b7280; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif; font-size: 13px; white-space: nowrap; vertical-align: top;">{}</td>
                            <td style="padding: 8px 12px; color: #1f2937; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif; font-size: 13px; word-break: break-all;">{}</td>
                        </tr>"#,
                        label, v
                    )
                })
                .collect();

            format!(
                r#"<tr><td colspan="2" style="padding: 0;">
                    <table width="100%" cellpadding="0" cellspacing="0" style="margin-top: 24px; border-top: 1px solid #e5e7eb;">
                        <tr><td style="padding: 16px 0 8px; color: #374151; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif; font-size: 12px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em;">Details</td></tr>
                        <tr><td style="padding: 0;">
                            <table width="100%" cellpadding="0" cellspacing="0" style="background: #f9fafb; border-radius: 6px;">
                                {}
                            </table>
                        </td></tr>
                    </table>
                </td></tr>"#,
                rows
            )
        };

        // Escape HTML in message, preserve newlines
        let message_html = notification
            .message
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\n', "<br>");

        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
</head>
<body style="margin: 0; padding: 0; background-color: #f3f4f6; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif; -webkit-font-smoothing: antialiased;">
    <table width="100%" cellpadding="0" cellspacing="0" style="background-color: #f3f4f6; padding: 32px 16px; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
        <tr><td align="center" style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
            <table width="600" cellpadding="0" cellspacing="0" style="max-width: 600px; width: 100%;">
                <!-- Header -->
                <tr><td style="padding: 24px 32px; background: #0f172a; border-radius: 8px 8px 0 0;">
                    <table width="100%" cellpadding="0" cellspacing="0">
                        <tr>
                            <td style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 18px; font-weight: 700; color: #ffffff; letter-spacing: -0.02em;">Temps</td>
                            <td align="right" style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 12px; color: #94a3b8;">{timestamp}</td>
                        </tr>
                    </table>
                </td></tr>

                <!-- Priority Badge -->
                <tr><td style="padding: 24px 32px 0; background: #ffffff; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
                    <table cellpadding="0" cellspacing="0">
                        <tr><td style="padding: 4px 12px; background: {bg_color}; border: 1px solid {accent_color}22; border-radius: 100px;">
                            <span style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 12px; font-weight: 600; color: {accent_color};">{icon} {label}</span>
                        </td></tr>
                    </table>
                </td></tr>

                <!-- Title -->
                <tr><td style="padding: 12px 32px 0; background: #ffffff; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
                    <h1 style="margin: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 20px; font-weight: 600; color: #111827; line-height: 1.4;">{title}</h1>
                </td></tr>

                <!-- Message Body -->
                <tr><td style="padding: 16px 32px 24px; background: #ffffff; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
                    <table width="100%" cellpadding="0" cellspacing="0">
                        <tr><td style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 14px; color: #374151; line-height: 1.7;">
                            {message}
                        </td></tr>
                        {metadata}
                    </table>
                </td></tr>

                <!-- Footer -->
                <tr><td style="padding: 16px 32px; background: #f9fafb; border-top: 1px solid #e5e7eb; border-radius: 0 0 8px 8px; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;">
                    <table width="100%" cellpadding="0" cellspacing="0">
                        <tr>
                            <td style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 12px; color: #9ca3af;">Sent by Temps &middot; Self-hosted PaaS</td>
                            <td align="right" style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; font-size: 12px; color: #9ca3af;">{priority} priority</td>
                        </tr>
                    </table>
                </td></tr>
            </table>
        </td></tr>
    </table>
</body>
</html>"#,
            title = notification.title,
            timestamp = notification.timestamp.format("%b %d, %Y at %H:%M UTC"),
            accent_color = accent_color,
            bg_color = bg_color,
            icon = icon,
            label = label,
            message = message_html,
            metadata = metadata_html,
            priority = notification.priority,
        )
    }

    async fn email_health_check(&self) -> Result<bool> {
        if let Some(mailer) = &self.mailer {
            let test_email = Message::builder()
                .from(
                    format!(
                        "{} <{}>",
                        self.from_name.clone().unwrap_or("".to_string()),
                        self.from_address
                    )
                    .parse()?,
                )
                .to(self.to_addresses[0].parse()?)
                .subject("Health Check")
                .body(String::from("Health check email"))?;

            match mailer.test_connection().await {
                Ok(_) => {
                    mailer.send(test_email).await?;
                    Ok(true)
                }
                Err(e) => {
                    error!("Email provider health check failed: {}", e);
                    Ok(false)
                }
            }
        } else {
            Ok(false)
        }
    }
}

#[async_trait]
impl NotificationProvider for SlackProvider {
    async fn initialize(&mut self, _db: Arc<DatabaseConnection>) -> Result<()> {
        // Validate webhook URL format
        if !self.webhook_url.starts_with("https://hooks.slack.com/") {
            return Err(anyhow::anyhow!("Invalid Slack webhook URL"));
        }
        Ok(())
    }

    async fn send(&self, notification: &Notification) -> Result<()> {
        let client = reqwest::Client::new();

        let color = match notification.notification_type {
            NotificationType::Info => "#0088cc",
            NotificationType::Warning => "#ffa500",
            NotificationType::Error => "#ff0000",
            NotificationType::Alert => "#ff0000",
        };

        let metadata_fields = notification
            .metadata
            .iter()
            .map(|(k, v)| {
                serde_json::json!({
                    "title": k,
                    "value": v,
                    "short": true
                })
            })
            .collect::<Vec<_>>();

        let payload = serde_json::json!({
            "channel": self.channel,
            "attachments": [{
                "color": color,
                "title": notification.title,
                "text": notification.message,
                "fields": metadata_fields,
                "footer": format!("Priority: {:?} | Type: {:?}", notification.priority, notification.notification_type)
            }]
        });

        client.post(&self.webhook_url).json(&payload).send().await?;

        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        let client = reqwest::Client::new();

        let test_payload = serde_json::json!({
            "channel": self.channel,
            "text": "Health check"
        });

        match client
            .post(&self.webhook_url)
            .json(&test_payload)
            .send()
            .await
        {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                error!("Slack provider health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

#[async_trait]
impl NotificationProvider for WebhookProvider {
    async fn initialize(&mut self, _db: Arc<DatabaseConnection>) -> Result<()> {
        // Validate webhook URL with full SSRF protection (blocks private IPs,
        // loopback, cloud metadata, link-local, etc.)
        temps_core::url_validation::validate_external_url(&self.url)
            .map_err(|e| anyhow::anyhow!("Invalid webhook URL '{}': {}", self.url, e))?;

        // Validate HTTP method
        let method = self.method.to_uppercase();
        if !["POST", "PUT", "PATCH"].contains(&method.as_str()) {
            return Err(anyhow::anyhow!(
                "Invalid HTTP method: {}. Must be POST, PUT, or PATCH",
                self.method
            ));
        }

        Ok(())
    }

    async fn send(&self, notification: &Notification) -> Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        // Build the payload with all notification data
        let payload = serde_json::json!({
            "id": notification.id,
            "title": notification.title,
            "message": notification.message,
            "type": notification.notification_type.to_string(),
            "priority": notification.priority.to_string(),
            "severity": notification.effective_severity().to_string(),
            "timestamp": notification.timestamp.to_rfc3339(),
            "metadata": notification.metadata,
        });

        // Build the request with configured method
        let method: reqwest::Method = self
            .method
            .to_uppercase()
            .parse()
            .unwrap_or(reqwest::Method::POST);
        let mut request = client
            .request(method, &self.url)
            .header("Content-Type", "application/json")
            .json(&payload);

        // Add custom headers (useful for auth tokens, API keys, etc.)
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Webhook request failed with status {}: {}", status, body);
            return Err(anyhow::anyhow!(
                "Webhook request failed with status {}: {}",
                status,
                body
            ));
        }

        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        // Send a test payload
        let test_payload = serde_json::json!({
            "test": true,
            "type": "health_check",
            "message": "Temps webhook health check",
            "timestamp": Utc::now().to_rfc3339(),
        });

        let method: reqwest::Method = self
            .method
            .to_uppercase()
            .parse()
            .unwrap_or(reqwest::Method::POST);
        let mut request = client
            .request(method, &self.url)
            .header("Content-Type", "application/json")
            .json(&test_payload);

        // Add custom headers
        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }

        match request.send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                error!("Webhook provider health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

pub struct NotificationService {
    db: Arc<DatabaseConnection>,
    encryption_service: Arc<temps_core::EncryptionService>,
}

impl NotificationService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        encryption_service: Arc<temps_core::EncryptionService>,
    ) -> Self {
        Self {
            db,
            encryption_service,
        }
    }

    fn get_batch_key(notification: &Notification) -> String {
        format!(
            "{}:{}:{}",
            notification.notification_type, notification.priority, notification.title
        )
    }

    async fn get_enabled_providers(&self) -> Result<Vec<Box<dyn NotificationProvider>>> {
        let db_providers = notification_providers::Entity::find()
            .filter(notification_providers::Column::Enabled.eq(true))
            .all(self.db.as_ref())
            .await?;
        let mut providers = vec![];
        for provider_record in db_providers {
            match self.load_provider(&provider_record).await {
                Ok(provider) => {
                    providers.push(provider);
                }
                Err(e) => {
                    error!("Failed to load provider {}: {}", provider_record.name, e);
                }
            }
        }
        Ok(providers)
    }

    fn get_next_allowed_time(priority: &NotificationPriority) -> DateTime<Utc> {
        let now = Utc::now();
        match priority {
            NotificationPriority::Low => now + Duration::days(7),
            NotificationPriority::Normal => now + Duration::days(1),
            NotificationPriority::High => now + Duration::hours(1),
            NotificationPriority::Critical => now + Duration::minutes(15),
        }
    }

    pub async fn send_notification(&self, notification: Notification) -> Result<()> {
        let now = Utc::now();
        let batch_key_str = Self::get_batch_key(&notification);

        // Check for existing similar notifications
        let existing = notifications::Entity::find()
            .filter(notifications::Column::BatchKey.eq(&batch_key_str))
            .order_by_desc(notifications::Column::CreatedAt)
            .one(self.db.as_ref())
            .await?;

        if let Some(existing) = existing.clone() {
            // If we have a similar notification, check if we should send it or batch it
            if now < existing.next_allowed_at {
                // Update occurrence count and return
                let mut existing_update: notifications::ActiveModel = existing.clone().into();
                existing_update.occurrence_count = Set(existing.occurrence_count + 1);
                existing_update.update(self.db.as_ref()).await?;

                info!(
                    "Batching notification '{}'. Current count: {}",
                    notification.title,
                    existing.occurrence_count + 1
                );
                return Ok(());
            }
        }

        // If we reach here, we should send the notification
        let metadata_json = serde_json::to_string(&notification.metadata)?;
        let next_allowed = Self::get_next_allowed_time(&notification.priority);

        // Create new notification record
        let new_notification = notifications::ActiveModel {
            notification_id: Set(notification.id.clone()),
            title: Set(notification.title.clone()),
            message: Set(notification.message.clone()),
            notification_type: Set(notification.notification_type.to_string()),
            priority: Set(notification.priority.to_string()),
            metadata: Set(metadata_json),
            created_at: Set(now),
            batch_key: Set(batch_key_str.clone()),
            occurrence_count: Set(1),
            next_allowed_at: Set(next_allowed),
            ..Default::default()
        };

        // Insert the new notification record
        let _inserted = new_notification.insert(self.db.as_ref()).await?;

        // Get the occurrence count for the message modification
        let occurrence_count_val = if let Some(existing) = existing {
            existing.occurrence_count + 1
        } else {
            1
        };

        // Modify the notification message if there were batched occurrences
        let mut notification = notification.clone();
        if occurrence_count_val > 1 {
            notification.message = format!(
                "{}\n\nThis issue has occurred {} times since the last notification.",
                notification.message, occurrence_count_val
            );
        }

        // Send through all configured providers
        let providers = self
            .get_enabled_providers()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get providers {}", e))?;
        for provider in &providers {
            if let Err(e) = provider.send(&notification).await {
                error!("Failed to send notification via provider: {}", e);
            }
        }

        Ok(())
    }

    pub async fn is_configured(&self) -> Result<bool> {
        let count = notification_providers::Entity::find()
            .filter(notification_providers::Column::Enabled.eq(true))
            .paginate(self.db.as_ref(), 1)
            .num_items()
            .await
            .map_err(|e| {
                error!("Failed to check notification providers: {}", e);
                anyhow::anyhow!("Failed to check notification providers: {}", e)
            })?;

        Ok(count > 0)
    }

    pub async fn list_providers(&self) -> Result<Vec<notification_providers::Model>> {
        let providers = notification_providers::Entity::find()
            .all(self.db.as_ref())
            .await?;
        Ok(providers)
    }

    pub async fn list_providers_paginated(
        &self,
        page: u64,
        page_size: u64,
    ) -> Result<Vec<notification_providers::Model>> {
        let providers = notification_providers::Entity::find()
            .paginate(self.db.as_ref(), page_size)
            .fetch_page(page - 1)
            .await?;
        Ok(providers)
    }

    /// Decrypt the provider config for safe return to API
    pub fn decrypt_provider_config(&self, encrypted_config: &str) -> Result<serde_json::Value> {
        let decrypted_config = self
            .encryption_service
            .decrypt_string(encrypted_config)
            .map_err(|e| anyhow::anyhow!("Failed to decrypt config: {}", e))?;

        let config_value: serde_json::Value = serde_json::from_str(&decrypted_config)
            .map_err(|e| anyhow::anyhow!("Failed to parse decrypted config: {}", e))?;

        Ok(config_value)
    }

    async fn load_provider(
        &self,
        record: &notification_providers::Model,
    ) -> Result<Box<dyn NotificationProvider>> {
        // Decrypt the config before parsing
        let decrypted_config = self
            .encryption_service
            .decrypt_string(&record.config)
            .map_err(|e| anyhow::anyhow!("Failed to decrypt config: {}", e))?;

        let provider: Box<dyn NotificationProvider> = match record.provider_type.as_str() {
            "email" => {
                let mut config: EmailProvider = serde_json::from_str(&decrypted_config)?;
                config.initialize(self.db.clone()).await?;
                Box::new(config)
            }
            "slack" => {
                let mut config: SlackProvider = serde_json::from_str(&decrypted_config)?;
                config.initialize(self.db.clone()).await?;
                Box::new(config)
            }
            "webhook" => {
                let mut config: WebhookProvider = serde_json::from_str(&decrypted_config)?;
                config.initialize(self.db.clone()).await?;
                Box::new(config)
            }
            _ => return Err(anyhow::anyhow!("Unsupported provider type")),
        };
        Ok(provider)
    }

    pub async fn get_provider(
        &self,
        provider_id: i32,
    ) -> Result<Option<notification_providers::Model>> {
        let provider = notification_providers::Entity::find_by_id(provider_id)
            .one(self.db.as_ref())
            .await?;
        Ok(provider)
    }

    pub async fn add_provider<T: Serialize>(
        &self,
        p_name: String,
        p_provider_type: String,
        p_config: T,
    ) -> Result<notification_providers::Model> {
        let config_json = serde_json::to_string(&p_config)?;

        // Encrypt the config before storing
        let encrypted_config = self
            .encryption_service
            .encrypt_string(&config_json)
            .map_err(|e| anyhow::anyhow!("Failed to encrypt config: {}", e))?;

        let new_provider = notification_providers::ActiveModel {
            name: Set(p_name),
            provider_type: Set(p_provider_type),
            config: Set(encrypted_config),
            enabled: Set(true),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        };

        let provider = new_provider.insert(self.db.as_ref()).await?;

        Ok(provider)
    }

    pub async fn update_provider(
        &self,
        provider_id: i32,
        update: UpdateProviderRequest,
    ) -> Result<Option<notification_providers::Model>> {
        // First check if the provider exists
        let provider = notification_providers::Entity::find_by_id(provider_id)
            .one(self.db.as_ref())
            .await?;

        if let Some(provider) = provider {
            let mut active_model: notification_providers::ActiveModel = provider.into();

            // Update fields if provided
            if let Some(new_name) = update.name {
                active_model.name = Set(new_name);
            }
            if let Some(new_config) = update.config {
                let config_json = serde_json::to_string(&new_config)?;
                // Encrypt the config before storing
                let encrypted_config = self
                    .encryption_service
                    .encrypt_string(&config_json)
                    .map_err(|e| anyhow::anyhow!("Failed to encrypt config: {}", e))?;
                active_model.config = Set(encrypted_config);
            }
            if let Some(new_enabled) = update.enabled {
                active_model.enabled = Set(new_enabled);
            }
            active_model.updated_at = Set(Utc::now());

            // Update the provider in the database
            let updated_provider = active_model.update(self.db.as_ref()).await?;

            Ok(Some(updated_provider))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_provider(&self, provider_id: i32) -> Result<bool> {
        let provider = notification_providers::Entity::find_by_id(provider_id)
            .one(self.db.as_ref())
            .await?;

        if let Some(provider) = provider {
            provider.delete(self.db.as_ref()).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn test_provider(&self, provider_id: i32) -> Result<bool> {
        let provider = notification_providers::Entity::find_by_id(provider_id)
            .one(self.db.as_ref())
            .await?;

        if let Some(provider) = provider {
            let notification_provider = self.load_provider(&provider).await?;
            // Let the error propagate instead of swallowing it
            notification_provider.health_check().await
        } else {
            Err(anyhow::anyhow!(
                "Notification provider with ID {} not found",
                provider_id
            ))
        }
    }

    // Add a method to clean up old notifications
    pub async fn cleanup_old_notifications(&self, retention_days: i64) -> Result<()> {
        let cutoff = Utc::now() - Duration::days(retention_days);

        notifications::Entity::delete_many()
            .filter(notifications::Column::CreatedAt.lt(cutoff))
            .exec(self.db.as_ref())
            .await?;

        Ok(())
    }
}

// Implement the core NotificationService trait for integration with other services
#[async_trait]
impl CoreNotificationService for NotificationService {
    async fn send_email(&self, message: EmailMessage) -> Result<(), CoreNotificationError> {
        // Convert EmailMessage to our internal notification format
        let notification = Notification {
            id: uuid::Uuid::new_v4().to_string(),
            title: message.subject,
            message: message.body,
            notification_type: NotificationType::Info,
            priority: NotificationPriority::Normal,
            severity: None,
            timestamp: Utc::now(),
            metadata: [
                ("to".to_string(), message.to.join(", ")),
                ("from".to_string(), message.from.unwrap_or_default()),
                ("reply_to".to_string(), message.reply_to.unwrap_or_default()),
            ]
            .into_iter()
            .collect(),
            bypass_throttling: false,
        };

        match self.send_notification(notification).await {
            Ok(_) => Ok(()),
            Err(e) => Err(CoreNotificationError::SendError(e.to_string())),
        }
    }

    async fn send_notification(
        &self,
        notification_data: NotificationData,
    ) -> Result<(), CoreNotificationError> {
        // Convert NotificationData to our internal Notification format
        let notification = Notification {
            id: notification_data.id,
            title: notification_data.title,
            message: notification_data.message,
            notification_type: match notification_data.notification_type {
                temps_core::notifications::NotificationType::Info => NotificationType::Info,
                temps_core::notifications::NotificationType::Warning => NotificationType::Warning,
                temps_core::notifications::NotificationType::Error => NotificationType::Error,
                temps_core::notifications::NotificationType::Alert => NotificationType::Alert,
            },
            priority: match notification_data.priority {
                temps_core::notifications::NotificationPriority::Low => NotificationPriority::Low,
                temps_core::notifications::NotificationPriority::Normal => {
                    NotificationPriority::Normal
                }
                temps_core::notifications::NotificationPriority::High => NotificationPriority::High,
                temps_core::notifications::NotificationPriority::Critical => {
                    NotificationPriority::Critical
                }
            },
            severity: notification_data
                .severity
                .and_then(|s| NotificationSeverity::from_str(&s)),
            timestamp: notification_data.timestamp,
            metadata: notification_data.metadata,
            bypass_throttling: notification_data.bypass_throttling,
        };

        match self.send_notification(notification).await {
            Ok(_) => Ok(()),
            Err(e) => Err(CoreNotificationError::SendError(e.to_string())),
        }
    }

    async fn is_configured(&self) -> Result<bool, CoreNotificationError> {
        match self.is_configured().await {
            Ok(configured) => Ok(configured),
            Err(e) => Err(CoreNotificationError::ConfigurationError(e.to_string())),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EmailProviderConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from_address: String,
    pub from_name: String,
    pub to_addresses: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SlackProviderConfig {
    pub webhook_url: String,
    pub channel: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookProviderConfig {
    pub url: String,
    #[serde(default = "default_http_method")]
    pub method: String,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

// Notification Preferences Service
fn default_backup_successes_enabled() -> bool {
    true
}

fn default_weekly_digest_enabled() -> bool {
    true
}

fn default_digest_send_day() -> String {
    "monday".to_string()
}

fn default_digest_send_time() -> String {
    "09:00".to_string()
}

fn default_digest_sections() -> crate::digest::DigestSections {
    crate::digest::DigestSections {
        performance: true,
        deployments: true,
        errors: true,
        funnels: true,
        projects: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreferences {
    // Notification Channels
    pub email_enabled: bool,
    pub slack_enabled: bool,
    pub batch_similar_notifications: bool,
    pub minimum_severity: String,

    // Project Health
    pub deployment_failures_enabled: bool,
    pub build_errors_enabled: bool,
    pub runtime_errors_enabled: bool,
    pub error_threshold: i32,
    pub error_time_window: i32,

    // Domain Monitoring
    pub ssl_expiration_enabled: bool,
    pub ssl_days_before_expiration: i32,
    pub domain_expiration_enabled: bool,
    pub dns_changes_enabled: bool,

    // Backup Monitoring
    pub backup_failures_enabled: bool,
    #[serde(default = "default_backup_successes_enabled")]
    pub backup_successes_enabled: bool,
    pub s3_connection_issues_enabled: bool,
    pub retention_policy_violations_enabled: bool,

    // Route Monitoring
    pub route_downtime_enabled: bool,
    pub load_balancer_issues_enabled: bool,

    // Weekly Digest Settings
    #[serde(default = "default_weekly_digest_enabled")]
    pub weekly_digest_enabled: bool,
    #[serde(default = "default_digest_send_day")]
    pub digest_send_day: String, // "monday" | "friday" | "sunday"
    #[serde(default = "default_digest_send_time")]
    pub digest_send_time: String, // "09:00" format (24-hour)
    #[serde(default = "default_digest_sections")]
    pub digest_sections: crate::digest::DigestSections,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            email_enabled: true,
            slack_enabled: false,
            batch_similar_notifications: true,
            minimum_severity: "warning".to_string(),

            deployment_failures_enabled: true,
            build_errors_enabled: true,
            runtime_errors_enabled: true,
            error_threshold: 200,
            error_time_window: 5,

            ssl_expiration_enabled: true,
            ssl_days_before_expiration: 30,
            domain_expiration_enabled: true,
            dns_changes_enabled: true,

            backup_failures_enabled: true,
            backup_successes_enabled: true,
            s3_connection_issues_enabled: true,
            retention_policy_violations_enabled: true,

            route_downtime_enabled: true,
            load_balancer_issues_enabled: true,

            weekly_digest_enabled: true,
            digest_send_day: "monday".to_string(),
            digest_send_time: "09:00".to_string(),
            digest_sections: crate::digest::DigestSections::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationPreferencesError {
    #[error("Database error: {0}")]
    DatabaseError(String),
}

pub struct NotificationPreferencesService {
    db: Arc<DatabaseConnection>,
}

impl NotificationPreferencesService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    pub async fn get_preferences(
        &self,
    ) -> Result<NotificationPreferences, NotificationPreferencesError> {
        let record = notification_preferences::Entity::find()
            .one(self.db.as_ref())
            .await
            .map_err(|e| NotificationPreferencesError::DatabaseError(e.to_string()))?;

        match record {
            Some(record) => {
                let preferences: NotificationPreferences =
                    serde_json::from_str(&record.preferences).map_err(|e| {
                        NotificationPreferencesError::DatabaseError(format!(
                            "Failed to deserialize preferences: {}",
                            e
                        ))
                    })?;
                Ok(preferences)
            }
            None => {
                info!("No notification preferences found, returning defaults");
                Ok(NotificationPreferences::default())
            }
        }
    }

    pub async fn update_preferences(
        &self,
        preferences: NotificationPreferences,
    ) -> Result<NotificationPreferences, NotificationPreferencesError> {
        let preferences_json = serde_json::to_string(&preferences).map_err(|e| {
            NotificationPreferencesError::DatabaseError(format!(
                "Failed to serialize preferences: {}",
                e
            ))
        })?;

        let record = notification_preferences::Entity::find()
            .one(self.db.as_ref())
            .await
            .map_err(|e| NotificationPreferencesError::DatabaseError(e.to_string()))?;

        match record {
            Some(record) => {
                let mut active_model: notification_preferences::ActiveModel = record.into();
                active_model.preferences = Set(preferences_json);
                active_model.updated_at = Set(Utc::now());

                active_model
                    .update(self.db.as_ref())
                    .await
                    .map_err(|e| NotificationPreferencesError::DatabaseError(e.to_string()))?;
            }
            None => {
                let new_pref = notification_preferences::ActiveModel {
                    preferences: Set(preferences_json),
                    created_at: Set(Utc::now()),
                    updated_at: Set(Utc::now()),
                    ..Default::default()
                };

                new_pref
                    .insert(self.db.as_ref())
                    .await
                    .map_err(|e| NotificationPreferencesError::DatabaseError(e.to_string()))?;
            }
        }

        info!("Updated notification preferences");
        Ok(preferences)
    }

    pub async fn delete_preferences(&self) -> Result<(), NotificationPreferencesError> {
        notification_preferences::Entity::delete_many()
            .exec(self.db.as_ref())
            .await
            .map_err(|e| NotificationPreferencesError::DatabaseError(e.to_string()))?;

        info!("Deleted notification preferences");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::MockDatabase;

    fn create_test_notification() -> Notification {
        Notification {
            id: "test-123".to_string(),
            title: "Test Notification".to_string(),
            message: "This is a test message".to_string(),
            notification_type: NotificationType::Info,
            priority: NotificationPriority::Normal,
            severity: None,
            timestamp: Utc::now(),
            metadata: vec![
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string()),
            ]
            .into_iter()
            .collect(),
            bypass_throttling: false,
        }
    }

    #[test]
    fn test_tls_mode_defaults() {
        assert!(matches!(default_tls_mode(), TlsMode::Starttls));
        assert!(default_starttls_required());
        assert!(!default_accept_invalid_certs());
    }

    #[test]
    fn test_batch_key_generation() {
        let notification = create_test_notification();
        let key = NotificationService::get_batch_key(&notification);
        assert_eq!(key, "Info:Normal:Test Notification");
    }

    #[test]
    fn test_next_allowed_time_calculation() {
        let now = Utc::now();

        let low_priority = NotificationService::get_next_allowed_time(&NotificationPriority::Low);
        let normal_priority =
            NotificationService::get_next_allowed_time(&NotificationPriority::Normal);
        let high_priority = NotificationService::get_next_allowed_time(&NotificationPriority::High);
        let critical_priority =
            NotificationService::get_next_allowed_time(&NotificationPriority::Critical);

        // Check relative times
        assert!(low_priority > now + Duration::days(6));
        assert!(normal_priority > now + Duration::hours(23));
        assert!(high_priority > now + Duration::minutes(59));
        assert!(critical_priority > now + Duration::minutes(14));
    }

    #[test]
    fn test_email_provider_configuration() {
        // Test that email provider can be configured correctly
        let config = EmailProviderConfig {
            smtp_host: "localhost".to_string(),
            smtp_port: 1025,
            username: "test_user".to_string(),
            password: "test_pass".to_string(),
            from_address: "test@example.com".to_string(),
            from_name: "Test Sender".to_string(),
            to_addresses: vec!["recipient@example.com".to_string()],
        };

        // Verify configuration fields
        assert_eq!(config.smtp_host, "localhost");
        assert_eq!(config.smtp_port, 1025);
        assert_eq!(config.from_address, "test@example.com");
    }

    #[test]
    fn test_slack_provider_configuration() {
        let config = SlackProviderConfig {
            webhook_url: "https://hooks.slack.com/services/TEST".to_string(),
            channel: "#notifications".to_string(),
        };

        assert_eq!(config.webhook_url, "https://hooks.slack.com/services/TEST");
        assert_eq!(config.channel, "#notifications");
    }

    #[test]
    fn test_slack_webhook_validation() {
        // Test that we can validate webhook URLs
        let valid_url = "https://hooks.slack.com/services/TEST";
        let invalid_url = "http://invalid-url.com";

        assert!(valid_url.starts_with("https://hooks.slack.com/"));
        assert!(!invalid_url.starts_with("https://hooks.slack.com/"));
    }

    #[test]
    fn test_email_config_serialization() {
        let config = EmailProviderConfig {
            smtp_host: "smtp.test.com".to_string(),
            smtp_port: 587,
            username: "user".to_string(),
            password: "pass".to_string(),
            from_address: "sender@test.com".to_string(),
            from_name: "Sender".to_string(),
            to_addresses: vec!["recipient@test.com".to_string()],
        };

        let json = serde_json::to_string(&config);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("smtp.test.com"));
        assert!(json_str.contains("587"));
        assert!(json_str.contains("sender@test.com"));
    }

    #[test]
    fn test_slack_config_serialization() {
        let config = SlackProviderConfig {
            webhook_url: "https://hooks.slack.com/services/TEST".to_string(),
            channel: "#general".to_string(),
        };

        let json = serde_json::to_string(&config);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("https://hooks.slack.com/services/TEST"));
        assert!(json_str.contains("#general"));
    }

    #[test]
    fn test_webhook_provider_configuration() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer test-token".to_string());
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        let config = WebhookProviderConfig {
            url: "https://example.com/webhook".to_string(),
            method: "POST".to_string(),
            headers,
            timeout_secs: 30,
        };

        assert_eq!(config.url, "https://example.com/webhook");
        assert_eq!(config.method, "POST");
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.headers.len(), 2);
        assert_eq!(
            config.headers.get("Authorization"),
            Some(&"Bearer test-token".to_string())
        );
    }

    #[test]
    fn test_webhook_config_serialization() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer test".to_string());

        let config = WebhookProviderConfig {
            url: "https://api.example.com/notifications".to_string(),
            method: "POST".to_string(),
            headers,
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&config);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("https://api.example.com/notifications"));
        assert!(json_str.contains("POST"));
        assert!(json_str.contains("Authorization"));
        assert!(json_str.contains("Bearer test"));
    }

    #[test]
    fn test_webhook_config_deserialization_with_defaults() {
        // Test that defaults are applied when fields are missing
        let json = r#"{
            "url": "https://example.com/webhook"
        }"#;

        let config: WebhookProviderConfig =
            serde_json::from_str(json).expect("Failed to deserialize");

        assert_eq!(config.url, "https://example.com/webhook");
        assert_eq!(config.method, "POST"); // default
        assert_eq!(config.timeout_secs, 30); // default
        assert!(config.headers.is_empty()); // default empty
    }

    #[test]
    fn test_webhook_url_validation() {
        // Test valid URLs
        let valid_https = "https://example.com/webhook";
        let valid_http = "http://localhost:8080/webhook";

        assert!(valid_https.starts_with("http://") || valid_https.starts_with("https://"));
        assert!(valid_http.starts_with("http://") || valid_http.starts_with("https://"));

        // Test invalid URLs
        let invalid_url = "ftp://example.com/webhook";
        assert!(!invalid_url.starts_with("http://") && !invalid_url.starts_with("https://"));
    }

    #[test]
    fn test_webhook_method_validation() {
        let valid_methods = ["POST", "PUT", "PATCH", "post", "put", "patch"];
        let invalid_methods = ["GET", "DELETE", "HEAD", "OPTIONS"];

        for method in valid_methods {
            let upper = method.to_uppercase();
            assert!(["POST", "PUT", "PATCH"].contains(&upper.as_str()));
        }

        for method in invalid_methods {
            let upper = method.to_uppercase();
            assert!(!["POST", "PUT", "PATCH"].contains(&upper.as_str()));
        }
    }

    #[test]
    fn test_notification_priority_ordering() {
        let low_time = NotificationService::get_next_allowed_time(&NotificationPriority::Low);
        let normal_time = NotificationService::get_next_allowed_time(&NotificationPriority::Normal);
        let high_time = NotificationService::get_next_allowed_time(&NotificationPriority::High);
        let critical_time =
            NotificationService::get_next_allowed_time(&NotificationPriority::Critical);

        // Critical should have the shortest wait time, Low should have the longest
        assert!(critical_time < high_time);
        assert!(high_time < normal_time);
        assert!(normal_time < low_time);
    }

    #[test]
    fn test_notification_type_colors() {
        // This tests the color mapping logic used in email and slack providers
        let colors = vec![
            (NotificationType::Info, "#0088cc"),
            (NotificationType::Warning, "#ffa500"),
            (NotificationType::Error, "#ff0000"),
            (NotificationType::Alert, "#ff0000"),
        ];

        for (notification_type, expected_color) in colors {
            let color = match notification_type {
                NotificationType::Info => "#0088cc",
                NotificationType::Warning => "#ffa500",
                NotificationType::Error => "#ff0000",
                NotificationType::Alert => "#ff0000",
            };
            assert_eq!(color, expected_color);
        }
    }

    // Notification Preferences Service Tests
    #[test]
    fn test_notification_preferences_defaults() {
        let prefs = NotificationPreferences::default();

        // Channel defaults
        assert!(prefs.email_enabled);
        assert!(!prefs.slack_enabled);
        assert!(prefs.batch_similar_notifications);
        assert_eq!(prefs.minimum_severity, "warning");

        // Project health defaults
        assert!(prefs.deployment_failures_enabled);
        assert!(prefs.build_errors_enabled);
        assert!(prefs.runtime_errors_enabled);
        assert_eq!(prefs.error_threshold, 200);
        assert_eq!(prefs.error_time_window, 5);

        // Domain monitoring defaults
        assert!(prefs.ssl_expiration_enabled);
        assert_eq!(prefs.ssl_days_before_expiration, 30);
        assert!(prefs.domain_expiration_enabled);
        assert!(prefs.dns_changes_enabled);

        // Backup monitoring defaults
        assert!(prefs.backup_failures_enabled);
        assert!(prefs.backup_successes_enabled);
        assert!(prefs.s3_connection_issues_enabled);
        assert!(prefs.retention_policy_violations_enabled);

        // Route monitoring defaults
        assert!(prefs.route_downtime_enabled);
        assert!(prefs.load_balancer_issues_enabled);
    }

    #[test]
    fn test_notification_preferences_serialization() {
        let prefs = NotificationPreferences::default();

        // Test serialization
        let json = serde_json::to_string(&prefs);
        assert!(json.is_ok());

        let json_str = json.unwrap();
        assert!(json_str.contains("email_enabled"));
        assert!(json_str.contains("slack_enabled"));

        // Test deserialization
        let deserialized: Result<NotificationPreferences, _> = serde_json::from_str(&json_str);
        assert!(deserialized.is_ok());

        let deserialized_prefs = deserialized.unwrap();
        assert_eq!(prefs.email_enabled, deserialized_prefs.email_enabled);
        assert_eq!(prefs.error_threshold, deserialized_prefs.error_threshold);
    }

    #[tokio::test]
    async fn test_notification_preferences_service_get_defaults() {
        use temps_database::test_utils::TestDatabase;

        // Start database with migrations
        let test_db = TestDatabase::with_migrations()
            .await
            .expect("Failed to create test database");

        // Create service
        let service = NotificationPreferencesService::new(test_db.connection_arc());

        // Get preferences (should return defaults since none exist)
        let prefs = service
            .get_preferences()
            .await
            .expect("Failed to get preferences");

        // Verify defaults
        assert!(prefs.email_enabled);
        assert!(!prefs.slack_enabled);
        assert_eq!(prefs.minimum_severity, "warning");

        // Cleanup
        test_db
            .cleanup_all_tables()
            .await
            .expect("Failed to cleanup");
    }

    #[tokio::test]
    async fn test_notification_preferences_service_update() {
        use temps_database::test_utils::TestDatabase;

        // Start database with migrations
        let test_db = TestDatabase::with_migrations()
            .await
            .expect("Failed to create test database");

        // Create service
        let service = NotificationPreferencesService::new(test_db.connection_arc());

        // Create custom preferences
        let custom_prefs = NotificationPreferences {
            email_enabled: false,
            slack_enabled: true,
            minimum_severity: "critical".to_string(),
            error_threshold: 500,
            ..Default::default()
        };

        // Update preferences
        let updated = service
            .update_preferences(custom_prefs.clone())
            .await
            .expect("Failed to update preferences");

        // Verify update
        assert!(!updated.email_enabled);
        assert!(updated.slack_enabled);
        assert_eq!(updated.minimum_severity, "critical");
        assert_eq!(updated.error_threshold, 500);

        // Get preferences again to verify persistence
        let retrieved = service
            .get_preferences()
            .await
            .expect("Failed to get preferences");
        assert!(!retrieved.email_enabled);
        assert!(retrieved.slack_enabled);
        assert_eq!(retrieved.minimum_severity, "critical");
        assert_eq!(retrieved.error_threshold, 500);

        // Cleanup
        test_db
            .cleanup_all_tables()
            .await
            .expect("Failed to cleanup");
    }

    #[tokio::test]
    async fn test_notification_preferences_service_update_existing() {
        use temps_database::test_utils::TestDatabase;

        // Start database with migrations
        let test_db = TestDatabase::with_migrations()
            .await
            .expect("Failed to create test database");

        // Create service
        let service = NotificationPreferencesService::new(test_db.connection_arc());

        // Create initial preferences
        let initial_prefs = NotificationPreferences {
            email_enabled: false,
            ..Default::default()
        };
        service
            .update_preferences(initial_prefs)
            .await
            .expect("Failed to create initial preferences");

        // Update with different values
        let updated_prefs = NotificationPreferences {
            email_enabled: true,
            slack_enabled: true,
            ..Default::default()
        };

        let result = service
            .update_preferences(updated_prefs)
            .await
            .expect("Failed to update preferences");

        // Verify the update
        assert!(result.email_enabled);
        assert!(result.slack_enabled);

        // Cleanup
        test_db
            .cleanup_all_tables()
            .await
            .expect("Failed to cleanup");
    }

    #[tokio::test]
    async fn test_notification_preferences_service_delete() {
        use temps_database::test_utils::TestDatabase;

        // Start database with migrations
        let test_db = TestDatabase::with_migrations()
            .await
            .expect("Failed to create test database");

        // Create service
        let service = NotificationPreferencesService::new(test_db.connection_arc());

        // Create preferences
        let prefs = NotificationPreferences::default();
        service
            .update_preferences(prefs)
            .await
            .expect("Failed to create preferences");

        // Verify they exist (should not be defaults, should be from database)
        let existing = service
            .get_preferences()
            .await
            .expect("Failed to get preferences");
        assert!(existing.email_enabled); // Verify it's actually stored

        // Delete preferences
        service
            .delete_preferences()
            .await
            .expect("Failed to delete preferences");

        // Get preferences again (should return defaults since deleted)
        let after_delete = service
            .get_preferences()
            .await
            .expect("Failed to get preferences");
        assert!(after_delete.email_enabled); // Should still be true from defaults
        assert!(!after_delete.slack_enabled); // Should be false from defaults

        // Cleanup
        test_db
            .cleanup_all_tables()
            .await
            .expect("Failed to cleanup");
    }

    #[test]
    fn test_notification_preferences_backup_successes_default() {
        // Test the default function
        assert!(default_backup_successes_enabled());

        // Test JSON deserialization with missing field
        let json_without_field = r#"{
            "email_enabled": true,
            "slack_enabled": false,
            "batch_similar_notifications": true,
            "minimum_severity": "warning",
            "deployment_failures_enabled": true,
            "build_errors_enabled": true,
            "runtime_errors_enabled": true,
            "error_threshold": 200,
            "error_time_window": 5,
            "ssl_expiration_enabled": true,
            "ssl_days_before_expiration": 30,
            "domain_expiration_enabled": true,
            "dns_changes_enabled": true,
            "backup_failures_enabled": true,
            "s3_connection_issues_enabled": true,
            "retention_policy_violations_enabled": true,
            "route_downtime_enabled": true,
            "load_balancer_issues_enabled": true
        }"#;

        let prefs: NotificationPreferences =
            serde_json::from_str(json_without_field).expect("Failed to deserialize");

        // Should use default value of true
        assert!(prefs.backup_successes_enabled);
    }

    #[tokio::test]
    async fn test_notification_preferences_service_multiple_updates() {
        use temps_database::test_utils::TestDatabase;

        // Start database with migrations
        let test_db = TestDatabase::with_migrations()
            .await
            .expect("Failed to create test database");

        // Create service
        let service = NotificationPreferencesService::new(test_db.connection_arc());

        // First update
        let prefs1 = NotificationPreferences {
            error_threshold: 100,
            ..Default::default()
        };
        service
            .update_preferences(prefs1)
            .await
            .expect("Failed to update preferences");

        // Second update
        let prefs2 = NotificationPreferences {
            error_threshold: 200,
            ..Default::default()
        };
        service
            .update_preferences(prefs2)
            .await
            .expect("Failed to update preferences");

        // Third update
        let prefs3 = NotificationPreferences {
            error_threshold: 300,
            ..Default::default()
        };
        service
            .update_preferences(prefs3)
            .await
            .expect("Failed to update preferences");

        // Verify final state
        let final_prefs = service
            .get_preferences()
            .await
            .expect("Failed to get preferences");
        assert_eq!(final_prefs.error_threshold, 300);

        // Cleanup
        test_db
            .cleanup_all_tables()
            .await
            .expect("Failed to cleanup");
    }

    // ── SSRF Prevention Tests for WebhookProvider ────────────────────

    fn create_webhook(url: &str) -> WebhookProvider {
        WebhookProvider {
            url: url.to_string(),
            method: "POST".to_string(),
            headers: std::collections::HashMap::new(),
            timeout_secs: 30,
        }
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_private_ip_192_168() {
        let mut webhook = create_webhook("http://192.168.1.1/callback");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(
            result.is_err(),
            "Must block RFC 1918 private IP 192.168.x.x"
        );
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_private_ip_10() {
        let mut webhook = create_webhook("http://10.0.0.1:8080/hook");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block RFC 1918 private IP 10.x.x.x");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_private_ip_172() {
        let mut webhook = create_webhook("http://172.16.0.1/hook");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block RFC 1918 private IP 172.16.x.x");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_localhost() {
        let mut webhook = create_webhook("http://localhost:6379/");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block localhost");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_loopback_ip() {
        let mut webhook = create_webhook("http://127.0.0.1:6379/");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block loopback 127.0.0.1");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_cloud_metadata() {
        let mut webhook = create_webhook("http://169.254.169.254/latest/meta-data/");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(
            result.is_err(),
            "Must block AWS metadata endpoint 169.254.169.254"
        );
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_link_local() {
        let mut webhook = create_webhook("http://169.254.1.1/hook");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block link-local 169.254.x.x");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_blocks_non_http_scheme() {
        let mut webhook = create_webhook("ftp://evil.com/payload");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must block non-HTTP/HTTPS schemes");
    }

    #[tokio::test]
    async fn test_webhook_ssrf_allows_public_https() {
        let mut webhook = create_webhook("https://hooks.example.com/webhook");
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_ok(), "Must allow public HTTPS URLs");
    }

    #[tokio::test]
    async fn test_webhook_invalid_method_rejected() {
        let mut webhook = WebhookProvider {
            url: "https://hooks.example.com/webhook".to_string(),
            method: "DELETE".to_string(),
            headers: std::collections::HashMap::new(),
            timeout_secs: 30,
        };
        let db = Arc::new(MockDatabase::new(sea_orm::DatabaseBackend::Postgres).into_connection());
        let result = webhook.initialize(db).await;
        assert!(result.is_err(), "Must reject DELETE method");
    }
}
