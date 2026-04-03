use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

use super::{AiCliProvider, AiCliStatus, AiRunConfig, AiRunResult};
use crate::error::AgentError;

pub struct ClaudeCliProvider;

#[async_trait]
impl AiCliProvider for ClaudeCliProvider {
    fn name(&self) -> &str {
        "claude_cli"
    }

    async fn check_installed(&self) -> bool {
        Command::new("claude")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn get_status(&self) -> AiCliStatus {
        // Check if installed
        let version_output = Command::new("claude")
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
                    provider: "claude_cli".into(),
                    installed: false,
                    version: None,
                    authenticated: false,
                    auth_method: None,
                    email: None,
                    subscription_type: None,
                    setup_hint: Some(
                        "Install Claude CLI: npm install -g @anthropic-ai/claude-code".into(),
                    ),
                };
            }
        };

        // Check auth status
        let auth_output = Command::new("claude")
            .args(["auth", "status"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        let (authenticated, auth_method, email, subscription_type, setup_hint) = match auth_output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    let logged_in = json.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false);
                    if logged_in {
                        (
                            true,
                            json.get("authMethod").and_then(|v| v.as_str()).map(String::from),
                            json.get("email").and_then(|v| v.as_str()).map(String::from),
                            json.get("subscriptionType").and_then(|v| v.as_str()).map(String::from),
                            None,
                        )
                    } else {
                        (false, None, None, None, Some(
                            "Run 'claude setup-token' on the server to authenticate, or set ANTHROPIC_API_KEY in autopilot settings.".into(),
                        ))
                    }
                } else {
                    (false, None, None, None, Some(
                        "Run 'claude setup-token' on the server to authenticate.".into(),
                    ))
                }
            }
            _ => (false, None, None, None, Some(
                "Run 'claude setup-token' on the server to authenticate, or set ANTHROPIC_API_KEY in autopilot settings.".into(),
            )),
        };

        AiCliStatus {
            provider: "claude_cli".into(),
            installed,
            version,
            authenticated,
            auth_method,
            email,
            subscription_type,
            setup_hint,
        }
    }

    async fn run(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg(&config.prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--max-turns")
            .arg(config.max_turns.to_string())
            .arg("--dangerously-skip-permissions")
            .arg("--verbose")
            .current_dir(&config.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if !config.api_key.is_empty() {
            cmd.env("ANTHROPIC_API_KEY", &config.api_key);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::AiCliNotInstalled {
                    provider: self.name().to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        // Stream stdout line by line, calling on_event for each JSON line
        let stdout_handle = child.stdout.take().expect("stdout was piped");
        let on_event = config.on_event.clone();

        let stream_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout_handle);
            let mut lines = reader.lines();
            let mut all_output = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_output.push_str(&line);
                all_output.push('\n');

                // Call the real-time callback if provided
                if let Some(ref cb) = on_event {
                    cb(line).await;
                }
            }

            all_output
        });

        // Wait for process to finish with timeout
        let wait_result = tokio::time::timeout(config.timeout, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(AgentError::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                return Err(AgentError::AiCliTimeout {
                    provider: self.name().to_string(),
                    timeout_secs: config.timeout.as_secs(),
                });
            }
        };

        // Get the accumulated output from the stream task
        let stdout = stream_task.await.unwrap_or_default();
        let exit_code = status.code().unwrap_or(-1);

        if !status.success() {
            // Read stderr for error message
            let stderr = String::new(); // stderr was already consumed
            return Err(AgentError::AiCliFailed {
                provider: self.name().to_string(),
                exit_code,
                stderr,
            });
        }

        let (tokens_input, tokens_output, model) = parse_claude_output(&stdout);

        Ok(AiRunResult {
            output: stdout,
            exit_code,
            tokens_input,
            tokens_output,
            model,
            changed_files: None,
        })
    }

    async fn continue_conversation(&self, config: AiRunConfig) -> Result<AiRunResult, AgentError> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut cmd = Command::new("claude");
        cmd.arg("--print")
            .arg("--continue") // Continue the most recent conversation in this directory
            .arg(&config.prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--max-turns")
            .arg(config.max_turns.to_string())
            .arg("--dangerously-skip-permissions")
            .arg("--verbose")
            .current_dir(&config.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if !config.api_key.is_empty() {
            cmd.env("ANTHROPIC_API_KEY", &config.api_key);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::AiCliNotInstalled {
                    provider: self.name().to_string(),
                }
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdout_handle = child.stdout.take().expect("stdout was piped");
        let on_event = config.on_event.clone();

        let stream_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout_handle);
            let mut lines = reader.lines();
            let mut all_output = String::new();

            while let Ok(Some(line)) = lines.next_line().await {
                all_output.push_str(&line);
                all_output.push('\n');

                if let Some(ref cb) = on_event {
                    cb(line).await;
                }
            }

            all_output
        });

        let wait_result = tokio::time::timeout(config.timeout, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(AgentError::Io(e)),
            Err(_) => {
                let _ = child.kill().await;
                return Err(AgentError::AiCliTimeout {
                    provider: self.name().to_string(),
                    timeout_secs: config.timeout.as_secs(),
                });
            }
        };

        let stdout = stream_task.await.unwrap_or_default();
        let exit_code = status.code().unwrap_or(-1);

        if !status.success() {
            let stderr = String::new();
            return Err(AgentError::AiCliFailed {
                provider: self.name().to_string(),
                exit_code,
                stderr,
            });
        }

        let (tokens_input, tokens_output, model) = parse_claude_output(&stdout);

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

/// Parse Claude CLI JSON output for token usage and model information.
/// Claude CLI may emit JSON objects per line (JSON Lines format).
fn parse_claude_output(output: &str) -> (Option<i32>, Option<i32>, Option<String>) {
    let mut tokens_input: Option<i32> = None;
    let mut tokens_output: Option<i32> = None;
    let mut model: Option<String> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            // Look for usage object at top level or nested
            if let Some(usage) = value.get("usage") {
                if tokens_input.is_none() {
                    tokens_input = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                }
                if tokens_output.is_none() {
                    tokens_output = usage
                        .get("output_tokens")
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
    fn test_parse_claude_output_with_usage() {
        let output =
            r#"{"model":"claude-3-5-sonnet","usage":{"input_tokens":150,"output_tokens":42}}"#;
        let (input, output_tokens, model) = parse_claude_output(output);
        assert_eq!(input, Some(150));
        assert_eq!(output_tokens, Some(42));
        assert_eq!(model.as_deref(), Some("claude-3-5-sonnet"));
    }

    #[test]
    fn test_parse_claude_output_empty() {
        let output = "no json here";
        let (input, output_tokens, model) = parse_claude_output(output);
        assert!(input.is_none());
        assert!(output_tokens.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn test_claude_provider_name() {
        let provider = ClaudeCliProvider;
        assert_eq!(provider.name(), "claude_cli");
    }
}
