//! Domain management commands via HTTP API
//!
//! Provides CLI commands for managing domains and TLS certificates through the
//! Temps HTTP API. Supports creating domains with ACME challenges (HTTP-01 or DNS-01),
//! managing certificate orders, and importing custom certificates.

use anyhow::Context;
use chrono::Utc;
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use temps_core::EncryptionService;
use temps_database::establish_connection;
use temps_entities::domains;
use x509_parser::prelude::*;

/// Domain and certificate management commands
#[derive(Args)]
pub struct DomainCommand {
    #[command(subcommand)]
    pub command: DomainSubcommand,
}

#[derive(Subcommand)]
pub enum DomainSubcommand {
    /// Create a new domain and request a TLS certificate via Let's Encrypt
    Add(AddDomainCommand),
    /// List all domains and their certificate status
    #[command(alias = "ls")]
    List(ListDomainsApiCommand),
    /// Show details for a specific domain
    Show(ShowDomainCommand),
    /// Delete a domain
    #[command(alias = "rm")]
    Delete(DeleteDomainCommand),
    /// Import a custom certificate for a domain (direct database access)
    Import(ImportCertificateCommand),
    /// Provision a certificate via HTTP-01 challenge
    Provision(ProvisionDomainCommand),
    /// Manage ACME certificate orders
    Order(OrderCommand),
}

/// Challenge type for Let's Encrypt validation
#[derive(Clone, ValueEnum, Debug)]
pub enum ChallengeType {
    /// HTTP-01 challenge (requires port 80 accessible)
    #[value(name = "http-01")]
    Http01,
    /// DNS-01 challenge (required for wildcard domains)
    #[value(name = "dns-01")]
    Dns01,
}

impl std::fmt::Display for ChallengeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChallengeType::Http01 => write!(f, "http-01"),
            ChallengeType::Dns01 => write!(f, "dns-01"),
        }
    }
}

// ========================================
// API-based commands
// ========================================

/// Create a new domain and request a TLS certificate
#[derive(Args)]
pub struct AddDomainCommand {
    /// Domain name (e.g., "example.com" or "*.example.com")
    #[arg(long, short = 'd')]
    pub domain: String,

    /// Challenge type for Let's Encrypt validation
    #[arg(long, short = 'c', value_enum)]
    pub challenge: ChallengeType,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,
}

/// List all domains via API
#[derive(Args)]
pub struct ListDomainsApiCommand {
    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Output as JSON
    #[arg(long, default_value = "false")]
    pub json: bool,
}

/// Show details for a specific domain
#[derive(Args)]
pub struct ShowDomainCommand {
    /// Domain ID
    #[arg(long)]
    pub id: i32,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Output as JSON
    #[arg(long, default_value = "false")]
    pub json: bool,
}

/// Delete a domain
#[derive(Args)]
pub struct DeleteDomainCommand {
    /// Domain name to delete
    #[arg(long, short = 'd')]
    pub domain: String,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Skip confirmation
    #[arg(long, short = 'y', default_value = "false")]
    pub yes: bool,
}

/// Provision a certificate via HTTP-01 challenge
#[derive(Args)]
pub struct ProvisionDomainCommand {
    /// Domain name to provision
    #[arg(long, short = 'd')]
    pub domain: String,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,
}

/// ACME certificate order management
#[derive(Args)]
pub struct OrderCommand {
    #[command(subcommand)]
    pub command: OrderSubcommand,
}

#[derive(Subcommand)]
pub enum OrderSubcommand {
    /// Create (or recreate) an ACME order for a domain
    Create(OrderCreateCommand),
    /// Show ACME order details (includes live challenge validation status)
    Show(OrderShowCommand),
    /// Cancel an ACME order
    Cancel(OrderCancelCommand),
    /// Finalize an ACME order (complete challenge and obtain certificate)
    Finalize(OrderFinalizeCommand),
    /// List all ACME orders
    #[command(alias = "ls")]
    List(OrderListCommand),
}

/// Create a new ACME order
#[derive(Args)]
pub struct OrderCreateCommand {
    /// Domain ID to create order for
    #[arg(long)]
    pub domain_id: i32,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,
}

/// Show ACME order details
#[derive(Args)]
pub struct OrderShowCommand {
    /// Domain ID to show order for
    #[arg(long)]
    pub domain_id: i32,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Output as JSON
    #[arg(long, default_value = "false")]
    pub json: bool,
}

/// Cancel an ACME order
#[derive(Args)]
pub struct OrderCancelCommand {
    /// Domain ID to cancel order for
    #[arg(long)]
    pub domain_id: i32,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Skip confirmation
    #[arg(long, short = 'y', default_value = "false")]
    pub yes: bool,
}

/// Finalize an ACME order
#[derive(Args)]
pub struct OrderFinalizeCommand {
    /// Domain ID to finalize order for
    #[arg(long)]
    pub domain_id: i32,

    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,
}

/// List all ACME orders
#[derive(Args)]
pub struct OrderListCommand {
    /// Temps API URL
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,

