//! `ClaudeCodeSession` — owns one long-lived `claude` process per agent.
//!
//! The connector is the sole producer of `SessionEvent`s for the session
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
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

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
use parking_lot::Mutex as ParkingMutex;
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
    /// Out-of-band crash notification channel.
    ///
    /// When the reader observes EOF / parse-error and the resulting `[TurnAborted,
    /// SessionCrashed]` `append_batch` itself fails (DB error, store closed),
    /// the reader best-effort-sends the agent id on this channel so the
    /// `SessionRegistry` can still trigger respawn. Treated as equivalent to
    /// observing `SessionCrashed` in the event stream; idempotency dedupes
    /// duplicates.
    pub crash_fallback_tx: Option<mpsc::Sender<AgentId>>,
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
            .field("crash_fallback_tx", &self.crash_fallback_tx.is_some())
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

type StderrRing = Arc<ParkingMutex<VecDeque<String>>>;
type OpenTurn = Arc<ParkingMutex<Option<TurnId>>>;
type ToolNames = Arc<ParkingMutex<HashMap<String, String>>>;

pub struct ClaudeCodeSession {
    session_id: Uuid,
    agent_id: AgentId,
    turns_tx: mpsc::Sender<Turn>,
    store: Arc<SqliteStore>,
    cancel: CancellationToken,
    /// Set to `true` by `shutdown()` (and `Drop`) BEFORE sending SIGTERM. The
    /// reader observes this on EOF/parse-error to disambiguate intentional
    /// teardown from a real crash: when set, the reader emits nothing and
    /// `shutdown()` itself appends `SessionEnded` after drain.
    shutting_down: Arc<AtomicBool>,
    child: Mutex<Option<Child>>,
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    writer_handle: Mutex<Option<JoinHandle<()>>>,
    stderr_handle: Mutex<Option<JoinHandle<()>>>,
}

/// How the claude process is being launched — fresh or resumed.
#[derive(Debug, Clone, Copy)]
enum LaunchMode {
    /// `--session-id <id>` (no on-disk session yet).
    Fresh,
    /// `--resume <id>` — caller already verified the on-disk session exists.
    Resume,
    /// `--session-id <id>` after a `--resume` failure observed at the stderr level.
    /// Tracks the metadata flag for the `SessionStarted { resumed: true }` event.
    ResumeFallback,
}

#[derive(Debug)]
enum ResumeOutcome {
    /// `--resume` is running fine (still alive after the 2s probe, or exited
    /// successfully within the window). Caller proceeds with `LaunchMode::Resume`.
    Ok,
    /// `--resume` exited non-zero with a not-found-style stderr. Caller falls
    /// back to `LaunchMode::ResumeFallback` (`--session-id`).
    FallbackToSessionId,
    /// `--resume` failed in a way that is NOT a missing-session signal — surface
    /// the error to the caller; do not silently fallback.
    HardError(String),
}

impl LaunchMode {
    const fn resumed(self) -> bool {
        matches!(self, Self::Resume | Self::ResumeFallback)
    }
}

impl ClaudeCodeSession {
    /// Spawn a fresh claude subprocess (`--session-id <id>`) and start the
    /// reader/writer tasks. Persists `SessionStarted { resumed: false }`.
    #[tracing::instrument(level = "debug", skip(cfg), fields(session_id = %cfg.session_id, agent_id = %cfg.agent_id))]
    pub async fn start(cfg: SessionConfig) -> Result<Self, ConnectorError> {
        Self::launch(cfg, LaunchMode::Fresh).await
    }

    /// Resume an existing claude session. Tries `claude --resume <id>` first;
    /// if that exits within 2s with a "session not found"-style stderr, falls
    /// back to `claude --session-id <id>` and persists `SessionStarted { resumed: true }`
    /// with a `fallback=true` metadata flag.
    ///
    /// The fallback gate combines (`exit_code` != 0) AND (stderr matches
    /// `RESUME_NOT_FOUND_PATTERNS`) — neither alone — to reduce false positives.
    /// On fallback the `session.fallback_to_session_id` metric is recorded.
    #[tracing::instrument(level = "debug", skip(cfg), fields(session_id = %session_id, agent_id = %cfg.agent_id))]
    pub async fn resume(mut cfg: SessionConfig, session_id: Uuid) -> Result<Self, ConnectorError> {
        cfg.session_id = session_id;
        match Self::try_launch_resume(&cfg).await {
            ResumeOutcome::Ok => Self::launch(cfg, LaunchMode::Resume).await,
            ResumeOutcome::FallbackToSessionId => {
                nephila_store::metrics::record_session_fallback_to_session_id(
                    &session_id.to_string(),
                );
                Self::launch(cfg, LaunchMode::ResumeFallback).await
            }
            ResumeOutcome::HardError(stderr) => Err(ConnectorError::Process {
                exit_code: None,
                stderr,
            }),
        }
    }

