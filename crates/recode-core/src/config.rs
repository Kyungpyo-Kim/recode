use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{ApprovalPolicy, ExecutionPolicy, RetryPolicy, TimeoutPolicy};
use crate::storage::DEFAULT_STATE_DIR;

pub const DEFAULT_LOG_LEVEL: &str = "info";
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
pub const DEFAULT_PROVIDER: &str = "openai-compatible";
pub const DEFAULT_CONFIG_FILE: &str = "recode.toml";
pub const DEFAULT_MAX_ATTEMPTS: u32 = 1;
pub const DEFAULT_PROVIDER_MODE: &str = "openai_compatible";
pub const DEFAULT_PROVIDER_MODEL: &str = "gpt-4.1-mini";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    OpenAiCompatible,
    Codex,
}

impl ProviderMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openai-compatible" | "openai_compatible" | "openai" => Some(Self::OpenAiCompatible),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfig {
    pub name: String,
    pub mode: ProviderMode,
    pub base_url: Option<String>,
    pub api_key_env: String,
    pub model: String,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: DEFAULT_PROVIDER.to_string(),
            mode: ProviderMode::OpenAiCompatible,
            base_url: Some("https://api.openai.com/v1".to_string()),
            api_key_env: "OPENAI_API_KEY".to_string(),
            model: DEFAULT_PROVIDER_MODEL.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecodeConfig {
    pub state_dir: PathBuf,
    pub log_level: String,
    pub default_provider: String,
    pub provider: ProviderConfig,
    pub default_timeout_secs: u64,
    pub default_max_attempts: u32,
    pub approval_policy: ApprovalPolicy,
    pub config_path: Option<PathBuf>,
}

impl Default for RecodeConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            default_provider: DEFAULT_PROVIDER.to_string(),
            provider: ProviderConfig::default(),
            default_timeout_secs: DEFAULT_TIMEOUT_SECS,
            default_max_attempts: DEFAULT_MAX_ATTEMPTS,
            approval_policy: ApprovalPolicy::Manual,
            config_path: None,
        }
    }
}

