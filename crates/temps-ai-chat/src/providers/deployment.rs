//! The deployment-failure context provider (ADR-023, consumer #1).
//!
//! Seeds a chat from a failed deployment: its state + reason, and each failed
//! step's error message. `context_id` is the deployment's integer id.
//!
//! v1 seeds from the deployment/job rows (always present, no extra I/O); a fast
//! follow-up enriches the system context with the failed jobs' log *tails* via
//! `LogService::get_log_content(job.log_id)`.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use temps_entities::types::JobStatus;
use temps_entities::{deployment_jobs, deployments};

use crate::provider::{ConversationContextProvider, ConversationSeed};

const SYSTEM_PREAMBLE: &str = "You are a senior platform/DevOps engineer helping a developer debug a FAILED Temps \
deployment. Use the failure context below. Identify the most likely root cause and concrete, ordered fixes, \
grounded strictly in the evidence — do not invent file names, versions, or causes you cannot see. Ask a brief \
clarifying question only when essential. Be concise and practical.";

/// Seeds deployment-failure debugging chats.
pub struct DeploymentChatProvider {
    db: Arc<DatabaseConnection>,
}

impl DeploymentChatProvider {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ConversationContextProvider for DeploymentChatProvider {
    fn context_type(&self) -> &'static str {
        "deployment"
    }

    async fn seed(&self, project_id: i32, context_id: &str) -> Option<ConversationSeed> {
        let deployment_id: i32 = context_id.parse().ok()?;
        let dep = deployments::Entity::find_by_id(deployment_id)
            .one(self.db.as_ref())
            .await
            .ok()??;
        // Authorization belt-and-braces: the row must belong to this project.
        if dep.project_id != project_id {
            return None;
        }

        let jobs = deployment_jobs::Entity::find()
            .filter(deployment_jobs::Column::DeploymentId.eq(deployment_id))
            .all(self.db.as_ref())
            .await
            .ok()?;
        let failed: Vec<&deployment_jobs::Model> = jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Failure))
            .collect();

        let mut ctx = String::new();
        ctx.push_str(SYSTEM_PREAMBLE);
        ctx.push_str("\n\n--- Failure context ---\n");
        ctx.push_str(&format!("Deployment #{} — state: {}\n", dep.id, dep.state));
        if let Some(branch) = &dep.branch_ref {
            ctx.push_str(&format!("Branch: {branch}\n"));
        }
        if let Some(commit) = &dep.commit_sha {
            let short: String = commit.chars().take(8).collect();
            ctx.push_str(&format!("Commit: {short}\n"));
        }
        if let Some(reason) = &dep.cancelled_reason {
            ctx.push_str(&format!("Failure reason: {reason}\n"));
        }
        if failed.is_empty() {
            ctx.push_str("No individual job failures were recorded.\n");
        } else {
            ctx.push_str("Failed steps:\n");
            for j in &failed {
                ctx.push_str(&format!(
                    "- '{}': {}\n",
                    j.job_id,
                    j.error_message.as_deref().unwrap_or("(no error message)")
                ));
            }
        }

        let metadata = serde_json::json!({
            "deployment_id": deployment_id,
            "state": dep.state,
            "failed_job_ids": failed.iter().map(|j| j.job_id.clone()).collect::<Vec<_>>(),
        });

        Some(ConversationSeed {
            system: ctx,
            first_assistant: None,
            title: Some(format!("Debug deployment #{deployment_id}")),
            metadata: Some(metadata),
        })
    }
}
