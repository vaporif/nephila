//! `ClaudeCodeSession` — owns one long-lived `claude` process per agent.
//!
//! Slice 1b: the connector is the sole producer of `SessionEvent`s for the
//! aggregate. Events are persisted via `SqliteStore::append_batch` (or
//! `append_batch_with_blobs` for oversized tool-result payloads) directly
//! from the reader/writer tasks; the store stamps sequence per ADR-0002.
//!
//! Architecture:
//!   - one `tokio::process::Child` running the `claude` binary in stream-json mode
//!   - reader task: consumes stdout JSON lines, runs them through `Coalescer`,
//!     translates `ClaudeOutput` frames into `SessionEvent`s, batches the events
//!     produced by a single frame, and writes them in one `append_batch` call
//!   - writer task: serialises `Turn` requests to `claude` stdin and persists
//!     the `*PromptQueued`/`*PromptDelivered` event pair
//!   - stderr drain task: pulls stderr line-by-line into a bounded ring buffer
//!     so the OS pipe never fills (would otherwise deadlock claude)
//!
//! Channel bounds:
//!   - `turns_tx`:    mpsc bound 16 (small — `send_turn` backpressures here)
//!   - stderr ring: `VecDeque` cap 256 lines (drop oldest)
//!
//! `SessionConfig` holds concrete `Arc<SqliteStore>` and `Arc<SqliteBlobReader>`
//! handles. The plan's `Arc<dyn DomainEventStore>` shape is not currently
//! workable — `DomainEventStore` and `BlobReader` use RPITIT (`-> impl Future`)
//! which is not dyn-compatible without a wrapper, and `append_batch_with_blobs`
//! lives on `SqliteStore` rather than the trait. Concrete types match the
//! existing pattern in `crates/lifecycle/src/lifecycle_supervisor.rs`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use chrono::Utc;
use claude_codes::io::{ClaudeInput, ClaudeOutput, ContentBlock};
use nephila_core::id::{AgentId, CheckpointId};
use nephila_core::session_event::{InterruptSnapshot, SessionEvent, ToolResultPayload};
use nephila_eventsourcing::aggregate::EventSourced;
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::snapshot::Snapshot;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::{PreparedBlob, SqliteBlobReader, prepare_blob};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::ConnectorError;
use crate::stream::Coalescer;

const TURNS_CHANNEL_BOUND: usize = 16;
const STDERR_RING_CAP: usize = 256;
const STDERR_TAIL_LINES: usize = 32;
const TOOL_RESULT_INLINE_MAX: usize = 256 * 1024;

#[derive(Clone)]
pub struct SessionConfig {
    pub claude_binary: PathBuf,
    pub session_id: Uuid,
    pub agent_id: AgentId,
    pub working_dir: PathBuf,
    pub mcp_endpoint: String,
    pub permission_mode: String,
    pub store: Arc<SqliteStore>,
    pub blob_reader: Arc<SqliteBlobReader>,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("claude_binary", &self.claude_binary)
            .field("session_id", &self.session_id)
            .field("agent_id", &self.agent_id)
            .field("working_dir", &self.working_dir)
            .field("mcp_endpoint", &self.mcp_endpoint)
            .field("permission_mode", &self.permission_mode)
            .finish_non_exhaustive()
    }
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
type ToolNames = Arc<std::sync::Mutex<HashMap<String, String>>>;

pub struct ClaudeCodeSession {
    session_id: Uuid,
    agent_id: AgentId,
    turns_tx: mpsc::Sender<Turn>,
    store: Arc<SqliteStore>,
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
            store,
            blob_reader: _,
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
        let cancel = CancellationToken::new();

