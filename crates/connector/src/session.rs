//! `ClaudeCodeSession` — owns one long-lived `claude` process per agent.
//!
//! Slice 1a: in-process producer that emits `SessionEventDraft`s to a
//! `tokio::sync::broadcast` channel. Slice 1b promotes drafts to durable
//! `SessionEvent`s and replaces the broadcast with `DomainEventStore` calls.
//!
//! Architecture:
//!   - one `tokio::process::Child` running the `claude` binary in stream-json mode
//!   - reader task: consumes stdout JSON lines, runs them through `Coalescer`,
//!     and translates `ClaudeOutput` frames into `SessionEventDraft`s
//!   - writer task: serialises `Turn` requests to `claude` stdin and bookkeeps
//!     the `*PromptQueued`/`*PromptDelivered` event pair
//!   - stderr drain task: pulls stderr line-by-line into a bounded ring buffer
//!     so the OS pipe never fills (would otherwise deadlock claude)
//!
//! Channel bounds:
//!   - `turns_tx`:    mpsc bound 16 (small — `send_turn` backpressures here)
//!   - `drafts_tx`:   broadcast bound 4096 (per spec `§subscribe_after`)
//!   - stderr ring: `VecDeque` cap 256 lines (drop oldest)

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use chrono::Utc;
use claude_codes::io::{ClaudeInput, ClaudeOutput, ContentBlock};
use nephila_core::id::AgentId;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::ConnectorError;
use crate::event_draft::SessionEventDraft;
use crate::stream::Coalescer;

const TURNS_CHANNEL_BOUND: usize = 16;
const DRAFTS_CHANNEL_BOUND: usize = 4096;
const STDERR_RING_CAP: usize = 256;
const STDERR_TAIL_LINES: usize = 32;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub claude_binary: PathBuf,
    pub session_id: Uuid,
    pub agent_id: AgentId,
    pub working_dir: PathBuf,
    pub mcp_endpoint: String,
    pub permission_mode: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PromptSource {
    Agent,
    Human,
}

pub type TurnId = Uuid;

#[allow(clippy::struct_field_names)]
struct Turn {
    source: PromptSource,
    text: String,
    turn_id: TurnId,
}

type StderrRing = Arc<std::sync::Mutex<VecDeque<String>>>;
type OpenTurn = Arc<std::sync::Mutex<Option<TurnId>>>;

pub struct ClaudeCodeSession {
    session_id: Uuid,
    agent_id: AgentId,
    turns_tx: mpsc::Sender<Turn>,
    drafts_tx: broadcast::Sender<SessionEventDraft>,
    cancel: CancellationToken,
    child: Mutex<Option<Child>>,
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    writer_handle: Mutex<Option<JoinHandle<()>>>,
    stderr_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ClaudeCodeSession {
    /// Spawn a new claude subprocess and start the reader/writer tasks.
    #[tracing::instrument(level = "debug", skip(cfg), fields(session_id = %cfg.session_id, agent_id = %cfg.agent_id))]
    pub async fn start(cfg: SessionConfig) -> Result<Self, ConnectorError> {
        let SessionConfig {
            claude_binary,
            session_id,
            agent_id,
            working_dir,
            mcp_endpoint,
            permission_mode,
        } = cfg;

        let mcp_config = build_mcp_config(&mcp_endpoint);
        let mcp_config_path = working_dir.join(".mcp.json");
        tokio::fs::write(&mcp_config_path, &mcp_config)
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("failed to write .mcp.json: {e}"),
            })?;

