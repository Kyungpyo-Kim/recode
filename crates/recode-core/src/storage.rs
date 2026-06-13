use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::model::{ExecutionPolicy, RunRecord, SessionRecord};

pub const DEFAULT_STATE_DIR: &str = ".recode/state";

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn init_session(&self, name: impl Into<String>) -> Result<SessionRecord> {
        let session = SessionRecord::new(name);
        self.save_session(&session)?;
        Ok(session)
    }

    pub fn init_session_with_policy(
        &self,
        name: impl Into<String>,
        policy: ExecutionPolicy,
    ) -> Result<SessionRecord> {
        let session = SessionRecord::new_with_policy(name, policy);
        self.save_session(&session)?;
        Ok(session)
    }

    pub fn load_session(&self, id: Uuid) -> Result<SessionRecord> {
        let path = self.session_path(id);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read session file: {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse session file: {}", path.display()))
    }

    pub fn save_session(&self, session: &SessionRecord) -> Result<()> {
        fs::create_dir_all(self.sessions_dir()).context("failed to create session directory")?;
        let path = self.session_path(session.id);
        let json = serde_json::to_string_pretty(session).context("failed to serialize session")?;
        fs::write(&path, json)
            .with_context(|| format!("failed to write session file: {}", path.display()))?;
        Ok(())
    }

    pub fn save_run(&self, run: &RunRecord) -> Result<()> {
        fs::create_dir_all(self.runs_dir()).context("failed to create run directory")?;
        fs::create_dir_all(self.logs_dir()).context("failed to create log directory")?;
        let path = self.run_path(run.id);
        let json = serde_json::to_string_pretty(run).context("failed to serialize run")?;
        fs::write(&path, json)
            .with_context(|| format!("failed to write run file: {}", path.display()))?;
        Ok(())
    }

    pub fn load_run(&self, id: Uuid) -> Result<RunRecord> {
        let path = self.run_path(id);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read run file: {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse run file: {}", path.display()))
    }

    pub fn list_runs(&self) -> Result<Vec<RunRecord>> {
        let dir = self.runs_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read run directory: {}", dir.display()))?
        {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let raw = fs::read_to_string(entry.path())?;
            let run: RunRecord = serde_json::from_str(&raw)?;
            entries.push((modified, run));
        }

        entries.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        Ok(entries.into_iter().map(|(_, run)| run).collect())
    }

    pub fn stdout_log_path(&self, run_id: Uuid) -> PathBuf {
        self.logs_dir().join(format!("{run_id}.stdout.log"))
    }

    pub fn stderr_log_path(&self, run_id: Uuid) -> PathBuf {
        self.logs_dir().join(format!("{run_id}.stderr.log"))
    }

    pub fn exit_code_path(&self, run_id: Uuid) -> PathBuf {
        self.logs_dir().join(format!("{run_id}.exit-code"))
    }

    pub fn cancel_request_path(&self, run_id: Uuid) -> PathBuf {
        self.cancels_dir().join(format!("{run_id}.cancel"))
    }

    pub fn request_run_cancel(&self, run_id: Uuid) -> Result<PathBuf> {
        fs::create_dir_all(self.cancels_dir()).context("failed to create cancel directory")?;
        let path = self.cancel_request_path(run_id);
        fs::write(&path, b"cancel\n")
            .with_context(|| format!("failed to write cancel request: {}", path.display()))?;
        Ok(path)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read session directory: {}", dir.display()))?
        {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let raw = fs::read_to_string(entry.path())?;
            let session: SessionRecord = serde_json::from_str(&raw)?;
            entries.push((modified, session));
        }

        entries.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        Ok(entries.into_iter().map(|(_, session)| session).collect())
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn cancels_dir(&self) -> PathBuf {
        self.root.join("cancels")
    }

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn run_path(&self, id: Uuid) -> PathBuf {
        self.runs_dir().join(format!("{id}.json"))
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new(Path::new(DEFAULT_STATE_DIR))
    }
}