impl RecodeConfig {
    pub fn execution_policy(&self) -> ExecutionPolicy {
        ExecutionPolicy {
            retry: RetryPolicy {
                max_attempts: self.default_max_attempts,
            },
            timeout: TimeoutPolicy {
                step_timeout_secs: self.default_timeout_secs,
            },
            approval: self.approval_policy,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartialConfig {
    pub state_dir: Option<PathBuf>,
    pub log_level: Option<String>,
    pub default_provider: Option<String>,
    pub provider_mode: Option<ProviderMode>,
    pub provider_base_url: Option<String>,
    pub provider_api_key_env: Option<String>,
    pub provider_model: Option<String>,
    pub default_timeout_secs: Option<u64>,
    pub default_max_attempts: Option<u32>,
    pub approval_policy: Option<ApprovalPolicy>,
}

impl PartialConfig {
    pub fn merge(mut self, other: Self) -> Self {
        if other.state_dir.is_some() {
            self.state_dir = other.state_dir;
        }
        if other.log_level.is_some() {
            self.log_level = other.log_level;
        }
        if other.default_provider.is_some() {
            self.default_provider = other.default_provider;
        }
        if other.provider_mode.is_some() {
            self.provider_mode = other.provider_mode;
        }
        if other.provider_base_url.is_some() {
            self.provider_base_url = other.provider_base_url;
        }
        if other.provider_api_key_env.is_some() {
            self.provider_api_key_env = other.provider_api_key_env;
        }
        if other.provider_model.is_some() {
            self.provider_model = other.provider_model;
        }
        if other.default_timeout_secs.is_some() {
            self.default_timeout_secs = other.default_timeout_secs;
        }
        if other.default_max_attempts.is_some() {
            self.default_max_attempts = other.default_max_attempts;
        }
        if other.approval_policy.is_some() {
            self.approval_policy = other.approval_policy;
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct ConfigLoader {
    cwd: PathBuf,
    env_overrides: PartialConfig,
}

impl ConfigLoader {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            env_overrides: Self::env_partial(),
        }
    }

    pub fn with_env(cwd: impl Into<PathBuf>, env_overrides: PartialConfig) -> Self {
        Self {
            cwd: cwd.into(),
            env_overrides,
        }
    }

    pub fn load(
        &self,
        config_path: Option<PathBuf>,
        cli_overrides: PartialConfig,
    ) -> Result<RecodeConfig> {
        let resolved_config_path = config_path.or_else(|| {
            let candidate = self.cwd.join(DEFAULT_CONFIG_FILE);
            candidate.exists().then_some(candidate)
        });

        let file_partial = match resolved_config_path.as_ref() {
            Some(path) => Self::read_file(path)?,
            None => PartialConfig::default(),
        };

        let merged = PartialConfig::default()
            .merge(file_partial)
            .merge(self.env_overrides.clone())
            .merge(cli_overrides);

        let mut config = RecodeConfig::default();
        if let Some(state_dir) = merged.state_dir {
            config.state_dir = state_dir;
        }
        if let Some(log_level) = merged.log_level {
            config.log_level = log_level;
        }
        if let Some(default_provider) = merged.default_provider {
            config.default_provider = default_provider;
        }
        config.provider.name = config.default_provider.clone();
        if let Some(provider_mode) = merged.provider_mode {
            config.provider.mode = provider_mode;
        }
        if let Some(provider_base_url) = merged.provider_base_url {
            config.provider.base_url = Some(provider_base_url);
        }
        if let Some(provider_api_key_env) = merged.provider_api_key_env {
            config.provider.api_key_env = provider_api_key_env;
        }
        if let Some(provider_model) = merged.provider_model {
            config.provider.model = provider_model;
        }
        if let Some(default_timeout_secs) = merged.default_timeout_secs {
            config.default_timeout_secs = default_timeout_secs;
        }
        if let Some(default_max_attempts) = merged.default_max_attempts {
            config.default_max_attempts = default_max_attempts;
        }
        if let Some(approval_policy) = merged.approval_policy {
            config.approval_policy = approval_policy;
        }
        config.config_path = resolved_config_path;
        Ok(config)
    }

    fn read_file(path: &Path) -> Result<PartialConfig> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file: {}", path.display()))
    }

    fn env_partial() -> PartialConfig {
        PartialConfig {
            state_dir: env::var_os("RECODE_STATE_DIR").map(PathBuf::from),
            log_level: env::var("RECODE_LOG_LEVEL").ok(),
            default_provider: env::var("RECODE_DEFAULT_PROVIDER").ok(),
            provider_mode: env::var("RECODE_PROVIDER_MODE")
                .ok()
                .and_then(|raw| ProviderMode::parse(&raw)),
            provider_base_url: env::var("RECODE_PROVIDER_BASE_URL").ok(),
            provider_api_key_env: env::var("RECODE_PROVIDER_API_KEY_ENV").ok(),
            provider_model: env::var("RECODE_PROVIDER_MODEL").ok(),
            default_timeout_secs: env::var("RECODE_DEFAULT_TIMEOUT_SECS")
                .ok()
                .and_then(|raw| raw.parse().ok()),
            default_max_attempts: env::var("RECODE_DEFAULT_MAX_ATTEMPTS")
                .ok()
                .and_then(|raw| raw.parse().ok()),
            approval_policy: env::var("RECODE_APPROVAL_POLICY")
                .ok()
                .and_then(|raw| ApprovalPolicy::parse(&raw)),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn loads_defaults_when_no_file_or_overrides_exist() {
        let temp = tempdir().unwrap();
        let loader = ConfigLoader::with_env(temp.path(), PartialConfig::default());

        let config = loader.load(None, PartialConfig::default()).unwrap();

        assert_eq!(config, RecodeConfig::default());
    }

    #[test]
    fn loads_values_from_recode_toml() {
        let temp = tempdir().unwrap();
        let config_path = temp.path().join("recode.toml");
        fs::write(
            &config_path,
            r#"
state_dir = ".custom/state"
log_level = "debug"
default_provider = "codex"
provider_mode = "codex"
provider_base_url = "https://api.openai.com/v1"
provider_api_key_env = "OPENAI_API_KEY"
provider_model = "codex-mini-latest"
default_timeout_secs = 42
default_max_attempts = 3
approval_policy = "on_failure"
"#,
        )
        .unwrap();
        let loader = ConfigLoader::with_env(temp.path(), PartialConfig::default());

        let config = loader.load(None, PartialConfig::default()).unwrap();

        assert_eq!(config.state_dir, PathBuf::from(".custom/state"));
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.default_provider, "codex");
        assert_eq!(config.provider.name, "codex");
        assert_eq!(config.provider.mode, ProviderMode::Codex);
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(config.provider.api_key_env, "OPENAI_API_KEY");
        assert_eq!(config.provider.model, "codex-mini-latest");
        assert_eq!(config.default_timeout_secs, 42);
        assert_eq!(config.default_max_attempts, 3);
        assert_eq!(config.approval_policy, ApprovalPolicy::OnFailure);
        assert_eq!(config.config_path.as_deref(), Some(config_path.as_path()));
    }

    #[test]
    fn applies_env_overrides_on_top_of_file() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("recode.toml"),
            r#"
state_dir = ".file/state"
log_level = "info"
default_provider = "file-provider"
default_timeout_secs = 60
default_max_attempts = 2
approval_policy = "manual"
"#,
        )
        .unwrap();
        let loader = ConfigLoader::with_env(
            temp.path(),
            PartialConfig {
                state_dir: Some(PathBuf::from(".env/state")),
                log_level: Some("warn".into()),
                default_provider: None,
                provider_mode: Some(ProviderMode::Codex),
                provider_base_url: Some("https://example.invalid/v1".into()),
                provider_api_key_env: Some("CODEX_API_KEY".into()),
                provider_model: Some("codex-test".into()),
                default_timeout_secs: Some(90),
                default_max_attempts: Some(4),
                approval_policy: Some(ApprovalPolicy::Never),
            },
        );

        let config = loader.load(None, PartialConfig::default()).unwrap();

        assert_eq!(config.state_dir, PathBuf::from(".env/state"));
        assert_eq!(config.log_level, "warn");
        assert_eq!(config.default_provider, "file-provider");
        assert_eq!(config.provider.name, "file-provider");
        assert_eq!(config.provider.mode, ProviderMode::Codex);
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("https://example.invalid/v1")
        );
        assert_eq!(config.provider.api_key_env, "CODEX_API_KEY");
        assert_eq!(config.provider.model, "codex-test");
        assert_eq!(config.default_timeout_secs, 90);
        assert_eq!(config.default_max_attempts, 4);
        assert_eq!(config.approval_policy, ApprovalPolicy::Never);
    }

    #[test]
    fn applies_cli_overrides_on_top_of_env_and_file() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("recode.toml"),
            r#"
state_dir = ".file/state"
log_level = "info"
default_provider = "file-provider"
default_timeout_secs = 60
default_max_attempts = 2
approval_policy = "manual"
"#,
        )
        .unwrap();
        let loader = ConfigLoader::with_env(
            temp.path(),
            PartialConfig {
                state_dir: Some(PathBuf::from(".env/state")),
                log_level: Some("warn".into()),
                default_provider: Some("env-provider".into()),
                provider_mode: Some(ProviderMode::Codex),
                provider_base_url: Some("https://env.invalid/v1".into()),
                provider_api_key_env: Some("ENV_API_KEY".into()),
                provider_model: Some("env-model".into()),
                default_timeout_secs: Some(90),
                default_max_attempts: Some(4),
                approval_policy: Some(ApprovalPolicy::Never),
            },
        );