        let mut child = Command::new(&claude_binary)
            .arg("--print")
            .arg("--verbose")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--session-id")
            .arg(session_id.to_string())
            .arg("--mcp-config")
            .arg(&mcp_config_path)
            .arg("--permission-mode")
            .arg(&permission_mode)
            .current_dir(&working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("failed to spawn claude: {e}"),
            })?;

        let stdin = child.stdin.take().ok_or_else(|| ConnectorError::Process {
            exit_code: None,
            stderr: "claude child has no stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ConnectorError::Process {
            exit_code: None,
            stderr: "claude child has no stdout".into(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| ConnectorError::Process {
            exit_code: None,
            stderr: "claude child has no stderr".into(),
        })?;

        let (turns_tx, turns_rx) = mpsc::channel::<Turn>(TURNS_CHANNEL_BOUND);
        let (drafts_tx, _) = broadcast::channel::<SessionEventDraft>(DRAFTS_CHANNEL_BOUND);
        let cancel = CancellationToken::new();

        let stderr_ring: StderrRing = Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(
            STDERR_RING_CAP,
        )));
        let open_turn: OpenTurn = Arc::new(std::sync::Mutex::new(None));

        let _ = drafts_tx.send(SessionEventDraft::SessionStarted {
            resumed: false,
            ts: Utc::now(),
        });

        let stderr_handle = tokio::spawn(stderr_drain_task(stderr, Arc::clone(&stderr_ring)));

        let reader_handle = tokio::spawn(reader_task(
            stdout,
            drafts_tx.clone(),
            Arc::clone(&open_turn),
            Arc::clone(&stderr_ring),
            cancel.clone(),
            session_id,
        ));

        let writer_handle = tokio::spawn(writer_task(
            stdin,
            turns_rx,
            drafts_tx.clone(),
            Arc::clone(&open_turn),
            cancel.clone(),
            session_id,
        ));

        Ok(Self {
            session_id,
            agent_id,
            turns_tx,
            drafts_tx,
            cancel,
            child: Mutex::new(Some(child)),
            reader_handle: Mutex::new(Some(reader_handle)),
            writer_handle: Mutex::new(Some(writer_handle)),
            stderr_handle: Mutex::new(Some(stderr_handle)),
        })
    }

    #[must_use]
    pub fn subscribe_drafts(&self) -> broadcast::Receiver<SessionEventDraft> {
        self.drafts_tx.subscribe()
    }

    /// Enqueue a turn for delivery to claude.
    ///
    /// Returns the freshly minted `TurnId`. Backpressures (bounded mpsc) when
    /// >16 turns are inflight.
    #[tracing::instrument(level = "debug", skip(self, text), fields(session_id = %self.session_id))]
    pub async fn send_turn(
        &self,
        source: PromptSource,
        text: String,
    ) -> Result<TurnId, ConnectorError> {
        let turn_id = Uuid::new_v4();
        self.turns_tx
            .send(Turn {
                source,
                text,
                turn_id,
            })
            .await
            .map_err(|_| ConnectorError::Process {
                exit_code: None,
                stderr: "writer task closed".into(),
            })?;
        Ok(turn_id)
    }

    /// Graceful teardown: cancel tasks, close stdin, await drain, reap child.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn shutdown(self) -> Result<(), ConnectorError> {
        // Signal cancellation so the reader/writer tasks fall out of their loops.
        self.cancel.cancel();

        // Take and join the task handles.
        let reader = self.reader_handle.lock().await.take();
        let writer = self.writer_handle.lock().await.take();
        let stderr = self.stderr_handle.lock().await.take();

        if let Some(h) = writer {
            let _ = h.await;
        }
        if let Some(h) = reader {
            let _ = h.await;
        }
        if let Some(h) = stderr {
            let _ = h.await;
        }

        // Append SessionEnded; broadcast may have no subscribers, so ignore the
        // send result.
        let _ = self
            .drafts_tx
            .send(SessionEventDraft::SessionEnded { ts: Utc::now() });

        // Reap the child. With kill_on_drop = false, we need to do this explicitly.
        let maybe_child = self.child.lock().await.take();
        if let Some(mut child) = maybe_child {
            // Best-effort: SIGTERM, then wait. If wait errors, log via tracing.
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                use nix::sys::signal::{Signal, kill};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(pid.cast_signed()), Signal::SIGTERM);
            }
            match child.wait().await {
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "child wait failed during shutdown"),
            }
        }

        Ok(())
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.session_id
    }

    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }
}

impl Drop for ClaudeCodeSession {
    fn drop(&mut self) {
        // Best-effort: panic-cleanup only. Operators should call shutdown().
        self.cancel.cancel();
        let child_opt = self.child.try_lock().ok().and_then(|mut g| g.take());
        // `mut` is unused on tokio 1.51 (`Child::id(&self) -> Option<u32>`),
        // but we mark it anyway so the binding stays correct if a future
        // tokio bump returns `Child::id` to `&mut self` (its signature has
        // wavered historically) or this Drop grows a `child.start_kill()`
        // call. The lint suppression is paired with this comment so the
        // intent isn't lost.
        #[allow(unused_mut)]
        if let Some(mut child) = child_opt {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                use nix::sys::signal::{Signal, kill};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(pid.cast_signed()), Signal::SIGTERM);
            }
            // Plain drop: with kill_on_drop=false, tokio's Child::drop does NOT
            // kill or block but the runtime's reaper still tracks the PID.
            drop(child);
        }
    }
}

fn build_mcp_config(mcp_endpoint: &str) -> String {
    let cfg = serde_json::json!({
        "mcpServers": {
            "nephila": {
                "type": "streamable-http",
                "url": mcp_endpoint,
            }
        }
    });
    serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".into())
}

async fn stderr_drain_task(stderr: tokio::process::ChildStderr, ring: StderrRing) {
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if let Ok(mut g) = ring.lock() {
            if g.len() == STDERR_RING_CAP {
                g.pop_front();
            }
            g.push_back(line);
        }
    }
}

