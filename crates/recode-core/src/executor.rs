use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::engine::{AttemptOutcome, StepRunner};
use crate::model::{AttemptStatus, SessionRecord, StepRecord, TaskRecord};

const SHELL_PREFIXES: &[&str] = &["cmd:", "shell:", "exec:"];

#[derive(Debug, Clone, Default)]
pub struct ExecutorOptions {
    pub stream_output: bool,
    pub use_pty: bool,
    pub cancel_path: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct ExecutorBridge {
    options: ExecutorOptions,
}

impl ExecutorBridge {
    pub fn new() -> Self {
        Self::with_options(ExecutorOptions::default())
    }

    pub fn with_options(options: ExecutorOptions) -> Self {
        Self { options }
    }

    fn command_for_step<'a>(&self, step: &'a StepRecord) -> Option<&'a str> {
        let title = step.title.trim();
        SHELL_PREFIXES
            .iter()
            .find_map(|prefix| title.strip_prefix(prefix).map(str::trim))
            .filter(|command| !command.is_empty())
    }

    fn run_command(&self, command: &str, timeout_secs: u64) -> AttemptOutcome {
        let mut child = match spawn_command(command, &self.options) {
            Ok(child) => child,
            Err(error) => {
                return AttemptOutcome::failed(format!("failed to spawn command: {error}"));
            }
        };

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
                };
            }

            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return AttemptOutcome::timed_out(format!(
                    "command timed out after {}s: {}",
                    timeout.as_secs(),
                    command
                ));
            }

            match child.wait_timeout(poll) {
                Ok(Some(status)) => {
                    if self.options.stream_output {
                        return if status.success() {
                            AttemptOutcome::succeeded(format!("command succeeded: {command}"))
                        } else {
                            AttemptOutcome::failed(format!("command failed: {command}"))
                        };
                    }

                    return match child.wait_with_output() {
                        Ok(output) if output.status.success() => {
                            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            let summary = if stdout.is_empty() {
                                format!("command succeeded: {command}")
                            } else {
                                stdout
                            };
                            AttemptOutcome::succeeded(summary)
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            let summary = if stderr.is_empty() {
                                if stdout.is_empty() {
                                    format!("command failed: {command}")
                                } else {
                                    stdout
                                }
                            } else {
                                stderr
                            };
                            AttemptOutcome::failed(summary)
                        }
                        Err(error) => AttemptOutcome::failed(format!(
                            "failed to collect command output: {error}"
                        )),
                    };
                }
                Ok(None) => continue,
                Err(error) => {
                    return AttemptOutcome::failed(format!("failed to wait for command: {error}"));
                }
            }
        }
    }
}

impl StepRunner for ExecutorBridge {
    fn run_step(
        &mut self,
        session: &SessionRecord,
        _task: &TaskRecord,
        step: &StepRecord,
        _attempt_number: u32,
    ) -> AttemptOutcome {
        if let Some(command) = self.command_for_step(step) {
            self.run_command(command, session.policy.timeout.step_timeout_secs)
        } else {
            AttemptOutcome::succeeded(format!(
                "no executor mapping for step '{}', treated as operator noop",
                step.title
            ))
        }
    }
}

fn cancel_requested(cancel_path: Option<&Path>) -> bool {
    cancel_path.is_some_and(Path::exists)
}

fn spawn_command(command: &str, options: &ExecutorOptions) -> std::io::Result<Child> {
    let mut cmd = if options.use_pty {
        pty_shell_command(command)
    } else {
        shell_command(command)
    };

    if options.stream_output {
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }

    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(error) if options.use_pty => {
            let mut fallback = shell_command(command);
            if options.stream_output {
                fallback
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
            } else {
                fallback.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
            fallback.spawn().map_err(|_| error)
        }
        Err(error) => Err(error),
    }
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

    use tempfile::tempdir;

    use super::{ExecutorBridge, ExecutorOptions};
    use crate::{
        ApprovalPolicy, ExecutionPolicy, RetryPolicy, SessionRecord, StepRecord, StepRunner,
        TaskRecord, TimeoutPolicy,
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

    #[test]
    fn non_command_step_is_a_noop_success() {
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("plan"),
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
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: printf bridge-ok"),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Succeeded);
        assert_eq!(outcome.summary.unwrap(), "bridge-ok");
    }

    #[test]
    fn failing_command_surfaces_as_failed_attempt() {
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: exit 7"),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::Failed);
    }

    #[test]
    fn long_running_command_times_out() {
        let mut runner = ExecutorBridge::new();
        let outcome = runner.run_step(
            &session_with_timeout(1),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: sleep 2"),
            1,
        );

        assert_eq!(outcome.status, crate::AttemptStatus::TimedOut);
        assert!(outcome.summary.unwrap().contains("timed out"));
    }

    #[test]
    fn streaming_mode_returns_generic_success_summary() {
        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            stream_output: true,
            ..ExecutorOptions::default()
        });
        let outcome = runner.run_step(
            &session_with_timeout(5),
            &TaskRecord::new("task", vec![]),
            &StepRecord::new("cmd: printf stream-ok"),
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
}
