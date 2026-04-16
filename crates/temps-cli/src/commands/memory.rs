//! `temps memory` subcommand — read/write workflow memory from an
//! operator's shell without having to curl + jq the HTTP API.
//!
//! This is a thin HTTP client over the versioned `/api/v1/projects/
//! {project_id}/workflows/{slug}/memory` namespace introduced in PR 2.3.
//! Same surface the sandbox-side bash `memory` script uses — this is just
//! the operator-ergonomic sibling, usable without a sandbox.
//!
//! Shape contract: DTOs here mirror the server-side DTOs in
//! `temps-workspace::handlers::memory`. If that handler's request/response
//! shape changes, update these structs in the same PR — the server is
//! permissive on responses but strict on requests (`deny_unknown_fields`
//! is not yet applied here, but will come as part of the PR 1.1 follow-up
//! to memory DTOs).

use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

/// Workflow memory management (versioned `/v1/memory` API).
#[derive(Args)]
pub struct MemoryCommand {
    #[command(subcommand)]
    pub command: MemorySubcommand,
}

#[derive(Subcommand)]
pub enum MemorySubcommand {
    /// List recent facts for a workflow
    #[command(alias = "ls")]
    List(MemoryListCommand),
    /// Full-text search facts by substring
    Search(MemorySearchCommand),
    /// Write a new fact
    Write(MemoryWriteCommand),
    /// Replace an old fact with a new one (keeps audit trail)
    Supersede(MemorySupersedeCommand),
    /// Hard-delete a fact (rarely needed — prefer `supersede`)
    #[command(alias = "rm")]
    Delete(MemoryDeleteCommand),
}

// ── Common args ────────────────────────────────────────────────────────────

/// Shared flags every memory subcommand takes. Factored out so the flag
/// names + env-var behavior stay identical across subcommands.
#[derive(Args, Clone)]
pub struct ApiArgs {
    /// API base URL (e.g., "http://localhost:3000").
    #[arg(long, env = "TEMPS_API_URL")]
    pub api_url: String,
    /// API bearer token.
    #[arg(long, env = "TEMPS_API_TOKEN")]
    pub api_token: String,
    /// Numeric project id this memory belongs to.
    #[arg(long, env = "TEMPS_PROJECT_ID")]
    pub project_id: i32,
    /// Workflow slug (scope key — memory is per-workflow, not per-project).
    #[arg(long, env = "TEMPS_WORKFLOW_SLUG")]
    pub slug: String,
}

// ── Subcommand args ────────────────────────────────────────────────────────