        let stderr_ring: StderrRing = Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(
            STDERR_RING_CAP,
        )));
        let open_turn: OpenTurn = Arc::new(std::sync::Mutex::new(None));
        let tool_names: ToolNames = Arc::new(std::sync::Mutex::new(HashMap::new()));

        // SessionStarted lands as the very first envelope so consumers can
        // anchor `Session::default_state()`. Failure here is fatal: without
        // the bootstrap event, replay produces a malformed aggregate.
        let started = SessionEvent::SessionStarted {
            session_id,
            agent_id,
            model: None,
            working_dir: working_dir.clone(),
            mcp_endpoint: mcp_endpoint.clone(),
            resumed: false,
            ts: Utc::now(),
        };
        append_one(&store, session_id, &started)
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("persist SessionStarted: {e}"),
            })?;

        let stderr_handle = tokio::spawn(stderr_drain_task(stderr, Arc::clone(&stderr_ring)));

        let reader_handle = tokio::spawn(reader_task(
            stdout,
            Arc::clone(&store),
            Arc::clone(&open_turn),
            Arc::clone(&tool_names),
            Arc::clone(&stderr_ring),
            cancel.clone(),
            session_id,
        ));

        let writer_handle = tokio::spawn(writer_task(
            stdin,
            turns_rx,
            Arc::clone(&store),
            Arc::clone(&open_turn),
            cancel.clone(),
            session_id,
        ));

        Ok(Self {
            session_id,
            agent_id,
            turns_tx,
            store,
            cancel,
            child: Mutex::new(Some(child)),
            reader_handle: Mutex::new(Some(reader_handle)),
            writer_handle: Mutex::new(Some(writer_handle)),
            stderr_handle: Mutex::new(Some(stderr_handle)),
        })
    }

    /// Aggregate id consumers pass to `store.subscribe_after("session", ..)`.
    #[must_use]
    pub fn aggregate_id(&self) -> String {
        self.session_id.to_string()
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
    ///
    /// On clean shutdown, materializes the final `Session` state by replaying
    /// from `store.load_events` and writes a snapshot at `last_seq` (Trigger A
    /// per plan step 17). Snapshot save failures do NOT fail shutdown — the
    /// store-side Trigger B will eventually catch up on the next subscribe.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn shutdown(self) -> Result<(), ConnectorError> {
        self.cancel.cancel();

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

        // Persist SessionEnded; failures here are logged but non-fatal.
        // Subsequent snapshot save sees the SessionEnded event in its replay.
        let ended = SessionEvent::SessionEnded { ts: Utc::now() };
        if let Err(e) = append_one(&self.store, self.session_id, &ended).await {
            tracing::warn!(error = %e, "failed to persist SessionEnded");
        }

        // Trigger A: materialize final aggregate state and snapshot it.
        if let Err(e) = save_terminal_snapshot(&self.store, self.session_id).await {
            tracing::warn!(error = %e, "Trigger A snapshot save failed (non-fatal)");
        }

        // `kill_on_drop = false`, so reap explicitly: SIGTERM, then wait.
        let maybe_child = self.child.lock().await.take();
        if let Some(mut child) = maybe_child {
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

    /// Read the OS pid of the underlying claude child, if it is still alive.
    ///
    /// Briefly locks the child mutex, reads `Child::id()`, releases. The
    /// returned pid is a snapshot; callers must tolerate a TOCTOU race where
    /// the child exits between this read and a subsequent syscall (e.g.
    /// `kill`). Returns `None` if the child has been reaped (post `shutdown`)
    /// or `Child::id()` has otherwise yielded `None`.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn pid(&self) -> Option<u32> {
        let guard = self.child.lock().await;
        guard.as_ref().and_then(Child::id)
    }

    /// SIGSTOP the underlying claude OS process.
    ///
    /// **Important:** this halts the entire process — including any in-flight
    /// tool call claude is waiting on. Operators should only invoke `pause()`
    /// at a safe point (typically immediately after a `CheckpointReached`).
    /// Resume with [`Self::resume_paused`].
    ///
    /// On non-Unix platforms this is a no-op (compile-time elided).
    /// Returns `ConnectorError::Process` if the child is no longer alive.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn pause(&self) -> Result<(), ConnectorError> {
        #[cfg(unix)]
        {
            let pid = self.pid().await.ok_or_else(|| ConnectorError::Process {
                exit_code: None,
                stderr: "pause: no pid (child already gone)".into(),
            })?;
            send_signal(pid, nix::sys::signal::Signal::SIGSTOP).map_err(|e| {
                ConnectorError::Process {
                    exit_code: None,
                    stderr: format!("pause: SIGSTOP failed: {e}"),
                }
            })?;
        }
        Ok(())
    }

    /// SIGCONT a previously-paused claude OS process.
    ///
    /// On non-Unix platforms this is a no-op (compile-time elided).
    /// Returns `ConnectorError::Process` if the child is no longer alive.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn resume_paused(&self) -> Result<(), ConnectorError> {
        #[cfg(unix)]
        {
            let pid = self.pid().await.ok_or_else(|| ConnectorError::Process {
                exit_code: None,
                stderr: "resume: no pid (child already gone)".into(),
            })?;
            send_signal(pid, nix::sys::signal::Signal::SIGCONT).map_err(|e| {
                ConnectorError::Process {
                    exit_code: None,
                    stderr: format!("resume: SIGCONT failed: {e}"),
                }
            })?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: nix::sys::signal::Signal) -> nix::Result<()> {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid.cast_signed()), signal)
}

