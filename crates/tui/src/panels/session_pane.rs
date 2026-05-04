//! Minimal `SessionPane` rendering finalized assistant messages and prompts.
//!
//! Slice 1b: read-only consumer of `nephila_core::SessionEvent`. Slice 2 adds
//! the `tui-textarea` input box.
//!
//! Glyphs follow spec `§SessionPane.Pane behavior`:
//!   `YOU →` (HumanPromptQueued), `AGENT →` (AgentPromptQueued),
//!   `ASSIST` (AssistantMessage final), `TOOL` (ToolCall),
//!   `↳ result` (ToolResult), `✓ CHECKPOINT` (CheckpointReached),
//!   `✓ END` (TurnCompleted), `↺ ABORT` (TurnAborted),
//!   `✗ CRASH` (SessionCrashed).

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use nephila_core::session_event::SessionEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};

use crate::layout::{focused_border_style, focused_border_type};

#[derive(Debug, Clone)]
pub struct RenderedRow {
    pub glyph: &'static str,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

pub struct SessionPane {
    pub events: VecDeque<RenderedRow>,
    pub focused: bool,
}

impl Default for SessionPane {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionPane {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            events: VecDeque::new(),
            focused: false,
        }
    }

    /// Append a row for `ev`. Non-user-visible events (`SessionStarted`,
    /// `*PromptDelivered`, in-flight `AssistantMessage` chunks) are dropped.
    pub fn push_event(&mut self, ev: SessionEvent) {
        if let Some(row) = render_row(ev) {
            self.events.push_back(row);
        }
    }
}

fn render_row(ev: SessionEvent) -> Option<RenderedRow> {
    match ev {
        SessionEvent::HumanPromptQueued { text, ts, .. } => Some(RenderedRow {
            glyph: "YOU →",
            text,
            timestamp: ts,
        }),
        SessionEvent::AgentPromptQueued { text, ts, .. } => Some(RenderedRow {
            glyph: "AGENT →",
            text,
            timestamp: ts,
        }),
        SessionEvent::AssistantMessage {
            delta_text,
            is_final: true,
            ts,
            ..
        } => Some(RenderedRow {
            glyph: "ASSIST",
            text: delta_text,
            timestamp: ts,
        }),
        SessionEvent::ToolCall { tool_name, ts, .. } => Some(RenderedRow {
            glyph: "TOOL",
            text: tool_name,
            timestamp: ts,
        }),
        SessionEvent::ToolResult {
            tool_use_id,
            is_error,
            ts,
            ..
        } => Some(RenderedRow {
            glyph: "↳ result",
            text: if is_error {
                format!("ERR ({tool_use_id})")
            } else {
                format!("ok ({tool_use_id})")
            },
            timestamp: ts,
        }),
        SessionEvent::CheckpointReached {
            checkpoint_id, ts, ..
        } => Some(RenderedRow {
            glyph: "✓ CHECKPOINT",
            text: checkpoint_id.to_string(),
            timestamp: ts,
        }),
        SessionEvent::TurnCompleted {
            stop_reason, ts, ..
        } => Some(RenderedRow {
            glyph: "✓ END",
            text: stop_reason,
            timestamp: ts,
        }),
        SessionEvent::TurnAborted { reason, ts, .. } => Some(RenderedRow {
            glyph: "↺ ABORT",
            text: reason,
            timestamp: ts,
        }),
        SessionEvent::SessionCrashed { reason, ts, .. } => Some(RenderedRow {
            glyph: "✗ CRASH",
            text: reason.lines().next().unwrap_or("").to_owned(),
            timestamp: ts,
        }),
        SessionEvent::PromptDeliveryFailed { reason, ts, .. } => Some(RenderedRow {
            glyph: "✗ DROP",
            text: reason,
            timestamp: ts,
        }),
        // Drop: in-flight deltas, lifecycle markers, *PromptDelivered.
        SessionEvent::AssistantMessage {
            is_final: false, ..
        }
        | SessionEvent::SessionStarted { .. }
        | SessionEvent::SessionEnded { .. }
        | SessionEvent::HumanPromptDelivered { .. }
        | SessionEvent::AgentPromptDelivered { .. } => None,
    }
}

impl Widget for &SessionPane {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(focused_border_style(self.focused))
            .border_type(focused_border_type(self.focused))
            .title("Session");
        let items: Vec<ListItem> = self
            .events
            .iter()
            .map(|r| {
                ListItem::new(format!(
                    "{} {} {}",
                    r.timestamp.format("%H:%M:%S"),
                    r.glyph,
                    r.text
                ))
            })
            .collect();
        List::new(items).block(block).render(area, buf);
    }
}