    /// Probe `claude --resume <id>` non-destructively: spawn, wait up to 2s
    /// for exit, classify the outcome.
    ///
    /// `Ok` means the child is alive after 2s — we kill it and signal "go ahead
    /// and run the real `--resume` launch". `FallbackToSessionId` means the
    /// child exited non-zero AND its stderr matches the not-found pattern.
    /// `HardError` means a non-zero exit with an unrecognized stderr — the
    /// caller propagates as a `ConnectorError::Process`.
    async fn try_launch_resume(cfg: &SessionConfig) -> ResumeOutcome {
        use std::time::Duration;

        let mcp_config = build_mcp_config(&cfg.mcp_endpoint);
        let mcp_config_path = cfg.working_dir.join(".mcp.json");
        if let Err(e) = tokio::fs::write(&mcp_config_path, &mcp_config).await {
            return ResumeOutcome::HardError(format!("write .mcp.json: {e}"));
        }

        let mut child = match Command::new(&cfg.claude_binary)
            .arg("--print")
            .arg("--verbose")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages")
            .arg("--resume")
            .arg(cfg.session_id.to_string())
            .arg("--mcp-config")
            .arg(&mcp_config_path)
            .arg("--permission-mode")
            .arg(&cfg.permission_mode)
            .current_dir(&cfg.working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ResumeOutcome::HardError(format!("spawn --resume: {e}")),
        };

        if child.stderr.is_none() {
            return ResumeOutcome::HardError("--resume probe: no stderr pipe".into());
        }

        let probe_duration = Duration::from_secs(2);
        let mut stderr_buf = Vec::new();

        // Race the probe timeout against draining stderr. Whichever resolves
        // first wins; on timeout we kill the child so the caller can re-spawn.
        let exit = {
            use tokio::io::AsyncReadExt;
            let Some(mut stderr_pipe) = child.stderr.take() else {
                return ResumeOutcome::HardError("--resume probe: no stderr pipe".into());
            };
            tokio::select! {
                wait_res = tokio::time::timeout(probe_duration, child.wait()) => match wait_res {
                    Ok(Ok(status)) => status,
                    Ok(Err(e)) => {
                        return ResumeOutcome::HardError(format!("--resume wait: {e}"));
                    }
                    Err(_) => {
                        let _ = child.kill().await;
                        return ResumeOutcome::Ok;
                    }
                },
                read_res = stderr_pipe.read_to_end(&mut stderr_buf) => {
                    // stderr closed first; usually means the child has exited
                    // or is about to. Fall through to wait for the exit status.
                    let _ = read_res;
                    match tokio::time::timeout(probe_duration, child.wait()).await {
                        Ok(Ok(status)) => status,
                        Ok(Err(e)) => {
                            return ResumeOutcome::HardError(format!("--resume wait: {e}"));
                        }
                        Err(_) => {
                            let _ = child.kill().await;
                            return ResumeOutcome::Ok;
                        }
                    }
                }
            }
        };

        if exit.success() {
            // Exited cleanly within 2s — unusual for a long-lived session, but
            // treat as success. The caller will re-spawn with `--resume`.
            return ResumeOutcome::Ok;
        }

        let stderr_text = String::from_utf8_lossy(&stderr_buf).into_owned();
        if crate::resume::is_resume_not_found(&stderr_text) {
            ResumeOutcome::FallbackToSessionId
        } else {
            ResumeOutcome::HardError(format!(
                "--resume failed (exit={:?}): {stderr_text}",
                exit.code()
            ))
        }
    }

