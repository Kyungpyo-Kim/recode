use chrono::Utc;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::json;
use wait_timeout::ChildExt;

use crate::engine::{AttemptOutcome, StepRunner};
use crate::model::{
    AttemptStatus, RunMode, RunRecord, SessionRecord, StepKind, StepRecord, TaskRecord,
};
use crate::{ProviderConfig, ProviderMode};

#[derive(Debug, Clone, Default)]
pub struct ExecutorOptions {
    pub stream_output: bool,
    pub use_pty: bool,
    pub cancel_path: Option<PathBuf>,
    pub background: bool,
    pub provider: Option<ProviderConfig>,
}

#[derive(Debug, Default, Clone)]
pub struct ExecutorBridge {
    options: ExecutorOptions,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

const MAX_CHAT_CONTEXT_MESSAGES: usize = 12;

struct StreamingChatRequest {
    client: Client,
    endpoint: String,
    headers: HeaderMap,
    provider: ProviderConfig,
    messages: Vec<ChatMessage>,
    request_messages: Vec<ChatMessage>,
}

impl ExecutorBridge {
    pub fn new() -> Self {
        Self::with_options(ExecutorOptions::default())
    }

    pub fn with_options(options: ExecutorOptions) -> Self {
        Self { options }
    }

    fn command_for_step<'a>(&self, step: &'a StepRecord) -> Option<&'a str> {
        match step.kind {
            StepKind::Shell => step
                .command
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty()),
            StepKind::LlmChat | StepKind::Operator => None,
        }
    }

    fn prompt_for_step<'a>(&self, step: &'a StepRecord) -> Option<&'a str> {
        match step.kind {
            StepKind::LlmChat => step
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|prompt| !prompt.is_empty()),
            StepKind::Shell | StepKind::Operator => None,
        }
    }

    fn open_log_file(path: Option<&str>) -> std::io::Result<Option<File>> {
        match path {
            Some(path) => Ok(Some(File::create(path)?)),
            None => Ok(None),
        }
    }

    fn run_command(&self, command: &str, run: &RunRecord, timeout_secs: u64) -> AttemptOutcome {
        let stdout_file = match Self::open_log_file(run.stdout_log_path.as_deref()) {
            Ok(file) => file,
            Err(error) => {
                return AttemptOutcome::failed(format!("failed to open stdout log: {error}"));
            }
        };
        let stderr_file = match Self::open_log_file(run.stderr_log_path.as_deref()) {
            Ok(file) => file,
            Err(error) => {
                return AttemptOutcome::failed(format!("failed to open stderr log: {error}"));
            }
        };

        let mut child = match spawn_command(command, run, &self.options, stdout_file, stderr_file) {
            Ok(child) => child,
            Err(error) => {
                return AttemptOutcome::failed(format!("failed to spawn command: {error}"));
            }
        };
        let child_pid = child.id();

        if self.options.background {
            return AttemptOutcome {
                status: AttemptStatus::Running,
                summary: Some(format!("command running in background: {command}")),
                pid: Some(child_pid),
            };
        }

        let timeout = Duration::from_secs(timeout_secs.max(1));
        let poll = Duration::from_millis(100);
        let started = Instant::now();

        loop {
            if cancel_requested(self.options.cancel_path.as_deref()) {
                let _ = child.kill();
                let _ = child.wait();
                return AttemptOutcome {
                    status: AttemptStatus::Cancelled,
                    summary: Some(format!(
                        "command cancelled via {}: {}",
                        self.options
                            .cancel_path
                            .as_deref()
                            .map(Path::display)
                            .map(|path| path.to_string())
                            .unwrap_or_else(|| "cancel signal".into()),
                        command
                    )),
                    pid: Some(child_pid),
                };
            }

            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return AttemptOutcome::timed_out(format!(
                    "command timed out after {}s: {}",
                    timeout.as_secs(),
                    command
                ))
                .with_pid(child_pid);
            }

            match child.wait_timeout(poll) {
                Ok(Some(status)) => {
                    if self.options.stream_output {
                        return if status.success() {
                            AttemptOutcome::succeeded(format!("command succeeded: {command}"))
                                .with_pid(child_pid)
                        } else {
                            AttemptOutcome::failed(format!("command failed: {command}"))
                                .with_pid(child_pid)
                        };
                    }

                    return match status.code() {
                        Some(0) => {
                            AttemptOutcome::succeeded(format!("command succeeded: {command}"))
                                .with_pid(child_pid)
                        }
                        _ => AttemptOutcome::failed(format!("command failed: {command}"))
                            .with_pid(child_pid),
                    };
                }
                Ok(None) => continue,
                Err(error) => {
                    return AttemptOutcome::failed(format!("failed to wait for command: {error}"))
                        .with_pid(child_pid);
                }
            }
        }
    }

    fn run_llm_chat(
        &self,
        step: &StepRecord,
        prompt: &str,
        run: &RunRecord,
        timeout_secs: u64,
    ) -> AttemptOutcome {
        if self.options.background {
            return AttemptOutcome::failed("llm_chat does not support background mode yet");
        }

        let mut messages = load_prior_chat_messages(step, run.transcript_artifact_path.as_deref());
        messages.push(ChatMessage {
            role: String::from("user"),
            content: prompt.to_string(),
        });
        let request_messages = trim_chat_messages(&messages, MAX_CHAT_CONTEXT_MESSAGES);

        let provider = self.options.provider.clone().unwrap_or_default();
        let provider_name = provider.name.clone();
        let provider_model = provider.model.clone();
        let provider_api_key_env = provider.api_key_env.clone();
        let base_url = provider
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let _ = write_json_artifact(
            run.request_artifact_path.as_deref(),
            &json!({
                "run_id": run.id,
                "recorded_at": Utc::now(),
                "provider": {
                    "name": provider_name,
                    "mode": provider.mode,
                    "model": provider_model,
                    "base_url": base_url.clone(),
                    "api_key_env": provider_api_key_env,
                },
                "request": {
                    "messages": request_messages
                }
            }),
        );
        let api_key = match std::env::var(&provider.api_key_env) {
            Ok(value) if !value.trim().is_empty() => value,
            Ok(_) | Err(_) => {
                let _ = write_log_line(
                    run.stderr_log_path.as_deref(),
                    &format!("missing API key env: {}", provider.api_key_env),
                );
                return AttemptOutcome::failed(format!(
                    "missing API key env: {}",
                    provider.api_key_env
                ));
            }
        };
        let endpoint = match provider.mode {
            ProviderMode::OpenAiCompatible | ProviderMode::Codex => {
                format!("{}/chat/completions", base_url.trim_end_matches('/'))
            }
        };

        let _ = write_log_line(
            run.stdout_log_path.as_deref(),
            &format!(
                "provider={} mode={:?} model={} endpoint={}",
                provider.name, provider.mode, provider.model, endpoint
            ),
        );
        let _ = write_log_line(run.stdout_log_path.as_deref(), "request:");
        for message in &request_messages {
            let _ = write_log_line(
                run.stdout_log_path.as_deref(),
                &format!("{}: {}", message.role, message.content),
            );
        }
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {api_key}");
        let auth_value = match HeaderValue::from_str(&bearer) {
            Ok(value) => value,
            Err(error) => return AttemptOutcome::failed(format!("invalid auth header: {error}")),
        };
        headers.insert(AUTHORIZATION, auth_value);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = match Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                return AttemptOutcome::failed(format!("failed to build HTTP client: {error}"));
            }
        };

        if self.options.stream_output {
            return self.run_llm_chat_streaming(
                StreamingChatRequest {
                    client,
                    endpoint,
                    headers,
                    provider,
                    messages,
                    request_messages,
                },
                run,
            );
        }

        let payload = json!({
            "model": provider.model,
            "messages": request_messages
        });

        let response = match client.post(endpoint).headers(headers).json(&payload).send() {
            Ok(response) => response,
            Err(error) => {
                let _ = write_log_line(
                    run.stderr_log_path.as_deref(),
                    &format!("request failed: {error}"),
                );
                return AttemptOutcome::failed(format!("llm_chat request failed: {error}"));
            }
        };

        let status = response.status();
        let body = match response.text() {
            Ok(body) => body,
            Err(error) => {
                let _ = write_log_line(
                    run.stderr_log_path.as_deref(),
                    &format!("failed to read response body: {error}"),
                );
                return AttemptOutcome::failed(format!(
                    "failed to read llm_chat response body: {error}"
                ));
            }
        };

        if !status.is_success() {
            let _ = write_log_line(run.stderr_log_path.as_deref(), &body);
            let _ = write_json_artifact(
                run.response_artifact_path.as_deref(),
                &json!({
                    "run_id": run.id,
                    "recorded_at": Utc::now(),
                    "ok": false,
                    "status": status.as_u16(),
                    "provider": {
                        "name": provider.name,
                        "model": provider.model,
                    },
                    "body": body,
                }),
            );
            return AttemptOutcome::failed(format!("llm_chat HTTP {}", status.as_u16()));
        }

        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = write_log_line(
                    run.stderr_log_path.as_deref(),
                    &format!("invalid JSON response: {error}"),
                );
                return AttemptOutcome::failed(format!("invalid llm_chat JSON response: {error}"));
            }
        };

        let content = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.clone());

        let _ = write_json_artifact(
            run.response_artifact_path.as_deref(),
            &json!({
                "run_id": run.id,
                "recorded_at": Utc::now(),
                "ok": true,
                "status": status.as_u16(),
                "provider": {
                    "name": provider.name,
                    "model": provider.model,
                },
                "usage": parsed.get("usage").cloned(),
                "body": parsed,
                "content": content,
            }),
        );

        let _ = write_log_line(run.stdout_log_path.as_deref(), "response:");
        let _ = write_log_line(run.stdout_log_path.as_deref(), &content);
        messages.push(ChatMessage {
            role: String::from("assistant"),
            content: content.clone(),
        });
        let _ = write_json_artifact(
            run.transcript_artifact_path.as_deref(),
            &json!({
                "run_id": run.id,
                "recorded_at": Utc::now(),
                "messages": messages,
                "sent_context_messages": request_messages.len(),
                "stored_transcript_messages": messages.len()
            }),
        );

        AttemptOutcome::succeeded(format!("llm_chat completed: {} chars", content.len()))
    }

    fn run_llm_chat_streaming(
        &self,
        request: StreamingChatRequest,
        run: &RunRecord,
    ) -> AttemptOutcome {
        let StreamingChatRequest {
            client,
            endpoint,
            headers,
            provider,
            mut messages,
            request_messages,
        } = request;
        let payload = json!({
            "model": provider.model,
            "messages": request_messages,
            "stream": true
        });

        let response = match client.post(endpoint).headers(headers).json(&payload).send() {
            Ok(response) => response,
            Err(error) => {
                let _ = write_log_line(
                    run.stderr_log_path.as_deref(),
                    &format!("request failed: {error}"),
                );
                return AttemptOutcome::failed(format!("llm_chat request failed: {error}"));
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = read_response_body(response)
                .unwrap_or_else(|error| format!("failed to read llm_chat error body: {error}"));
            let _ = write_log_line(run.stderr_log_path.as_deref(), &body);
            let _ = write_json_artifact(
                run.response_artifact_path.as_deref(),
                &json!({
                    "run_id": run.id,
                    "recorded_at": Utc::now(),
                    "ok": false,
                    "status": status.as_u16(),
                    "provider": {
                        "name": provider.name,
                        "model": provider.model,
                    },
                    "body": body,
                }),
            );
            return AttemptOutcome::failed(format!("llm_chat HTTP {}", status.as_u16()));
        }

        let mut response = response;
        let mut stream = BufReader::new(&mut response);
        let mut line = String::new();
        let mut content = String::new();
        let mut saw_done = false;

        let _ = persist_streaming_transcript(
            run.transcript_artifact_path.as_deref(),
            &messages,
            &content,
            request_messages.len(),
            false,
        );
        let _ = write_log_line(run.stdout_log_path.as_deref(), "response:");

        loop {
            line.clear();
            match stream.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    let _ = write_log_line(
                        run.stderr_log_path.as_deref(),
                        &format!("failed to read streaming response: {error}"),
                    );
                    return AttemptOutcome::failed(format!(
                        "failed to read llm_chat streaming response: {error}"
                    ));
                }
            }

            let trimmed = line.trim();
            if trimmed.is_empty() || !trimmed.starts_with("data:") {
                continue;
            }

            let data = trimmed.trim_start_matches("data:").trim();
            if data == "[DONE]" {
                saw_done = true;
                break;
            }

            let Some(delta) = parse_stream_delta(data) else {
                continue;
            };

            content.push_str(&delta);
            print!("{delta}");
            let _ = std::io::stdout().flush();
            let _ = append_text(run.stdout_log_path.as_deref(), &delta);
            let _ = persist_streaming_transcript(
                run.transcript_artifact_path.as_deref(),
                &messages,
                &content,
                request_messages.len(),
                false,
            );
        }

        if !content.is_empty() {
            println!();
        }

        messages.push(ChatMessage {
            role: String::from("assistant"),
            content: content.clone(),
        });
        let _ = write_json_artifact(
            run.response_artifact_path.as_deref(),
            &json!({
                "run_id": run.id,
                "recorded_at": Utc::now(),
                "ok": true,
                "status": status.as_u16(),
                "provider": {
                    "name": provider.name,
                    "model": provider.model,
                },
                "streaming": true,
                "stream_completed": saw_done,
                "content": content,
            }),
        );
        let _ = persist_streaming_transcript(
            run.transcript_artifact_path.as_deref(),
            &messages,
            "",
            request_messages.len(),
            saw_done,
        );

        AttemptOutcome::succeeded(format!("llm_chat streamed: {} chars", content.len()))
    }
}

