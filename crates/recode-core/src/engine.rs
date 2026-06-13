use chrono::Utc;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    ApprovalPolicy, AttemptRecord, AttemptStatus, RunMode, RunRecord, SessionRecord, SessionStatus,
    StepKind, StepRecord, StepStatus, TaskRecord, TaskStatus,
};
use crate::storage::SessionStore;

#[derive(Debug, Clone)]
pub struct AttemptOutcome {
    pub status: AttemptStatus,
    pub summary: Option<String>,
    pub pid: Option<u32>,
}

impl AttemptOutcome {
    pub fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::Succeeded,
            summary: Some(summary.into()),
            pid: None,
        }
    }

    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::Failed,
            summary: Some(summary.into()),
            pid: None,
        }
    }

    pub fn timed_out(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::TimedOut,
            summary: Some(summary.into()),
            pid: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StepSpec {
    pub title: String,
    pub kind: StepKind,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub requires_approval: bool,
}

impl StepSpec {
    pub fn new(title: impl Into<String>) -> Self {
        Self::from_legacy_text(title, false)
    }

    pub fn requires_approval(title: impl Into<String>) -> Self {
        Self::from_legacy_text(title, true)
    }

    pub fn shell(title: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: StepKind::Shell,
            command: Some(command.into()),
            prompt: None,
            requires_approval: false,
        }
    }

    pub fn llm_chat(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: StepKind::LlmChat,
            command: None,
            prompt: Some(prompt.into()),
            requires_approval: false,
        }
    }

    pub fn operator(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: StepKind::Operator,
            command: None,
            prompt: None,
            requires_approval: false,
        }
    }

    fn from_legacy_text(title: impl Into<String>, requires_approval: bool) -> Self {
        let title = title.into();
        let (kind, command) = StepKind::from_legacy_title(&title);
        Self {
            title,
            kind,
            command,
            prompt: None,
            requires_approval,
        }
    }
}

pub trait StepRunner {
    fn run_mode(&self) -> RunMode {
        RunMode::Foreground
    }

    fn run_step(
        &mut self,
        session: &SessionRecord,
        task: &TaskRecord,
        step: &StepRecord,
        run: &RunRecord,
        attempt_number: u32,
    ) -> AttemptOutcome;
}

impl AttemptOutcome {
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStepDisposition {
    Executed,
    WaitingApproval,
}

#[derive(Debug, Clone)]
pub struct RunStepResult {
    pub session: SessionRecord,
    pub task_id: Uuid,
    pub step_id: Uuid,
    pub step_title: String,
    pub run_id: Option<Uuid>,
    pub run_pid: Option<u32>,
    pub stdout_log_path: Option<String>,
    pub stderr_log_path: Option<String>,
    pub request_artifact_path: Option<String>,
    pub response_artifact_path: Option<String>,
    pub transcript_artifact_path: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub llm_prompt_tokens: Option<u64>,
    pub llm_completion_tokens: Option<u64>,
    pub llm_total_tokens: Option<u64>,
    pub disposition: RunStepDisposition,
    pub attempt_number: Option<u32>,
    pub attempt_status: Option<AttemptStatus>,
    pub max_attempts: u32,
    pub retryable: bool,
}

#[derive(Debug, Clone)]
pub struct RunAllResult {
    pub session: SessionRecord,
    pub runs: Vec<RunStepResult>,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("session contains no runnable task")]
    NoRunnableTask,
    #[error("task has no runnable step")]
    NoRunnableStep,
    #[error("task not found: {0}")]
    TaskNotFound(Uuid),
    #[error("step not found: {0}")]
    StepNotFound(Uuid),
    #[error("session is waiting for approval")]
    ApprovalRequired,
}

#[derive(Debug, Clone)]
pub struct WorkflowEngine {
    store: SessionStore,
}

