use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    AttemptRecord, AttemptStatus, SessionRecord, SessionStatus, StepRecord, StepStatus, TaskRecord,
    TaskStatus,
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

#[derive(Debug, Clone)]
pub struct RunStepResult {
    pub session: SessionRecord,
    pub task_id: Uuid,
    pub step_id: Uuid,
    pub attempt_number: u32,
    pub attempt_status: AttemptStatus,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("session contains no runnable task")]
    NoRunnableTask,
    #[error("task has no runnable step")]
    NoRunnableStep,
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
        let mut session = self.store.load_session(session_id)?;
        let task = TaskRecord::new(title, step_titles);
        session.tasks.push(task);
        session.touch();
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn run_next_step<R: StepRunner>(
        &self,
        session_id: Uuid,
        runner: &mut R,
    ) -> anyhow::Result<RunStepResult> {
        let mut session = self.store.load_session(session_id)?;
        let Some(task_index) = session.tasks.iter().position(task_has_runnable_step) else {
            return Err(EngineError::NoRunnableTask.into());
        };

        session.status = SessionStatus::Running;

        let task_snapshot = session.tasks[task_index].clone();
        let Some(step_index) = task_snapshot.steps.iter().position(step_is_runnable) else {
            return Err(EngineError::NoRunnableStep.into());
        };
        let step_snapshot = task_snapshot.steps[step_index].clone();
        let attempt_number = step_snapshot.attempts.len() as u32 + 1;
        let outcome = runner.run_step(&session, &task_snapshot, &step_snapshot, attempt_number);
        let now = Utc::now();

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
                    StepStatus::Failed
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
            attempt_number,
            attempt_status: outcome.status,
        })
    }
}

fn task_has_runnable_step(task: &TaskRecord) -> bool {
    if task.is_finished() {
        return false;
    }

    task.steps.iter().any(step_is_runnable)
}

fn step_is_runnable(step: &StepRecord) -> bool {
    !step.is_terminal()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
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

        assert_eq!(result.attempt_number, 1);
        assert_eq!(result.attempt_status, AttemptStatus::Succeeded);
        assert_eq!(step.status, StepStatus::Completed);
        assert_eq!(step.attempts.len(), 1);
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(result.session.status, SessionStatus::Running);
    }

    #[test]
    fn marks_task_and_session_failed_when_step_fails() {
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
    fn reruns_failed_step_and_completes_task_on_successful_retry() {
        let temp = tempdir().unwrap();
        let store = SessionStore::new(temp.path());
        let engine = WorkflowEngine::new(store.clone());
        let session = store.init_session("demo").unwrap();
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
}
