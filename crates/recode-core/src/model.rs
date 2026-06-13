use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: Uuid,
    pub name: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub policy: ExecutionPolicy,
    pub tasks: Vec<TaskRecord>,
}

impl SessionRecord {
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_policy(name, ExecutionPolicy::default())
    }

    pub fn new_with_policy(name: impl Into<String>, policy: ExecutionPolicy) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            status: SessionStatus::Created,
            created_at: now,
            updated_at: now,
            policy,
            tasks: Vec::new(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Manual,
    OnFailure,
    Never,
}

impl ApprovalPolicy {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "on_failure" | "on-failure" => Some(Self::OnFailure),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Created,
    Running,
    WaitingApproval,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub retry: RetryPolicy,
    pub timeout: TimeoutPolicy,
    #[serde(default = "default_approval_policy")]
    pub approval: ApprovalPolicy,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            retry: RetryPolicy::default(),
            timeout: TimeoutPolicy::default(),
            approval: default_approval_policy(),
        }
    }
}

fn default_approval_policy() -> ApprovalPolicy {
    ApprovalPolicy::Manual
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 1 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutPolicy {
    pub step_timeout_secs: u64,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            step_timeout_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: Uuid,
    pub title: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub steps: Vec<StepRecord>,
}

impl TaskRecord {
    pub fn new(title: impl Into<String>, steps: Vec<StepRecord>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            status: TaskStatus::Planned,
            created_at: now,
            updated_at: now,
            steps,
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.status, TaskStatus::Completed | TaskStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Planned,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub id: Uuid,
    pub title: String,
    pub status: StepStatus,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub approval_granted: bool,
    pub attempts: Vec<AttemptRecord>,
}

impl StepRecord {
    pub fn new(title: impl Into<String>) -> Self {
        Self::new_with_approval(title, false)
    }

    pub fn new_with_approval(title: impl Into<String>, requires_approval: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            status: StepStatus::Planned,
            requires_approval,
            approval_granted: false,
            attempts: Vec::new(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.status, StepStatus::Completed | StepStatus::Skipped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Planned,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub id: Uuid,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    pub number: u32,
    pub status: AttemptStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub task_id: Uuid,
    pub step_id: Uuid,
    pub attempt_number: u32,
    pub status: RunStatus,
    pub mode: RunMode,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub stdout_log_path: Option<String>,
    #[serde(default)]
    pub stderr_log_path: Option<String>,
    #[serde(default)]
    pub exit_code_path: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

impl RunRecord {
    pub fn new(
        session_id: Uuid,
        task_id: Uuid,
        step_id: Uuid,
        attempt_number: u32,
        mode: RunMode,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            task_id,
            step_id,
            attempt_number,
            status: RunStatus::Running,
            mode,
            started_at: Utc::now(),
            finished_at: None,
            pid: None,
            stdout_log_path: None,
            stderr_log_path: None,
            exit_code_path: None,
            summary: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

impl From<AttemptStatus> for RunStatus {
    fn from(value: AttemptStatus) -> Self {
        match value {
            AttemptStatus::Running => Self::Running,
            AttemptStatus::Succeeded => Self::Succeeded,
            AttemptStatus::Failed => Self::Failed,
            AttemptStatus::TimedOut => Self::TimedOut,
            AttemptStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    Foreground,
    Background,
}