    /// Temps API token
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,

    /// Output as JSON
    #[arg(long, default_value = "false")]
    pub json: bool,
}

// ========================================
// Import command (direct database access)
// ========================================

/// Import a custom certificate for a domain
#[derive(Args)]
pub struct ImportCertificateCommand {
    /// Domain name (e.g., "*.localho.st" or "app.example.com")
    #[arg(long, short = 'd')]
    pub domain: String,

    /// Path to the certificate file (PEM format)
    #[arg(long, short = 'c')]
    pub certificate: PathBuf,

    /// Path to the private key file (PEM format)
    #[arg(long, short = 'k')]
    pub private_key: PathBuf,

    /// Database URL
    #[arg(long, env = "TEMPS_DATABASE_URL")]
    pub database_url: String,

    /// Data directory containing the encryption key
    #[arg(long, env = "TEMPS_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Overwrite existing certificate for this domain
    #[arg(long, default_value = "false")]
    pub force: bool,
}

// ========================================
// API response types
// ========================================

#[derive(Debug, Deserialize, Serialize)]
struct DomainResponse {
    id: i32,
    domain: String,
    status: String,
    expiration_time: Option<i64>,
    last_renewed: Option<i64>,
    dns_challenge_token: Option<String>,
    dns_challenge_value: Option<String>,
    last_error: Option<String>,
    last_error_type: Option<String>,
    is_wildcard: bool,
    verification_method: String,
    created_at: i64,
    updated_at: i64,
    certificate: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DomainChallengeResponse {
    domain: String,
    txt_records: Vec<TxtRecord>,
    status: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct TxtRecord {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
enum ProvisionApiResponse {
    #[serde(rename = "error")]
    Error(DomainErrorResponse),
    #[serde(rename = "complete")]
    Complete(DomainResponse),
    #[serde(rename = "pending")]
    Pending(DomainChallengeResponse),
}

#[derive(Debug, Deserialize, Serialize)]
struct DomainErrorResponse {
    message: String,
    code: String,
    details: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ListDomainsResponse {
    domains: Vec<DomainResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AcmeOrderResponse {
    id: i32,
    order_url: String,
    domain_id: i32,
    email: String,
    status: String,
    identifiers: serde_json::Value,
    authorizations: Option<serde_json::Value>,
    finalize_url: Option<String>,
    certificate_url: Option<String>,
    error: Option<String>,
    error_type: Option<String>,
    created_at: i64,
    updated_at: i64,
    expires_at: Option<i64>,
    challenge_validation: Option<ChallengeValidationStatus>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChallengeValidationStatus {
    #[serde(rename = "type")]
    challenge_type: String,
    url: String,
    status: String,
    validated: Option<String>,
    error: Option<ChallengeError>,
    token: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChallengeError {
    #[serde(rename = "type")]
    error_type: String,
    detail: String,
    status: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct ListOrdersResponse {
    orders: Vec<AcmeOrderResponse>,
}

#[derive(Debug, Serialize)]
struct CreateDomainRequest {
    domain: String,
    challenge_type: String,
}

// ========================================
// Command execution
// ========================================

impl DomainCommand {
    pub fn execute(self) -> anyhow::Result<()> {
        let rt = tokio::runtime::Runtime::new()?;

        rt.block_on(async {
            match self.command {
                DomainSubcommand::Add(cmd) => execute_add(cmd).await,
                DomainSubcommand::List(cmd) => execute_list_api(cmd).await,
                DomainSubcommand::Show(cmd) => execute_show(cmd).await,
                DomainSubcommand::Delete(cmd) => execute_delete(cmd).await,
                DomainSubcommand::Import(cmd) => execute_import(cmd).await,
                DomainSubcommand::Provision(cmd) => execute_provision(cmd).await,
                DomainSubcommand::Order(cmd) => match cmd.command {
                    OrderSubcommand::Create(c) => execute_order_create(c).await,
                    OrderSubcommand::Show(c) => execute_order_show(c).await,
                    OrderSubcommand::Cancel(c) => execute_order_cancel(c).await,
                    OrderSubcommand::Finalize(c) => execute_order_finalize(c).await,
                    OrderSubcommand::List(c) => execute_order_list(c).await,
                },
            }
        })
    }
}

// ========================================
// Helper: build API URL
// ========================================

fn api_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

// ========================================
// Helper: handle API error responses
// ========================================

async fn handle_api_error(response: reqwest::Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow::anyhow!("API request failed (HTTP {}): {}", status, body)
}

// ========================================
// Helper: format millis timestamp
// ========================================

fn format_millis_timestamp(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_millis_date(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "N/A".to_string())
}

// ========================================
// Add domain
// ========================================

async fn execute_add(cmd: AddDomainCommand) -> anyhow::Result<()> {
    let is_wildcard = cmd.domain.starts_with("*.");

    // Enforce dns-01 for wildcard domains
    if is_wildcard {
        if let ChallengeType::Http01 = cmd.challenge {
            return Err(anyhow::anyhow!(
                "Wildcard domains (*.example.com) require DNS-01 challenge. Use --challenge dns-01"
            ));
        }
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "   Creating Domain & Requesting Certificate"
            .bright_white()
            .bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!();
    println!(
        "  {} {}",
        "Domain:".bright_white(),
        cmd.domain.bright_cyan()
    );
    println!(
        "  {} {}",
        "Challenge:".bright_white(),
        cmd.challenge.to_string().bright_cyan()
    );
    println!(
        "  {} {}",
        "Type:".bright_white(),
        if is_wildcard { "Wildcard" } else { "Single" }.bright_cyan()
    );
    println!();

    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, "/domains");

    let request = CreateDomainRequest {
        domain: cmd.domain.clone(),
        challenge_type: cmd.challenge.to_string(),
    };

    println!("{} Requesting certificate...", "→".bright_blue());

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let domain_resp: DomainResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    println!(
        "  {} Domain created (ID: {})",
        "✓".bright_green(),
        domain_resp.id
    );

    // Show domain details
    print_domain_details(&domain_resp);

    // Show challenge instructions based on challenge type
    print_challenge_instructions(&cmd.challenge, &domain_resp);

    println!();
    Ok(())
}

// ========================================
// Print challenge instructions (like UI)
// ========================================

fn print_challenge_instructions(challenge_type: &ChallengeType, domain: &DomainResponse) {
    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_yellow()
    );

    match challenge_type {
        ChallengeType::Dns01 => {
            println!(
                "{}",
                "   DNS-01 Challenge Instructions".bright_yellow().bold()
            );
            println!(
                "{}",
                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_yellow()
            );
            println!();
            println!("  Add the following DNS TXT record to verify domain ownership:");
            println!();

            if let (Some(token), Some(value)) =
                (&domain.dns_challenge_token, &domain.dns_challenge_value)
            {
                println!(
                    "  {} {}",
                    "Record Name:".bright_white().bold(),
                    format!("_acme-challenge.{}", domain.domain.trim_start_matches("*."))
                        .bright_cyan()
                );
                println!(
                    "  {} {}",
                    "Record Type:".bright_white().bold(),
                    "TXT".bright_cyan()
                );
                println!(
                    "  {} {}",
                    "Record Value:".bright_white().bold(),
                    value.bright_cyan()
                );
                let _ = token; // token is stored but value is what goes in DNS
            } else {
                println!("  {} Challenge data not yet available.", "ℹ".bright_blue());
                println!(
                    "  {} Use 'temps domain order show --domain-id {}' to check challenge details.",
                    "→".bright_blue(),
                    domain.id
                );
            }

            println!();
            println!(
                "  {} After adding the DNS record:",
                "Next steps:".bright_white().bold()
            );
            println!("    1. Wait for DNS propagation (usually 1-5 minutes)");
            println!(
                "    2. Verify: {}",
                format!(
                    "dig TXT _acme-challenge.{}",
                    domain.domain.trim_start_matches("*.")
                )
                .bright_white()
            );
            println!(
                "    3. Finalize: {}",
                format!("temps domain order finalize --domain-id {}", domain.id).bright_white()
            );
        }
        ChallengeType::Http01 => {
            println!(
                "{}",
                "   HTTP-01 Challenge Instructions".bright_yellow().bold()
            );
            println!(
                "{}",
                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_yellow()
            );
            println!();
            println!("  The HTTP-01 challenge requires port 80 to be publicly accessible.");
            println!("  Temps will automatically serve the challenge token.");
            println!();
            println!(
                "  {} Ensure your domain {} points to this server.",
                "→".bright_blue(),
                domain.domain.bright_cyan()
            );
            println!();
            println!("  {} Next steps:", "Next steps:".bright_white().bold());
            println!(
                "    1. Verify DNS: {}",
                format!("dig A {}", domain.domain).bright_white()
            );
            println!(
                "    2. Provision: {}",
                format!("temps domain provision -d {}", domain.domain).bright_white()
            );
            println!(
                "    {} Or finalize via order: {}",
                "→".bright_blue(),
                format!("temps domain order finalize --domain-id {}", domain.id).bright_white()
            );
        }
    }
}

// ========================================
// Print domain details
// ========================================

fn print_domain_details(domain: &DomainResponse) {
    println!();
    println!(
        "  {} {}",
        "Domain:".bright_white(),
        domain.domain.bright_cyan()
    );
    println!(
        "  {} {}",
        "ID:".bright_white(),
        domain.id.to_string().bright_cyan()
    );

    let status_colored = match domain.status.as_str() {
        "active" => domain.status.bright_green(),
        "pending" | "pending_dns" | "pending_validation" | "pending_http" => {
            domain.status.bright_yellow()
        }
        "failed" | "expired" => domain.status.bright_red(),
        _ => domain.status.normal(),
    };
    println!("  {} {}", "Status:".bright_white(), status_colored);
    println!(
        "  {} {}",
        "Type:".bright_white(),
        if domain.is_wildcard {
            "Wildcard"
        } else {
            "Single"
        }
        .bright_cyan()
    );
    println!(
        "  {} {}",
        "Verification:".bright_white(),
        domain.verification_method.bright_cyan()
    );

    if let Some(exp) = domain.expiration_time {
        println!(
            "  {} {}",
            "Expires:".bright_white(),
            format_millis_timestamp(exp).bright_cyan()
        );
    }

    if let Some(ref err) = domain.last_error {
        println!("  {} {}", "Last Error:".bright_white(), err.bright_red());
    }
}

// ========================================
// List domains (API)
// ========================================

async fn execute_list_api(cmd: ListDomainsApiCommand) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, "/domains");

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let list_resp: ListDomainsResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list_resp.domains)
                .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?
        );
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "                      Domain Certificates"
            .bright_white()
            .bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!();

    if list_resp.domains.is_empty() {
        println!("  {} No domains configured.", "ℹ".bright_blue());
        println!();
        return Ok(());
    }

    println!(
        "  {:<5} {:<40} {:<18} {:<12} {:<12}",
        "ID".bright_white().bold(),
        "DOMAIN".bright_white().bold(),
        "STATUS".bright_white().bold(),
        "TYPE".bright_white().bold(),
        "EXPIRES".bright_white().bold()
    );
    println!("  {}", "─".repeat(90));

    for domain in &list_resp.domains {
        let status_colored = match domain.status.as_str() {
            "active" => domain.status.bright_green(),
            "pending" | "pending_dns" | "pending_validation" | "pending_http" => {
                domain.status.bright_yellow()
            }
            "failed" | "expired" => domain.status.bright_red(),
            _ => domain.status.normal(),
        };

        let domain_type = if domain.is_wildcard {
            "wildcard"
        } else {
            "single"
        };

        let expiration = domain
            .expiration_time
            .map(format_millis_date)
            .unwrap_or_else(|| "N/A".to_string());

        println!(
            "  {:<5} {:<40} {:<18} {:<12} {:<12}",
            domain.id.to_string().bright_white(),
            domain.domain.bright_cyan(),
            status_colored,
            domain_type,
            expiration
        );
    }

    println!();
    Ok(())
}

// ========================================
// Show domain
// ========================================

async fn execute_show(cmd: ShowDomainCommand) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, &format!("/domains/{}", cmd.id));

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let domain_resp: DomainResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&domain_resp)
                .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?
        );
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "                      Domain Details".bright_white().bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );

    print_domain_details(&domain_resp);
    println!(
        "  {} {}",
        "Created:".bright_white(),
        format_millis_timestamp(domain_resp.created_at).bright_cyan()
    );
    println!(
        "  {} {}",
        "Updated:".bright_white(),
        format_millis_timestamp(domain_resp.updated_at).bright_cyan()
    );
    if let Some(renewed) = domain_resp.last_renewed {
        println!(
            "  {} {}",
            "Last Renewed:".bright_white(),
            format_millis_timestamp(renewed).bright_cyan()
        );
    }

    println!();
    Ok(())
}

