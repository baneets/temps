use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

use super::{AiCliProvider, AiCliStatus, AiRunConfig, AiRunResult};
use crate::error::AgentError;

pub struct CodexCliProvider;

#[async_trait]
impl AiCliProvider for CodexCliProvider {
    fn name(&self) -> &str {
        "codex_cli"
    }

    async fn check_installed(&self) -> bool {
        Command::new("codex")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn get_status(&self) -> AiCliStatus {
        let version_output = Command::new("codex")
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        let (installed, version) = match version_output {
            Ok(output) if output.status.success() => {
                let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (true, if ver.is_empty() { None } else { Some(ver) })
            }
            _ => {
                return AiCliStatus {
                    provider: "codex_cli".into(),
                    installed: false,
                    version: None,
                    authenticated: false,
                    auth_method: None,
                    email: None,
                    subscription_type: None,
                    setup_hint: Some("Install Codex CLI: npm install -g @openai/codex".into()),
                };
            }
        };

        // Codex uses OPENAI_API_KEY env var — no built-in auth status command.
        // We consider it "authenticated" if the env var is set or if the user
        // provides an API key in the autopilot config.
        let has_env_key = std::env::var("OPENAI_API_KEY").is_ok();

        AiCliStatus {
            provider: "codex_cli".into(),
            installed,
            version,
            authenticated: has_env_key,
            auth_method: if has_env_key {
                Some("api_key".into())
            } else {
                None
            },
            email: None,
            subscription_type: None,
            setup_hint: if has_env_key {
                None
            } else {
                Some("Set OPENAI_API_KEY environment variable, or provide an API key in autopilot settings.".into())
            },
        }
    }

    async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        let mut cmd = Command::new("codex");
        cmd.arg("exec")
            .arg(&config.prompt)
            .arg("--full-auto")
            .arg("--json")
            .current_dir(&config.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if !config.api_key.is_empty() {
            cmd.env("OPENAI_API_KEY", &config.api_key);
        }

        let child_future = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::AiCliNotInstalled {
                    provider: self.name().to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let output = tokio::time::timeout(config.timeout, child_future.wait_with_output())
            .await
            .map_err(|_| AgentError::AiCliTimeout {
                provider: self.name().to_string(),
                timeout_secs: config.timeout.as_secs(),
            })?
            .map_err(AgentError::Io)?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(AgentError::AiCliFailed {
                provider: self.name().to_string(),
                exit_code,
                stderr,
            });
        }

        // Try to parse token usage from JSON lines in stdout
        let (tokens_input, tokens_output, model) = parse_codex_output(&stdout);

        Ok(AiRunResult {
            output: stdout,
            exit_code,
            tokens_input,
            tokens_output,
            model,
            changed_files: None,
        })
    }
}

/// Parse Codex CLI JSON output for token usage and model information.
fn parse_codex_output(output: &str) -> (Option<i32>, Option<i32>, Option<String>) {
    let mut tokens_input: Option<i32> = None;
    let mut tokens_output: Option<i32> = None;
    let mut model: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(usage) = value.get("usage") {
                if tokens_input.is_none() {
                    tokens_input = usage
                        .get("prompt_tokens")
                        .or_else(|| usage.get("input_tokens"))
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                }
                if tokens_output.is_none() {
                    tokens_output = usage
                        .get("completion_tokens")
                        .or_else(|| usage.get("output_tokens"))
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                }
            }
            if model.is_none() {
                model = value
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
    }

    (tokens_input, tokens_output, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_codex_output_with_usage() {
        let output = r#"{"model":"gpt-4o","usage":{"prompt_tokens":200,"completion_tokens":80}}"#;
        let (input, output_tokens, model) = parse_codex_output(output);
        assert_eq!(input, Some(200));
        assert_eq!(output_tokens, Some(80));
        assert_eq!(model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn test_parse_codex_output_empty() {
        let output = "plain text output";
        let (input, output_tokens, model) = parse_codex_output(output);
        assert!(input.is_none());
        assert!(output_tokens.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn test_codex_provider_name() {
        let provider = CodexCliProvider;
        assert_eq!(provider.name(), "codex_cli");
    }
}
