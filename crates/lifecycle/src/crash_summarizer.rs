use nephila_core::checkpoint::{ChannelEntry, L2Chunk, ReducerKind};
use nephila_core::error::Result;
use nephila_core::event::{EventType, McpEvent};
use nephila_core::id::EntryId;
use nephila_core::objective::{ObjectiveNode, ObjectiveTree};
use nephila_core::summarizer::Summarizer;
use std::collections::BTreeMap;

/// Generates checkpoint channels from MCP logs without an LLM — used after crashes.
#[derive(Default)]
pub struct CrashSummarizer;

impl CrashSummarizer {
    fn collect_objective_values(node: &ObjectiveNode, out: &mut Vec<serde_json::Value>) {
        out.push(serde_json::json!({
            "id": node.id,
            "description": node.description,
            "status": node.status.to_string(),
        }));
        for child in &node.children {
            Self::collect_objective_values(child, out);
        }
    }
}

impl Summarizer for CrashSummarizer {
    async fn generate_channels(
        &self,
        mcp_log: &[McpEvent],
        objectives: &ObjectiveTree,
    ) -> Result<BTreeMap<String, ChannelEntry>> {
        let mut channels = BTreeMap::new();

        // objectives channel (overwrite reducer)
        let mut obj_values = Vec::new();
        Self::collect_objective_values(&objectives.root, &mut obj_values);
        channels.insert(
            "objectives".to_owned(),
            ChannelEntry {
                reducer: ReducerKind::Overwrite,
                value: serde_json::Value::Array(obj_values),
            },
        );

        // progress_summary channel (overwrite reducer)
        let filtered: Vec<&McpEvent> = mcp_log
            .iter()
            .filter(|e| matches!(e.event_type, EventType::ToolCall | EventType::ToolResult))
            .collect();
        let start = filtered.len().saturating_sub(20);
        let recent = &filtered[start..];

        let mut lines = Vec::with_capacity(recent.len() + 1);
        lines.push("Recent tool activity:".to_owned());
        for event in recent {
            let tool_name = event
                .content
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let event_label = match event.event_type {
                EventType::ToolCall => "call",
                EventType::ToolResult => "result",
                _ => "event",
            };
            lines.push(format!(
                "- [{event_label}] {tool_name} at {}",
                event.timestamp
            ));
        }
        channels.insert(
            "progress_summary".to_owned(),
            ChannelEntry {
                reducer: ReducerKind::Overwrite,
                value: serde_json::Value::String(lines.join("\n")),
            },
        );

        // decisions channel (append reducer, starts empty)
        channels.insert(
            "decisions".to_owned(),
            ChannelEntry {
                reducer: ReducerKind::Append,
                value: serde_json::Value::Array(vec![]),
            },
        );

        // blockers channel (append reducer, starts empty)
        channels.insert(
            "blockers".to_owned(),
            ChannelEntry {
                reducer: ReducerKind::Append,
                value: serde_json::Value::Array(vec![]),
            },
        );

        Ok(channels)
    }

    async fn generate_l2(&self, mcp_log: &[McpEvent]) -> Result<Vec<L2Chunk>> {
        let chunk_size = 10;
        let chunks: Vec<L2Chunk> = mcp_log
            .chunks(chunk_size)
            .map(|chunk| {
                let content_lines: Vec<String> = chunk
                    .iter()
                    .map(|e| {
                        let tool = e
                            .content
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        format!("[{:?}] {} at {}", e.event_type, tool, e.timestamp)
                    })
                    .collect();

                let tags: Vec<String> = chunk
                    .iter()
                    .filter_map(|e| {
                        e.content
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_owned())
                    })
                    .collect();

                L2Chunk {
                    id: EntryId::new(),
                    content: content_lines.join("\n"),
                    tags,
                }
            })
            .collect();

        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nephila_core::id::{AgentId, ObjectiveId};
    use nephila_core::objective::{ObjectiveNode, ObjectiveStatus, ObjectiveTree};

    fn make_event(event_type: EventType, tool: &str) -> McpEvent {
        McpEvent {
            id: uuid::Uuid::new_v4(),
            agent_id: AgentId::new(),
            event_type,
            timestamp: Utc::now(),
            content: serde_json::json!({ "tool": tool }),
            objective_id: None,
        }
    }

    fn make_tree() -> ObjectiveTree {
        let now = Utc::now();
        ObjectiveTree {
            root: ObjectiveNode {
                id: ObjectiveId::new(),
                parent_id: None,
                agent_id: None,
                description: "Root objective".into(),
                status: ObjectiveStatus::InProgress,
                children: vec![],
                created_at: now,
                updated_at: now,
            },
        }
    }

    #[tokio::test]
    async fn generate_channels_with_mock_log() {
        let summarizer = CrashSummarizer;
        let events = vec![
            make_event(EventType::ToolCall, "read_file"),
            make_event(EventType::ToolResult, "read_file"),
            make_event(EventType::ToolCall, "write_file"),
            make_event(EventType::ToolResult, "write_file"),
            make_event(EventType::StateChange, "ignored"),
        ];
        let tree = make_tree();

        let channels = summarizer.generate_channels(&events, &tree).await.unwrap();

        // Check all required channels are present
        assert!(channels.contains_key("objectives"));
        assert!(channels.contains_key("progress_summary"));
        assert!(channels.contains_key("decisions"));
        assert!(channels.contains_key("blockers"));

        // progress_summary should contain tool names but not ignored events
        let summary = channels["progress_summary"].value.as_str().unwrap();
        assert!(summary.contains("read_file"));
        assert!(summary.contains("write_file"));
        assert!(!summary.contains("ignored"));

        // objectives should be an array with the root objective
        let objs = channels["objectives"].value.as_array().unwrap();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0]["description"], "Root objective");

        // decisions and blockers should be empty arrays
        assert_eq!(channels["decisions"].value.as_array().unwrap().len(), 0);
        assert_eq!(channels["blockers"].value.as_array().unwrap().len(), 0);
    }
}