// ========================================
// Delete domain
// ========================================

async fn execute_delete(cmd: DeleteDomainCommand) -> anyhow::Result<()> {
    if !cmd.yes {
        println!(
            "{} Are you sure you want to delete domain '{}'? Use --yes to confirm.",
            "⚠".bright_yellow(),
            cmd.domain.bright_cyan()
        );
        return Ok(());
    }

    let client = reqwest::Client::new();
    let url = api_url(
        &cmd.api_url,
        &format!("/domains/{}", urlencoding::encode(&cmd.domain)),
    );

    let response = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    println!(
        "  {} Domain '{}' deleted successfully.",
        "✓".bright_green(),
        cmd.domain.bright_cyan()
    );

    Ok(())
}

// ========================================
// Provision domain (HTTP-01)
// ========================================

async fn execute_provision(cmd: ProvisionDomainCommand) -> anyhow::Result<()> {
    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "   Provisioning Certificate (HTTP-01)"
            .bright_white()
            .bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!();

    let client = reqwest::Client::new();
    let url = api_url(
        &cmd.api_url,
        &format!("/domains/{}/provision", urlencoding::encode(&cmd.domain)),
    );

    println!(
        "{} Provisioning certificate for {}...",
        "→".bright_blue(),
        cmd.domain.bright_cyan()
    );

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let provision_resp: ProvisionApiResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    match provision_resp {
        ProvisionApiResponse::Complete(domain) => {
            println!(
                "  {} Certificate provisioned successfully!",
                "✓".bright_green()
            );
            print_domain_details(&domain);
        }
        ProvisionApiResponse::Pending(challenge) => {
            println!(
                "  {} Challenge is pending. DNS records needed:",
                "⏳".bright_yellow()
            );
            for record in &challenge.txt_records {
                println!(
                    "    {} {} = {}",
                    "TXT".bright_white(),
                    record.name.bright_cyan(),
                    record.value.bright_white()
                );
            }
        }
        ProvisionApiResponse::Error(err) => {
            println!(
                "  {} Provisioning failed: {}",
                "✗".bright_red(),
                err.message.bright_red()
            );
            if let Some(details) = err.details {
                println!("    {}", details);
            }
        }
    }

    println!();
    Ok(())
}

