//! Per-agent pump task: drains `resilient_subscribe` into a shared
//! `RenderedBuffer` (and optionally an activity-glyph mpsc + a HITL request
//! mpsc).
//!
//! `spawn_pump` is exposed so callers (the e2e test, the orchestrator)
//! can wire one pump per agent. The task terminates on stream end
//! (`SessionEnded`, `SessionCrashed`) or when the returned `JoinHandle`
//! is dropped.

use std::sync::Arc;

use futures::StreamExt;
use nephila_core::id::AgentId;
use nephila_core::session_event::{InterruptSnapshot, SessionEvent};
use nephila_store::SqliteStore;
use nephila_store::resilient_subscribe::resilient_subscribe;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{RenderedBuffer, append_event};
use crate::panels::agent_tree::AgentActivityUpdate;

/// HITL request sent from the pump to the App. The App drains a dedicated
/// mpsc receiver in its tick loop and opens `Modal::HitlResponse`.
#[derive(Debug, Clone)]
pub struct HitlRequest {
    pub agent_id: AgentId,
    pub question: String,
    pub options: Vec<String>,
}

/// Pump-task wiring channels. All fields optional so the e2e test can opt
/// out of the activity/hitl plumbing without spinning up dummy receivers.
#[derive(Default)]
pub struct PumpChannels {
    pub activity_tx: Option<mpsc::Sender<AgentActivityUpdate>>,
    pub hitl_tx: Option<mpsc::Sender<HitlRequest>>,
}

/// Spawn a pump task subscribing to the given session aggregate. Each event
/// is decoded into a `SessionEvent` and appended to `buffer`; activity
/// updates and HITL requests fan out via the optional senders in `chans`.
/// Returns the `JoinHandle` so callers can shut the pump down deterministically.
#[must_use]
#[tracing::instrument(
    level = "debug",
    skip(store, buffer, chans),
    fields(%agent_id, aggregate_id = %session_aggregate_id),
)]
pub fn spawn_pump(
    store: Arc<SqliteStore>,
    agent_id: AgentId,
    session_aggregate_id: String,
    buffer: RenderedBuffer,
    chans: PumpChannels,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = resilient_subscribe(store, "session".to_owned(), session_aggregate_id, 0);

        while let Some(item) = stream.next().await {
            let env = match item {
                Ok(env) => env,
                Err(e) => {
                    tracing::warn!(
                        target: "nephila_tui::session_pane::pump",
                        error = %e,
                        "stream error; pump exiting",
                    );
                    break;
                }
            };
            let ev: SessionEvent = match serde_json::from_value(env.payload.clone()) {
                Ok(ev) => ev,
                Err(e) => {
                    tracing::warn!(
                        target: "nephila_tui::session_pane::pump",
                        error = %e,
                        "decode SessionEvent failed; skipping",
                    );
                    continue;
                }
            };

            if let Some(tx) = chans.activity_tx.as_ref() {
                let update = AgentActivityUpdate::from_event(agent_id, &ev);
                let _ = tx.try_send(update);
            }

            if let Some(tx) = chans.hitl_tx.as_ref()
                && let SessionEvent::CheckpointReached {
                    interrupt: Some(InterruptSnapshot::Hitl { question, options }),
                    ..
                } = &ev
            {
                let _ = tx.try_send(HitlRequest {
                    agent_id,
                    question: question.clone(),
                    options: options.clone(),
                });
            }

            let stop = matches!(
                &ev,
                SessionEvent::SessionEnded { .. } | SessionEvent::SessionCrashed { .. }
            );
            append_event(&buffer, ev);
            if stop {
                break;
            }
        }
    })
}
