use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Directive {
    Continue,
    PrepareReset,
    Pause,
    Abort,
}

impl Directive {
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(Self::Continue)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectiveResponse {
    pub directive: Directive,
    pub reason: Option<String>,
    pub metadata: DirectiveMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectiveMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_estimated: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injected_message: Option<String>,
}
