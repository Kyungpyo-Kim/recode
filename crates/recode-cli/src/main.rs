use std::env;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::{Args, Parser, Subcommand};
use recode_core::{
    AttemptOutcome, ConfigLoader, ExecutionPolicy, ExecutorBridge, ExecutorOptions, PartialConfig,
    RetryPolicy, SessionStore, StepRunner, StepSpec, TimeoutPolicy, WorkflowEngine,
};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "recode", version, about = "Recode CLI")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    log_level: Option<String>,
    #[arg(long, global = true)]
    default_provider: Option<String>,
    #[arg(long, global = true)]
    provider_mode: Option<String>,
    #[arg(long, global = true)]
    provider_base_url: Option<String>,
    #[arg(long, global = true)]
    provider_api_key_env: Option<String>,
    #[arg(long, global = true)]
    provider_model: Option<String>,
    #[arg(long, global = true)]
    default_timeout_secs: Option<u64>,
    #[arg(long, global = true)]
    default_max_attempts: Option<u32>,
    #[arg(long, global = true)]
    approval_policy: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
    Config,
    Session(SessionCommand),
    Task(TaskCommand),
    Run(RunCommand),
}

#[derive(Debug, Args)]
struct RunCommand {
    #[command(subcommand)]
    action: RunAction,
}

#[derive(Debug, Subcommand)]
enum RunAction {
    List,
    Inspect {
        #[arg(long)]
        id: String,
    },
    Reconcile {
        #[arg(long)]
        id: String,
    },
    Cancel {
        #[arg(long)]
        id: String,
    },
}

#[derive(Debug, Args)]
struct SessionCommand {
    #[command(subcommand)]
    action: SessionAction,
}

#[derive(Debug, Subcommand)]
enum SessionAction {
    Init {
        #[arg(long)]
        name: String,
        #[arg(long)]
        max_attempts: Option<u32>,
        #[arg(long)]
        step_timeout_secs: Option<u64>,
    },
    Inspect {
        #[arg(long)]
        id: String,
    },
    RunAll {
        #[arg(long)]
        id: String,
        #[command(flatten)]
        execution: ExecutionArgs,
    },
}

#[derive(Debug, Args)]
struct TaskCommand {
    #[command(subcommand)]
    action: TaskAction,
}

#[derive(Debug, Subcommand)]
enum TaskAction {
    Create {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        title: String,
        #[arg(
            long = "step",
            help = "Legacy step text. cmd:/shell:/exec: prefixes become explicit shell steps; plain text becomes operator steps."
        )]
        steps: Vec<String>,
        #[arg(
            long = "approval-step",
            help = "Legacy approval-gated step text. cmd:/shell:/exec: prefixes become explicit shell steps gated by approval."
        )]
        approval_steps: Vec<String>,
        #[arg(
            long = "shell-step",
            help = "Explicit shell step in the form <title>::<command>."
        )]
        shell_steps: Vec<String>,
        #[arg(long = "operator-step", help = "Explicit operator/noop step title.")]
        operator_steps: Vec<String>,
        #[arg(
            long = "chat-step",
            help = "Explicit llm_chat step in the form <title>::<prompt>."
        )]
        chat_steps: Vec<String>,
    },
    RunNext {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        task_id: Option<String>,
        #[command(flatten)]
        execution: ExecutionArgs,
    },
    ApproveStep {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        step_id: String,
    },
}

#[derive(Debug, Args, Clone, Default)]
struct ExecutionArgs {
    #[arg(long)]
    outcome: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    stream: bool,
    #[arg(long)]
    pty: bool,
    #[arg(long)]
    cancel_file: Option<PathBuf>,
    #[arg(long)]
    background: bool,
}

enum RunnerMode {
    Bridge(ExecutorBridge),
    Manual(ManualStepRunner),
}

struct ManualStepRunner {
    outcome: AttemptOutcome,
}

impl StepRunner for ManualStepRunner {
    fn run_step(
        &mut self,
        _session: &recode_core::SessionRecord,
        _task: &recode_core::TaskRecord,
        _step: &recode_core::StepRecord,
        _run: &recode_core::RunRecord,
        _attempt_number: u32,
    ) -> AttemptOutcome {
        self.outcome.clone()
    }
}