#[derive(Debug, Default, Clone)]
struct LlmResponseSummary {
    provider: Option<String>,
    model: Option<String>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl WorkflowEngine {
    pub fn new(store: SessionStore) -> Self {
        Self { store }
    }

    pub fn create_task(
        &self,
        session_id: Uuid,
        title: impl Into<String>,
        step_titles: Vec<String>,
    ) -> anyhow::Result<SessionRecord> {
        self.create_task_with_steps(
            session_id,
            title,
            step_titles.into_iter().map(StepSpec::new).collect(),
        )
    }

    pub fn create_task_with_steps(
        &self,
        session_id: Uuid,
        title: impl Into<String>,
        steps: Vec<StepSpec>,
    ) -> anyhow::Result<SessionRecord> {
        let mut session = self.store.load_session(session_id)?;
        let task = TaskRecord::new(
            title,
            steps
                .into_iter()
                .map(|step| {
                    StepRecord::new_with_kind(
                        step.title,
                        step.kind,
                        step.command,
                        step.prompt,
                        step.requires_approval,
                    )
                })
                .collect(),
        );
        session.tasks.push(task);
        session.touch();
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn approve_step(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        step_id: Uuid,
    ) -> anyhow::Result<SessionRecord> {
        let mut session = self.store.load_session(session_id)?;
        let Some(task_index) = session.tasks.iter().position(|task| task.id == task_id) else {
            return Err(EngineError::TaskNotFound(task_id).into());
        };
        let Some(step_index) = session.tasks[task_index]
            .steps
            .iter()
            .position(|step| step.id == step_id)
        else {
            return Err(EngineError::StepNotFound(step_id).into());
        };

        {
            let task = &mut session.tasks[task_index];
            let step = &mut task.steps[step_index];
            step.approval_granted = true;
            if step.status == StepStatus::WaitingApproval {
                step.status = StepStatus::Planned;
            }

            if task.status == TaskStatus::WaitingApproval {
                task.status = TaskStatus::Planned;
            }
        }

        refresh_session_status(&mut session);
        session.touch();
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn run_next_step<R: StepRunner>(
        &self,
        session_id: Uuid,
        runner: &mut R,
    ) -> anyhow::Result<RunStepResult> {
        let session = self.store.load_session(session_id)?;
        if session_waiting_approval(&session) {
            return Err(EngineError::ApprovalRequired.into());
        }
        let Some(task_index) = session.tasks.iter().position(task_has_runnable_step) else {
            return Err(EngineError::NoRunnableTask.into());
        };

        self.run_next_step_at_index(session, task_index, runner)
    }

    pub fn run_task_next_step<R: StepRunner>(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        runner: &mut R,
    ) -> anyhow::Result<RunStepResult> {
        let session = self.store.load_session(session_id)?;
        let Some(task_index) = session.tasks.iter().position(|task| task.id == task_id) else {
            return Err(EngineError::TaskNotFound(task_id).into());
        };
        if session.tasks[task_index].status == TaskStatus::WaitingApproval {
            return Err(EngineError::ApprovalRequired.into());
        }
        if !task_has_runnable_step(&session.tasks[task_index]) {
            return Err(EngineError::NoRunnableStep.into());
        }

        self.run_next_step_at_index(session, task_index, runner)
    }

    pub fn run_all<R: StepRunner>(
        &self,
        session_id: Uuid,
        runner: &mut R,
    ) -> anyhow::Result<RunAllResult> {
        let mut runs = Vec::new();

        loop {
            match self.run_next_step(session_id, runner) {
                Ok(result) => runs.push(result),
                Err(error) => {
                    if matches!(
                        error.downcast_ref::<EngineError>(),
                        Some(EngineError::NoRunnableTask | EngineError::ApprovalRequired)
                    ) {
                        let session = self.store.load_session(session_id)?;
                        return Ok(RunAllResult { session, runs });
                    }
                    return Err(error);
                }
            }
        }
    }

    fn run_next_step_at_index<R: StepRunner>(
        &self,
        mut session: SessionRecord,
        task_index: usize,
        runner: &mut R,
    ) -> anyhow::Result<RunStepResult> {
        session.status = SessionStatus::Running;

        let task_snapshot = session.tasks[task_index].clone();
        let Some(step_index) = task_snapshot.steps.iter().position(step_is_runnable) else {
            return Err(EngineError::NoRunnableStep.into());
        };
        let step_snapshot = task_snapshot.steps[step_index].clone();

        if step_requires_manual_approval(&session, &step_snapshot) {
            let task_id = task_snapshot.id;
            let step_id = step_snapshot.id;
            let step_title = step_snapshot.title.clone();
            let max_attempts = session.policy.retry.max_attempts.max(1);
            {
                let task = &mut session.tasks[task_index];
                task.status = TaskStatus::WaitingApproval;
                task.touch();
                let step = &mut task.steps[step_index];
                step.status = StepStatus::WaitingApproval;
            }
            session.status = SessionStatus::WaitingApproval;
            session.touch();
            self.store.save_session(&session)?;
            return Ok(RunStepResult {
                session,
                task_id,
                step_id,
                step_title,
                run_id: None,
                run_pid: None,
                stdout_log_path: None,
                stderr_log_path: None,
                request_artifact_path: None,
                response_artifact_path: None,
                transcript_artifact_path: None,
                llm_provider: None,
                llm_model: None,
                llm_prompt_tokens: None,
                llm_completion_tokens: None,
                llm_total_tokens: None,
                disposition: RunStepDisposition::WaitingApproval,
                attempt_number: None,
                attempt_status: None,
                max_attempts,
                retryable: false,
            });
        }

        let attempt_number = step_snapshot.attempts.len() as u32 + 1;
        let max_attempts = session.policy.retry.max_attempts.max(1);
        let mut run = RunRecord::new(
            session.id,
            task_snapshot.id,
            step_snapshot.id,
            attempt_number,
            runner.run_mode(),
        );
        run.stdout_log_path = Some(self.store.stdout_log_path(run.id).display().to_string());
        run.stderr_log_path = Some(self.store.stderr_log_path(run.id).display().to_string());
        run.exit_code_path = Some(self.store.exit_code_path(run.id).display().to_string());
        run.request_artifact_path = Some(
            self.store
                .request_artifact_path(run.id)
                .display()
                .to_string(),
        );
        run.response_artifact_path = Some(
            self.store
                .response_artifact_path(run.id)
                .display()
                .to_string(),
        );
        run.transcript_artifact_path = Some(
            self.store
                .transcript_artifact_path(run.id)
                .display()
                .to_string(),
        );
        self.store.save_run(&run)?;
        let outcome = runner.run_step(
            &session,
            &task_snapshot,
            &step_snapshot,
            &run,
            attempt_number,
        );
        let now = Utc::now();
        let retryable = is_retryable(outcome.status, attempt_number, max_attempts);
        let requires_failure_approval = outcome_requires_failure_approval(&session, outcome.status);
        run.status = outcome.status.into();
        run.finished_at = match outcome.status {
            AttemptStatus::Running => None,
            _ => Some(now),
        };
        run.pid = outcome.pid;
        run.summary = outcome.summary.clone();
        self.store.save_run(&run)?;

        let task_failed;
        {
            let task = &mut session.tasks[task_index];
            task.status = TaskStatus::Running;
            task.touch();

            let step = &mut task.steps[step_index];
            step.status = StepStatus::Running;
            let attempt = AttemptRecord {
                id: Uuid::new_v4(),
                run_id: Some(run.id),
                number: attempt_number,
                status: outcome.status,
                started_at: run.started_at,
                finished_at: run.finished_at,
                summary: outcome.summary,
            };
            step.attempts.push(attempt);
            step.status = match outcome.status {
                AttemptStatus::Succeeded => StepStatus::Completed,
                AttemptStatus::Failed | AttemptStatus::TimedOut | AttemptStatus::Cancelled => {
                    if requires_failure_approval {
                        step.approval_granted = false;
                        StepStatus::WaitingApproval
                    } else if retryable {
                        StepStatus::Planned
                    } else {
                        StepStatus::Failed
                    }
                }
                AttemptStatus::Running => StepStatus::Running,
            };

            let all_steps_completed = task
                .steps
                .iter()
                .all(|step| step.status == StepStatus::Completed);
            task_failed = task.steps[step_index].status == StepStatus::Failed;

            if all_steps_completed {
                task.status = TaskStatus::Completed;
            } else if task.steps[step_index].status == StepStatus::WaitingApproval {
                task.status = TaskStatus::WaitingApproval;
            } else if task_failed {
                task.status = TaskStatus::Failed;
            } else {
                task.status = TaskStatus::Running;
            }
        }

        session.status = if session
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
        {
            SessionStatus::Completed
        } else if session
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::WaitingApproval)
        {
            SessionStatus::WaitingApproval
        } else if task_failed {
            SessionStatus::Failed
        } else {
            SessionStatus::Running
        };
        session.touch();

        self.store.save_session(&session)?;

        let llm_summary = llm_response_summary(run.response_artifact_path.as_deref());

        Ok(RunStepResult {
            session,
            task_id: task_snapshot.id,
            step_id: step_snapshot.id,
            step_title: step_snapshot.title,
            run_id: Some(run.id),
            run_pid: run.pid,
            stdout_log_path: run.stdout_log_path.clone(),
            stderr_log_path: run.stderr_log_path.clone(),
            request_artifact_path: run.request_artifact_path.clone(),
            response_artifact_path: run.response_artifact_path.clone(),
            transcript_artifact_path: run.transcript_artifact_path.clone(),
            llm_provider: llm_summary.provider,
            llm_model: llm_summary.model,
            llm_prompt_tokens: llm_summary.prompt_tokens,
            llm_completion_tokens: llm_summary.completion_tokens,
            llm_total_tokens: llm_summary.total_tokens,
            disposition: RunStepDisposition::Executed,
            attempt_number: Some(attempt_number),
            attempt_status: Some(outcome.status),
            max_attempts,
            retryable,
        })
    }

    pub fn cancel_run(&self, run_id: Uuid) -> anyhow::Result<RunRecord> {
        let mut run = self.store.load_run(run_id)?;
        self.store.request_run_cancel(run_id)?;
        if run.summary.is_none() {
            run.summary = Some(String::from("cancel requested"));
        }
        self.store.save_run(&run)?;
        Ok(run)
    }

    pub fn reconcile_run(&self, run_id: Uuid) -> anyhow::Result<RunRecord> {
        let mut run = self.store.load_run(run_id)?;
        if run.status != crate::model::RunStatus::Running {
            return Ok(run);
        }

        let Some(exit_code_path) = run.exit_code_path.clone() else {
            return Ok(run);
        };
        let exit_code_raw = match std::fs::read_to_string(&exit_code_path) {
            Ok(raw) => raw,
            Err(_) => return Ok(run),
        };
        let exit_code = exit_code_raw.trim().parse::<i32>().unwrap_or(1);

        let cancelled = self.store.cancel_request_path(run.id).exists();
        let attempt_status = if cancelled {
            AttemptStatus::Cancelled
        } else if exit_code == 0 {
            AttemptStatus::Succeeded
        } else {
            AttemptStatus::Failed
        };

        run.status = attempt_status.into();
        run.finished_at = Some(Utc::now());
        if run.summary.is_none() {
            run.summary = Some(match attempt_status {
                AttemptStatus::Succeeded => String::from("background run completed"),
                AttemptStatus::Cancelled => String::from("background run cancelled"),
                AttemptStatus::Failed => {
                    format!("background run failed with exit code {exit_code}")
                }
                AttemptStatus::TimedOut => String::from("background run timed out"),
                AttemptStatus::Running => String::from("background run still running"),
            });
        }
        self.store.save_run(&run)?;

        let mut session = self.store.load_session(run.session_id)?;
        let max_attempts = session.policy.retry.max_attempts.max(1);
        let Some(task_index) = session.tasks.iter().position(|task| task.id == run.task_id) else {
            return Ok(run);
        };
        let Some(step_index) = session.tasks[task_index]
            .steps
            .iter()
            .position(|step| step.id == run.step_id)
        else {
            return Ok(run);
        };

        let retryable = is_retryable(attempt_status, run.attempt_number, max_attempts);
        let requires_failure_approval = outcome_requires_failure_approval(&session, attempt_status);
        let task_failed;
        {
            let task = &mut session.tasks[task_index];
            let step = &mut task.steps[step_index];
            if let Some(attempt) = step
                .attempts
                .iter_mut()
                .find(|attempt| attempt.run_id == Some(run.id))
            {
                attempt.status = attempt_status;
                attempt.finished_at = run.finished_at;
                attempt.summary = run.summary.clone();
            }

            step.status = match attempt_status {
                AttemptStatus::Succeeded => StepStatus::Completed,
                AttemptStatus::Failed | AttemptStatus::TimedOut | AttemptStatus::Cancelled => {
                    if requires_failure_approval {
                        step.approval_granted = false;
                        StepStatus::WaitingApproval
                    } else if retryable {
                        StepStatus::Planned
                    } else {
                        StepStatus::Failed
                    }
                }
                AttemptStatus::Running => StepStatus::Running,
            };

            let any_steps_running = task
                .steps
                .iter()
                .any(|step| step.status == StepStatus::Running);
            let any_waiting_approval = task
                .steps
                .iter()
                .any(|step| step.status == StepStatus::WaitingApproval);
            let all_steps_completed = task
                .steps
                .iter()
                .all(|step| step.status == StepStatus::Completed);
            task_failed = task
                .steps
                .iter()
                .any(|step| step.status == StepStatus::Failed);

            task.status = if all_steps_completed {
                TaskStatus::Completed
            } else if any_waiting_approval {
                TaskStatus::WaitingApproval
            } else if task_failed {
                TaskStatus::Failed
            } else if any_steps_running {
                TaskStatus::Running
            } else {
                TaskStatus::Planned
            };
            task.touch();
        }

        session.status = if session
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
        {
            SessionStatus::Completed
        } else if session
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::WaitingApproval)
        {
            SessionStatus::WaitingApproval
        } else if task_failed {
            SessionStatus::Failed
        } else if session
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::Running)
        {
            SessionStatus::Running
        } else {
            SessionStatus::Created
        };
        session.touch();
        self.store.save_session(&session)?;

        Ok(run)
    }
}