// ========================================
// Order: Create
// ========================================

async fn execute_order_create(cmd: OrderCreateCommand) -> anyhow::Result<()> {
    println!();
    println!(
        "{} Creating ACME order for domain ID {}...",
        "→".bright_blue(),
        cmd.domain_id
    );

    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, &format!("/domains/{}/order", cmd.domain_id));

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let challenge_resp: DomainChallengeResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    println!(
        "  {} ACME order created for {}",
        "✓".bright_green(),
        challenge_resp.domain.bright_cyan()
    );
    println!(
        "  {} {}",
        "Status:".bright_white(),
        challenge_resp.status.bright_yellow()
    );

    if !challenge_resp.txt_records.is_empty() {
        println!();
        println!(
            "  {} Add the following DNS TXT record(s):",
            "DNS Records:".bright_white().bold()
        );
        println!();
        for record in &challenge_resp.txt_records {
            println!(
                "    {} {}",
                "Name:".bright_white(),
                record.name.bright_cyan()
            );
            println!(
                "    {} {}",
                "Value:".bright_white(),
                record.value.bright_white()
            );
            println!();
        }
        println!(
            "  {} After DNS propagation, finalize: {}",
            "→".bright_blue(),
            format!("temps domain order finalize --domain-id {}", cmd.domain_id).bright_white()
        );
    }

    println!();
    Ok(())
}

