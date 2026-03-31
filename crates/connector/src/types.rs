use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // serde_json::Value contains f64
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // contains ContentBlock
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // serde_json::Value contains f64
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)] // contains ContentBlock
pub struct Response {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip_json() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let decoded: Message = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, decoded);
    }

    #[test]
    fn tool_use_content_block_serializes() {
        let block = ContentBlock::ToolUse {
            id: "tu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/foo"}),
        };
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(json.contains("\"type\":\"tool_use\""));
        assert!(json.contains("\"name\":\"read_file\""));
    }

    #[test]
    fn tool_result_content_block_serializes() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "file contents".into(),
            is_error: false,
        };
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(json.contains("\"type\":\"tool_result\""));
    }

    #[test]
    fn response_roundtrip_json() {
        let resp = Response {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
            },
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let decoded: Response = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(resp, decoded);
    }

    #[test]
    fn stop_reason_variants_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&StopReason::EndTurn).expect("serialize"),
            "\"end_turn\""
        );
        assert_eq!(
            serde_json::to_string(&StopReason::MaxTokens).expect("serialize"),
            "\"max_tokens\""
        );
    }
}