fn read_response_body(mut response: reqwest::blocking::Response) -> std::io::Result<String> {
    let mut body = String::new();
    response
        .read_to_string(&mut body)
        .map_err(std::io::Error::other)?;
    Ok(body)
}

fn parse_stream_delta(data: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()?
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .map(str::to_string)
}

fn persist_streaming_transcript(
    path: Option<&str>,
    messages: &[ChatMessage],
    assistant_content: &str,
    sent_context_messages: usize,
    stream_completed: bool,
) -> std::io::Result<()> {
    let mut persisted_messages = messages.to_vec();
    if stream_completed {
        if persisted_messages
            .last()
            .map(|message| message.role.as_str())
            != Some("assistant")
        {
            persisted_messages.push(ChatMessage {
                role: String::from("assistant"),
                content: assistant_content.to_string(),
            });
        }
    } else {
        persisted_messages.push(ChatMessage {
            role: String::from("assistant"),
            content: assistant_content.to_string(),
        });
    }

    write_json_artifact(
        path,
        &json!({
            "recorded_at": Utc::now(),
            "messages": persisted_messages,
            "sent_context_messages": sent_context_messages,
            "stored_transcript_messages": persisted_messages.len(),
            "streaming": true,
            "stream_completed": stream_completed,
        }),
    )
}

