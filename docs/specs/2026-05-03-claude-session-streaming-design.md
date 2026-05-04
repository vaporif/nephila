# Claude Session Streaming Design

Status: draft (revised after multi-reviewer + verification pass)
Date: 2026-05-03

## Summary

Replace the current per-turn `claude -p` subprocess model and the TTY-handoff `attach_agent_session` flow with:

1. One long-lived `claude --print --verbose --input-format stream-json --output-format stream-json` process per agent, owned by a rewritten `ClaudeCodeSession` in the connector crate.
2. A new `Session` aggregate (real `EventSourced` impl, with `apply` reducer) in `nephila-eventsourcing` that records every prompt, assistant message, tool call, tool result, checkpoint, and lifecycle event for an agent's session.
3. A new `SessionPane` in the TUI that subscribes to its agent's event stream and renders a live, scrollable history with an inline multi-line input box for the human.
4. A rewritten `SessionSupervisor` that drives the autonomous loop by reacting to `(CheckpointReached, TurnCompleted)` event pairs (instead of process-exit polling) and calling `session.send_turn(...)` on the same long-lived process.

The headless `-p` connector is removed in slice 5. The TTY-handoff attach is **kept as an opt-in escape hatch** (hotkey `a`) until a UX-parity matrix for the embedded pane is signed off — see *UX parity gap* below. The agent's checkpoint MCP tools (`serialize_and_persist`, `request_human_input`) keep their existing semantics.

## Motivation

Today's `ClaudeCodeConnector` (`crates/connector/src/claude_code.rs`) spawns a fresh `claude` process for every turn:

```
spawn(prompt) -> claude -p <prompt> -> exit -> supervisor sees exit ->
respawn(next prompt) -> ...
```

Costs:
- Cold-start latency on every turn (process launch, MCP reconnect, context reload via `get_session_checkpoint`).
- No live message visibility outside the process. The TUI sees only the final stdout string.
- The TTY-handoff attach in `crates/tui/src/lib.rs:704` blanks the TUI to give claude the terminal. The user can't see other agents, can't switch agents without exiting claude, and any session history before the attach is gone.

A persistent stream-json process per agent removes the cold-start, exposes a continuous message stream, and lets the TUI render the session as a regular panel.

## Goals

- Persistent claude session per agent. Turns flow over stdin without restarting the process.
- Embedded live-history panel in the TUI, with an opt-in escape hatch to the real claude TUI for features the panel doesn't yet replicate.
- Single source of truth for session activity for *consumer fan-out and replay*: the persisted event log. Multiple consumers (TUI, supervisor, future tools) read from it without coordinating.
- Crash resilience. Restart nephila and the panel can replay full history. Crash claude and the orchestrator restarts it with `--resume <session_id>` (or `--session-id <session_id>` if no on-disk session exists yet); consumers reconnect by replaying events past their last-seen sequence.

## Non-goals

- Replacing the API connectors (`anthropic_api.rs`, `openai_compatible.rs`). They keep `TaskConnector`. Unifying them would force a stream shape on request/response APIs that genuinely don't have one. Future work if it ever matters.
- New OS-level windowing. No popup terminals, no web UI. Embedded ratatui pane only.
- Auto-checkpointing. The agent still decides when to call `serialize_and_persist` / `request_human_input`. The system prompt that teaches it that does not need to change.
- The SessionEvent log is **not** authoritative for claude's conversation state. Claude persists its own JSONL transcript at `~/.claude/projects/<encoded-cwd>/<session_id>.jsonl` (one line per message); `--resume` rehydrates from there. The SessionEvent log is for consumer fan-out, replay-on-focus, and audit. This separation is load-bearing — see *Failure modes* below.

## Architecture overview

```
┌─ crates/eventsourcing ─────────────────────────────────────────┐
│  Session (new EventSourced aggregate)                          │
│    State: SessionState { phase, open_turn, last_checkpoint, ..}│
│    Event: SessionEvent (variants below)                        │
│    apply(event) enforces invariants                            │
│                                                                │
│  DomainEventStore (existing) gains:                            │
│    + subscribe_after(...)                                      │
│    + append_batch(...)                                         │
│    + prune_aggregate(...)                                      │
└────────▲──────────────────────────────────────▲────────────────┘
         │ append/append_batch                  │ subscribe_after
         │                                      │
┌────────┴────────────────────────┐  ┌──────────┴─────────────────┐
│ crates/connector                │  │ Consumers                  │
│  ClaudeCodeSession (rewrite)    │  │  • crates/tui SessionPane  │
│   owns one claude process       │  │  • crates/lifecycle        │
│   (stream-json mode)            │  │      SessionSupervisor     │
│   sole producer per session     │  │  • bin/orchestrator        │
│   coalesces deltas              │  │      SessionRegistry       │
│   exposes send_turn(text)       │  │  • future: metrics, audit, │
│                                 │  │           Slack, etc.      │
└─────────────────────────────────┘  └────────────────────────────┘
```

**Single-producer-per-aggregate invariant.** Only `ClaudeCodeSession` calls `append`/`append_batch` for a given `session_id`. Supervisor and SessionPane do *not* append directly — they call `session.send_turn(...)`, which routes the corresponding `*PromptSent` event through the connector. This invariant lets us assign sequences without inter-process coordination (see *Store primitives*).

## Components

### `Session` aggregate (new, in `nephila-core`)