impl Drop for ClaudeCodeSession {
    fn drop(&mut self) {
        // Panic-cleanup path. Operators should prefer `shutdown().await`.
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
    store: Arc<SqliteStore>,
    open_turn: OpenTurn,
    tool_names: ToolNames,
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
                    handle_eof(&store, session_id, &open_turn, &stderr_ring).await;
                    break;
                }
                Err(e) => {
                    let tail = snapshot_stderr_tail(&stderr_ring);
                    let crashed = SessionEvent::SessionCrashed {
                        reason: format!("stdout read error: {e}\n--- stderr tail ---\n{tail}"),
                        exit_code: None,
                        ts: Utc::now(),
                    };
                    let _ = append_one(&store, session_id, &crashed).await;
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
                let crashed = SessionEvent::SessionCrashed {
                    reason: format!("unparseable frame: {e}\n--- stderr tail ---\n{tail}"),
                    exit_code: None,
                    ts: Utc::now(),
                };
                let _ = append_one(&store, session_id, &crashed).await;
                break;
            }
        };

        process_frame(
            frame,
            &store,
            &open_turn,
            &tool_names,
            &mut coalescer,
            &mut active_message_ids,
            session_id,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_frame(
    frame: ClaudeOutput,
    store: &Arc<SqliteStore>,
    open_turn: &OpenTurn,
    tool_names: &ToolNames,
    coalescer: &mut Coalescer,
    active_message_ids: &mut Vec<String>,
    session_id: Uuid,
) {
    let mut events: Vec<SessionEvent> = Vec::new();
    // Tool-result spillover: at most one per frame in practice (claude emits
    // each tool-result block in its own frame), but the API allows several.
    let mut blobs: Vec<PreparedBlob> = Vec::new();

    match frame {
        ClaudeOutput::System(_)
        | ClaudeOutput::User(_)
        | ClaudeOutput::ControlRequest(_)
        | ClaudeOutput::ControlResponse(_)
        | ClaudeOutput::Error(_)
        | ClaudeOutput::RateLimitEvent(_) => {
            // Slice 1b records these in observability metrics only.
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
                            events.push(ev);
                        }
                    }
                    ContentBlock::ToolUse(t) => {
                        record_tool_use(tool_names, &t.id, &t.name);
                        events.push(SessionEvent::ToolCall {
                            tool_use_id: t.id,
                            tool_name: t.name,
                            input: t.input,
                            ts: Utc::now(),
                        });
                    }
                    ContentBlock::McpToolUse(t) => {
                        record_tool_use(tool_names, &t.id, &t.name);
                        events.push(SessionEvent::ToolCall {
                            tool_use_id: t.id,
                            tool_name: t.name,
                            input: t.input,
                            ts: Utc::now(),
                        });
                    }
                    ContentBlock::ToolResult(r) => {
                        let raw =
                            serde_json::to_value(&r.content).unwrap_or(serde_json::Value::Null);
                        let is_error = r.is_error.unwrap_or(false);
                        let prepared =
                            build_tool_result_event(&r.tool_use_id, &raw, is_error, &mut events);
                        if let Some(p) = prepared {
                            blobs.push(p);
                        }
                    }
                    ContentBlock::McpToolResult(r) => {
                        let is_error = r.is_error.unwrap_or(false);
                        let prepared = build_tool_result_event(
                            &r.tool_use_id,
                            &r.content,
                            is_error,
                            &mut events,
                        );
                        if let Some(p) = prepared {
                            blobs.push(p);
                        }
                        // Look up the tool name to decide whether this is a
                        // checkpoint round-trip. Only a successful result
                        // counts as `CheckpointReached`; an error is treated
                        // as a regular tool result.
                        if !is_error
                            && let Some(name) = lookup_tool_name(tool_names, &r.tool_use_id)
                            && matches!(
                                name.as_str(),
                                "serialize_and_persist" | "request_human_input"
                            )
                        {
                            match derive_checkpoint(&r.content, name.as_str()) {
                                Ok(Some(ev)) => events.push(ev),
                                Ok(None) => {}
                                Err(reason) => {
                                    events.clear();
                                    blobs.clear();
                                    events.push(SessionEvent::SessionCrashed {
                                        reason: format!("malformed checkpoint_id: {reason}"),
                                        exit_code: None,
                                        ts: Utc::now(),
                                    });
                                }
                            }
                        }
                    }
                    // thinking/image/server-tool blocks: not surfaced in slice 1b.
                    _ => {}
                }
            }
            // `stop_reason` on an assistant frame marks message close — both
            // for real claude (final partial frame) and the fake fixture
            // (second/last frame).
            if asst.message.stop_reason.is_some() {
                if let Some(ev) = coalescer.finalize(&message_id) {
                    events.push(ev);
                }
                active_message_ids.retain(|id| id != &message_id);
            }
        }
        ClaudeOutput::Result(r) => {
            // Flush still-open assistant messages so renderers see final text
            // before `TurnCompleted` lands.
            for id in active_message_ids.drain(..) {
                if let Some(ev) = coalescer.finalize(&id) {
                    events.push(ev);
                }
            }

            let stop_reason = r.stop_reason.unwrap_or_else(|| "end_turn".into());
            let turn_id = open_turn.lock().ok().and_then(|mut g| g.take());

            if let Some(turn_id) = turn_id {
                events.push(SessionEvent::TurnCompleted {
                    turn_id,
                    stop_reason,
                    ts: Utc::now(),
                });
            } else {
                tracing::debug!("Result frame received with no open turn");
            }
        }
    }

    if events.is_empty() {
        return;
    }

    if let Err(e) = append_batch(store, session_id, events, blobs).await {
        tracing::error!(error = %e, "failed to append SessionEvent batch");
    }
}

