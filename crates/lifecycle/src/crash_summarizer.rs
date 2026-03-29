use meridian_core::checkpoint::{L0State, L2Chunk, ObjectiveSnapshot};
use meridian_core::error::Result;
use meridian_core::event::{EventType, McpEvent};
use meridian_core::id::EntryId;
use meridian_core::objective::{ObjectiveNode, ObjectiveTree};
use meridian_core::summarizer::Summarizer;

/// Generates checkpoint layers from MCP logs without an LLM — used after crashes.
#[derive(Default)]
pub struct CrashSummarizer;

impl CrashSummarizer {
    fn collect_snapshots(node: &ObjectiveNode, out: &mut Vec<ObjectiveSnapshot>) {
        out.push(ObjectiveSnapshot {
            id: node.id,
            description: node.description.clone(),
            status: node.status.to_string(),
        });
        for child in &node.children {
            Self::collect_snapshots(child, out);
        }
    }
}

impl Summarizer for CrashSummarizer {
    async fn generate_l0(
        &self,
        _mcp_log: &[McpEvent],
        objectives: &ObjectiveTree,
    ) -> Result<L0State> {
        let mut snapshots = Vec::new();
        Self::collect_snapshots(&objectives.root, &mut snapshots);

        Ok(L0State {
            objectives: snapshots,
            next_steps: vec!["Resume from last checkpoint".to_owned()],
        })
    }

    async fn generate_l1(
        &self,
        mcp_log: &[McpEvent],
        _objectives: &ObjectiveTree,
    ) -> Result<String> {
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
            lines.push(format!("- [{event_label}] {tool_name} at {}", event.timestamp));
        }

        Ok(lines.join("\n"))
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
    use meridian_core::id::{AgentId, ObjectiveId};
    use meridian_core::objective::{ObjectiveNode, ObjectiveStatus, ObjectiveTree};

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
    async fn generate_l1_with_mock_log() {
        let summarizer = CrashSummarizer;
        let events = vec![
            make_event(EventType::ToolCall, "read_file"),
            make_event(EventType::ToolResult, "read_file"),
            make_event(EventType::ToolCall, "write_file"),
            make_event(EventType::ToolResult, "write_file"),
            make_event(EventType::StateChange, "ignored"),
        ];
        let tree = make_tree();

        let summary = summarizer.generate_l1(&events, &tree).await.unwrap();
        assert!(!summary.is_empty());
        assert!(summary.contains("read_file"));
        assert!(summary.contains("write_file"));
        assert!(!summary.contains("ignored"));
    }
}
