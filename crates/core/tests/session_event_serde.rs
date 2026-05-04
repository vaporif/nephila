use chrono::{TimeZone, Utc};
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session_event::{InterruptSnapshot, SessionEvent, ToolResultPayload};
use std::path::PathBuf;
use uuid::Uuid;

fn ts() -> chrono::DateTime<chrono::Utc> {
    Utc.with_ymd_and_hms(2026, 5, 4, 12, 0, 0).unwrap()
}

fn roundtrip(event: &SessionEvent) {
    let json = serde_json::to_string(event).expect("serialize");
    let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(*event, back, "roundtrip mismatch for {}", event.kind());
}

#[test]
fn session_started_roundtrips() {
    roundtrip(&SessionEvent::SessionStarted {
        session_id: Uuid::nil(),
        agent_id: AgentId(Uuid::nil()),
        model: Some("claude-3-5-sonnet".into()),
        working_dir: PathBuf::from("/tmp/work"),
        mcp_endpoint: "http://localhost:3000".into(),
        resumed: false,
        ts: ts(),
    });
}

#[test]
fn prompt_lifecycle_roundtrips() {
    roundtrip(&SessionEvent::AgentPromptQueued {
        turn_id: Uuid::nil(),
        text: "hello".into(),
        ts: ts(),
    });
    roundtrip(&SessionEvent::AgentPromptDelivered {
        turn_id: Uuid::nil(),
        ts: ts(),
    });
    roundtrip(&SessionEvent::HumanPromptQueued {
        turn_id: Uuid::nil(),
        text: "hi".into(),
        ts: ts(),
    });
    roundtrip(&SessionEvent::HumanPromptDelivered {
        turn_id: Uuid::nil(),
        ts: ts(),
    });
    roundtrip(&SessionEvent::PromptDeliveryFailed {
        turn_id: Uuid::nil(),
        reason: "broken pipe".into(),
        ts: ts(),
    });
}

#[test]
fn assistant_message_roundtrips() {
    roundtrip(&SessionEvent::AssistantMessage {
        message_id: "m1".into(),
        seq_in_message: 0,
        delta_text: "hello".into(),
        is_final: true,
        truncated: false,
        ts: ts(),
    });
}

#[test]
fn tool_call_and_result_roundtrip() {
    roundtrip(&SessionEvent::ToolCall {
        tool_use_id: "tu1".into(),
        tool_name: "read".into(),
        input: serde_json::json!({"path": "/tmp"}),
        ts: ts(),
    });
    roundtrip(&SessionEvent::ToolResult {
        tool_use_id: "tu1".into(),
        output: ToolResultPayload::Inline(serde_json::json!({"ok": true})),
        is_error: false,
        ts: ts(),
    });
    roundtrip(&SessionEvent::ToolResult {
        tool_use_id: "tu1".into(),
        output: ToolResultPayload::Spilled {
            hash: "deadbeef".into(),
            original_len: 1024,
            snippet: "first KB...".into(),
        },
        is_error: false,
        ts: ts(),
    });
}

#[test]
fn checkpoint_reached_with_interrupt_variants_roundtrip() {
    roundtrip(&SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::nil()),
        interrupt: None,
        ts: ts(),
    });
    roundtrip(&SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::nil()),
        interrupt: Some(InterruptSnapshot::Hitl {
            question: "ok?".into(),
            options: vec!["yes".into(), "no".into()],
        }),
        ts: ts(),
    });
    roundtrip(&SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::nil()),
        interrupt: Some(InterruptSnapshot::Pause),
        ts: ts(),
    });
    roundtrip(&SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(Uuid::nil()),
        interrupt: Some(InterruptSnapshot::Drain),
        ts: ts(),
    });
}

#[test]
fn turn_completed_aborted_roundtrip() {
    roundtrip(&SessionEvent::TurnCompleted {
        turn_id: Uuid::nil(),
        stop_reason: "end_turn".into(),
        ts: ts(),
    });
    roundtrip(&SessionEvent::TurnAborted {
        turn_id: Uuid::nil(),
        reason: "interrupted".into(),
        ts: ts(),
    });
}

#[test]
fn session_crashed_and_ended_roundtrip() {
    roundtrip(&SessionEvent::SessionCrashed {
        reason: "exit non-zero".into(),
        exit_code: Some(1),
        ts: ts(),
    });
    roundtrip(&SessionEvent::SessionEnded { ts: ts() });
}

#[test]
fn type_tag_is_pascal_case_per_serde_default() {
    let json = serde_json::to_value(&SessionEvent::SessionEnded { ts: ts() }).unwrap();
    assert_eq!(json["type"], "SessionEnded");
}