    async fn launch(cfg: SessionConfig, mode: LaunchMode) -> Result<Self, ConnectorError> {
        let SessionConfig {
            claude_binary,
            session_id,
            agent_id,
            working_dir,
            mcp_endpoint,
            permission_mode,
            store,
            blob_reader: _,
            crash_fallback_tx,
        } = cfg;

        let mcp_config = build_mcp_config(&mcp_endpoint);
        let mcp_config_path = working_dir.join(".mcp.json");
        tokio::fs::write(&mcp_config_path, &mcp_config).await?;

        let mut command = Command::new(&claude_binary);
        command
            .arg("--print")
            .arg("--verbose")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--include-partial-messages");
        match mode {
            LaunchMode::Fresh | LaunchMode::ResumeFallback => {
                command.arg("--session-id").arg(session_id.to_string());
            }
            LaunchMode::Resume => {
                command.arg("--resume").arg(session_id.to_string());
            }
        }
        let mut child = command
            .arg("--mcp-config")
            .arg(&mcp_config_path)
            .arg("--permission-mode")
            .arg(&permission_mode)
            .current_dir(&working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()?;

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

        let stderr_ring: StderrRing =
            Arc::new(ParkingMutex::new(VecDeque::with_capacity(STDERR_RING_CAP)));
        let open_turn: OpenTurn = Arc::new(ParkingMutex::new(None));
        let tool_names: ToolNames = Arc::new(ParkingMutex::new(HashMap::new()));
        let shutting_down = Arc::new(AtomicBool::new(false));

        // SessionStarted lands as the very first envelope so consumers can
        // anchor `Session::default_state()`. Failure here is fatal: without
        // the bootstrap event, replay produces a malformed aggregate.
        let started = SessionEvent::SessionStarted {
            session_id,
            agent_id,
            model: None,
            working_dir: working_dir.clone(),
            mcp_endpoint: mcp_endpoint.clone(),
            resumed: mode.resumed(),
            ts: Utc::now(),
        };
        if matches!(mode, LaunchMode::ResumeFallback) {
            // The `SessionStarted` variant has no metadata field, so we
            // surface the resume-fallback via a tracing span field where
            // dashboards and audit logs can distinguish it. The
            // `session.fallback_to_session_id` counter recorded in
            // `Self::resume` is the operator-visible signal.
            tracing::info!(
                session_id = %session_id,
                agent_id = %agent_id,
                fallback = true,
                "session resumed via --session-id fallback after --resume failed",
            );
        }
        append_one(&store, session_id, &started).await?;

        let stderr_handle = tokio::spawn(stderr_drain_task(stderr, Arc::clone(&stderr_ring)));

        let reader_handle = tokio::spawn(reader_task(
            stdout,
            Arc::clone(&store),
            Arc::clone(&open_turn),
            Arc::clone(&tool_names),
            Arc::clone(&stderr_ring),
            cancel.clone(),
            session_id,
            agent_id,
            Arc::clone(&shutting_down),
            crash_fallback_tx,
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
            shutting_down,
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
    /// from `store.load_events` and writes a snapshot at `last_seq`
    /// (producer-side Trigger A). Snapshot save failures do NOT fail
    /// shutdown — the store-side Trigger B catches up on the next subscribe.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn shutdown(self) -> Result<(), ConnectorError> {
        self.shutdown_in_place().await
    }

    /// Same teardown as [`Self::shutdown`] but takes `&self`, so it can be
    /// invoked through an `Arc<ClaudeCodeSession>` (used by
    /// `nephila_lifecycle::ClaudeCodeDriver` on a `Drain` interrupt).
    ///
    /// Idempotent: the `JoinHandle`s and child are stored in
    /// `Mutex<Option<_>>` and `take()`-ed on first call, so re-entry is a
    /// no-op for everything except the (cheap) flag and cancel-token writes.
    #[tracing::instrument(level = "debug", skip(self), fields(session_id = %self.session_id))]
    pub async fn shutdown_in_place(&self) -> Result<(), ConnectorError> {
        // Order matters: `shutting_down` MUST be set before we cancel and
        // before SIGTERM lands. Release ordering ensures the reader's
        // Acquire load observes this write before its EOF/parse-error
        // branch decides whether to emit `SessionCrashed`.
        self.shutting_down.store(true, Ordering::Release);
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
        // Set shutting_down before SIGTERM so the reader's EOF branch sees
        // it as intentional teardown, not a crash.
        self.shutting_down.store(true, Ordering::Release);
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
        let mut g = ring.lock();
        if g.len() == STDERR_RING_CAP {
            g.pop_front();
        }
        g.push_back(line);
    }
}

// `(?i)` is intentional and applies to all alternations: it folds case for
// the keyword group (`api_key`, `secret`, ...) and harmlessly over-matches
// case-variants of `sk-`, `ghp_`, and `Bearer`. Real credentials of those
// schemes are case-sensitive and lowercase, so case-folding only redacts
// non-credential strings that happen to match — the safe direction.
static REDACTION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{30,}|Bearer\s+[A-Za-z0-9._~+/=-]{20,}|(api[_-]?key|secret|token|password)[=:]\s*\S+)",
    )
    .expect("redaction regex compiles")
});