// ========================================
// Order: Show
// ========================================

async fn execute_order_show(cmd: OrderShowCommand) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, &format!("/domains/{}/order", cmd.domain_id));

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let order_resp: AcmeOrderResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&order_resp)
                .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?
        );
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "                    ACME Order Details"
            .bright_white()
            .bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!();

    println!(
        "  {} {}",
        "Order ID:".bright_white(),
        order_resp.id.to_string().bright_cyan()
    );
    println!(
        "  {} {}",
        "Domain ID:".bright_white(),
        order_resp.domain_id.to_string().bright_cyan()
    );
    println!(
        "  {} {}",
        "Email:".bright_white(),
        order_resp.email.bright_cyan()
    );

    let status_colored = match order_resp.status.as_str() {
        "valid" | "ready" => order_resp.status.bright_green(),
        "pending" | "processing" => order_resp.status.bright_yellow(),
        "invalid" | "expired" | "deactivated" | "revoked" => order_resp.status.bright_red(),
        _ => order_resp.status.normal(),
    };
    println!("  {} {}", "Status:".bright_white(), status_colored);

    println!(
        "  {} {}",
        "Order URL:".bright_white(),
        order_resp.order_url.bright_white()
    );
    println!(
        "  {} {}",
        "Created:".bright_white(),
        format_millis_timestamp(order_resp.created_at).bright_cyan()
    );

    if let Some(expires) = order_resp.expires_at {
        println!(
            "  {} {}",
            "Expires:".bright_white(),
            format_millis_timestamp(expires).bright_cyan()
        );
    }

    if let Some(ref err) = order_resp.error {
        println!("  {} {}", "Error:".bright_white(), err.bright_red());
    }

    // Show challenge validation status
    if let Some(ref validation) = order_resp.challenge_validation {
        println!();
        println!("  {}", "Challenge Validation:".bright_white().bold());

        let validation_status = match validation.status.as_str() {
            "valid" => validation.status.bright_green(),
            "pending" => validation.status.bright_yellow(),
            "invalid" => validation.status.bright_red(),
            _ => validation.status.normal(),
        };

        println!(
            "    {} {}",
            "Type:".bright_white(),
            validation.challenge_type.bright_cyan()
        );
        println!("    {} {}", "Status:".bright_white(), validation_status);
        println!(
            "    {} {}",
            "Token:".bright_white(),
            validation.token.bright_white()
        );

        if let Some(ref validated) = validation.validated {
            println!(
                "    {} {}",
                "Validated:".bright_white(),
                validated.bright_green()
            );
        }

        if let Some(ref err) = validation.error {
            println!(
                "    {} {} ({})",
                "Error:".bright_white(),
                err.detail.bright_red(),
                err.error_type
            );
        }
    }

    println!();
    Ok(())
}

// ========================================
// Order: Cancel
// ========================================

async fn execute_order_cancel(cmd: OrderCancelCommand) -> anyhow::Result<()> {
    if !cmd.yes {
        println!(
            "{} Are you sure you want to cancel the order for domain ID {}? Use --yes to confirm.",
            "⚠".bright_yellow(),
            cmd.domain_id
        );
        return Ok(());
    }

    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, &format!("/domains/{}/order", cmd.domain_id));

    println!(
        "{} Cancelling ACME order for domain ID {}...",
        "→".bright_blue(),
        cmd.domain_id
    );

    let response = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let domain_resp: DomainResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    println!(
        "  {} Order cancelled for domain '{}'",
        "✓".bright_green(),
        domain_resp.domain.bright_cyan()
    );
    println!(
        "  {} {}",
        "Status:".bright_white(),
        domain_resp.status.bright_yellow()
    );

    println!();
    Ok(())
}