        let config = loader
            .load(
                None,
                PartialConfig {
                    state_dir: Some(PathBuf::from(".cli/state")),
                    log_level: None,
                    default_provider: Some("cli-provider".into()),
                    provider_mode: Some(ProviderMode::OpenAiCompatible),
                    provider_base_url: Some("https://cli.invalid/v1".into()),
                    provider_api_key_env: Some("CLI_API_KEY".into()),
                    provider_model: Some("cli-model".into()),
                    default_timeout_secs: Some(120),
                    default_max_attempts: Some(5),
                    approval_policy: Some(ApprovalPolicy::Manual),
                },
            )
            .unwrap();

        assert_eq!(config.state_dir, PathBuf::from(".cli/state"));
        assert_eq!(config.log_level, "warn");
        assert_eq!(config.default_provider, "cli-provider");
        assert_eq!(config.provider.name, "cli-provider");
        assert_eq!(config.provider.mode, ProviderMode::OpenAiCompatible);
        assert_eq!(
            config.provider.base_url.as_deref(),
            Some("https://cli.invalid/v1")
        );
        assert_eq!(config.provider.api_key_env, "CLI_API_KEY");
        assert_eq!(config.provider.model, "cli-model");
        assert_eq!(config.default_timeout_secs, 120);
        assert_eq!(config.default_max_attempts, 5);
        assert_eq!(config.approval_policy, ApprovalPolicy::Manual);
    }
}