#[derive(Args)]
pub struct MemoryListCommand {
    #[command(flatten)]
    pub api: ApiArgs,
    /// Max number of facts to return. Server caps at its own limit.
    #[arg(long, default_value = "20")]
    pub limit: u64,
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemorySearchCommand {
    #[command(flatten)]
    pub api: ApiArgs,
    /// Query string (matched against `fact` column with FTS).
    pub query: String,
    #[arg(long, default_value = "10")]
    pub limit: u64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemoryWriteCommand {
    #[command(flatten)]
    pub api: ApiArgs,
    /// The fact text itself.
    pub fact: String,
    /// Comma-separated tags (e.g. `error_group_id:42,file:src/auth.ts`).
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    /// Optional confidence override in [0.0, 1.0].
    #[arg(long)]
    pub confidence: Option<f32>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemorySupersedeCommand {
    #[command(flatten)]
    pub api: ApiArgs,
    /// ID of the fact being replaced.
    pub fact_id: i64,
    /// Replacement fact text.
    #[arg(long)]
    pub by: String,
    /// Comma-separated tags for the new fact.
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemoryDeleteCommand {
    #[command(flatten)]
    pub api: ApiArgs,
    /// ID of the fact to delete.
    pub fact_id: i64,
    #[arg(long)]
    pub quiet: bool,
}

// ── Wire DTOs (mirror server-side DTOs) ────────────────────────────────────

#[derive(Debug, Serialize)]
struct WriteBody {
    fact: String,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<f32>,
}

#[derive(Debug, Serialize)]
struct SupersedeBody {
    new_fact: String,
    new_tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryFactResponse {
    id: i64,
    fact: String,
    #[serde(default)]
    tags: Vec<String>,
    confidence: f32,
    times_used: i32,
    #[serde(default)]
    superseded_by: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MemoryListResponse {
    facts: Vec<MemoryFactResponse>,
    total: usize,
}

#[derive(Debug, Deserialize)]
struct ProblemDetail {
    title: Option<String>,
    detail: Option<String>,
}

// ── HTTP helpers ───────────────────────────────────────────────────────────

/// Build the absolute URL for a `/api/v1/projects/{pid}/workflows/{slug}/
/// memory` path. `path` is appended after `/memory` and may be empty or
/// start with "/". Trailing slashes on `base` are tolerated.
fn memory_url(base: &str, project_id: i32, slug: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    // urlencoding the slug guards against an adventurous operator who
    // passes e.g. "my workflow" or "foo/bar" — axum would 404 on an
    // unencoded slash, and a percent-encoded one flows through cleanly.
    let slug = urlencoding::encode(slug);
    format!("{base}/api/v1/projects/{project_id}/workflows/{slug}/memory{path}")
}

fn make_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build HTTP client")
}

async fn api_error(response: reqwest::Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(problem) = serde_json::from_str::<ProblemDetail>(&body) {
        anyhow::anyhow!(
            "API error ({}): {} - {}",
            status,
            problem.title.unwrap_or_default(),
            problem.detail.unwrap_or_default()
        )
    } else {
        anyhow::anyhow!("API error ({}): {}", status, body)
    }
}

fn print_fact(f: &MemoryFactResponse) {
    let superseded = f
        .superseded_by
        .map(|id| format!(" [superseded by #{}]", id).bright_red().to_string())
        .unwrap_or_default();
    println!(
        "  [{id}] (conf={conf:.2}, used={used}){sup} {fact}",
        id = f.id.to_string().bright_cyan(),
        conf = f.confidence,
        used = f.times_used,
        sup = superseded,
        fact = f.fact,
    );
    if !f.tags.is_empty() {
        println!("      tags: {}", f.tags.join(", ").bright_black());
    }
}

// ── Dispatch ───────────────────────────────────────────────────────────────

impl MemoryCommand {
    pub fn execute(self) -> anyhow::Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            match self.command {
                MemorySubcommand::List(c) => execute_list(c).await,
                MemorySubcommand::Search(c) => execute_search(c).await,
                MemorySubcommand::Write(c) => execute_write(c).await,
                MemorySubcommand::Supersede(c) => execute_supersede(c).await,
                MemorySubcommand::Delete(c) => execute_delete(c).await,
            }
        })
    }
}

async fn execute_list(cmd: MemoryListCommand) -> anyhow::Result<()> {
    let url = memory_url(&cmd.api.api_url, cmd.api.project_id, &cmd.api.slug, "");
    let response = make_client()
        .get(&url)
        .bearer_auth(&cmd.api.api_token)
        .query(&[("limit", cmd.limit)])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let data: MemoryListResponse = response.json().await?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "total": data.total,
                "facts": data.facts.iter().map(|f| serde_json::json!({
                    "id": f.id,
                    "fact": f.fact,
                    "tags": f.tags,
                    "confidence": f.confidence,
                    "times_used": f.times_used,
                    "superseded_by": f.superseded_by,
                })).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    if data.facts.is_empty() {
        println!(
            "No facts stored for workflow {} in project {}.",
            cmd.api.slug.bright_cyan(),
            cmd.api.project_id
        );
        return Ok(());
    }
    println!();
    for f in &data.facts {
        print_fact(f);
    }
    println!();
    println!("  {} {} fact(s)", "→".bright_black(), data.total);
    Ok(())
}

async fn execute_search(cmd: MemorySearchCommand) -> anyhow::Result<()> {
    let url = memory_url(
        &cmd.api.api_url,
        cmd.api.project_id,
        &cmd.api.slug,
        "/search",
    );
    let response = make_client()
        .get(&url)
        .bearer_auth(&cmd.api.api_token)
        .query(&[("q", cmd.query.as_str())])
        .query(&[("limit", cmd.limit)])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let data: MemoryListResponse = response.json().await?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &data
                    .facts
                    .iter()
                    .map(|f| serde_json::json!({
                        "id": f.id,
                        "fact": f.fact,
                        "tags": f.tags,
                        "confidence": f.confidence,
                        "times_used": f.times_used,
                    }))
                    .collect::<Vec<_>>()
            )?
        );
        return Ok(());
    }

    if data.facts.is_empty() {
        println!("No matches.");
        return Ok(());
    }
    for f in &data.facts {
        print_fact(f);
    }
    Ok(())
}