// ========================================
// Order: Finalize
// ========================================

async fn execute_order_finalize(cmd: OrderFinalizeCommand) -> anyhow::Result<()> {
    println!();
    println!(
        "{} Finalizing ACME order for domain ID {}...",
        "→".bright_blue(),
        cmd.domain_id
    );

    let client = reqwest::Client::new();
    let url = api_url(
        &cmd.api_url,
        &format!("/domains/{}/order/finalize", cmd.domain_id),
    );

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let domain_resp: DomainResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    match domain_resp.status.as_str() {
        "active" => {
            println!("  {} Certificate issued successfully!", "✓".bright_green());
            print_domain_details(&domain_resp);
        }
        "failed" => {
            println!("  {} Certificate issuance failed.", "✗".bright_red());
            print_domain_details(&domain_resp);
            println!();
            println!(
                "  {} You can recreate the order: {}",
                "→".bright_blue(),
                format!("temps domain order create --domain-id {}", cmd.domain_id).bright_white()
            );
        }
        _ => {
            println!(
                "  {} Order finalized. Current status: {}",
                "ℹ".bright_blue(),
                domain_resp.status.bright_yellow()
            );
            print_domain_details(&domain_resp);
        }
    }

    println!();
    Ok(())
}

// ========================================
// Order: List
// ========================================

async fn execute_order_list(cmd: OrderListCommand) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let url = api_url(&cmd.api_url, "/orders");

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", cmd.api_token))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;

    if !response.status().is_success() {
        return Err(handle_api_error(response).await);
    }

    let list_resp: ListOrdersResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list_resp.orders)
                .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?
        );
        return Ok(());
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!(
        "{}",
        "                      ACME Orders".bright_white().bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_cyan()
    );
    println!();

    if list_resp.orders.is_empty() {
        println!("  {} No ACME orders found.", "ℹ".bright_blue());
        println!();
        return Ok(());
    }

    println!(
        "  {:<6} {:<12} {:<15} {:<30} {:<20}",
        "ID".bright_white().bold(),
        "DOMAIN ID".bright_white().bold(),
        "STATUS".bright_white().bold(),
        "EMAIL".bright_white().bold(),
        "CREATED".bright_white().bold()
    );
    println!("  {}", "─".repeat(85));

    for order in &list_resp.orders {
        let status_colored = match order.status.as_str() {
            "valid" | "ready" => order.status.bright_green(),
            "pending" | "processing" => order.status.bright_yellow(),
            "invalid" | "expired" | "deactivated" | "revoked" => order.status.bright_red(),
            _ => order.status.normal(),
        };

        println!(
            "  {:<6} {:<12} {:<15} {:<30} {:<20}",
            order.id.to_string().bright_white(),
            order.domain_id.to_string().bright_cyan(),
            status_colored,
            order.email,
            format_millis_date(order.created_at)
        );
    }

    println!();
    Ok(())
}

// ========================================
// Import certificate (direct database access)
// ========================================