fn write_json_artifact(path: Option<&str>, value: &serde_json::Value) -> std::io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(std::io::Error::other)?;
    writeln!(file)
}

fn load_prior_chat_messages(
    step: &StepRecord,
    current_transcript_path: Option<&str>,
) -> Vec<ChatMessage> {
    let Some(previous_run_id) = step.attempts.last().and_then(|attempt| attempt.run_id) else {
        return Vec::new();
    };
    let Some(current_transcript_path) = current_transcript_path else {
        return Vec::new();
    };
    let Some(parent) = Path::new(current_transcript_path).parent() else {
        return Vec::new();
    };
    let previous_path = parent.join(format!("{previous_run_id}.transcript.json"));
    let Ok(raw) = std::fs::read_to_string(previous_path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    parsed
        .get("messages")
        .and_then(|value| serde_json::from_value::<Vec<ChatMessage>>(value.clone()).ok())
        .unwrap_or_default()
}

fn trim_chat_messages(messages: &[ChatMessage], max_messages: usize) -> Vec<ChatMessage> {
    if messages.len() <= max_messages {
        return messages.to_vec();
    }
    messages[messages.len().saturating_sub(max_messages)..].to_vec()
}

impl StepRunner for ExecutorBridge {
    fn run_mode(&self) -> RunMode {
        if self.options.background {
            RunMode::Background
        } else {
            RunMode::Foreground
        }
    }

    fn run_step(
        &mut self,
        session: &SessionRecord,
        _task: &TaskRecord,
        step: &StepRecord,
        run: &RunRecord,
        _attempt_number: u32,
    ) -> AttemptOutcome {
        if let Some(command) = self.command_for_step(step) {
            self.run_command(command, run, session.policy.timeout.step_timeout_secs)
        } else if let Some(prompt) = self.prompt_for_step(step) {
            self.run_llm_chat(step, prompt, run, session.policy.timeout.step_timeout_secs)
        } else {
            AttemptOutcome::succeeded(format!(
                "no executor mapping for step '{}', treated as operator noop",
                step.title
            ))
        }
    }
}

fn write_log_line(path: Option<&str>, line: &str) -> std::io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

fn append_text(path: Option<&str>, text: &str) -> std::io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    write!(file, "{text}")
}