#[must_use]
pub fn redact_stderr(s: &str) -> String {
    REDACTION_RE.replace_all(s, "<redacted>").into_owned()
}

fn snapshot_stderr_tail(ring: &StderrRing) -> String {
    let joined = {
        let g = ring.lock();
        let n = g.len().min(STDERR_TAIL_LINES);
        let start = g.len() - n;
        g.iter().skip(start).cloned().collect::<Vec<_>>().join("\n")
    };
    redact_stderr(&joined)
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
    agent_id: AgentId,
    shutting_down: Arc<AtomicBool>,
    crash_fallback_tx: Option<mpsc::Sender<AgentId>>,
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
                    handle_terminal(
                        &store,
                        session_id,
                        agent_id,
                        &open_turn,
                        &stderr_ring,
                        &shutting_down,
                        crash_fallback_tx.as_ref(),
                        TerminalCause::Eof,
                    )
                    .await;
                    break;
                }
                Err(e) => {
                    handle_terminal(
                        &store,
                        session_id,
                        agent_id,
                        &open_turn,
                        &stderr_ring,
                        &shutting_down,
                        crash_fallback_tx.as_ref(),
                        TerminalCause::ReadError(format!("{e}")),
                    )
                    .await;
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
                handle_terminal(
                    &store,
                    session_id,
                    agent_id,
                    &open_turn,
                    &stderr_ring,
                    &shutting_down,
                    crash_fallback_tx.as_ref(),
                    TerminalCause::ParseError(format!("{e}")),
                )
                .await;
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

#[derive(Debug)]
enum TerminalCause {
    Eof,
    ReadError(String),
    ParseError(String),
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
            // Observability-only frames; no SessionEvents emitted.
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
                        let (_name, prepared) = build_tool_result_event(
                            tool_names,
                            &r.tool_use_id,
                            &raw,
                            is_error,
                            &mut events,
                        );
                        if let Some(p) = prepared {
                            blobs.push(p);
                        }
                    }
                    ContentBlock::McpToolResult(r) => {
                        let is_error = r.is_error.unwrap_or(false);
                        let (name, prepared) = build_tool_result_event(
                            tool_names,
                            &r.tool_use_id,
                            &r.content,
                            is_error,
                            &mut events,
                        );
                        if let Some(p) = prepared {
                            blobs.push(p);
                        }
                        // Only a successful result counts as `CheckpointReached`;
                        // an error is treated as a regular tool result.
                        if !is_error
                            && let Some(name) = name
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
                    // thinking/image/server-tool blocks: not currently surfaced.
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
            let turn_id = open_turn.lock().take();

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
    tool_names
        .lock()
        .insert(tool_use_id.to_owned(), tool_name.to_owned());
}

fn lookup_tool_name(tool_names: &ToolNames, tool_use_id: &str) -> Option<String> {
    tool_names.lock().remove(tool_use_id)
}

/// Builds a `ToolResult` event (inline or spilled per `TOOL_RESULT_INLINE_MAX`)
/// and appends it to `events`. Drains the `tool_names` entry for `tool_use_id`
/// regardless of arm so plain (Read/Edit/Bash/...) and MCP results both
/// release their map slot exactly once. Returns the resolved tool name (if
/// previously recorded) and the `PreparedBlob` when spillover occurred.
fn build_tool_result_event(
    tool_names: &ToolNames,
    tool_use_id: &str,
    raw_output: &serde_json::Value,
    is_error: bool,
    events: &mut Vec<SessionEvent>,
) -> (Option<String>, Option<PreparedBlob>) {
    let name = lookup_tool_name(tool_names, tool_use_id);
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
            return (name, None);
        }
    };
    if bytes.len() <= TOOL_RESULT_INLINE_MAX {
        events.push(SessionEvent::ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            output: ToolResultPayload::Inline(raw_output.clone()),
            is_error,
            ts: Utc::now(),
        });
        return (name, None);
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
    (name, Some(prepared))
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