fn session_waiting_approval(session: &SessionRecord) -> bool {
    session.status == SessionStatus::WaitingApproval
        || session
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::WaitingApproval)
}

fn task_has_runnable_step(task: &TaskRecord) -> bool {
    if task.is_finished() || task.status == TaskStatus::WaitingApproval {
        return false;
    }

    task.steps.iter().any(step_is_runnable)
}

fn step_is_runnable(step: &StepRecord) -> bool {
    !step.is_terminal() && step.status != StepStatus::WaitingApproval
}

fn step_requires_manual_approval(session: &SessionRecord, step: &StepRecord) -> bool {
    step.requires_approval
        && !step.approval_granted
        && !matches!(session.policy.approval, ApprovalPolicy::Never)
}

fn outcome_requires_failure_approval(session: &SessionRecord, status: AttemptStatus) -> bool {
    matches!(session.policy.approval, ApprovalPolicy::OnFailure)
        && matches!(
            status,
            AttemptStatus::Failed | AttemptStatus::TimedOut | AttemptStatus::Cancelled
        )
}

fn refresh_session_status(session: &mut SessionRecord) {
    if session
        .tasks
        .iter()
        .all(|task| task.status == TaskStatus::Completed)
    {
        session.status = SessionStatus::Completed;
    } else if session
        .tasks
        .iter()
        .any(|task| task.status == TaskStatus::WaitingApproval)
    {
        session.status = SessionStatus::WaitingApproval;
    } else if session
        .tasks
        .iter()
        .any(|task| task.status == TaskStatus::Failed)
    {
        session.status = SessionStatus::Failed;
    } else {
        session.status = SessionStatus::Created;
    }
}