async fn execute_import(cmd: ImportCertificateCommand) -> anyhow::Result<()> {
    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue()
    );
    println!(
        "{}",
        "                  Import Custom Certificate"
            .bright_blue()
            .bold()
    );
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_blue()
    );
    println!();

    // Get data directory
    let data_dir = get_data_dir(&cmd.data_dir)?;

    // Load encryption key
    let encryption_key = load_encryption_key(&data_dir)?;
    let encryption_service = EncryptionService::new(&encryption_key)
        .map_err(|e| anyhow::anyhow!("Failed to create encryption service: {}", e))?;

    // Read certificate and private key files
    println!(
        "{} Reading certificate from: {}",
        "→".bright_blue(),
        cmd.certificate.display()
    );
    let certificate_pem = fs::read_to_string(&cmd.certificate).with_context(|| {
        format!(
            "Failed to read certificate file: {}",
            cmd.certificate.display()
        )
    })?;

    println!(
        "{} Reading private key from: {}",
        "→".bright_blue(),
        cmd.private_key.display()
    );
    let private_key_pem = fs::read_to_string(&cmd.private_key).with_context(|| {
        format!(
            "Failed to read private key file: {}",
            cmd.private_key.display()
        )
    })?;

    // Validate certificate format and extract expiration
    let expiration_time = validate_and_parse_certificate(&certificate_pem, &cmd.domain)?;

    // Validate private key format
    validate_private_key(&private_key_pem)?;

    // Encrypt the private key
    println!("{} Encrypting private key...", "→".bright_blue());
    let encrypted_private_key = encryption_service
        .encrypt_string(&private_key_pem)
        .map_err(|e| anyhow::anyhow!("Failed to encrypt private key: {}", e))?;

    // Connect to database
    println!("{} Connecting to database...", "→".bright_blue());
    let db = establish_connection(&cmd.database_url).await?;

    // Check if domain already exists
    let existing = domains::Entity::find()
        .filter(domains::Column::Domain.eq(&cmd.domain))
        .one(db.as_ref())
        .await?;

    let is_wildcard = cmd.domain.starts_with("*.");

    if let Some(existing_domain) = existing {
        if !cmd.force {
            return Err(anyhow::anyhow!(
                "Domain '{}' already exists. Use --force to overwrite.",
                cmd.domain
            ));
        }

        println!(
            "{} Updating existing domain certificate...",
            "→".bright_yellow()
        );

        // Update existing domain
        let mut domain_update: domains::ActiveModel = existing_domain.into();
        domain_update.certificate = Set(Some(certificate_pem.clone()));
        domain_update.private_key = Set(Some(encrypted_private_key));
        domain_update.expiration_time = Set(Some(expiration_time));
        domain_update.status = Set("active".to_string());
        domain_update.last_renewed = Set(Some(Utc::now()));
        domain_update.last_error = Set(None);
        domain_update.last_error_type = Set(None);
        domain_update.verification_method = Set("manual".to_string());
        domain_update.updated_at = Set(Utc::now());

        domain_update.update(db.as_ref()).await?;
    } else {
        println!(
            "{} Creating new domain with certificate...",
            "→".bright_blue()
        );

        // Create new domain
        let new_domain = domains::ActiveModel {
            domain: Set(cmd.domain.clone()),
            certificate: Set(Some(certificate_pem.clone())),
            private_key: Set(Some(encrypted_private_key)),
            expiration_time: Set(Some(expiration_time)),
            status: Set("active".to_string()),
            is_wildcard: Set(is_wildcard),
            verification_method: Set("manual".to_string()),
            last_renewed: Set(Some(Utc::now())),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            ..Default::default()
        };

        new_domain.insert(db.as_ref()).await?;
    }

    println!();
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_green()
    );
    println!("{} Certificate imported successfully!", "✓".bright_green());
    println!(
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".bright_green()
    );
    println!();
    println!(
        "  {} {}",
        "Domain:".bright_white(),
        cmd.domain.bright_cyan()
    );
    println!(
        "  {} {}",
        "Type:".bright_white(),
        if is_wildcard {
            "Wildcard"
        } else {
            "Single domain"
        }
        .bright_cyan()
    );
    println!(
        "  {} {}",
        "Expires:".bright_white(),
        expiration_time
            .format("%Y-%m-%d %H:%M:%S UTC")
            .to_string()
            .bright_cyan()
    );
    println!("  {} {}", "Status:".bright_white(), "active".bright_green());
    println!();

    Ok(())
}

// ========================================
// Certificate validation helpers
// ========================================

fn get_data_dir(data_dir: &Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = data_dir {
        Ok(dir.clone())
    } else {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
        Ok(home.join(".temps"))
    }
}

fn load_encryption_key(data_dir: &Path) -> anyhow::Result<String> {
    let encryption_key_path = data_dir.join("encryption_key");

    if !encryption_key_path.exists() {
        return Err(anyhow::anyhow!(
            "Encryption key not found at {}. Run 'temps setup' first to initialize the data directory.",
            encryption_key_path.display()
        ));
    }

    let key = fs::read_to_string(&encryption_key_path)
        .map_err(|e| anyhow::anyhow!("Failed to read encryption key: {}", e))?;

    Ok(key.trim().to_string())
}