fn record_tool_use(tool_names: &ToolNames, tool_use_id: &str, tool_name: &str) {
    if let Ok(mut g) = tool_names.lock() {
        g.insert(tool_use_id.to_owned(), tool_name.to_owned());
    }
}

fn lookup_tool_name(tool_names: &ToolNames, tool_use_id: &str) -> Option<String> {
    tool_names
        .lock()
        .ok()
        .and_then(|mut g| g.remove(tool_use_id))
}

/// Builds a `ToolResult` event (inline or spilled per `TOOL_RESULT_INLINE_MAX`)
/// and appends it to `events`. Returns the `PreparedBlob` if spillover occurred.
fn build_tool_result_event(
    tool_use_id: &str,
    raw_output: &serde_json::Value,
    is_error: bool,
    events: &mut Vec<SessionEvent>,
) -> Option<PreparedBlob> {
    let bytes = match serde_json::to_vec(raw_output) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "tool result re-serialise failed; emitting null inline");
            events.push(SessionEvent::ToolResult {
                tool_use_id: tool_use_id.to_owned(),
                output: ToolResultPayload::Inline(serde_json::Value::Null),
                is_error,
                ts: Utc::now(),
            });
            return None;
        }
    };
    if bytes.len() <= TOOL_RESULT_INLINE_MAX {
        events.push(SessionEvent::ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            output: ToolResultPayload::Inline(raw_output.clone()),
            is_error,
            ts: Utc::now(),
        });
        return None;
    }
    let prepared = prepare_blob(&bytes);
    events.push(SessionEvent::ToolResult {
        tool_use_id: tool_use_id.to_owned(),
        output: ToolResultPayload::Spilled {
            hash: prepared.hash.clone(),
            original_len: prepared.original_len,
            snippet: prepared.snippet.clone(),
        },
        is_error,
        ts: Utc::now(),
    });
    Some(prepared)
}

