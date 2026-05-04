use chrono::{TimeZone, Utc};
use nephila_connector::event_draft::SessionEventDraft;
use nephila_tui::panels::session_pane::SessionPane;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use uuid::Uuid;

#[test]
fn renders_human_assistant_and_completion_rows() {
    let backend = TestBackend::new(60, 6);
    let mut term = Terminal::new(backend).expect("terminal");

    let mut pane = SessionPane::new();
    let ts = Utc.with_ymd_and_hms(2026, 5, 4, 12, 34, 56).unwrap();
    let turn_id = Uuid::new_v4();
    pane.push_draft(SessionEventDraft::HumanPromptQueued {
        turn_id,
        text: "echo OK".into(),
        ts,
    });
    pane.push_draft(SessionEventDraft::AssistantMessage {
        message_id: "msg-1".into(),
        seq_in_message: 0,
        delta_text: "OK".into(),
        is_final: true,
        ts,
    });
    pane.push_draft(SessionEventDraft::TurnCompleted {
        turn_id,
        stop_reason: "end_turn".into(),
        ts,
    });

    term.draw(|f| f.render_widget(&pane, f.area()))
        .expect("draw");

    let buf = term.backend().buffer().clone();
    let text: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();

    assert!(text.contains("YOU →"), "missing YOU →; rendered = {text:?}");
    assert!(
        text.contains("ASSIST"),
        "missing ASSIST; rendered = {text:?}"
    );
    assert!(text.contains('✓'), "missing ✓; rendered = {text:?}");
}