/// Reader-side terminal handler.
///
/// On `shutting_down == true`, emits nothing — `shutdown()` itself appends
/// `SessionEnded` after reader drain. On `false`, emits `[TurnAborted (if open),
/// SessionCrashed]` in a SINGLE `append_batch` call so consumers observing
/// `SessionCrashed` always see a preceding `TurnAborted` when a turn was open.
///
/// If the `append_batch` itself fails (DB error etc.), the registry's crash
/// subscription will never see `SessionCrashed` — so we surface the failure
/// via the optional `crash_fallback_tx` mpsc channel as an out-of-band signal.
#[allow(clippy::too_many_arguments)]
async fn handle_terminal(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
    agent_id: AgentId,
    open_turn: &OpenTurn,
    stderr_ring: &StderrRing,
    shutting_down: &AtomicBool,
    crash_fallback_tx: Option<&mpsc::Sender<AgentId>>,
    cause: TerminalCause,
) {
    if shutting_down.load(Ordering::Acquire) {
        // Intentional teardown: reader emits nothing, shutdown() writes
        // SessionEnded after drain.
        return;
    }

    let mut events = Vec::new();
    let open = open_turn.lock().take();
    if let Some(turn_id) = open {
        events.push(SessionEvent::TurnAborted {
            turn_id,
            reason: "session_crashed".into(),
            ts: Utc::now(),
        });
    }
    let tail = snapshot_stderr_tail(stderr_ring);
    let reason = match cause {
        TerminalCause::Eof => format!("EOF\n--- stderr tail ---\n{tail}"),
        TerminalCause::ReadError(e) => {
            format!("stdout read error: {e}\n--- stderr tail ---\n{tail}")
        }
        TerminalCause::ParseError(e) => {
            format!("unparseable frame: {e}\n--- stderr tail ---\n{tail}")
        }
    };
    events.push(SessionEvent::SessionCrashed {
        reason,
        exit_code: None,
        ts: Utc::now(),
    });

    if let Err(e) = append_batch(store, session_id, events, Vec::new()).await {
        tracing::error!(
            %agent_id,
            %session_id,
            error = ?e,
            "failed to append crash events; registry will not see SessionCrashed via store",
        );
        nephila_store::metrics::record_crash_append_failed(&session_id.to_string());
        if let Some(tx) = crash_fallback_tx {
            let _ = tx.try_send(agent_id);
        }
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
                *open_turn.lock() = Some(turn.turn_id);
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
    let payload = serde_json::to_value(event)?;
    // Sequence is stamped by the writer thread per ADR-0002 / ADR-0003.
    Ok(EventEnvelope::new(
        nephila_eventsourcing::envelope::NewEventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".to_owned(),
            aggregate_id: session_id.to_string(),
            event_type: event.kind().to_owned(),
            payload,
            trace_id: TraceId(session_id.to_string()),
            outcome: None,
            timestamp: Utc::now(),
            context_snapshot: None,
            metadata: HashMap::new(),
        },
    ))
}

async fn append_one(
    store: &Arc<SqliteStore>,
    session_id: Uuid,
    event: &SessionEvent,
) -> Result<(), ConnectorError> {
    let env = make_envelope(session_id, event)?;
    store.append_batch(vec![env]).await?;
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
        store.append_batch(envelopes).await?;
    } else {
        store.append_batch_with_blobs(envelopes, blobs).await?;
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
        state = state
            .apply_envelope(env)
            .map_err(nephila_eventsourcing::store::EventStoreError::serialization)?;
        last_seq = env.sequence();
    }
    let state_value = serde_json::to_value(&state)
        .map_err(nephila_eventsourcing::store::EventStoreError::serialization)?;
    let snap = Snapshot {
        aggregate_type: "session".to_owned(),
        aggregate_id: agg_id,
        sequence: last_seq,
        state: state_value,
        timestamp: Utc::now(),
    };
    store.save_snapshot(&snap).await
}

#[cfg(test)]
mod tool_names_cleanup_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn fresh_tool_names() -> ToolNames {
        Arc::new(ParkingMutex::new(HashMap::new()))
    }

    #[test]
    fn plain_tool_result_drains_tool_names_entry() {
        let names = fresh_tool_names();
        record_tool_use(&names, "tool-1", "Read");
        assert_eq!(names.lock().len(), 1);

        let raw = serde_json::Value::String("ok".into());
        let mut events: Vec<SessionEvent> = Vec::new();
        let _prepared = build_tool_result_event(&names, "tool-1", &raw, false, &mut events);

        assert_eq!(
            names.lock().len(),
            0,
            "ToolResult handling must drain the tool_names entry",
        );
    }

    #[test]
    fn mcp_tool_result_drains_tool_names_entry() {
        let names = fresh_tool_names();
        record_tool_use(&names, "tool-2", "serialize_and_persist");

        let raw = serde_json::Value::Object(serde_json::Map::new());
        let mut events: Vec<SessionEvent> = Vec::new();
        let _prepared = build_tool_result_event(&names, "tool-2", &raw, false, &mut events);

        assert_eq!(names.lock().len(), 0);
    }
}
