use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::engine::{AttemptOutcome, StepRunner};
use crate::model::{AttemptStatus, RunMode, RunRecord, SessionRecord, StepRecord, TaskRecord};

const SHELL_PREFIXES: &[&str] = &["cmd:", "shell:", "exec:"];

#[derive(Debug, Clone, Default)]
pub struct ExecutorOptions {
    pub stream_output: bool,
    pub use_pty: bool,
    pub cancel_path: Option<PathBuf>,
    pub background: bool,
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
}
