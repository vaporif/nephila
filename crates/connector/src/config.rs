use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub struct SpawnConfig {
    pub directory: PathBuf,
    pub mcp_endpoint: String,
    pub request_config: Option<RequestConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestConfig {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub system: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_config_roundtrip_json() {
        let cfg = RequestConfig {
            model: "claude-sonnet-4-6-20250514".into(),
            max_tokens: 4096,
            temperature: Some(0.7),
            system: Some("You are helpful.".into()),
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let decoded: RequestConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.model, "claude-sonnet-4-6-20250514");
        assert_eq!(decoded.max_tokens, 4096);
    }

    #[test]
    fn spawn_config_holds_directory_and_endpoint() {
        let cfg = SpawnConfig {
            directory: PathBuf::from("/tmp/work"),
            mcp_endpoint: "http://localhost:8080/mcp".into(),
            request_config: None,
        };
        assert_eq!(cfg.directory, PathBuf::from("/tmp/work"));
        assert_eq!(cfg.mcp_endpoint, "http://localhost:8080/mcp");
    }
}