impl StepRunner for RunnerMode {
    fn run_step(
        &mut self,
        session: &recode_core::SessionRecord,
        task: &recode_core::TaskRecord,
        step: &recode_core::StepRecord,
        run: &recode_core::RunRecord,
        attempt_number: u32,
    ) -> AttemptOutcome {
        match self {
            RunnerMode::Bridge(runner) => runner.run_step(session, task, step, run, attempt_number),
            RunnerMode::Manual(runner) => runner.run_step(session, task, step, run, attempt_number),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        let payload = json!({
            "ok": false,
            "error": error.to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir()?;
    let config_loader = ConfigLoader::new(cwd);
    let config = config_loader.load(cli.config.clone(), cli_partial(&cli))?;
    let store = SessionStore::new(config.state_dir.clone());
    let engine = WorkflowEngine::new(store.clone());

    let payload = match cli.command {
        Command::Version => json!({
            "ok": true,
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "config": config,
        }),
        Command::Config => json!({
            "ok": true,
            "config": config,
        }),
        Command::Session(command) => match command.action {
            SessionAction::Init {
                name,
                max_attempts,
                step_timeout_secs,
            } => {
                let policy = ExecutionPolicy {
                    retry: RetryPolicy {
                        max_attempts: max_attempts.unwrap_or(config.default_max_attempts),
                    },
                    timeout: TimeoutPolicy {
                        step_timeout_secs: step_timeout_secs.unwrap_or(config.default_timeout_secs),
                    },
                    approval: config.approval_policy,
                };
                let session = store.init_session_with_policy(name, policy)?;
                json!({
                    "ok": true,
                    "config": config,
                    "session": session,
                })
            }
            SessionAction::Inspect { id } => {
                let session = store.load_session(id.parse()?)?;
                json!({
                    "ok": true,
                    "config": config,
                    "session": session,
                })
            }
            SessionAction::RunAll { id, execution } => {
                let mut runner = runner_for(execution, config.provider.clone());
                let result = engine.run_all(id.parse()?, &mut runner)?;
                json!({
                    "ok": true,
                    "config": config,
                    "execution_mode": runner_mode_name(&runner),
                    "run_count": result.runs.len(),
                    "runs": result.runs.into_iter().map(json_for_run_step_result).collect::<Vec<_>>(),
                    "session": result.session,
                })
            }
        },
        Command::Task(command) => match command.action {
            TaskAction::Create {
                session_id,
                title,
                steps,
                approval_steps,
                shell_steps,
                operator_steps,
                chat_steps,
            } => {
                let step_specs = collect_step_specs(
                    steps,
                    approval_steps,
                    shell_steps,
                    operator_steps,
                    chat_steps,
                )?;
                let session =
                    engine.create_task_with_steps(session_id.parse()?, title, step_specs)?;
                let task = session
                    .tasks
                    .last()
                    .cloned()
                    .ok_or_else(|| anyhow!("task was not persisted"))?;
                json!({
                    "ok": true,
                    "config": config,
                    "task": task,
                    "session": session,
                })
            }
            TaskAction::RunNext {
                session_id,
                task_id,
                execution,
            } => {
                let mut runner = runner_for(execution, config.provider.clone());
                let result = match task_id {
                    Some(task_id) => engine.run_task_next_step(
                        session_id.parse()?,
                        task_id.parse()?,
                        &mut runner,
                    )?,
                    None => engine.run_next_step(session_id.parse()?, &mut runner)?,
                };
                json!({
                    "ok": true,
                    "config": config,
                    "execution_mode": runner_mode_name(&runner),
                    "result": json_for_run_step_result(result),
                })
            }
            TaskAction::ApproveStep {
                session_id,
                task_id,
                step_id,
            } => {
                let session =
                    engine.approve_step(session_id.parse()?, task_id.parse()?, step_id.parse()?)?;
                json!({
                    "ok": true,
                    "config": config,
                    "session": session,
                })
            }
        },
        Command::Run(command) => match command.action {
            RunAction::List => {
                let runs = store.list_runs()?;
                json!({
                    "ok": true,
                    "config": config,
                    "runs": runs,
                })
            }
            RunAction::Inspect { id } => {
                let run = store.load_run(id.parse()?)?;
                json!({
                    "ok": true,
                    "config": config,
                    "run": run,
                })
            }
            RunAction::Reconcile { id } => {
                let run = engine.reconcile_run(id.parse()?)?;
                json!({
                    "ok": true,
                    "config": config,
                    "run": run,
                })
            }
            RunAction::Cancel { id } => {
                let run = engine.cancel_run(id.parse()?)?;
                json!({
                    "ok": true,
                    "config": config,
                    "run": run,
                    "cancel_request_path": store.cancel_request_path(run.id),
                })
            }
        },
    };

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn runner_for(execution: ExecutionArgs, provider: recode_core::ProviderConfig) -> RunnerMode {
    match execution.outcome {
        Some(outcome) => RunnerMode::Manual(ManualStepRunner {
            outcome: parse_outcome(&outcome, execution.summary),
        }),
        None => RunnerMode::Bridge(ExecutorBridge::with_options(ExecutorOptions {
            stream_output: execution.stream,
            use_pty: execution.pty,
            cancel_path: execution.cancel_file,
            background: execution.background,
            provider: Some(provider),
        })),
    }
}

fn runner_mode_name(runner: &RunnerMode) -> &'static str {
    match runner {
        RunnerMode::Bridge(_) => "executor_bridge",
        RunnerMode::Manual(_) => "manual_outcome",
    }
}

fn cli_partial(cli: &Cli) -> PartialConfig {
    PartialConfig {
        state_dir: cli.state_dir.clone(),
        log_level: cli.log_level.clone(),
        default_provider: cli.default_provider.clone(),
        provider_mode: cli
            .provider_mode
            .as_deref()
            .and_then(recode_core::ProviderMode::parse),
        provider_base_url: cli.provider_base_url.clone(),
        provider_api_key_env: cli.provider_api_key_env.clone(),
        provider_model: cli.provider_model.clone(),
        default_timeout_secs: cli.default_timeout_secs,
        default_max_attempts: cli.default_max_attempts,
        approval_policy: cli
            .approval_policy
            .as_deref()
            .and_then(recode_core::ApprovalPolicy::parse),
    }
}

fn collect_step_specs(
    steps: Vec<String>,
    approval_steps: Vec<String>,
    shell_steps: Vec<String>,
    operator_steps: Vec<String>,
    chat_steps: Vec<String>,
) -> Result<Vec<StepSpec>> {
    let mut specs: Vec<StepSpec> = steps.into_iter().map(StepSpec::new).collect();
    specs.extend(approval_steps.into_iter().map(StepSpec::requires_approval));

    for raw in shell_steps {
        let (title, command) = split_titled_value(&raw, "shell-step")?;
        specs.push(StepSpec::shell(title, command));
    }

    for title in operator_steps {
        specs.push(StepSpec::operator(title));
    }

    for raw in chat_steps {
        let (title, prompt) = split_titled_value(&raw, "chat-step")?;
        specs.push(StepSpec::llm_chat(title, prompt));
    }

    if specs.is_empty() {
        return Err(anyhow!("at least one step flag must be provided"));
    }

    Ok(specs)
}

fn split_titled_value(raw: &str, flag_name: &str) -> Result<(String, String)> {
    let Some((title, value)) = raw.split_once("::") else {
        return Err(anyhow!("--{flag_name} must use the form <title>::<value>"));
    };
    let title = title.trim();
    let value = value.trim();
    if title.is_empty() || value.is_empty() {
        return Err(anyhow!(
            "--{flag_name} requires non-empty <title> and <value>"
        ));
    }
    Ok((title.to_string(), value.to_string()))
}

fn parse_outcome(outcome: &str, summary: Option<String>) -> AttemptOutcome {
    match outcome.trim().to_ascii_lowercase().as_str() {
        "success" | "succeeded" | "ok" => {
            AttemptOutcome::succeeded(summary.unwrap_or_else(|| "step completed".into()))
        }
        "timeout" | "timed_out" | "timed-out" => {
            AttemptOutcome::timed_out(summary.unwrap_or_else(|| "step timed out".into()))
        }
        "cancel" | "cancelled" | "canceled" => AttemptOutcome {
            status: recode_core::AttemptStatus::Cancelled,
            summary: Some(summary.unwrap_or_else(|| "step cancelled".into())),
            pid: None,
        },
        "fail" | "failed" | "error" => {
            AttemptOutcome::failed(summary.unwrap_or_else(|| "step failed".into()))
        }
        _ => AttemptOutcome::failed(summary.unwrap_or_else(|| "invalid CLI outcome".into())),
    }
}

fn json_for_run_step_result(result: recode_core::RunStepResult) -> serde_json::Value {
    json!({
        "task_id": result.task_id,
        "step_id": result.step_id,
        "step_title": result.step_title,
        "run_id": result.run_id,
        "run_pid": result.run_pid,
        "stdout_log_path": result.stdout_log_path,
        "stderr_log_path": result.stderr_log_path,
        "request_artifact_path": result.request_artifact_path,
        "response_artifact_path": result.response_artifact_path,
        "transcript_artifact_path": result.transcript_artifact_path,
        "llm_provider": result.llm_provider,
        "llm_model": result.llm_model,
        "llm_prompt_tokens": result.llm_prompt_tokens,
        "llm_completion_tokens": result.llm_completion_tokens,
        "llm_total_tokens": result.llm_total_tokens,
        "disposition": result.disposition,
        "attempt_number": result.attempt_number,
        "attempt_status": result.attempt_status,
        "max_attempts": result.max_attempts,
        "retryable": result.retryable,
        "session": result.session,
    })
}