fn cancel_requested(cancel_path: Option<&Path>) -> bool {
    cancel_path.is_some_and(Path::exists)
}

fn spawn_command(
    command: &str,
    run: &RunRecord,
    options: &ExecutorOptions,
    stdout_file: Option<File>,
    stderr_file: Option<File>,
) -> std::io::Result<Child> {
    let wrapped_command = if options.background {
        wrap_background_command(command, run)
    } else {
        command.to_string()
    };

    let mut cmd = if options.use_pty {
        pty_shell_command(&wrapped_command)
    } else {
        shell_command(&wrapped_command)
    };

    if options.stream_output {
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    } else {
        cmd.stdout(match stdout_file {
            Some(file) => Stdio::from(file),
            None => Stdio::piped(),
        })
        .stderr(match stderr_file {
            Some(file) => Stdio::from(file),
            None => Stdio::piped(),
        });
    }

    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(error) if options.use_pty => {
            let mut fallback = shell_command(&wrapped_command);
            if options.stream_output {
                fallback
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
            } else {
                fallback.stdout(Stdio::null()).stderr(Stdio::null());
            }
            fallback.spawn().map_err(|_| error)
        }
        Err(error) => Err(error),
    }
}

fn wrap_background_command(command: &str, run: &RunRecord) -> String {
    let exit_code_path = run
        .exit_code_path
        .as_deref()
        .unwrap_or("/tmp/recode.exit-code");
    format!(
        "{{ {command}; rc=$?; printf '%s\n' \"$rc\" > '{}' ; exit 0; }}",
        shell_single_quote(exit_code_path)
    )
}