/// Extracts `checkpoint_id` from an MCP tool-result content payload and
/// constructs a `CheckpointReached` event. Returns `Ok(None)` when no
/// checkpoint id is present (e.g. early stages of `request_human_input`),
/// `Err(reason)` when present-but-malformed.
fn derive_checkpoint(
    content: &serde_json::Value,
    tool_name: &str,
) -> Result<Option<SessionEvent>, String> {
    let id_str = match content.get("checkpoint_id") {
        Some(serde_json::Value::String(s)) => s,
        Some(other) => {
            return Err(format!("checkpoint_id is not a string: {other}"));
        }
        None => return Ok(None),
    };
    let uuid = Uuid::parse_str(id_str).map_err(|e| format!("{id_str:?}: {e}"))?;
    let interrupt = match tool_name {
        "request_human_input" => extract_hitl_snapshot(content),
        _ => None,
    };
    Ok(Some(SessionEvent::CheckpointReached {
        checkpoint_id: CheckpointId(uuid),
        interrupt,
        ts: Utc::now(),
    }))
}

fn extract_hitl_snapshot(content: &serde_json::Value) -> Option<InterruptSnapshot> {
    let question = content.get("question").and_then(|v| v.as_str())?.to_owned();
    let options = content
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(InterruptSnapshot::Hitl { question, options })
}

async fn handle_eof(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
    open_turn: &OpenTurn,
    stderr_ring: &StderrRing,
) {
    let mut events = Vec::new();
    if let Some(turn_id) = open_turn.lock().ok().and_then(|mut g| g.take()) {
        events.push(SessionEvent::TurnAborted {
            turn_id,
            reason: "session_crashed".into(),
            ts: Utc::now(),
        });
    }
    let tail = snapshot_stderr_tail(stderr_ring);
    events.push(SessionEvent::SessionCrashed {
        reason: format!("EOF\n--- stderr tail ---\n{tail}"),
        exit_code: None,
        ts: Utc::now(),
    });
    if let Err(e) = append_batch(store, session_id, events, Vec::new()).await {
        tracing::error!(error = %e, "failed to persist EOF crash batch");
    }
}

async fn writer_task(
    mut stdin: ChildStdin,
    mut turns_rx: mpsc::Receiver<Turn>,
    store: Arc<SqliteStore>,
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
            PromptSource::Human => SessionEvent::HumanPromptQueued {
                turn_id: turn.turn_id,
                text: turn.text.clone(),
                ts: Utc::now(),
            },
            PromptSource::Agent => SessionEvent::AgentPromptQueued {
                turn_id: turn.turn_id,
                text: turn.text.clone(),
                ts: Utc::now(),
            },
        };
        if let Err(e) = append_one(&store, session_id, &queued).await {
            tracing::error!(error = %e, "failed to persist *PromptQueued");
        }

        let payload = ClaudeInput::user_message(turn.text, session_id);
        let mut bytes = match serde_json::to_vec(&payload) {
            Ok(b) => b,
            Err(e) => {
                let failed = SessionEvent::PromptDeliveryFailed {
                    turn_id: turn.turn_id,
                    reason: format!("serialize ClaudeInput: {e}"),
                    ts: Utc::now(),
                };
                let _ = append_one(&store, session_id, &failed).await;
                continue;
            }
        };
        bytes.push(b'\n');

        match stdin.write_all(&bytes).await {
            Ok(()) => {
                if let Err(e) = stdin.flush().await {
                    let failed = SessionEvent::PromptDeliveryFailed {
                        turn_id: turn.turn_id,
                        reason: format!("stdin flush: {e}"),
                        ts: Utc::now(),
                    };
                    let _ = append_one(&store, session_id, &failed).await;
                    continue;
                }
                // ORDERING INVARIANT: emit `*PromptDelivered` *before*
                // publishing `open_turn`. The reader can only emit
                // `TurnCompleted` after a `Result` frame from claude, which
                // claude can only send for a turn it has already received —
                // i.e. one for which we successfully wrote to its stdin.
                // Publishing `open_turn` first opens a tiny window where the
                // reader could race ahead and emit `TurnCompleted` before
                // subscribers see `*PromptDelivered`, violating the contract
                // "every `*PromptQueued{turn_id}` is followed by exactly one
                // of `{TurnCompleted, TurnAborted, PromptDeliveryFailed}`".
                let delivered = match turn.source {
                    PromptSource::Human => SessionEvent::HumanPromptDelivered {
                        turn_id: turn.turn_id,
                        ts: Utc::now(),
                    },
                    PromptSource::Agent => SessionEvent::AgentPromptDelivered {
                        turn_id: turn.turn_id,
                        ts: Utc::now(),
                    },
                };
                if let Err(e) = append_one(&store, session_id, &delivered).await {
                    tracing::error!(error = %e, "failed to persist *PromptDelivered");
                }
                if let Ok(mut g) = open_turn.lock() {
                    *g = Some(turn.turn_id);
                }
            }
            Err(e) => {
                let failed = SessionEvent::PromptDeliveryFailed {
                    turn_id: turn.turn_id,
                    reason: format!("stdin write: {e}"),
                    ts: Utc::now(),
                };
                let _ = append_one(&store, session_id, &failed).await;
                break;
            }
        }
    }
}

