//! `SessionPane` — embedded TUI panel for a single agent's session.
//!
//! Slice 1b: read-only consumer of `nephila_core::SessionEvent`. Slice 2 adds
//! the input box (`input` submodule), the per-agent pump task that pushes
//! events into the shared `RenderedRow` buffer, and the `bind_session` /
//! `submit_text` plumbing for human prompts.
//!
//! Glyphs follow spec `§SessionPane.Pane behavior`:
//!   `YOU →` (HumanPromptQueued), `AGENT →` (AgentPromptQueued),
//!   `ASSIST` (AssistantMessage final), `TOOL` (ToolCall),
//!   `↳ result` (ToolResult), `✓ CHECKPOINT` (CheckpointReached),
//!   `✓ END` (TurnCompleted), `↺ ABORT` (TurnAborted),
//!   `✗ CRASH` (SessionCrashed).

pub mod input;
pub mod pump;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use nephila_connector::session::{ClaudeCodeSession, PromptSource};
use nephila_core::session_event::SessionEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};

use crate::layout::{focused_border_style, focused_border_type};

/// Shared, unbounded queue of rendered rows produced by a per-agent pump task.
/// `Mutex<VecDeque<_>>` is fine here: the pump task pushes from one tokio
/// worker, the TUI render thread drains. Lock duration is microseconds —
/// shorter than any await point, so no async-aware lock is required.
pub type RenderedBuffer = Arc<Mutex<VecDeque<RenderedRow>>>;

#[derive(Debug, Clone)]
pub struct RenderedRow {
    pub glyph: &'static str,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

pub struct SessionPane {
    /// Per-agent buffer fed by a pump task. Wrapped in `Arc<Mutex<_>>` so the
    /// pump and the renderer can share it. Slice 4 hands the registry an
    /// `Arc::clone` of this; slice 2 wires it manually in tests / demo.
    pub buffer: RenderedBuffer,
    pub focused: bool,
    pub input: input::InputState,
    /// Slice 2: the bound session (or `None` if no agent is focused). Holds
    /// an `Arc` so the writer task (spawned per-submit) outlives the pane's
    /// borrow window.
    session: Option<Arc<ClaudeCodeSession>>,
    /// Marker kept on-screen while we wait for the assistant to acknowledge
    /// a HITL prompt. Drained by the App's tick loop after the modal closes.
    pub awaiting_human_input: bool,
}

impl Default for SessionPane {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionPane {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            focused: false,
            input: input::InputState::new(),
            session: None,
            awaiting_human_input: false,
        }
    }

    /// Bind a freshly started session to the pane. The pane uses the handle to
    /// route human-typed prompts via `send_turn(PromptSource::Human, ...)`.
    pub fn bind_session(&mut self, session: Arc<ClaudeCodeSession>) {
        self.session = Some(session);
    }

    /// Drop the bound session handle. Used at shutdown to release the `Arc`
    /// so the caller can `Arc::try_unwrap` the session and call `shutdown()`.
    pub fn unbind_session(&mut self) {
        self.session = None;
    }

    #[must_use]
    pub fn buffer(&self) -> RenderedBuffer {
        Arc::clone(&self.buffer)
    }

    /// Submit a freshly typed prompt. Spawns the `send_turn` await on a
    /// detached tokio task so the TUI tick loop never blocks on stdin write
    /// backpressure (the connector's mpsc bound is 16; `send_turn` only ever
    /// awaits when 16 turns are already inflight).
    #[tracing::instrument(level = "debug", skip(self, text), fields(len = text.len()))]
    pub fn submit_text(&self, text: String) {
        let Some(session) = self.session.clone() else {
            tracing::warn!(
                target: "nephila_tui::session_pane",
                "submit_text called without a bound session; dropping prompt",
            );
            return;
        };
        tokio::spawn(async move {
            if let Err(e) = session.send_turn(PromptSource::Human, text).await {
                tracing::warn!(
                    target: "nephila_tui::session_pane",
                    error = %e,
                    "send_turn(Human) failed",
                );
            }
        });
    }

    /// Append a row for `ev` directly into the buffer. Used by tests and the
    /// legacy push API; the production path uses the per-agent pump task.
    pub fn push_event(&self, ev: SessionEvent) {
        if matches!(
            ev,
            SessionEvent::CheckpointReached {
                interrupt: Some(_),
                ..
            }
        ) {
            // Marker handled by the App-level HITL modal; render a placeholder
            // row so the operator sees something on the pane.
            if let Ok(mut buf) = self.buffer.lock() {
                buf.push_back(RenderedRow {
                    glyph: "↳ awaiting",
                    text: "human input".into(),
                    timestamp: Utc::now(),
                });
            }
            return;
        }
        if let Some(row) = render_row(ev)
            && let Ok(mut buf) = self.buffer.lock()
        {
            buf.push_back(row);
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
        let items: Vec<ListItem> = self.buffer.lock().map_or_else(
            |_| Vec::new(),
            |g| {
                g.iter()
                    .map(|r| {
                        ListItem::new(format!(
                            "{} {} {}",
                            r.timestamp.format("%H:%M:%S"),
                            r.glyph,
                            r.text
                        ))
                    })
                    .collect()
            },
        );
        List::new(items).block(block).render(area, buf);
    }
}

/// Collect a snapshot of the rendered rows as a single text blob. Used by
/// tests to assert the pane shows the human prompt without driving a real
/// `TestBackend` render.
#[must_use]
pub fn snapshot_text(buf: &RenderedBuffer) -> String {
    buf.lock().map_or_else(
        |_| String::new(),
        |g| {
            g.iter()
                .map(|r| format!("{} {}", r.glyph, r.text))
                .collect::<Vec<_>>()
                .join("\n")
        },
    )
}

/// Translate a single `SessionEvent` into a `RenderedRow` and append it to
/// `buf`. Used by the pump task; symmetric with `push_event` but operates on
/// the shared buffer directly.
pub fn append_event(buf: &RenderedBuffer, ev: SessionEvent) {
    if let SessionEvent::CheckpointReached {
        interrupt: Some(_), ..
    } = &ev
    {
        if let Ok(mut g) = buf.lock() {
            g.push_back(RenderedRow {
                glyph: "↳ awaiting",
                text: "human input".into(),
                timestamp: Utc::now(),
            });
        }
        return;
    }
    if let Some(row) = render_row(ev)
        && let Ok(mut g) = buf.lock()
    {
        g.push_back(row);
    }
}