fn shell_single_quote(input: &str) -> String {
    input.replace('\'', "'\\''")
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.args(["-lc", command]);
    cmd
}

#[cfg(windows)]
fn pty_shell_command(command: &str) -> Command {
    shell_command(command)
}

#[cfg(not(windows))]
fn pty_shell_command(command: &str) -> Command {
    let mut cmd = Command::new("script");
    cmd.args(["-qfec", command, "/dev/null"]);
    cmd
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{ExecutorBridge, ExecutorOptions};
    use crate::{
        ApprovalPolicy, ExecutionPolicy, RetryPolicy, RunMode, RunRecord, SessionRecord,
        StepRecord, StepRunner, TaskRecord, TimeoutPolicy,
    };

    fn session_with_timeout(timeout_secs: u64) -> SessionRecord {
        SessionRecord::new_with_policy(
            "demo",
            ExecutionPolicy {
                retry: RetryPolicy { max_attempts: 1 },
                timeout: TimeoutPolicy {
                    step_timeout_secs: timeout_secs,
                },
                approval: ApprovalPolicy::Never,
            },
        )
    }

    fn run_record(mode: RunMode, temp: &std::path::Path) -> RunRecord {
        let mut run = RunRecord::new(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), 1, mode);
        run.stdout_log_path = Some(temp.join("stdout.log").display().to_string());
        run.stderr_log_path = Some(temp.join("stderr.log").display().to_string());
        run.request_artifact_path = Some(temp.join("request.json").display().to_string());
        run.response_artifact_path = Some(temp.join("response.json").display().to_string());
        run.transcript_artifact_path = Some(temp.join("transcript.json").display().to_string());
        run
    }

    #[test]
    fn non_command_step_is_a_noop_success() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("plan"),
            &run_record(RunMode::Foreground, temp.path()),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Succeeded);
        assert!(
            outcome
                .summary
                .unwrap()
                .contains("no executor mapping for step 'plan'")
        );
    }

    #[test]
    fn command_step_runs_through_shell_bridge() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::new();
        let run = run_record(RunMode::Foreground, temp.path());
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: printf bridge-ok"),
            &run,
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Succeeded);
        assert!(outcome.summary.unwrap().contains("command succeeded"));
        assert!(std::path::Path::new(run.stdout_log_path.as_deref().unwrap()).exists());
    }

    #[test]
    fn failing_command_surfaces_as_failed_attempt() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: exit 7"),
            &run_record(RunMode::Foreground, temp.path()),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Failed);
    }

    #[test]
    fn long_running_command_times_out() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(1),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: sleep 2"),
            &run_record(RunMode::Foreground, temp.path()),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::TimedOut);
        assert!(outcome.summary.unwrap().contains("timed out"));
    }

    #[test]
    fn streaming_mode_returns_generic_success_summary() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            stream_output: true,
            ..ExecutorOptions::default()
        });
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: printf stream-ok"),
            &run_record(RunMode::Foreground, temp.path()),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Succeeded);
        assert_eq!(
            outcome.summary.unwrap(),
            "command succeeded: printf stream-ok"
        );
    }

    #[test]
    fn cancel_file_cancels_running_command() {
        let temp = tempdir().unwrap();
        let cancel_path = temp.path().join("cancel.signal");
        fs::write(&cancel_path, "stop").unwrap();

        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            cancel_path: Some(cancel_path.clone()),
            ..ExecutorOptions::default()
        });
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: sleep 2"),
            &run_record(RunMode::Foreground, temp.path()),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Cancelled);
        assert!(
            outcome
                .summary
                .unwrap()
                .contains(cancel_path.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn background_mode_returns_running_immediately() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            background: true,
            ..ExecutorOptions::default()
        });
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: sleep 2"),
            &run_record(RunMode::Background, temp.path()),
            1,
        );

        assert_eq!(runner.run_mode(), RunMode::Background);
        assert_eq!(outcome.status, crate::AttemptStatus::Running);
        assert!(outcome.pid.is_some());
    }

    #[test]
    fn llm_chat_missing_api_key_still_persists_request_artifact() {
        let temp = tempdir().unwrap();
        let mut runner = ExecutorBridge::with_options(ExecutorOptions::default());
        let run = run_record(RunMode::Foreground, temp.path());
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::llm_chat("ask", "hello"),
            &run,
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Failed);
        assert!(std::path::Path::new(run.request_artifact_path.as_deref().unwrap()).exists());
    }

    #[test]
    fn loads_prior_transcript_messages_for_followup_turn() {
        let temp = tempdir().unwrap();
        let previous_run_id = Uuid::new_v4();
        let transcript_path = temp
            .path()
            .join(format!("{previous_run_id}.transcript.json"));
        fs::write(
            &transcript_path,
            serde_json::to_string_pretty(&json!({
                "messages": [
                    {"role": "user", "content": "hello"},
                    {"role": "assistant", "content": "hi"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut step = StepRecord::llm_chat("ask", "follow up");
        step.attempts.push(crate::AttemptRecord {
            id: Uuid::new_v4(),
            run_id: Some(previous_run_id),
            number: 1,
            status: crate::AttemptStatus::Succeeded,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            summary: Some(String::from("done")),
        });

        let current_path = temp.path().join("current.transcript.json");
        let messages = super::load_prior_chat_messages(&step, current_path.to_str());

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[test]
    fn trims_chat_messages_to_recent_window() {
        let messages = (0..20)
            .map(|index| super::ChatMessage {
                role: if index % 2 == 0 {
                    String::from("user")
                } else {
                    String::from("assistant")
                },
                content: format!("m{index}"),
            })
            .collect::<Vec<_>>();

        let trimmed = super::trim_chat_messages(&messages, 12);

        assert_eq!(trimmed.len(), 12);
        assert_eq!(trimmed.first().unwrap().content, "m8");
        assert_eq!(trimmed.last().unwrap().content, "m19");
    }

    #[test]
    fn streaming_transcript_persists_partial_assistant_message() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("stream.transcript.json");
        let messages = vec![super::ChatMessage {
            role: String::from("user"),
            content: String::from("hello"),
        }];

        super::persist_streaming_transcript(path.to_str(), &messages, "par", 1, false).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let stored = parsed.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(
            stored[1].get("role").and_then(|v| v.as_str()),
            Some("assistant")
        );
        assert_eq!(
            stored[1].get("content").and_then(|v| v.as_str()),
            Some("par")
        );
        assert_eq!(
            parsed.get("stream_completed").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn completed_streaming_transcript_does_not_duplicate_assistant_message() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("stream-complete.transcript.json");
        let messages = vec![
            super::ChatMessage {
                role: String::from("user"),
                content: String::from("hello"),
            },
            super::ChatMessage {
                role: String::from("assistant"),
                content: String::from("done"),
            },
        ];

        super::persist_streaming_transcript(path.to_str(), &messages, "", 2, true).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let stored = parsed.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(
            stored[1].get("content").and_then(|v| v.as_str()),
            Some("done")
        );
        assert_eq!(
            parsed.get("stream_completed").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn parses_stream_delta_content() {
        let delta = super::parse_stream_delta(r#"{"choices":[{"delta":{"content":"hello"}}]}"#);

        assert_eq!(delta.as_deref(), Some("hello"));
    }

    #[test]
    fn ignores_stream_chunks_without_content_delta() {
        let delta = super::parse_stream_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#);

        assert!(delta.is_none());
    }
}