fn make_envelope(session_id: Uuid, event: &SessionEvent) -> Result<EventEnvelope, ConnectorError> {
    let payload = serde_json::to_value(event).map_err(|e| ConnectorError::Process {
        exit_code: None,
        stderr: format!("serialize SessionEvent: {e}"),
    })?;
    Ok(EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: session_id.to_string(),
        // Stamped by the writer thread per ADR-0002.
        sequence: 0,
        event_type: event.kind().to_owned(),
        payload,
        trace_id: TraceId(session_id.to_string()),
        outcome: None,
        timestamp: Utc::now(),
        context_snapshot: None,
        metadata: HashMap::new(),
    })
}

async fn append_one(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
    event: &SessionEvent,
) -> Result<(), ConnectorError> {
    let env = make_envelope(session_id, event)?;
    store
        .append_batch(vec![env])
        .await
        .map_err(|e| ConnectorError::Process {
            exit_code: None,
            stderr: format!("append: {e}"),
        })?;
    Ok(())
}

async fn append_batch(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
    events: Vec<SessionEvent>,
    blobs: Vec<PreparedBlob>,
) -> Result<(), ConnectorError> {
    if events.is_empty() {
        return Ok(());
    }
    let envelopes = events
        .iter()
        .map(|e| make_envelope(session_id, e))
        .collect::<Result<Vec<_>, _>>()?;
    if blobs.is_empty() {
        store
            .append_batch(envelopes)
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("append_batch: {e}"),
            })?;
    } else {
        store
            .append_batch_with_blobs(envelopes, blobs)
            .await
            .map_err(|e| ConnectorError::Process {
                exit_code: None,
                stderr: format!("append_batch_with_blobs: {e}"),
            })?;
    }
    Ok(())
}

async fn save_terminal_snapshot(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
) -> Result<(), nephila_eventsourcing::store::EventStoreError> {
    use nephila_core::session::Session;

    let agg_id = session_id.to_string();
    let events = store.load_events("session", &agg_id, 0).await?;
    if events.is_empty() {
        return Ok(());
    }
    let mut state = Session::default_state();
    let mut last_seq = 0u64;
    for env in &events {
        state = state.apply_envelope(env).map_err(|e| {
            nephila_eventsourcing::store::EventStoreError::Serialization(e.to_string())
        })?;
        last_seq = env.sequence;
    }
    let state_value = serde_json::to_value(&state)
        .map_err(|e| nephila_eventsourcing::store::EventStoreError::Serialization(e.to_string()))?;
    let snap = Snapshot {
        aggregate_type: "session".to_owned(),
        aggregate_id: agg_id,
        sequence: last_seq,
        state: state_value,
        timestamp: Utc::now(),
    };
    store.save_snapshot(&snap).await
}