fn validate_and_parse_certificate(
    cert_pem: &str,
    expected_domain: &str,
) -> anyhow::Result<chrono::DateTime<Utc>> {
    // Parse PEM certificate
    let (_, pem) = parse_x509_pem(cert_pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to parse certificate PEM: {:?}", e))?;

    // Parse X509 certificate
    let (_, cert) = X509Certificate::from_der(&pem.contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse X509 certificate: {:?}", e))?;

    // Get expiration time
    let not_after = cert.validity().not_after;
    let expiration_timestamp = not_after.timestamp();
    let expiration_time = chrono::DateTime::from_timestamp(expiration_timestamp, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid certificate expiration timestamp"))?;

    // Check if certificate is expired
    if expiration_time < Utc::now() {
        return Err(anyhow::anyhow!(
            "Certificate is already expired (expired on {})",
            expiration_time.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }

    // Check certificate domains (CN and SANs)
    let mut cert_domains: Vec<String> = Vec::new();

    // Get Common Name
    if let Some(cn) = cert.subject().iter_common_name().next() {
        if let Ok(cn_str) = cn.as_str() {
            cert_domains.push(cn_str.to_string());
        }
    }

    // Get Subject Alternative Names
    if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
        for name in &san_ext.value.general_names {
            if let GeneralName::DNSName(dns) = name {
                cert_domains.push(dns.to_string());
            }
        }
    }

    // Check if expected domain matches certificate
    let domain_matches = cert_domains.iter().any(|cert_domain| {
        if cert_domain == expected_domain {
            return true;
        }
        // Check wildcard matching
        if let Some(cert_suffix) = cert_domain.strip_prefix("*.") {
            if let Some(expected_suffix) = expected_domain.strip_prefix("*.") {
                return cert_suffix == expected_suffix;
            }
            // Check if expected is a subdomain of wildcard
            if let Some(expected_suffix) = expected_domain
                .strip_prefix(|c: char| c != '.')
                .and_then(|s| s.strip_prefix('.'))
            {
                return cert_suffix == expected_suffix;
            }
        }
        false
    });

    if !domain_matches {
        println!(
            "{} Certificate domains: {:?}",
            "⚠".bright_yellow(),
            cert_domains
        );
        println!(
            "{} Expected domain '{}' does not match certificate. Proceeding anyway...",
            "⚠".bright_yellow(),
            expected_domain
        );
    } else {
        println!("{} Certificate domain validated", "✓".bright_green());
    }

    println!(
        "{} Certificate expires: {}",
        "✓".bright_green(),
        expiration_time.format("%Y-%m-%d %H:%M:%S UTC")
    );

    Ok(expiration_time)
}

fn validate_private_key(key_pem: &str) -> anyhow::Result<()> {
    // Basic PEM format validation
    if !key_pem.contains("-----BEGIN") || !key_pem.contains("-----END") {
        return Err(anyhow::anyhow!(
            "Invalid private key format. Expected PEM format with BEGIN/END markers."
        ));
    }

    // Check for common private key types
    let valid_types = [
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN ENCRYPTED PRIVATE KEY-----",
    ];

    let has_valid_type = valid_types.iter().any(|t| key_pem.contains(t));
    if !has_valid_type {
        return Err(anyhow::anyhow!(
            "Unsupported private key type. Expected RSA, EC, or PKCS#8 private key in PEM format."
        ));
    }

    println!("{} Private key format validated", "✓".bright_green());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_private_key_rsa() {
        let rsa_key = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7MxszF8Pf0Q3/Y
-----END RSA PRIVATE KEY-----"#;

        assert!(validate_private_key(rsa_key).is_ok());
    }

    #[test]
    fn test_validate_private_key_pkcs8() {
        let pkcs8_key = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDRndVLkklx2zfF
-----END PRIVATE KEY-----"#;

        assert!(validate_private_key(pkcs8_key).is_ok());
    }

    #[test]
    fn test_validate_private_key_ec() {
        let ec_key = r#"-----BEGIN EC PRIVATE KEY-----
MHQCAQEEIBDl5iLbSt9+cjO0XBcY7TPLYJ1YK/sFsYl1qVRkuVQLoAcGBSuBBAAK
-----END EC PRIVATE KEY-----"#;

        assert!(validate_private_key(ec_key).is_ok());
    }

    #[test]
    fn test_validate_private_key_invalid() {
        let invalid_key = "not a valid key";
        assert!(validate_private_key(invalid_key).is_err());
    }

    #[test]
    fn test_validate_private_key_wrong_type() {
        let wrong_type = r#"-----BEGIN CERTIFICATE-----
MIIBkTCB+wIJAKHBfpeg...
-----END CERTIFICATE-----"#;

        assert!(validate_private_key(wrong_type).is_err());
    }

    #[test]
    fn test_challenge_type_display() {
        assert_eq!(ChallengeType::Http01.to_string(), "http-01");
        assert_eq!(ChallengeType::Dns01.to_string(), "dns-01");
    }

    #[test]
    fn test_wildcard_requires_dns01() {
        // Simulating the validation logic from execute_add
        let domain = "*.example.com";
        let is_wildcard = domain.starts_with("*.");
        assert!(is_wildcard);

        // HTTP-01 should be rejected for wildcards
        let challenge = ChallengeType::Http01;
        let should_reject = is_wildcard && matches!(challenge, ChallengeType::Http01);
        assert!(should_reject);

        // DNS-01 should be accepted for wildcards
        let challenge = ChallengeType::Dns01;
        let should_reject = is_wildcard && matches!(challenge, ChallengeType::Http01);
        assert!(!should_reject);
        let _ = challenge;
    }

    #[test]
    fn test_format_millis_timestamp() {
        let millis = 1700000000000_i64; // 2023-11-14
        let formatted = format_millis_timestamp(millis);
        assert!(formatted.contains("2023"));
        assert!(formatted.contains("UTC"));
    }

    #[test]
    fn test_format_millis_date() {
        let millis = 1700000000000_i64;
        let formatted = format_millis_date(millis);
        assert!(formatted.contains("2023"));
        assert!(!formatted.contains("UTC"));
    }

    #[test]
    fn test_api_url() {
        assert_eq!(
            api_url("http://localhost:3000", "/domains"),
            "http://localhost:3000/domains"
        );
        assert_eq!(
            api_url("http://localhost:3000/", "/domains"),
            "http://localhost:3000/domains"
        );
    }
}
