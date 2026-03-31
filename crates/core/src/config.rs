use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeridianConfig {
    pub meridian: CoreConfig,
    pub lifecycle: LifecycleConfig,
    pub supervision: SupervisionConfig,
    pub summarizer: SummarizerConfig,
    pub memory: MemoryConfig,
    pub tui: TuiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(default = "default_storage_backend")]
    pub storage_backend: String,
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_claude_binary")]
    pub claude_binary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LifecycleConfig {
    #[serde(default = "default_context_threshold_pct")]
    pub context_threshold_pct: u8,
    #[serde(default = "default_context_window_size")]
    pub context_window_size: u64,
    #[serde(default = "default_token_warn_pct")]
    pub token_warn_pct: u8,
    #[serde(default = "default_token_critical_pct")]
    pub token_critical_pct: u8,
    #[serde(default = "default_token_force_kill_pct")]
    pub token_force_kill_pct: u8,
    #[serde(default = "default_report_interval_normal")]
    pub token_report_interval_normal: u32,
    #[serde(default = "default_report_interval_warn")]
    pub token_report_interval_warn: u32,
    #[serde(default = "default_report_interval_critical")]
    pub token_report_interval_critical: u32,
    #[serde(default = "default_hang_timeout_secs")]
    pub hang_timeout_secs: u64,
    #[serde(default = "default_drain_timeout_secs")]
    pub drain_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisionConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: String,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_window_secs")]
    pub restart_window_secs: u64,
    #[serde(default = "default_max_agent_depth")]
    pub max_agent_depth: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizerConfig {
    #[serde(default = "default_summarizer_backend")]
    pub backend: String,
    pub local_llm_endpoint: Option<String>,
    pub local_llm_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_novelty_threshold")]
    pub novelty_threshold: f32,
    #[serde(default = "default_link_similarity_threshold")]
    pub link_similarity_threshold: f32,
    #[serde(default = "default_decay_access_threshold")]
    pub decay_access_threshold: u32,
    #[serde(default = "default_archive_after_cycles")]
    pub archive_after_cycles: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_refresh_rate_ms")]
    pub refresh_rate_ms: u64,
    #[serde(default = "default_max_event_log_lines")]
    pub max_event_log_lines: usize,
    #[serde(default)]
    pub hitl_timeout_secs: u64,
    #[serde(default = "default_max_hitl_rerequests")]
    pub max_hitl_rerequests: u32,
}

fn default_storage_backend() -> String {
    "sqlite".into()
}
fn default_sqlite_path() -> PathBuf {
    PathBuf::from("./meridian.db")
}
fn default_embedding_model() -> String {
    "BGESmallENV15".into()
}
fn default_claude_binary() -> String {
    "claude".into()
}
fn default_context_threshold_pct() -> u8 {
    80
}
fn default_context_window_size() -> u64 {
    200_000
}
fn default_token_warn_pct() -> u8 {
    60
}
fn default_token_critical_pct() -> u8 {
    75
}
fn default_token_force_kill_pct() -> u8 {
    85
}
fn default_report_interval_normal() -> u32 {
    10
}
fn default_report_interval_warn() -> u32 {
    3
}
fn default_report_interval_critical() -> u32 {
    1
}
fn default_hang_timeout_secs() -> u64 {
    300
}
fn default_drain_timeout_secs() -> u64 {
    60
}
fn default_strategy() -> String {
    "one_for_one".into()
}
fn default_max_restarts() -> u32 {
    5
}
fn default_restart_window_secs() -> u64 {
    60
}
fn default_max_agent_depth() -> u32 {
    3
}
fn default_summarizer_backend() -> String {
    "claude".into()
}
fn default_novelty_threshold() -> f32 {
    0.95
}
fn default_link_similarity_threshold() -> f32 {
    0.75
}
fn default_decay_access_threshold() -> u32 {
    5
}
fn default_archive_after_cycles() -> u32 {
    50
}
fn default_refresh_rate_ms() -> u64 {
    100
}
fn default_max_event_log_lines() -> usize {
    1000
}
fn default_max_hitl_rerequests() -> u32 {
    3
}

impl Default for MeridianConfig {
    fn default() -> Self {
        toml::from_str("[meridian]\n[lifecycle]\n[supervision]\n[summarizer]\n[memory]\n[tui]\n")
            .expect("default config must parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_minimal_config() {
        let toml_str = r#"
[meridian]
sqlite_path = "./test.db"

[lifecycle]

[supervision]

[summarizer]

[memory]

[tui]
"#;
        let config: MeridianConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.lifecycle.context_threshold_pct, 80);
        assert_eq!(config.memory.novelty_threshold, 0.95);
    }

    #[test]
    fn test_deserialize_full_config() {
        let toml_str = r#"
[meridian]
storage_backend = "sqlite"
sqlite_path = "./meridian.db"
embedding_model = "text-embedding-3-small"
claude_binary = "claude"

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
"#;
        let config: MeridianConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.meridian.storage_backend, "sqlite");
        assert_eq!(config.supervision.max_restarts, 5);
        assert_eq!(config.supervision.max_agent_depth, 3);
    }
}