Real `EventSourced` impl, second in the codebase after `Agent`. Sketch:

```rust
pub struct Session {
    pub id: SessionId,
    pub agent_id: AgentId,
    pub phase: SessionPhase,        // Starting | Running | WaitingHitl | Paused | Crashed | Ended
    pub open_turn: Option<TurnId>,  // Some(_) iff *PromptSent has not yet closed
    pub last_checkpoint: Option<CheckpointRef>,
    pub last_seq: u64,
}

pub enum SessionPhase { Starting, Running, WaitingHitl, Paused, Crashed, Ended }

impl EventSourced for Session {
    type Event = SessionEvent;
    type Command = SessionCommand;
    type Error = SessionError;
    fn apply(self, event: Self::Event) -> Self { /* see Invariants */ }
    fn handle(&self, cmd: Self::Command) -> Result<Vec<Self::Event>, Self::Error> { /* ... */ }
    fn default_state() -> Self { /* ... */ }
    fn aggregate_type() -> &'static str { "session" }
}
```

`SessionEvent` variants:

| Variant | Payload | Source |
|---|---|---|
| `SessionStarted` | `{ session_id, agent_id, model, working_dir, mcp_endpoint, resumed: bool }` | `ClaudeCodeSession::start` / `::resume` |
| `AgentPromptQueued` | `{ turn_id, text, ts }` | `ClaudeCodeSession::send_turn(Agent, ...)` — appended *before* stdin write |
| `AgentPromptDelivered` | `{ turn_id, ts }` | `ClaudeCodeSession::send_turn(Agent, ...)` — appended *after* successful stdin write |
| `HumanPromptQueued` / `HumanPromptDelivered` | same shape, distinguished source | `send_turn(Human, ...)` |
| `PromptDeliveryFailed` | `{ turn_id, reason, ts }` | writer task on `BrokenPipe` etc. |
| `AssistantMessage` | `{ message_id, seq_in_message, delta_text, is_final, ts }` | claude stream (coalesced — see *Streaming*) |
| `ToolCall` | `{ tool_use_id, tool_name, input_json, ts }` | claude stream |
| `ToolResult` | `{ tool_use_id, output_json, is_error, ts }` | claude stream |
| `CheckpointReached` | `{ checkpoint_id, interrupt: Option<InterruptSnapshot>, ts }` | claude stream — derived from `ContentBlock::McpToolResult` for `serialize_and_persist` / `request_human_input` (verified: `claude_codes::io::McpToolResultBlock` carries the tool's full return payload including `checkpoint_id`) |
| `TurnCompleted` | `{ turn_id, stop_reason, ts }` | claude stream (`ClaudeOutput::Result`) |
| `TurnAborted` | `{ turn_id, reason, ts }` | aggregate reducer on `SessionCrashed` with an open turn; or explicit cancellation |
| `SessionCrashed` | `{ reason, exit_code: Option<i32>, ts }` | reader task on EOF / parse error |
| `SessionEnded` | `{ ts }` | `ClaudeCodeSession::shutdown` |

Aggregate type: `"session"`. Aggregate id: the session UUID, **stable across `--resume`**. Slice-0 spike confirms this is the case (see *Open assumptions*); if it isn't, we add a projection table mapping `agent_id → current session_id` and key consumers off `agent_id`.

#### Invariants enforced by `apply`

- Every `*PromptQueued { turn_id }` is followed by exactly one of `{TurnCompleted, TurnAborted, PromptDeliveryFailed}` for the same `turn_id`. `SessionCrashed` implicitly emits `TurnAborted` for any open turn (handled in the connector reader before appending the crash event).
- After `SessionEnded`, no further events may append for the aggregate. The reducer transitions to a terminal state; `handle` rejects all commands.
- `AssistantMessage` deltas with the same `message_id` arrive in monotonically increasing `seq_in_message`, terminated by `is_final: true`. A truncated delta is still `is_final: false` with a `truncated: true` flag; the connector emits a synthesized `is_final: true` immediately after if it sees the message close from the stream.

### Streaming policy (assistant deltas)

claude streams partial assistant text token-by-token. Persisting every token would explode the event log. The connector coalesces:

- Buffer deltas in memory keyed by `message_id`.
- Flush a chunk when **any** of: 5 deltas accumulated, 250ms elapsed since last flush, the stream emits the message-close marker, or buffered size approaches the per-event payload cap (see *Payload cap*).
- The final chunk of a message has `is_final: true`. Renderers coalesce by `message_id`.

This bounds event volume to ~few-per-second per active turn instead of token-by-token. Coalescing happens entirely inside `ClaudeCodeSession`'s reader task; consumers see the same coalesced stream regardless of whether they live-subscribe or replay.

### Payload cap

Individual event payloads are capped at a configurable size (default 256 KiB). Larger payloads are split:

- For `AssistantMessage`: split into multiple deltas with the same `message_id`.
- For `ToolResult`: persist a marker `{ truncated: true, original_len: N, hash: ..., snippet: <first 8 KiB> }` and spill the full payload to a side blob store keyed by hash.

The side blob store is a v1 requirement, **not deferred**. ToolResults from `grep`/`read`/`find` exceed 256 KiB routinely; truncation as the only mechanism would lose data that consumers (audit, future replay-into-claude) need. Implementation: a `crates/store` `BlobStore` trait with a SQLite-blob-table default impl; ~80 lines. Slice 1b scope.

### `DomainEventStore` primitives (new methods)

```rust
fn subscribe_after(
    &self,
    aggregate_type: &str,
    aggregate_id: &str,
    since_sequence: u64,
) -> impl Stream<Item = Result<EventEnvelope, EventStoreError>> + Send;

fn append_batch(
    &self,
    envelopes: Vec<EventEnvelope>,
) -> Result<(), EventStoreError>;

fn prune_aggregate(
    &self,
    aggregate_type: &str,
    aggregate_id: &str,
    before_sequence: u64,
) -> Result<u64, EventStoreError>;
```

#### `subscribe_after` semantics — listener-first, head-second

The race that an earlier draft missed: if you snapshot `current_head` *first* and register the listener *second*, an append committing in between is lost — the broadcast fires before the listener exists, and the replay only goes up to the prior head. Correct order:

```
1. listener = sender.subscribe()        // attach FIRST; buffers live events
2. head_at_subscribe = current_head()
3. backfill = load_events(since_seq+1, head_at_subscribe)
4. yield backfill in order
5. yield listener.recv() with seq > head_at_subscribe (dedup against backfill)
```

Sequence assignment happens inside the writer thread (`crates/store/src/writer.rs`): `append` and `append_batch` compute `MAX(sequence)+1` per `(aggregate_type, aggregate_id)` *inside the writer's critical section* and stamp the envelope before the SQL insert. This is feasible because of the single-producer-per-aggregate invariant: `EventEnvelope::sequence` becomes assigned-by-store, not caller-supplied. The change to `EventEnvelope` is breaking; existing call sites (only in `Agent` flows) are updated as part of slice 1b.

Broadcast publish happens **inside** the writer-thread closure, after the SQL insert commits and before the next append runs. This guarantees publish ordering matches commit ordering.

#### Broadcast bookkeeping

Per `(aggregate_type, aggregate_id)` `tokio::sync::broadcast::Sender`s live in a `DashMap<(String, String), broadcast::Sender<EventEnvelope>>`, lazily initialized on first subscribe. Bound 4096 (sized to absorb one full tool-heavy turn).

`tokio::sync::broadcast::Sender` has no subscriber-drop notification — `Sender::closed()` fires on sender drop, not receiver drop. The DashMap entries are **not** garbage-collected on subscriber drop (the spec previously claimed this; it isn't implementable without a wrapper). Bound: one entry per aggregate ever seen in the process lifetime. For nephila this is bounded by `total agents × sessions-per-agent` over the process lifetime — typically O(100), worst case O(10k). Acceptable. If memory becomes an issue, add a periodic sweep that drops entries with `receiver_count() == 0` and `last_publish_age > 1h`.

`RecvError::Lagged` recovery: consumer falls back to `load_events(last_seen)` and re-subscribes. To avoid livelock under sustained burst, after K=3 lagged retries within 30s the consumer switches to polling `load_events` every 500ms for a 30s cooldown, then re-subscribes. Backfill `load_events` uses a separate read-only SQLite connection (not the writer thread) — see *Throughput*.

#### Throughput

The current `crates/store/src/writer.rs:17-25` funnels all writes through one OS thread. With "hundreds of events per turn" × N agents this becomes the system bottleneck. Mitigations:

- **`append_batch`**: the connector reader collects all events emitted from a single stream-json frame (e.g. one assistant message + tool calls + tool results) and submits them in one `INSERT` round-trip. Drops fsync count from per-event to per-frame.
- **`PRAGMA synchronous = NORMAL`**: explicit setting in `crates/store/src/lib.rs`. Safe with WAL (already enabled).
- **Read pool**: a separate `rusqlite::Connection` pool for `load_events` and `subscribe_after` backfill, so reads don't queue behind writes.

Slice 1b includes a benchmark gate: 1 agent emitting 500 events/turn over 10 turns must complete in <2s p95 with 4 concurrent subscribers. Below that, slice 1b doesn't merge.

#### Retention

`prune_aggregate(type, id, before_seq)` deletes events strictly before `before_seq`. Default policy in `SessionRegistry`:

- On `SessionEnded`: take a final snapshot, then prune everything before `last_seq - K` (K=200 to preserve recent tail for audit).
- Periodic sweep (daily): for sessions in terminal state for >7d, prune everything before the latest snapshot.

Hard-to-reverse: every consumer learns "events before X are gone." Documented as the consumer contract.

### `ClaudeCodeSession` (rewrite of `crates/connector/src/claude_code.rs`)

Owns one `tokio::process::Child` per session. Two internal tasks per session, plus a delta coalescer.

Spawn command (corrected from earlier draft — `--input-format stream-json` requires `--print --verbose` per `claude --help` and `claude-codes/src/cli.rs:646-656`):

```
claude --print --verbose
       --input-format stream-json
       --output-format stream-json
       --include-partial-messages
       --session-id <uuid>            # first start
       # OR
       --resume <uuid>                # crash recovery; falls back to --session-id if no on-disk session
       --mcp-config <path>
       --permission-mode <mode>
       --settings <json>
       (current_dir = <agent_workdir>)
```

`current_dir` must match the original session — claude's transcript file is keyed off encoded cwd, and resume fails silently with "no conversation found" on cwd mismatch (Anthropic SDK docs).

Tasks:

- **Reader**: `FramedRead<ChildStdout, JsonLines<ClaudeOutput>>`. For each frame: translate to one or more `SessionEvent`s and submit via `append_batch`. On EOF or parse error: if `open_turn.is_some()`, append `TurnAborted { turn_id, reason: "session_crashed" }` first, then `SessionCrashed`. Terminate the writer task via `CancellationToken`, return.
- **Writer**: receives `Turn { source, text, turn_id }` from a `mpsc::Sender<Turn>` (bounded 16). Sequence per turn:
  1. Append `*PromptQueued { turn_id, text }`.
  2. Write `ClaudeInput::User { content }` JSON line to claude's stdin.
  3. On success: append `*PromptDelivered { turn_id }`. On `BrokenPipe`: append `PromptDeliveryFailed { turn_id, reason }`.

The Queued/Delivered split fixes the prior "intent recorded but no following result" ambiguity (an earlier draft appended a single `*PromptSent` and gave consumers no way to distinguish in-flight from lost). The supervisor's restart/resume logic keys on `Delivered`, not `Queued`.

- **Coalescer**: an in-task buffer for `AssistantMessage` deltas (see *Streaming policy*).

Public API:

```rust
pub struct ClaudeCodeSession { /* private */ }

impl ClaudeCodeSession {
    pub async fn start(cfg: SessionConfig) -> Result<Self, ConnectorError>;

    /// Resume from a session id. Falls back to fresh start with the same id
    /// if no on-disk session exists yet (e.g. crash before first message).
    pub async fn resume(cfg: SessionConfig, session_id: SessionId) -> Result<Self, ConnectorError>;

    pub async fn send_turn(&self, source: PromptSource, text: String) -> Result<TurnId, ConnectorError>;

    /// Graceful: SIGTERM, await reader task, append `SessionEnded`, await writer drain.
    pub async fn shutdown(self) -> Result<(), ConnectorError>;

    pub fn id(&self) -> SessionId;
    pub fn agent_id(&self) -> AgentId;
}

pub enum PromptSource { Agent, Human }
```

`Drop` does **not** rely on `kill_on_drop`. The previous draft's `kill_on_drop(true)` raced the reader: SIGKILL fired synchronously in `Drop`, the reader saw EOF, and every non-`shutdown()` exit produced a spurious `SessionCrashed`. Replacement: explicit `Drop` impl sends SIGTERM, spawns a 500ms watchdog that escalates to SIGKILL, and lets the reader append `SessionEnded` (or `SessionCrashed` if SIGTERM didn't take). Callers should still prefer `shutdown().await` for clean teardown; `Drop` is the safety net.

Why nephila spawns directly and uses `claude-codes` only for type defs / framing: see *Decision 2* below.

### `SessionSupervisor` (rewrite of `crates/lifecycle/src/lifecycle_supervisor.rs`)

Subscribes once per active session via `subscribe_after(session_id, last_seen_seq)`. State machine per session:

```
on (CheckpointReached(interrupt), then TurnCompleted) for the same turn:
  match interrupt {
    None | Some(Drain) => emit SessionEnded, drop session handle
    Some(Hitl)         => mark session waiting; SessionPane will render the
                          question; do not auto-prompt
    Some(Pause)        => SIGSTOP the child until resume command; document
                          that Pause halts the process, not just the prompt loop
    None (post-task)   => compose next prompt, call session.send_turn(Agent, ...)
  }
on TurnAborted:
  treat as turn failure; do not auto-prompt; surface to operator
on PromptDeliveryFailed:
  log; supervisor will retry on next CheckpointReached if appropriate
on SessionCrashed:
  ask SessionRegistry to respawn; new RestartTracker semantics (below)
```

Key change vs earlier draft: the supervisor waits for `(CheckpointReached, TurnCompleted)` *both* before calling `send_turn(Agent, ...)`. `CheckpointReached` fires mid-turn (claude is still emitting); injecting a new prompt at that point would interleave with claude's in-flight output. `TurnCompleted` is the only safe boundary.

**RestartTracker semantics** (`crates/lifecycle/src/supervisor.rs:21-39`) change:

- Today's tracker increments on every `AgentExited` (= every turn). Window+threshold are calibrated for that.
- Persistent processes don't exit per turn. New mapping: `SessionCrashed` → `record_restart`. `SessionEnded` (clean) → `reset_tracker`. `TurnAborted` (recoverable) → no-op.
- Default `max_restarts` and `restart_window_secs` are recalibrated for the new units (5 crashes in 600s instead of 3 exits in 60s). Documented as a config break in slice 5 release notes.

The "compose next prompt" call replaces today's `compose_prompt` in `bin/src/orchestrator.rs:21`. Same logic, just invoked over a live channel instead of as input to a fresh subprocess.

### `SessionPane` (new TUI panel in `crates/tui/src/panels/`)

Embedded panel; complements (does **not** delete) `attach_agent_session`. Activated by Enter on an agent in the agent tree; the TTY-handoff attach moves to hotkey `a` until the parity matrix below is closed.

#### UX parity gap (vs `attach_agent_session`)

The TTY handoff at `crates/tui/src/lib.rs:704-746` gives the user the real claude TUI: markdown rendering, syntax-highlighted code, slash commands (`/clear`, `/compact`, `/cost`, `/init`, `/agents`, `/release-notes`), tab-completion on file paths, image paste, multi-line composer, native interrupt-this-turn keybinding, and interactive permission prompts. The embedded pane replicates a subset:

| Feature | Embedded pane (v1) | Notes |
|---|---|---|
| Live message stream | yes | structured render |
| Multi-line input | yes | `tui-textarea` widget; Ctrl+Enter submits |
| Markdown rendering | **deferred** | render plain text in v1; slice 6 candidate for `tui-markdown` |
| Code syntax highlight | **deferred** | same |
| Slash commands | **partial** | route `/clear`, `/compact`, `/cost` via control protocol; others fall through to attach |
| File-path tab complete | **dropped** | escape to attach for this |
| Image paste | **dropped** | escape to attach |
| Interactive permission prompts | **deferred** | `--permission-mode bypassPermissions` keeps current behavior; full handling needs control-protocol UI in slice 7 |
| Interrupt this turn | yes | new key (Ctrl+C in pane) sends SIGINT-equivalent control message |

Until slices 6-7 ship, hotkey `a` keeps the TTY handoff available. Slice 5 does **not** delete `attach_agent_session`; it's deleted later when the parity matrix is closed.

#### Pane behavior

- **On focus**: call `subscribe_after(session_id, 0)`. Spawn a per-pane pump task that pumps events into a `VecDeque<RenderedEvent>`. **No cap.** Replaces the earlier 2000-event cap that would have required a "scroll-back-to-store" feature the spec never designed. Memory cost is bounded by the 256 KiB-per-event payload cap and the snapshot/prune policy.
- **Coalesced render of partial assistant messages**: keep a `current_message: Option<MessageBuffer>` keyed by `message_id`. Append deltas to the buffer; finalize the buffer's row when `is_final: true` arrives.
- **Render**: vertically scrolling list, each event with timestamp and typed glyph (`AGENT →`, `YOU →`, `ASSIST`, `TOOL`, `↳ result`, `✓ CHECKPOINT`, `✗ CRASH`, `↺ ABORT`).
- **Input mode vs normal mode** (vim-flavored, to disambiguate Esc and global keys):
  - Normal mode: `i` enters input mode, `q` quits pane, `Tab` cycles focus, `j`/`k` scroll, Esc returns to global event log.
  - Input mode: keystrokes go to the input box, Ctrl+Enter submits, Esc returns to normal mode (does not leave pane).
- **HITL**: when the most recent `CheckpointReached` carries `Some(Hitl)`, the existing `Modal::HitlResponse` flow (`crates/tui/src/modal.rs:86-117`) is reused — full popup, full text, discrete options preserved. The pane shows a marker but doesn't try to fit a 500-char question into the input box.

#### Layout

`crates/tui/src/layout.rs:31-46` grows a "session focused" variant that **keeps the global event log visible**. Three columns: agent_tree (slim, 20%) | session_pane (wide, 60%) | event_log (slim, 20%). Operators retain visibility of cross-agent activity. Earlier draft's "third variant replacing event_log" regressed multi-agent observability.

Per-agent activity indicator in the agent tree: each agent row gets a glyph reflecting its last `SessionEvent` type (e.g. `✓` checkpoint, `▶` running turn, `✗` crashed). So the operator can spot trouble on unfocused agents without switching.

#### Pump-task lifecycle

One pump-task per agent (not per pane focus), started when the agent's `SessionStarted` is observed by the registry, ended on `SessionEnded`. The pump owns the per-agent ring buffer; panes read from it. Focus changes only swap which agent's buffer the renderer points at — they do not spawn or cancel tasks. This avoids the earlier draft's ambiguity about per-focus task cancellation.

### `SessionRegistry` (new, in `bin/src/orchestrator.rs`)

Holds `HashMap<AgentId, SessionHandle>`, where `SessionHandle = { session: Arc<ClaudeCodeSession>, pump: AbortHandle }`. Single async task subscribed to a global filter on `SessionCrashed` events across all aggregates.

On crash:
1. Drop the old handle (graceful shutdown if possible).
2. Call `ClaudeCodeSession::resume(session_id)`. Resume's fallback path tries `--resume <id>` first; on "no conversation found" (claude exit code or stderr match), retries with `--session-id <id>` for sessions that crashed before claude flushed their first message.
3. Insert the new handle.

On nephila restart: scan agents in active session phase from the store; call `resume(session_id)` for each. Consumers reconnect via `subscribe_after(last_seen)`.

**Cross-process double-resume**: nephila acquires a process-wide lockfile at `~/.nephila/<workdir-hash>.lock` on startup. Two nephila processes against the same workdir is a configuration bug; the second exits with a clear error. Without this, both processes would call `--resume` on overlapping sessions and produce undefined event interleaving (the SQLite UNIQUE constraint on `(aggregate_type, aggregate_id, sequence)` would surface as random `Storage` errors).

## Data flow: a single human prompt

```
You type "add a fuzz test" in SessionPane and hit Ctrl+Enter
    │
    ▼
SessionPane calls session.send_turn(Human, "add a fuzz test") -> turn_id
    │
    ▼
ClaudeCodeSession (writer task):
    1. append HumanPromptQueued { turn_id, text }
    2. write ClaudeInput::User to claude's stdin
    3. on success: append HumanPromptDelivered { turn_id }
       on BrokenPipe: append PromptDeliveryFailed { turn_id, reason }
    │
    └──► subscribers (this same SessionPane via the per-agent pump,
         SessionSupervisor) see the events within milliseconds via
         subscribe_after's broadcast tap. Pane re-renders with your line.
    │
    ▼
claude reads JSON, processes, streams output. Reader task:
    AssistantMessage deltas (coalesced)  → append → pane re-renders
    ToolCall                             → append → pane re-renders
    ToolResult                           → append (large output spilled to blob) → pane re-renders
    ContentBlock::McpToolResult for serialize_and_persist
                                         → append CheckpointReached { checkpoint_id, ... }
    ClaudeOutput::Result { stop_reason } → append TurnCompleted { turn_id, stop_reason }
    │
    ▼
Supervisor sees (CheckpointReached, TurnCompleted) pair → composes next prompt → send_turn(Agent, ...)
```

## Decisions and trade-offs

### Decision 1: Session as event-sourced aggregate (vs two-trait split)

We considered a smaller change: add a new `SessionConnector` trait alongside `TaskConnector`, claude becomes the only `SessionConnector`, internal `tokio::sync::broadcast` for fan-out, no persistence. We picked the event-sourced shape with a real reducer.

- Pros: TUI history persists across nephila restarts; new consumers attach without changing producers; aligns with `Agent`'s existing `EventSourced` impl; `apply` enforces invariants (turn boundaries, terminal-state lockout) so consumers don't re-derive them.
- Cons: more upfront work (reducer, `subscribe_after`, snapshot policy, retention primitive, blob spillover); larger event log on disk.

Captured separately in ADR `docs/adr/0001-session-as-event-sourced-aggregate.md`.

### Decision 2: `claude-codes` used only as a protocol library

We use `claude-codes` (https://docs.rs/claude-codes) for `ClaudeInput` / `ClaudeOutput` / `ContentBlock` / `McpToolResultBlock` / `JsonLines` framing, but not its `AsyncClient` / `SyncClient`. nephila spawns the `claude` process with `tokio::process::Command`.

Verified: the `claude_codes::io::*` and `claude_codes::protocol::*` modules are public (https://docs.rs/claude-codes/latest/claude_codes/io/index.html, https://docs.rs/claude-codes/latest/claude_codes/protocol/index.html).

- Pros: full control over CLI flags; lifecycle stays in nephila; if the crate goes stale or its protocol-version pin (currently CLI v2.1.3) becomes wrong, we replace the type defs without rewriting the spawn.
- Cons: ~150 lines of process glue we'd otherwise get free; if JSON shapes change in a future CLI release we still chase them.

### Decision 3: Persistent process per agent (vs spawn-per-turn) — and how we actually shed memory

Today: every turn = one `claude -p` process. New: one `claude --print --verbose --input-format stream-json` process per agent, alive across turns.

- Pros: no cold-start per turn (process launch + MCP reconnect + checkpoint reload). Streaming visibility for the TUI. Matches how the official Claude Agent SDK drives the CLI.
- Cons: long-lived process accumulates conversation context; eventual context-window pressure or memory bloat; crash semantics shift from "process exit = supervisor signal" to "stdout EOF mid-stream that we detect and recover from."

**Memory-shed mitigation (corrected from earlier draft)**: `--resume` does **not** shed memory — it rehydrates the entire JSONL transcript into the new process. To actually reclaim conversation context, we recycle by:

1. Agent calls `serialize_and_persist` (distills state into channels / L2 in nephila's checkpoint store).
2. `SessionEnded` is appended; we kill the bloated process.
3. We start a *fresh* process with a new `--session-id <new-uuid>` (not `--resume`) and the standard cold-start prompt that calls `get_session_checkpoint` to rebuild context from the distilled state.

This is the existing per-turn boot path, on demand. `--resume` is reserved for crash recovery only. The earlier draft conflated the two.

### Decision 4: Coexist with `BusEvent` transitionally

`crates/core/src/event.rs:32` defines `BusEvent`, an in-memory enum used for live notifications (state changes, token reports, HITL requests, shutdown). `SessionEvent` is for durable session activity; `BusEvent` is for ephemeral coordination signals.

- `BusEvent::CheckpointSaved` overlaps with `SessionEvent::CheckpointReached`. After the slice-0 spike confirms `McpToolResult` carries `checkpoint_id` (verified in this draft), we emit `CheckpointReached` from the connector reader only — not from the MCP handler. `BusEvent::CheckpointSaved` keeps its current emission for UI-layer compatibility but is marked deprecated in slice 5; consumers migrate to subscribe to the durable log. Slice 6 removes the bus emission.

## Snapshotting

Snapshots are real aggregate snapshots (not render caches). The `Snapshot.state` JSON is the serialized `Session` reducer state at sequence N — what `apply` would produce after replaying events 0..=N. Replay from snapshot: `Session::default_state() → snapshot.state → apply(events[N+1..])`.

For the SessionPane's render, a separate `session_render_cache` table holds the most recent K rendered events. The pane on focus loads the render cache (if recent), then `subscribe_after(cache.sequence)` to backfill. This keeps the aggregate-snapshot contract (`crates/eventsourcing/src/snapshot.rs`) intact.

Trigger policy:
- Aggregate snapshot: on `SessionEnded` and on first `subscribe_after` request when `current_seq - last_snapshot_seq > 1000`. **Not** every 200 appends — that fires roughly per turn for tool-heavy agents, paying the cost on every turn for a benefit only focused panes see.
- Render cache: invalidated on `SessionEnded`; refreshed lazily on `subscribe_after`.

This is a v1 policy. Revisit when we have real session-size data.

## Failure modes

| Failure | Detection | Response |
|---|---|---|
| claude process crashes mid-turn | reader task sees stdout EOF / non-zero exit | append `TurnAborted` (if open turn), then `SessionCrashed`; SessionRegistry observes and respawns; consumers see the gap and reconnect via `subscribe_after(last_seen)` |
| claude emits malformed JSON | reader task parse error | log; append `SessionCrashed { reason: "unparseable frame" }`; same recovery. We do not skip-and-continue — a malformed frame means we can't trust subsequent state. |
| stdin write fails (`BrokenPipe`) | writer task | append `PromptDeliveryFailed { turn_id, reason }`; the impending `SessionCrashed` from the reader handles cleanup |
| store append fails | `EventStoreError` from `append`/`append_batch` | propagate up; `send_turn` returns error; no event published. Caller decides retry. |
| consumer falls behind broadcast bound | `RecvError::Lagged` | re-subscribe from last persisted seq; after 3 retries in 30s, switch to polling for 30s cooldown |
| `--resume` finds no on-disk session | claude exit + "No conversation found" | `ClaudeCodeSession::resume` falls back to `--session-id <id>` (fresh start with the same id); appends `SessionStarted { resumed: true, fallback: true }` for telemetry |
| nephila process restarts | session handles vanish from `SessionRegistry` | on startup, registry scans agents in active session phase, calls `resume(session_id)` for each. Consumers reconnect via `subscribe_after(last_seen)`. |
| restart limit exceeded | `RestartTracker` (recalibrated for crash-not-exit semantics) | `SessionEnded`; agent moves to terminal state |
| two nephila processes against the same workdir | startup lockfile | second process exits with explicit error (avoids overlapping `--resume` calls) |
| ToolResult > 256 KiB | reader task | spill payload to blob store; persist a marker event with hash + first 8 KiB |

## Observability

The new primitives are silent failure surfaces unless we wire them. Required `tracing` integration:

- Span around `subscribe_after` backfill-join with attributes `since_sequence`, `head_at_subscribe`, `replayed_count`, `aggregate_id`. Integrates with `crates/eventsourcing/src/tracing.rs`'s existing `StoredSpan` infra.
- Counters: `subscribe_after.lagged_recovery_total`, `subscribe_after.backfill_rows`, `session_event.payload_truncated_total`, `session_event.blob_spilled_total`, `session.respawn_total`, `session.fallback_to_session_id_total`.
- Histogram: `append_batch.size`, `subscribe_after.head_lag` (head_at_subscribe − since_sequence).

These are slice 1b scope, not deferred. Without them, post-deploy debugging is blind.

## Testing

- **Unit**: `Session::apply` invariants (turn lifecycle, terminal-state lockout); `SessionEvent` (de)serialization round-trips; aggregate snapshot ↔ event replay equivalence.
- **Component**: `ClaudeCodeSession` driven against a fake claude binary (small Rust binary in `crates/connector/tests/fixtures/`) that reads stream-json on stdin and emits scripted responses. Covers happy turn, mid-turn crash, malformed frame, slow writer, slow reader, oversized ToolResult.
- **Concurrency**: `subscribe_after` ordering and dedup under concurrent `append_batch` calls (via the test fixture's deterministic scheduler); `Lagged` recovery determinism under sustained load; throughput target (1 agent × 500 events/turn × 10 turns < 2s p95 with 4 subscribers).
- **Integration**: end-to-end with the real `claude` binary, gated behind a `claude` cargo feature; runs in CI only when the binary is available.
- **TUI**: rendering tests using `ratatui::backend::TestBackend` with a fixed `Rect` per test, asserting on cell-grid contents directly. `insta` is *not* introduced in slice 1; ratatui buffer snapshots are notoriously flaky across terminal-size variance and crossterm versions, and the team has no in-tree workflow for `cargo insta review`. If snapshot tests prove valuable later, scope as a separate slice with explicit terminal-size pinning.
- **Crash recovery**: kill the fake-claude binary mid-turn; assert `TurnAborted` then `SessionCrashed` are appended in order, the registry respawns with `--resume`, the new process sees `SessionStarted { resumed: true }`, and the pane renders without losing pre-crash history. Separate test for the "no on-disk session yet" fallback to `--session-id`.
- **Crash-during-respawn**: kill nephila between `drop(old_handle)` and `insert(new_handle)`. Assert clean recovery on next nephila start (no orphaned claude processes, no double-resume).
- **Supervisor interleaving**: property test for `(CheckpointReached, TurnCompleted, send_turn_failure)` interleavings. Asserts: no `send_turn(Agent, ...)` is called before `TurnCompleted` for the active turn.

## Open assumptions to verify in slice 0 (spike)

A 1-day spike runs **before** slice 1. Outputs are appended to this spec as an appendix. Slice 1 does not start until the spike is green.

1. ✅ **Verified**: `claude --print --verbose --input-format stream-json --output-format stream-json` composes with `--session-id`, `--resume`, `--mcp-config`, `--permission-mode`, `--settings`, and `current_dir`. Confirmed via `claude --help` and `claude-codes/src/cli.rs:646-656` in this draft.
2. ✅ **Verified**: `claude_codes::protocol` and `claude_codes::io` modules are public (https://docs.rs/claude-codes/latest/claude_codes/{io,protocol}/index.html).
3. ✅ **Verified**: `serialize_and_persist` and `request_human_input` MCP tool calls return their results to claude over the HTTP MCP transport, and claude echoes the result back into stdout as `ContentBlock::McpToolResult { tool_use_id, content, is_error }`. The connector reader can derive `CheckpointReached { checkpoint_id, ... }` from the `content` payload. (An earlier draft of this spec speculated this might require side-subscribing to `BusEvent::CheckpointSaved`; verification refuted that.)
4. **Spike**: confirm session id stability across `--resume`. Spawn with `--session-id X`, exchange one turn, send SIGKILL, re-spawn with `--resume X`. Verify: (a) the resumed process operates against the same JSONL transcript, (b) the session id reported by the resumed process equals X. If (b) is false (claude mints a new id on resume), introduce a projection table `agent_session(agent_id, current_session_id)` and key consumers off `agent_id`.
5. **Spike**: `nephila-store`'s `SqliteStore` can host a `tokio::sync::broadcast::Sender` per aggregate. Build the prototype `subscribe_after` against a stub aggregate, exercise listener-first ordering, verify `Lagged` recovery, measure throughput.

If 4 or 5 fails, revise the spec before slice 1.

## Execution Strategy

**Subagents.** Slice 0 (spike) and slice 1a/1b (foundation) are sequential. Slices 2, 3, and 4 touch disjoint subsystems and can run in parallel after 1b lands. Slice 5 (cleanup) waits for all three. Slices 6 and 7 (UX parity rollups — markdown rendering, control-protocol permission UI) gate the deletion of `attach_agent_session`.

We do not use Agent teams. Subagent dispatch with the dependency graph below is sufficient.

## Task Dependency Graph

| Slice | Description | Predecessors | Tag |
|---|---|---|---|
| 0 | Spike: validate CLI flags, `McpToolResult` content shape, session-id stability across `--resume`, `subscribe_after` prototype throughput. ≤1 day. Output: appendix to this spec. | none | HITL |
| 1a | Transport foundation: `ClaudeCodeSession` happy-path stream-json reader/writer with delta coalescing; `SessionPane` skeleton rendering finalized assistant messages from a single agent's pump. End-to-end demo: spawn an agent, watch its messages stream into the pane. | [0] | HITL |
| 1b | Aggregate + store foundation: `Session` aggregate with `apply` reducer; `subscribe_after`, `append_batch`, `prune_aggregate` primitives; sequence-assigned-by-store; broadcast bookkeeping; blob-spillover store; observability spans/counters; throughput benchmark gate. | [0] | HITL |
| 2 | Human prompt injection. Multi-line input box (`tui-textarea`); `HumanPromptQueued`/`HumanPromptDelivered`; vim-flavored input/normal mode; Ctrl+Enter submit. End-to-end: type in pane, claude responds, pane renders both. | [1a, 1b] | AFK |
| 3 | Checkpoint-driven autonomy. Detect `serialize_and_persist`/`request_human_input` from `McpToolResult` blocks; supervisor reacts to `(CheckpointReached, TurnCompleted)` pairs; `Pause` SIGSTOPs the child; replaces today's process-exit-driven respawn. RestartTracker recalibrated. | [1a, 1b] | HITL |
| 4 | Crash + resume. Reader-task `TurnAborted+SessionCrashed` sequencing on EOF; `SessionRegistry` subscribes to crashes and calls `resume(session_id)` with fallback to `--session-id` for never-persisted sessions; cross-process lockfile. Consumers reconnect via `subscribe_after(last_seen)`. | [1a, 1b, 3] | AFK |
| 5 | Cleanup phase 1. Remove the `-p` spawn path in `claude_code.rs`. Hide `attach_agent_session` behind opt-in hotkey `a`. Update hotkeys and docs. **Do not delete attach yet.** Mark `BusEvent::CheckpointSaved` deprecated. | [2, 3, 4] | AFK |
| 6 | UX parity rollup phase 1: markdown rendering (`tui-markdown`), code syntax highlight, slash-command routing for `/clear`/`/compact`/`/cost`. | [5] | AFK |
| 7 | UX parity rollup phase 2: interactive permission prompts via control-protocol UI; complete parity matrix; delete `attach_agent_session` and `BusEvent::CheckpointSaved`. | [6] | HITL |

Slice 0 is HITL because the spike's outcomes change downstream slice scope. Slices 1a/1b are HITL because the aggregate shape, reducer invariants, and store primitives are architectural calls worth a human checkpoint. Slice 3 is HITL because the `(CheckpointReached, TurnCompleted)` pairing convention and `Pause` SIGSTOP semantics are surprising-without-context. Slice 7 is HITL because deleting the TTY handoff is irreversible.

## Agent Assignments

```
Slice 0 (spike):                              → rust-engineer  (1 day)
Slice 1a: Transport foundation                → rust-engineer
Slice 1b: Aggregate + store foundation        → rust-engineer
Slice 2:  Human prompt injection              → rust-engineer
Slice 3:  Checkpoint-driven autonomy          → rust-engineer
Slice 4:  Crash + resume                      → rust-engineer
Slice 5:  Cleanup phase 1                     → rust-engineer
Slice 6:  UX parity rollup phase 1            → rust-engineer
Slice 7:  UX parity rollup phase 2            → rust-engineer
Polish:                                       → rust-engineer
```

The diff is uniformly Rust across `crates/eventsourcing`, `crates/store`, `crates/connector`, `crates/lifecycle`, `crates/tui`, and `bin/`. No per-task specialist swap is warranted.