fn llm_response_summary(path: Option<&str>) -> LlmResponseSummary {
    let Some(path) = path else {
        return LlmResponseSummary::default();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return LlmResponseSummary::default();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return LlmResponseSummary::default();
    };

    LlmResponseSummary {
        provider: parsed
            .get("provider")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        model: parsed
            .get("provider")
            .and_then(|v| v.get("model"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        prompt_tokens: parsed
            .get("usage")
            .and_then(|v| v.get("prompt_tokens"))
            .and_then(|v| v.as_u64()),
        completion_tokens: parsed
            .get("usage")
            .and_then(|v| v.get("completion_tokens"))
            .and_then(|v| v.as_u64()),
        total_tokens: parsed
            .get("usage")
            .and_then(|v| v.get("total_tokens"))
            .and_then(|v| v.as_u64()),
    }
}

fn is_retryable(status: AttemptStatus, attempt_number: u32, max_attempts: u32) -> bool {
    matches!(status, AttemptStatus::Failed | AttemptStatus::TimedOut)
        && attempt_number < max_attempts
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;
    use crate::executor::{ExecutorBridge, ExecutorOptions};
    use crate::model::{ExecutionPolicy, RetryPolicy, RunStatus, TimeoutPolicy};
    use crate::storage::SessionStore;

    struct StubRunner {
        outcomes: Vec<AttemptOutcome>,
    }

    impl StubRunner {
        fn new(outcomes: Vec<AttemptOutcome>) -> Self {
            Self { outcomes }
        }
    }

    impl StepRunner for StubRunner {
        fn run_step(
            &mut self,
            _session: &SessionRecord,
            _task: &TaskRecord,
            _step: &StepRecord,
            _run: &RunRecord,
            _attempt_number: u32,
        ) -> AttemptOutcome {
            self.outcomes.remove(0)
        }
    }

    fn session_with_policy(
        store: &SessionStore,
        max_attempts: u32,
        timeout_secs: u64,
        approval: ApprovalPolicy,
    ) -> SessionRecord {
        let session = SessionRecord::new_with_policy(
            "demo",
            ExecutionPolicy {
                retry: RetryPolicy { max_attempts },
                timeout: TimeoutPolicy {
                    step_timeout_secs: timeout_secs,
                },
                approval,
            },
        );
        store.save_session(&session).unwrap();
        session
    }

    #[test]
    fn creates_task_with_planned_steps() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();

        let updated = engine
            .create_task(
                session.id,
                "bootstrap engine",
                vec!["plan".into(), "execute".into()],
            )
            .unwrap();

        assert_eq!(updated.tasks.len(), 1);
        assert_eq!(updated.tasks[0].status, TaskStatus::Planned);
        assert_eq!(updated.tasks[0].steps.len(), 2);
        assert_eq!(updated.tasks[0].steps[0].status, StepStatus::Planned);
        assert!(!updated.tasks[0].steps[0].requires_approval);
    }

    #[test]
    fn runs_next_step_and_keeps_task_running_until_last_step() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
        let updated = engine
            .create_task(
                session.id,
                "bootstrap engine",
                vec!["plan".into(), "execute".into()],
            )
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::succeeded("step ok")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let task = &result.session.tasks[0];
        let step = &task.steps[0];

        assert_eq!(result.disposition, RunStepDisposition::Executed);
        assert_eq!(result.step_title, "plan");
        assert_eq!(result.attempt_number, Some(1));
        assert_eq!(result.attempt_status, Some(AttemptStatus::Succeeded));
        assert!(!result.retryable);
        assert_eq!(step.status, StepStatus::Completed);
        assert_eq!(step.attempts.len(), 1);
        assert!(step.attempts[0].run_id.is_some());
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(result.session.status, SessionStatus::Running);

        let runs = store.list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Succeeded);
        assert_eq!(runs[0].attempt_number, 1);
        assert!(
            runs[0]
                .stdout_log_path
                .as_deref()
                .unwrap()
                .ends_with(".stdout.log")
        );
        assert!(
            runs[0]
                .stderr_log_path
                .as_deref()
                .unwrap()
                .ends_with(".stderr.log")
        );
        assert!(
            runs[0]
                .request_artifact_path
                .as_deref()
                .unwrap()
                .ends_with(".request.json")
        );
        assert!(
            runs[0]
                .response_artifact_path
                .as_deref()
                .unwrap()
                .ends_with(".response.json")
        );
        assert!(
            runs[0]
                .transcript_artifact_path
                .as_deref()
                .unwrap()
                .ends_with(".transcript.json")
        );
    }

    #[test]
    fn marks_task_and_session_failed_when_step_fails_without_retry_budget() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::failed("boom")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let task = &result.session.tasks[0];
        let step = &task.steps[0];

        assert_eq!(step.status, StepStatus::Failed);
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(result.session.status, SessionStatus::Failed);
        assert_eq!(step.attempts[0].summary.as_deref(), Some("boom"));
    }

    #[test]
    fn keeps_failed_step_runnable_while_retry_budget_remains() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 2, 300, ApprovalPolicy::Never);
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::failed("boom")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let step = &result.session.tasks[0].steps[0];

        assert!(result.retryable);
        assert_eq!(result.max_attempts, 2);
        assert_eq!(step.status, StepStatus::Planned);
        assert_eq!(result.session.tasks[0].status, TaskStatus::Running);
        assert_eq!(result.session.status, SessionStatus::Running);
    }

    #[test]
    fn reruns_failed_step_and_completes_task_on_successful_retry() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 2, 300, ApprovalPolicy::Never);
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut fail_runner = StubRunner::new(vec![AttemptOutcome::failed("boom")]);
        let first = engine.run_next_step(updated.id, &mut fail_runner).unwrap();
        let mut success_runner = StubRunner::new(vec![AttemptOutcome::succeeded("recovered")]);
        let second = engine
            .run_next_step(first.session.id, &mut success_runner)
            .unwrap();
        let task = &second.session.tasks[0];
        let step = &task.steps[0];

        assert_eq!(step.attempts.len(), 2);
        assert_eq!(step.attempts[1].number, 2);
        assert_eq!(step.status, StepStatus::Completed);
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(second.session.status, SessionStatus::Completed);
    }

    #[test]
    fn timed_out_attempt_is_retryable_with_budget() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 3, 10, ApprovalPolicy::Never);
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::timed_out("timeout")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let step = &result.session.tasks[0].steps[0];

        assert_eq!(result.attempt_status, Some(AttemptStatus::TimedOut));
        assert!(result.retryable);
        assert_eq!(step.status, StepStatus::Planned);
    }

    #[test]
    fn on_failure_policy_moves_failed_attempt_to_waiting_approval() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 2, 10, ApprovalPolicy::OnFailure);
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::failed("boom")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let step = &result.session.tasks[0].steps[0];

        assert_eq!(result.attempt_status, Some(AttemptStatus::Failed));
        assert!(result.retryable);
        assert_eq!(step.status, StepStatus::WaitingApproval);
        assert_eq!(result.session.tasks[0].status, TaskStatus::WaitingApproval);
        assert_eq!(result.session.status, SessionStatus::WaitingApproval);
    }

    #[test]
    fn approved_on_failure_step_can_resume_and_succeed() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 2, 10, ApprovalPolicy::OnFailure);
        let session = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut fail_runner = StubRunner::new(vec![AttemptOutcome::failed("boom")]);
        let waited = engine.run_next_step(session.id, &mut fail_runner).unwrap();
        let task = &waited.session.tasks[0];
        let step = &task.steps[0];

        let approved = engine
            .approve_step(waited.session.id, task.id, step.id)
            .unwrap();
        assert_eq!(approved.status, SessionStatus::Created);
        assert_eq!(approved.tasks[0].status, TaskStatus::Planned);
        assert_eq!(approved.tasks[0].steps[0].status, StepStatus::Planned);
        assert!(approved.tasks[0].steps[0].approval_granted);

        let mut success_runner = StubRunner::new(vec![AttemptOutcome::succeeded("recovered")]);
        let resumed = engine
            .run_next_step(approved.id, &mut success_runner)
            .unwrap();
        assert_eq!(resumed.session.status, SessionStatus::Completed);
        assert_eq!(resumed.session.tasks[0].steps[0].attempts.len(), 2);
        assert_eq!(resumed.session.tasks[0].steps[0].attempts[1].number, 2);
    }

    #[test]
    fn waits_for_approval_before_running_gated_step() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 1, 300, ApprovalPolicy::Manual);
        let session = engine
            .create_task_with_steps(
                session.id,
                "bootstrap engine",
                vec![StepSpec::requires_approval("dangerous step")],
            )
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::succeeded("should not run")]);
        let result = engine.run_next_step(session.id, &mut runner).unwrap();

        assert_eq!(result.disposition, RunStepDisposition::WaitingApproval);
        assert_eq!(result.attempt_number, None);
        assert_eq!(result.attempt_status, None);
        assert_eq!(result.session.status, SessionStatus::WaitingApproval);
        assert_eq!(result.session.tasks[0].status, TaskStatus::WaitingApproval);
        assert_eq!(
            result.session.tasks[0].steps[0].status,
            StepStatus::WaitingApproval
        );
        assert!(matches!(
            engine
                .run_next_step(session.id, &mut runner)
                .unwrap_err()
                .downcast_ref(),
            Some(EngineError::ApprovalRequired)
        ));
    }

    #[test]
    fn approved_step_becomes_runnable_and_can_complete() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 1, 300, ApprovalPolicy::Manual);
        let session = engine
            .create_task_with_steps(
                session.id,
                "bootstrap engine",
                vec![StepSpec::requires_approval("dangerous step")],
            )
            .unwrap();

        let mut wait_runner = StubRunner::new(vec![AttemptOutcome::succeeded("should not run")]);
        let waited = engine.run_next_step(session.id, &mut wait_runner).unwrap();
        let task = &waited.session.tasks[0];
        let step = &task.steps[0];

        let approved = engine
            .approve_step(waited.session.id, task.id, step.id)
            .unwrap();
        assert_eq!(approved.status, SessionStatus::Created);
        assert_eq!(approved.tasks[0].steps[0].status, StepStatus::Planned);
        assert!(approved.tasks[0].steps[0].approval_granted);

        let mut success_runner = StubRunner::new(vec![AttemptOutcome::succeeded("done")]);
        let result = engine
            .run_next_step(approved.id, &mut success_runner)
            .unwrap();
        assert_eq!(result.disposition, RunStepDisposition::Executed);
        assert_eq!(result.session.status, SessionStatus::Completed);
    }

    #[test]
    fn approval_policy_never_bypasses_wait_state() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 1, 300, ApprovalPolicy::Never);
        let session = engine
            .create_task_with_steps(
                session.id,
                "bootstrap engine",
                vec![StepSpec::requires_approval("dangerous step")],
            )
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::succeeded("done")]);
        let result = engine.run_next_step(session.id, &mut runner).unwrap();

        assert_eq!(result.disposition, RunStepDisposition::Executed);
        assert_eq!(result.session.status, SessionStatus::Completed);
        assert_eq!(result.attempt_number, Some(1));
    }

    #[test]
    fn runs_only_the_targeted_task_when_task_id_is_provided() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
        let session = engine
            .create_task(session.id, "task-a", vec!["a1".into(), "a2".into()])
            .unwrap();
        let session = engine
            .create_task(session.id, "task-b", vec!["b1".into()])
            .unwrap();
        let target_task_id = session.tasks[1].id;

        let mut runner = StubRunner::new(vec![AttemptOutcome::succeeded("task-b step")]);
        let result = engine
            .run_task_next_step(session.id, target_task_id, &mut runner)
            .unwrap();

        assert_eq!(result.task_id, target_task_id);
        assert_eq!(result.step_title, "b1");
        assert_eq!(result.session.tasks[0].steps[0].status, StepStatus::Planned);
        assert_eq!(
            result.session.tasks[1].steps[0].status,
            StepStatus::Completed
        );
    }

    #[test]
    fn run_all_executes_until_approval_boundary() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = session_with_policy(&store, 1, 300, ApprovalPolicy::Manual);
        let session = engine
            .create_task_with_steps(
                session.id,
                "task-a",
                vec![StepSpec::new("a1"), StepSpec::requires_approval("a2")],
            )
            .unwrap();
        let session = engine
            .create_task_with_steps(session.id, "task-b", vec![StepSpec::new("b1")])
            .unwrap();

        let mut runner = StubRunner::new(vec![
            AttemptOutcome::succeeded("a1 done"),
            AttemptOutcome::succeeded("should stop before this"),
        ]);
        let result = engine.run_all(session.id, &mut runner).unwrap();

        assert_eq!(result.runs.len(), 2);
        assert_eq!(result.runs[0].disposition, RunStepDisposition::Executed);
        assert_eq!(
            result.runs[1].disposition,
            RunStepDisposition::WaitingApproval
        );
        assert_eq!(result.session.status, SessionStatus::WaitingApproval);
        assert_eq!(result.session.tasks[1].steps[0].status, StepStatus::Planned);
    }

    #[test]
    fn cancel_run_writes_persisted_cancel_request() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
        let updated = engine
            .create_task(session.id, "bootstrap engine", vec!["plan".into()])
            .unwrap();

        let mut runner = StubRunner::new(vec![AttemptOutcome::succeeded("step ok")]);
        let result = engine.run_next_step(updated.id, &mut runner).unwrap();
        let run_id = result.run_id.unwrap();

        let run = engine.cancel_run(run_id).unwrap();
        let cancel_path = store.cancel_request_path(run_id);

        assert_eq!(run.id, run_id);
        assert!(cancel_path.exists());
        assert_eq!(run.summary.as_deref(), Some("step ok"));
    }

    #[test]
    fn reconcile_run_completes_background_attempt_and_session_state() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
        let session = engine
            .create_task(session.id, "bg", vec!["cmd: true".into()])
            .unwrap();

        let mut runner = ExecutorBridge::with_options(ExecutorOptions {
            background: true,
            ..ExecutorOptions::default()
        });
        let result = engine.run_next_step(session.id, &mut runner).unwrap();
        let run_id = result.run_id.unwrap();

        let mut run = engine.reconcile_run(run_id).unwrap();
        for _ in 0..20 {
            if run.status != RunStatus::Running {
                break;
            }
            thread::sleep(Duration::from_millis(100));
            run = engine.reconcile_run(run_id).unwrap();
        }
        let updated = store.load_session(session.id).unwrap();

        assert_eq!(run.status, RunStatus::Succeeded);
        assert_eq!(updated.status, SessionStatus::Completed);
        assert_eq!(updated.tasks[0].status, TaskStatus::Completed);
        assert_eq!(updated.tasks[0].steps[0].status, StepStatus::Completed);
        assert_eq!(
            updated.tasks[0].steps[0].attempts[0].status,
            AttemptStatus::Succeeded
        );
    }
}