fn snapshot_stderr_tail(ring: &StderrRing) -> String {
    ring.lock().map_or_else(
        |_| String::new(),
        |g| {
            let n = g.len().min(STDERR_TAIL_LINES);
            let start = g.len() - n;
            g.iter().skip(start).cloned().collect::<Vec<_>>().join("\n")
        },
    )
}

#[allow(clippy::too_many_arguments)]
async fn reader_task(
    stdout: tokio::process::ChildStdout,
    drafts_tx: broadcast::Sender<SessionEventDraft>,
    open_turn: OpenTurn,
    stderr_ring: StderrRing,
    cancel: CancellationToken,
    session_id: Uuid,
) {
    let mut reader = BufReader::new(stdout).lines();
    let mut coalescer = Coalescer::default();
    let mut active_message_ids: Vec<String> = Vec::new();

    loop {
        let line = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            res = reader.next_line() => match res {
                Ok(Some(l)) => l,
                Ok(None) => {
                    handle_eof(&drafts_tx, &open_turn, &stderr_ring);
                    break;
                }
                Err(e) => {
                    let tail = snapshot_stderr_tail(&stderr_ring);
                    let _ = drafts_tx.send(SessionEventDraft::SessionCrashed {
                        reason: format!("stdout read error: {e}\n--- stderr tail ---\n{tail}"),
                        exit_code: None,
                        ts: Utc::now(),
                    });
                    break;
                }
            },
        };

        if line.is_empty() {
            continue;
        }

        let frame = match ClaudeOutput::parse_json(&line) {
            Ok(f) => f,
            Err(e) => {
                let tail = snapshot_stderr_tail(&stderr_ring);
                let _ = drafts_tx.send(SessionEventDraft::SessionCrashed {
                    reason: format!("unparseable frame: {e}\n--- stderr tail ---\n{tail}"),
                    exit_code: None,
                    ts: Utc::now(),
                });
                break;
            }
        };

        process_frame(
            frame,
            &drafts_tx,
            &open_turn,
            &mut coalescer,
            &mut active_message_ids,
            session_id,
        );
    }
}

fn process_frame(
    frame: ClaudeOutput,
    drafts_tx: &broadcast::Sender<SessionEventDraft>,
    open_turn: &OpenTurn,
    coalescer: &mut Coalescer,
    active_message_ids: &mut Vec<String>,
    _session_id: Uuid,
) {
    match frame {
        ClaudeOutput::System(_)
        | ClaudeOutput::User(_)
        | ClaudeOutput::ControlRequest(_)
        | ClaudeOutput::ControlResponse(_)
        | ClaudeOutput::Error(_)
        | ClaudeOutput::RateLimitEvent(_) => {
            // Slice 1a doesn't surface these as drafts; slice 1b records them.
        }
        ClaudeOutput::Assistant(asst) => {
            let message_id = asst.message.id.clone();
            if !active_message_ids.contains(&message_id) {
                active_message_ids.push(message_id.clone());
            }
            for block in asst.message.content {
                match block {
                    ContentBlock::Text(t) => {
                        if let Some(ev) = coalescer.push_delta(&message_id, &t.text) {
                            let _ = drafts_tx.send(ev);
                        }
                    }
                    ContentBlock::ToolUse(t) => {
                        let _ = drafts_tx.send(SessionEventDraft::ToolCall {
                            tool_use_id: t.id,
                            tool_name: t.name,
                            input: t.input,
                            ts: Utc::now(),
                        });
                    }
                    ContentBlock::McpToolUse(t) => {
                        let _ = drafts_tx.send(SessionEventDraft::ToolCall {
                            tool_use_id: t.id,
                            tool_name: t.name,
                            input: t.input,
                            ts: Utc::now(),
                        });
                    }
                    ContentBlock::ToolResult(r) => {
                        let _ = drafts_tx.send(SessionEventDraft::ToolResult {
                            tool_use_id: r.tool_use_id,
                            output: serde_json::to_value(&r.content)
                                .unwrap_or(serde_json::Value::Null),
                            is_error: r.is_error.unwrap_or(false),
                            ts: Utc::now(),
                        });
                    }
                    ContentBlock::McpToolResult(r) => {
                        let _ = drafts_tx.send(SessionEventDraft::ToolResult {
                            tool_use_id: r.tool_use_id,
                            output: r.content,
                            is_error: r.is_error.unwrap_or(false),
                            ts: Utc::now(),
                        });
                    }
                    // Slice 1a does not yet emit drafts for thinking/image/server tools.
                    _ => {}
                }
            }
            // If the assistant message carries a stop_reason, finalize its
            // coalesced text. Real claude marks this on the final partial frame;
            // the fake fixture sets stop_reason on its second (last) frame.
            if asst.message.stop_reason.is_some() {
                if let Some(ev) = coalescer.finalize(&message_id) {
                    let _ = drafts_tx.send(ev);
                }
                active_message_ids.retain(|id| id != &message_id);
            }
        }
        ClaudeOutput::Result(r) => {
            // Flush any still-open assistant messages first so the renderer
            // sees the final text before TurnCompleted lands.
            for id in active_message_ids.drain(..) {
                if let Some(ev) = coalescer.finalize(&id) {
                    let _ = drafts_tx.send(ev);
                }
            }

            let stop_reason = r.stop_reason.unwrap_or_else(|| "end_turn".into());
            let turn_id = open_turn.lock().ok().and_then(|mut g| g.take());

            if let Some(turn_id) = turn_id {
                let _ = drafts_tx.send(SessionEventDraft::TurnCompleted {
                    turn_id,
                    stop_reason,
                    ts: Utc::now(),
                });
            } else {
                tracing::debug!("Result frame received with no open turn");
            }
        }
    }
}

