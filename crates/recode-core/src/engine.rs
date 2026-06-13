use chrono::Utc;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    ApprovalPolicy, AttemptRecord, AttemptStatus, SessionRecord, SessionStatus, StepRecord,
    StepStatus, TaskRecord, TaskStatus,
};
use crate::storage::SessionStore;

#[derive(Debug, Clone)]
pub struct AttemptOutcome {
    pub status: AttemptStatus,
    pub summary: Option<String>,
}

impl AttemptOutcome {
    pub fn succeeded(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::Succeeded,
            summary: Some(summary.into()),
        }
    }

    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::Failed,
            summary: Some(summary.into()),
        }
    }

    pub fn timed_out(summary: impl Into<String>) -> Self {
        Self {
            status: AttemptStatus::TimedOut,
            summary: Some(summary.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StepSpec {
    pub title: String,
    pub requires_approval: bool,
}

impl StepSpec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            requires_approval: false,
        }
    }

    pub fn requires_approval(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            requires_approval: true,
        }
    }
}

pub trait StepRunner {
    fn run_step(
        &mut self,
        session: &SessionRecord,
        task: &TaskRecord,
        step: &StepRecord,
        attempt_number: u32,
    ) -> AttemptOutcome;
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
                .map(|step| StepRecord::new_with_approval(step.title, step.requires_approval))
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
                disposition: RunStepDisposition::WaitingApproval,
                attempt_number: None,
                attempt_status: None,
                max_attempts,
                retryable: false,
            });
        }

        let attempt_number = step_snapshot.attempts.len() as u32 + 1;
        let max_attempts = session.policy.retry.max_attempts.max(1);
        let outcome = runner.run_step(&session, &task_snapshot, &step_snapshot, attempt_number);
        let now = Utc::now();
        let retryable = is_retryable(outcome.status, attempt_number, max_attempts);

        let task_failed;
        {
            let task = &mut session.tasks[task_index];
            task.status = TaskStatus::Running;
            task.touch();

            let step = &mut task.steps[step_index];
            step.status = StepStatus::Running;
            let attempt = AttemptRecord {
                id: Uuid::new_v4(),
                number: attempt_number,
                status: outcome.status,
                started_at: now,
                finished_at: Some(now),
                summary: outcome.summary,
            };
            step.attempts.push(attempt);
            step.status = match outcome.status {
                AttemptStatus::Succeeded => StepStatus::Completed,
                AttemptStatus::Failed | AttemptStatus::TimedOut | AttemptStatus::Cancelled => {
                    if retryable {
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
        } else if task_failed {
            SessionStatus::Failed
        } else {
            SessionStatus::Running
        };
        session.touch();

        self.store.save_session(&session)?;

        Ok(RunStepResult {
            session,
            task_id: task_snapshot.id,
            step_id: step_snapshot.id,
            step_title: step_snapshot.title,
            disposition: RunStepDisposition::Executed,
            attempt_number: Some(attempt_number),
            attempt_status: Some(outcome.status),
            max_attempts,
            retryable,
        })
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

fn is_retryable(status: AttemptStatus, attempt_number: u32, max_attempts: u32) -> bool {
    matches!(status, AttemptStatus::Failed | AttemptStatus::TimedOut)
        && attempt_number < max_attempts
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::model::{ExecutionPolicy, RetryPolicy, TimeoutPolicy};
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
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(result.session.status, SessionStatus::Running);
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
}
