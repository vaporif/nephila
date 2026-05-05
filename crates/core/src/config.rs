use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NephilaConfig {
    #[serde(default)]
    pub nephila: CoreConfig,
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub supervision: SupervisionConfig,
    #[serde(default)]
    pub summarizer: SummarizerConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub connector: ConnectorConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    #[default]
    Sqlite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreConfig {
    pub storage_backend: StorageBackend,
    pub sqlite_path: PathBuf,
    pub ferrex_config_path: Option<PathBuf>,
    pub l2_collection: String,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            storage_backend: StorageBackend::Sqlite,
            sqlite_path: PathBuf::from("./nephila.db"),
            ferrex_config_path: None,
            l2_collection: "nephila_l2_chunks".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct LifecycleConfig {
    pub context_threshold_pct: u8,
    pub context_window_size: u64,
    pub token_warn_pct: u8,
    pub token_critical_pct: u8,
    pub token_force_kill_pct: u8,
    pub token_report_interval_normal: u32,
    pub token_report_interval_warn: u32,
    pub token_report_interval_critical: u32,
    pub hang_timeout_secs: u64,
    pub drain_timeout_secs: u64,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            context_threshold_pct: 80,
            context_window_size: 200_000,
            token_warn_pct: 60,
            token_critical_pct: 75,
            token_force_kill_pct: 85,
            token_report_interval_normal: 10,
            token_report_interval_warn: 3,
            token_report_interval_critical: 1,
            hang_timeout_secs: 300,
            drain_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SupervisionConfig {
    pub default_strategy: String,
    pub max_restarts: u32,
    pub restart_window_secs: u64,
    pub max_agent_depth: u32,
}

impl Default for SupervisionConfig {
    fn default() -> Self {
        Self {
            default_strategy: "one_for_one".into(),
            // Counts crashes (not turn exits). With a streaming session the
            // supervisor records a restart only on `SessionCrashed`, which is
            // rarer than the old per-turn `claude -p` exit signal.
            max_restarts: 5,
            // 10-minute window: assumes each crash takes ~30s of recovery
            // (kill, lockfile, respawn, replay).
            restart_window_secs: 600,
            max_agent_depth: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizerConfig {
    pub backend: String,
    pub local_llm_endpoint: Option<String>,
    pub local_llm_model: Option<String>,
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self {
            backend: "claude".into(),
            local_llm_endpoint: None,
            local_llm_model: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub novelty_threshold: f32,
    pub link_similarity_threshold: f32,
    pub decay_access_threshold: u32,
    pub archive_after_cycles: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            novelty_threshold: 0.95,
            link_similarity_threshold: 0.75,
            decay_access_threshold: 5,
            archive_after_cycles: 50,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    pub refresh_rate_ms: u64,
    pub max_event_log_lines: usize,
    pub hitl_timeout_secs: u64,
    pub max_hitl_rerequests: u32,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            refresh_rate_ms: 100,
            max_event_log_lines: 1000,
            hitl_timeout_secs: 0,
            max_hitl_rerequests: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    pub claude_binary: String,
    pub anthropic_api_key_env: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_api_key_env: Option<String>,
    pub default_model: Option<String>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            claude_binary: "claude".into(),
            anthropic_api_key_env: None,
            openai_base_url: None,
            openai_api_key_env: None,
            default_model: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub host: String,
    pub port: u16,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_minimal_config() {
        let toml_str = r#"
[nephila]
sqlite_path = "./test.db"

[lifecycle]

[supervision]

[summarizer]

[memory]

[tui]
"#;
        let config: NephilaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.lifecycle.context_threshold_pct, 80);
        assert!((config.memory.novelty_threshold - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn test_deserialize_full_config() {
        let toml_str = r#"
[nephila]
storage_backend = "sqlite"
sqlite_path = "./nephila.db"
l2_collection = "nephila_l2_chunks"

[lifecycle]
context_threshold_pct = 80
context_window_size = 200000
token_warn_pct = 60
token_critical_pct = 75
token_force_kill_pct = 85
hang_timeout_secs = 300
drain_timeout_secs = 60

[supervision]
default_strategy = "one_for_one"
max_restarts = 5
restart_window_secs = 60
max_agent_depth = 3

[summarizer]
backend = "claude"

[memory]
novelty_threshold = 0.95
link_similarity_threshold = 0.75

[tui]
refresh_rate_ms = 100
max_event_log_lines = 1000
max_hitl_rerequests = 3

[connector]
claude_binary = "claude"
"#;
        let config: NephilaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.nephila.storage_backend, StorageBackend::Sqlite);
        assert_eq!(config.supervision.max_restarts, 5);
        assert_eq!(config.supervision.max_agent_depth, 3);
        assert_eq!(config.connector.claude_binary, "claude");
    }

    #[test]
    fn test_deserialize_config_with_connector() {
        let toml_str = r#"
[nephila]
[lifecycle]
[supervision]
[summarizer]
[memory]
[tui]
[connector]
claude_binary = "/usr/local/bin/claude"
anthropic_api_key_env = "MY_KEY"
"#;
        let config: NephilaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.connector.claude_binary, "/usr/local/bin/claude");
        assert_eq!(config.connector.anthropic_api_key_env.unwrap(), "MY_KEY");
    }

    #[test]
    fn test_default_config_has_connector() {
        let config = NephilaConfig::default();
        assert_eq!(config.connector.claude_binary, "claude");
    }

    #[test]
    fn test_mcp_config_defaults() {
        let config = NephilaConfig::default();
        assert_eq!(config.mcp.host, "127.0.0.1");
        assert_eq!(config.mcp.port, 0);
    }

    #[test]
    fn test_mcp_config_explicit_port() {
        let toml_str = r#"
[nephila]
[lifecycle]
[supervision]
[summarizer]
[memory]
[tui]
[mcp]
host = "0.0.0.0"
port = 8080
"#;
        let config: NephilaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.mcp.host, "0.0.0.0");
        assert_eq!(config.mcp.port, 8080);
    }
}