fn handle_eof(
    drafts_tx: &broadcast::Sender<SessionEventDraft>,
    open_turn: &OpenTurn,
    stderr_ring: &StderrRing,
) {
    if let Some(turn_id) = open_turn.lock().ok().and_then(|mut g| g.take()) {
        let _ = drafts_tx.send(SessionEventDraft::TurnAborted {
            turn_id,
            reason: "session_crashed".into(),
            ts: Utc::now(),
        });
    }
    let tail = snapshot_stderr_tail(stderr_ring);
    let _ = drafts_tx.send(SessionEventDraft::SessionCrashed {
        reason: format!("EOF\n--- stderr tail ---\n{tail}"),
        exit_code: None,
        ts: Utc::now(),
    });
}

async fn writer_task(
    mut stdin: ChildStdin,
    mut turns_rx: mpsc::Receiver<Turn>,
    drafts_tx: broadcast::Sender<SessionEventDraft>,
    open_turn: OpenTurn,
    cancel: CancellationToken,
    session_id: Uuid,
) {
    loop {
        let turn = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            t = turns_rx.recv() => match t {
                Some(t) => t,
                None => break,
            },
        };

        let queued = match turn.source {
            PromptSource::Human => SessionEventDraft::HumanPromptQueued {
                turn_id: turn.turn_id,
                text: turn.text.clone(),
                ts: Utc::now(),
            },
            PromptSource::Agent => SessionEventDraft::AgentPromptQueued {
                turn_id: turn.turn_id,
                text: turn.text.clone(),
                ts: Utc::now(),
            },
        };
        let _ = drafts_tx.send(queued);

        let payload = ClaudeInput::user_message(turn.text, session_id);
        let mut bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(e) => {
                let _ = drafts_tx.send(SessionEventDraft::PromptDeliveryFailed {
                    turn_id: turn.turn_id,
                    reason: format!("serialize ClaudeInput: {e}"),
                    ts: Utc::now(),
                });
                continue;
            }
        };
        bytes.push(b'\n');

        match stdin.write_all(&bytes).await {
            Ok(()) => {
                if let Err(e) = stdin.flush().await {
                    let _ = drafts_tx.send(SessionEventDraft::PromptDeliveryFailed {
                        turn_id: turn.turn_id,
                        reason: format!("stdin flush: {e}"),
                        ts: Utc::now(),
                    });
                    continue;
                }
                // ORDERING INVARIANT (slice 1a): emit `*PromptDelivered`
                // *before* publishing `open_turn`. The reader can only emit
                // `TurnCompleted` after a `Result` frame from claude, which
                // claude can only send for a turn it has already received —
                // i.e. one for which we successfully wrote to its stdin.
                // Publishing `open_turn` first opens a tiny window where the
                // reader could race ahead and emit `TurnCompleted` before
                // subscribers see `*PromptDelivered`, violating the contract
                // "every `*PromptQueued{turn_id}` is followed by exactly one
                // of `{TurnCompleted, TurnAborted, PromptDeliveryFailed}`".
                let delivered = match turn.source {
                    PromptSource::Human => SessionEventDraft::HumanPromptDelivered {
                        turn_id: turn.turn_id,
                        ts: Utc::now(),
                    },
                    PromptSource::Agent => SessionEventDraft::AgentPromptDelivered {
                        turn_id: turn.turn_id,
                        ts: Utc::now(),
                    },
                };
                let _ = drafts_tx.send(delivered);
                if let Ok(mut g) = open_turn.lock() {
                    *g = Some(turn.turn_id);
                }
            }
            Err(e) => {
                let _ = drafts_tx.send(SessionEventDraft::PromptDeliveryFailed {
                    turn_id: turn.turn_id,
                    reason: format!("stdin write: {e}"),
                    ts: Utc::now(),
                });
                break;
            }
        }
    }
}
