use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::model::{ExecutionPolicy, SessionRecord};

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

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new(Path::new(DEFAULT_STATE_DIR))
    }
}
