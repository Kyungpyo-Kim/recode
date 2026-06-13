use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::model::SessionRecord;

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