async fn execute_write(cmd: MemoryWriteCommand) -> anyhow::Result<()> {
    let url = memory_url(&cmd.api.api_url, cmd.api.project_id, &cmd.api.slug, "");
    let body = WriteBody {
        fact: cmd.fact,
        tags: cmd.tags,
        confidence: cmd.confidence,
    };
    let response = make_client()
        .post(&url)
        .bearer_auth(&cmd.api.api_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let fact: MemoryFactResponse = response.json().await?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": fact.id,
                "fact": fact.fact,
                "tags": fact.tags,
                "confidence": fact.confidence,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} #{}",
        "Saved fact".bright_green().bold(),
        fact.id.to_string().bright_cyan()
    );
    Ok(())
}

async fn execute_supersede(cmd: MemorySupersedeCommand) -> anyhow::Result<()> {
    let url = memory_url(
        &cmd.api.api_url,
        cmd.api.project_id,
        &cmd.api.slug,
        &format!("/{}/supersede", cmd.fact_id),
    );
    let body = SupersedeBody {
        new_fact: cmd.by,
        new_tags: cmd.tags,
    };
    let response = make_client()
        .post(&url)
        .bearer_auth(&cmd.api.api_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    let fact: MemoryFactResponse = response.json().await?;

    if cmd.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "old_id": cmd.fact_id,
                "new_id": fact.id,
                "fact": fact.fact,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} #{} {} #{}",
        "Superseded".bright_green().bold(),
        cmd.fact_id.to_string().bright_red(),
        "→".bright_black(),
        fact.id.to_string().bright_cyan()
    );
    Ok(())
}

async fn execute_delete(cmd: MemoryDeleteCommand) -> anyhow::Result<()> {
    let url = memory_url(
        &cmd.api.api_url,
        cmd.api.project_id,
        &cmd.api.slug,
        &format!("/{}", cmd.fact_id),
    );
    let response = make_client()
        .delete(&url)
        .bearer_auth(&cmd.api.api_token)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to API: {}", e))?;
    if !response.status().is_success() {
        return Err(api_error(response).await);
    }
    if !cmd.quiet {
        println!("{} #{}", "Deleted fact".bright_green().bold(), cmd.fact_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_url_builds_expected_shape() {
        let u = memory_url("http://api.example.com", 7, "my-workflow", "");
        assert_eq!(
            u,
            "http://api.example.com/api/v1/projects/7/workflows/my-workflow/memory"
        );
    }

    #[test]
    fn memory_url_trims_trailing_slash() {
        let u = memory_url("http://api.example.com/", 7, "wf", "/search");
        assert_eq!(
            u,
            "http://api.example.com/api/v1/projects/7/workflows/wf/memory/search"
        );
    }

    #[test]
    fn memory_url_encodes_risky_slugs() {
        // A slug with a slash must not break out of the expected path.
        // axum would 404 on the decoded form; percent-encoding keeps the
        // request reachable and lets the server decide (it'll return 404
        // for unknown slugs — the point is we don't smuggle path segments).
        let u = memory_url("http://api", 1, "foo/bar baz", "");
        assert!(
            u.contains("foo%2Fbar%20baz") || u.contains("foo%2Fbar+baz"),
            "slug not percent-encoded: {u}"
        );
    }

    #[test]
    fn write_body_skips_none_confidence() {
        let body = WriteBody {
            fact: "x".into(),
            tags: vec!["t".into()],
            confidence: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("confidence"));
        assert!(json.contains("\"fact\":\"x\""));
        assert!(json.contains("\"tags\":[\"t\"]"));
    }

    #[test]
    fn write_body_includes_some_confidence() {
        let body = WriteBody {
            fact: "x".into(),
            tags: vec![],
            confidence: Some(0.9),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"confidence\":0.9"));
    }
}
