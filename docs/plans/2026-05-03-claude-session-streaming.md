# Claude Session Streaming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or `/team-feature` to implement this plan task-by-task, per the Execution Strategy below. Steps use checkbox (`- [ ]`) syntax — these are **persistent durable state**, not visual decoration. The executor edits the plan file in place: `- [ ]` → `- [x]` the instant a step verifies, before moving on. On resume (new session, crash, takeover), the executor scans existing `- [x]` marks and skips them — these steps are NOT redone. TodoWrite mirrors this state in-session; the plan file is the source of truth across sessions.

## Session resume notes (2026-05-04)

**Read this section first if you are resuming execution. It supersedes the routing flow in `superpowers:executing-plans` for this plan.** Pre-flight decisions already made by the human partner; do not re-derive them.

- **Route:** `superpowers:subagent-driven-development`. Skip the brainstorming, writing-plans, and route-decision skills — they are already done.
- **Task 1 (Slice 0 Spike) — DEFERRED.** Operator runs the spike out-of-band and will append the appendix to the spec when convenient. Slices 1a/1b proceed on the spec's assumptions as written. If assumption 4 ultimately fails, slice 4 adds the `agent_session` projection table per its already-documented fallback. **Do NOT dispatch a Task 1 implementer.** Treat Task 1 as not-applicable; do NOT pre-flip its checkboxes (`- [x]` for unrun work would mislead future resumes).
- **Task 3 step 4 (`EventEnvelope::sequence` semantics) — RESOLVED:** Option 2 ("keep `u64`, document as 'stamped by store'"). Record the decision in `docs/adr/0002-eventenvelope-sequence-stamping.md` as the first sub-step of Task 3 step 4. The debug-assert + writer-overwrite contract test in step 4 still applies.
- **Scope cap — stop after Task 6.** Tasks 7, 8, 9 each carry HITL gates that cannot complete in one session: Task 7 wants two weeks of prod soak + rollback procedure signoff, Task 9 wants an operator permission-mode audit and a real-`claude` protocol experiment. After Task 6 review passes, surface "Tasks 7–9 require operator gates; stopping here" to the user and stop. Do NOT run the Polish phase yet — Polish belongs after Task 9.
- **First batch to dispatch:** **batch 2** (Tasks 2 + 3 in parallel) per the dependency graph. Tasks 2 and 3 own disjoint files except for the serialized integration window at end-of-batch (`crates/connector/src/session.rs` rewrite, `EventEnvelope::sequence` change, `session_smoke.rs` rewrite, workspace `Cargo.toml` edits) — Task 3's serialized steps run AFTER Task 2 commits to the integration branch. Re-read "File ownership (batch 2)" and "Sequenced integration" below before dispatching.
- **Subsequent batches:** batch 3 = Tasks 4 + 5 in parallel; batch 4 = Task 6 alone; then STOP.
- **Branch & working tree:** branch `migration-of-wrapper`. As of 2026-05-04 the plan + spec + ADR-0001 were staged but uncommitted; if they are still uncommitted on resume, commit them as a separate `docs:` commit before dispatching any implementer so the working tree is clean.
- **Toolchain assumed available:** Rust 1.94+ (cargo + rustc), `claude` CLI v2.1+ (only used by Task 1, which is deferred — implementers do not need it).
- **MANDATORY: Use `isolation: "worktree"` for every parallel implementer in batch 2 and batch 3.** A prior dispatch attempt (2026-05-04) ran Task 2 + Task 3 implementers concurrently in the same working tree. Each `git checkout slice-1a` / `git checkout slice-1b` from one agent yanked the working tree out from under the other, causing Task 3 to repeatedly stash its progress (7 partial stashes, all <200 lines) until its 600s watchdog killed it; Task 2 stalled at step 7 of 14 with no commit. The only safe pattern for parallel implementers in this repo is one git worktree per agent. Pass `isolation: "worktree"` on every `Agent` tool call dispatching a parallel implementer. Sequential dispatches (e.g. batch 4's lone Task 6) may use the main working tree. The recovery commit on 2026-05-04 reset slice-1a and slice-1b branches, dropped 7 stashes, removed the slice-1a uncommitted scaffolding, and unflipped Task 2 steps 1–7 (which had been marked done without a commit).

**Goal:** Replace nephila's per-turn `claude -p` spawn model with one long-lived stream-json claude process per agent, persist its activity through a new `Session` event-sourced aggregate, render it live in an embedded TUI pane, and let the supervisor drive autonomy off the resulting event stream.

**Architecture:** A rewritten `ClaudeCodeSession` in `crates/connector` owns one `claude --print --verbose --input-format stream-json --output-format stream-json` process per agent and is the sole producer of `SessionEvent`s for that aggregate. A new `Session` aggregate in `crates/core` (second `EventSourced` impl after `Agent`) reduces those events into a state machine. `crates/store` gains `subscribe_after`, `append_batch`, and `prune_aggregate` primitives plus a blob-spillover store for oversized payloads. Consumers — a new `SessionPane` in `crates/tui` and a rewritten `SessionSupervisor` in `crates/lifecycle` — read live + replay through `subscribe_after`. The legacy TTY-handoff `attach_agent_session` stays opt-in until the embedded pane achieves UX parity in slices 6 and 7.

**Tech Stack:** Rust (Tokio async, rusqlite SQLite + WAL, `tokio::sync::broadcast`, `tokio::process`); `claude-codes` crate for `ClaudeInput`/`ClaudeOutput`/`ContentBlock`/`McpToolResultBlock`/`JsonLines` framing only (we keep our own `tokio::process::Command` spawn); `nephila-eventsourcing` (`EventSourced` trait, `EventEnvelope`, `Snapshot`); `ratatui` + `tui-textarea` for the embedded pane; `tracing` for observability.

## Execution Strategy

**Subagents.** Slice 0 (spike) and slice 1a/1b (foundation) are sequential. Slices 2, 3, and 4 touch disjoint subsystems and can run in parallel after 1b lands. Slice 5 (cleanup) waits for all three. Slices 6 and 7 (UX parity rollups — markdown rendering, control-protocol permission UI) gate the deletion of `attach_agent_session`. Subagent dispatch with the dependency graph below is sufficient; we do not use Agent teams.

## Task Dependency Graph

- Task 1 [HITL] — Slice 0: Spike: depends on `none` → batch 1 *(DEFERRED to operator — see Session resume notes; do NOT dispatch)*
- Task 2 [HITL] — Slice 1a: Transport foundation: depends on `Task 1` → batch 2
- Task 3 [HITL] — Slice 1b: Aggregate + store foundation: depends on `Task 1` → batch 2 (parallel with Task 2 for most of its body; the `crates/connector/src/session.rs` rewrite at step 12, the `EventEnvelope::sequence` change, and the `session_smoke.rs` rewrite are a serialized integration window that runs **after** Task 2 commits — see *Sequenced integration* in *File ownership* below. If Task 1 spike Assumption 4 returns FAIL, Task 3 also adds `crates/store/src/schema.rs` projection-table migration; see Task 3 §Files for the conditional entry.)
- Task 4 [AFK] — Slice 2: Human prompt injection: depends on `Task 2, Task 3` → batch 3
- Task 5 [HITL] — Slice 3: Checkpoint-driven autonomy: depends on `Task 2, Task 3` → batch 3 (parallel with Task 4)
- Task 6 [AFK] — Slice 4: Crash + resume: depends on `Task 2, Task 3, Task 4, Task 5` → batch 4
- Task 7 [HITL] — Slice 5: Cleanup phase 1: depends on `Task 4, Task 5, Task 6` → batch 5
- Task 8 [AFK] — Slice 6: UX parity rollup phase 1: depends on `Task 7` → batch 6
- Task 9 [HITL] — Slice 7: UX parity rollup phase 2 (deletes attach): depends on `Task 8` → batch 7
- Polish [HITL] — post-implementation-polish: depends on `Task 9` → final batch

**File ownership (batch 2):** Task 2 owns `crates/connector/src/session.rs`, `crates/connector/src/stream.rs`, `crates/connector/tests/fixtures/fake_claude/`, `crates/tui/src/panels/session_pane.rs`, `crates/tui/src/panels.rs`, `crates/tui/tests/session_pane_render.rs`, and `bin/src/bin/demo_session.rs`. (The full rewrite/deletion of `crates/connector/src/claude_code.rs` lands later: slice 5 / Task 7 and slice 7 / Task 9 — not Task 2.) Task 3 owns `crates/core/src/session.rs`, `crates/core/src/session_event.rs`, `crates/eventsourcing/src/store.rs` (additions only — no signature breaks beyond `EventEnvelope::sequence`), `crates/store/src/domain_event.rs` (additions), `crates/store/src/blob.rs` (new), `crates/store/src/writer.rs` (sequence-stamping change). The `EventEnvelope::sequence` change is the one shared touch point: Task 3 owns the change; Task 2 consumes it via `nephila-eventsourcing`.

**Sequenced integration (end of batch 2).** Batch 2 is "parallel for Task 2 + most of Task 3, then a serialized integration window at the end." The serialized steps, in order, are: (a) the `EventEnvelope::sequence` change in `crates/eventsourcing`; (b) Task 3's rewrite of `crates/connector/src/session.rs` to swap connector drafts for the store (Task 3 step 12 — touches a Task-2-owned file, so it MUST run after Task 2 commits and merges to `main`); (c) the `crates/connector/tests/session_smoke.rs` rewrite. Workspace `Cargo.toml` edits are also shared touchpoints: Task 2 adds `claude-codes` to the root workspace and the connector crate; Task 3 may add `blake3` to workspace deps and `criterion` to `crates/store`. These edits are coordinated by sequencing the Task 3 `Cargo.toml` edits **after** Task 2's `Cargo.toml` edits land on `main`.

## Agent Assignments

- Task 1: Slice 0 — Spike → rust-engineer
- Task 2: Slice 1a — Transport foundation → rust-engineer
- Task 3: Slice 1b — Aggregate + store foundation → rust-engineer
- Task 4: Slice 2 — Human prompt injection → rust-engineer
- Task 5: Slice 3 — Checkpoint-driven autonomy → rust-engineer
- Task 6: Slice 4 — Crash + resume → rust-engineer
- Task 7: Slice 5 — Cleanup phase 1 → rust-engineer
- Task 8: Slice 6 — UX parity rollup phase 1 → rust-engineer
- Task 9: Slice 7 — UX parity rollup phase 2 → rust-engineer
- Polish: post-implementation-polish → rust-engineer

The diff is uniformly Rust across `crates/eventsourcing`, `crates/store`, `crates/connector`, `crates/lifecycle`, `crates/tui`, and `bin/`. No per-task specialist swap is warranted.

## Conventions for every task

- **TDD discipline.** Each step pair is: write the failing test → run it → write minimal code → run the same test until green. Don't write implementation before its test.
- **Commit cadence.** One commit per task (the `Commit:` line at the end of each task). Within a task, do not commit until all steps verify; flip checkboxes to `[x]` as you go but stage all changes for a single end-of-task commit. Message format: `<slice>: <one-line action>`. Example: `slice-1b: add subscribe_after backfill+listener join`. Push only at task end. Tasks running in parallel batches each work on their own branch (`slice-1a`, `slice-1b`, etc.), merged into `main` at end-of-batch integration.
- **Run formatter and lints after every code step.** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`. If clippy complains, fix before flipping the checkbox.
- **`fmt`/`clippy` cleanliness counts as test failure.** Do not flip a checkbox green if either is dirty.
- **No new public deps without justification.** The plan calls out `tui-textarea` (slice 2), `tui-markdown` and `syntect`-or-equivalent (slice 6) explicitly. Anything else: stop and ask.
- **Tracing.** Every new public async fn that crosses a subsystem boundary gets a `#[tracing::instrument]` with `level = "debug"` and `skip(self)`. Match existing style in `crates/eventsourcing/src/tracing.rs`.
- **Permission mode.** Slices 1-6 ship with `--permission-mode bypassPermissions` per spec §Decision; this is an explicit deferral. Before slice 7 merges, a HITL gate verifies the operator's permission audit (see Task 9 step 1).
- **Spike artifacts.** `crates/connector/examples/spike_stream_json.rs` and `crates/store/examples/spike_subscribe_after.rs` are throwaway. Task 7 step (cleanup) deletes both.

---

## Task 1: Slice 0 — Spike

**Purpose:** Validate the three load-bearing assumptions in the spec before slice 1 starts. Spike output is appended as an appendix to `docs/specs/2026-05-03-claude-session-streaming-design.md`. Time-box: 1 day. If assumption 4 or 5 fails, revise the spec before slice 1.

**Files:**
- Create: `crates/connector/examples/spike_stream_json.rs` — driver script for assumption 4
- Create: `crates/store/examples/spike_subscribe_after.rs` — driver script for assumption 5
- Modify: `docs/specs/2026-05-03-claude-session-streaming-design.md` — append `## Appendix: Slice 0 spike outcomes` section

- [ ] **Step 1: Verify spec assumption 1 (CLI flag composition) with `claude --help`.**

```
claude --help | rg -- '--input-format|--output-format|--session-id|--resume|--mcp-config|--permission-mode|--settings|--include-partial-messages'
```

Expected: every flag listed in `Spec §ClaudeCodeSession` is present and `--input-format stream-json` is documented as requiring `--print --verbose`. Capture output verbatim into the appendix.

- [ ] **Step 2: Verify spec assumption 2 (`claude_codes` modules public) by `cargo doc`.**

```
cargo add claude-codes --offline 2>&1 || true   # confirm or note version
cargo doc -p claude-codes --no-deps --open=false
ls target/doc/claude_codes/ | rg '^io|^protocol'
```

Expected: `target/doc/claude_codes/io/index.html` and `target/doc/claude_codes/protocol/index.html` exist. Capture URLs into appendix; if missing, vendor the relevant types into `crates/connector/src/protocol.rs` instead.

- [ ] **Step 3: Write spike driver `crates/connector/examples/spike_stream_json.rs` for assumption 4.**

```rust
// crates/connector/examples/spike_stream_json.rs
//! Spike: confirm session-id stability across --resume.
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    if std::env::var("CLAUDE_BIN").is_err() {
        eprintln!("CLAUDE_BIN not set; skipping spike");
        return Ok(());
    }
    let session_id = Uuid::new_v4().to_string();
    let workdir = tempfile::tempdir()?;

    // Phase 1: fresh start, exchange one turn, capture JSONL transcript path
    let mut child = Command::new(std::env::var("CLAUDE_BIN").unwrap())
        .args([
            "--print", "--verbose",
            "--input-format", "stream-json",
            "--output-format", "stream-json",
            "--include-partial-messages",
            "--session-id", &session_id,
            // SPIKE-ONLY: bypass permissions to focus on protocol shape; production path uses operator-supplied mode.
            "--permission-mode", "bypassPermissions",
        ])
        .current_dir(workdir.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().unwrap();
    // Regular byte string so `\n` is the actual 0x0A line terminator.
    // A raw byte string (`br#"...\n"#`) would emit two bytes (`\` + `n`) and claude's
    // line reader would block waiting for a real newline.
    stdin.write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"reply with the literal string OK\"}}\n").await?;
    stdin.flush().await?;
    drop(stdin);

    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut frames = Vec::new();
    while let Some(line) = lines.next_line().await? {
        frames.push(line);
        if frames.last().is_some_and(|l| l.contains(r#""type":"result""#)) { break; }
    }
    child.wait().await?;

    // Phase 2: SIGKILL didn't apply (already exited); now --resume with same id
    let mut resumed = Command::new(std::env::var("CLAUDE_BIN").unwrap())
        .args(["--print", "--verbose",
               "--input-format", "stream-json",
               "--output-format", "stream-json",
               "--resume", &session_id,
               // SPIKE-ONLY: bypass permissions to focus on protocol shape; production path uses operator-supplied mode.
               "--permission-mode", "bypassPermissions"])
        .current_dir(workdir.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = resumed.stdin.take().unwrap();
    stdin.write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"what session id are you?\"}}\n").await?;
    stdin.flush().await?;
    drop(stdin);

    let mut resume_frames = Vec::new();
    let mut lines = BufReader::new(resumed.stdout.take().unwrap()).lines();
    while let Some(line) = lines.next_line().await? {
        resume_frames.push(line);
        if resume_frames.last().is_some_and(|l| l.contains(r#""type":"result""#)) { break; }
    }
    resumed.wait().await?;

    println!("=== PHASE 1 FRAMES ===\n{}", frames.join("\n"));
    println!("=== PHASE 2 (RESUMED) FRAMES ===\n{}", resume_frames.join("\n"));
    println!("=== ASSUMPTION 4 RESULT ===");
    println!("session_id sent on resume: {session_id}");
    println!("Inspect frames above for `session_id` in the system/init frame; if it differs, projection table is required.");
    Ok(())
}
```

Run: `CLAUDE_BIN=$(which claude) cargo run -p nephila-connector --example spike_stream_json`
Expected: prints two frame batches and a "ASSUMPTION 4 RESULT" line. Eyeball the system/init frame from phase 2 — its `session_id` field must equal the one we passed.

These are local-dev examples; CI does not run them — spike validation is a HITL gate before slice 1 begins.

- [ ] **Step 4: Document assumption 4 result in spec appendix.**

If session id stable across resume → record "Assumption 4: PASS — same session_id on `--resume`." If not → record FAIL with the observed id, and add to appendix: "Slice 1b will introduce `agent_session(agent_id, current_session_id)` projection table (schema in `crates/store/src/schema.rs`); consumers key off `agent_id`."

- [ ] **Step 4b: Capture exact `claude --resume` failure phrase for missing sessions.**

Run `CLAUDE_BIN=$(which claude) cargo run -p nephila-connector --example spike_stream_json` after deleting `~/.claude/projects/<workdir>/conversations/`, capture stderr verbatim, and append the literal phrase to the spec appendix as `RESUME_NOT_FOUND_STDERR`. Slice 4's fallback regex is generated from this constant, not hand-written. If claude updates change the phrase, slice 4's tests break loudly rather than silently regressing.

- [ ] **Step 5: Write spike driver `crates/store/examples/spike_subscribe_after.rs` for assumption 5.**

```rust
// crates/store/examples/spike_subscribe_after.rs
//! Spike: prototype subscribe_after on top of the current writer thread.
use std::sync::Arc;
use std::time::Instant;
use dashmap::DashMap;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // Use SqliteStore::open_in_memory and a side DashMap<(String,String), broadcast::Sender>.
    // Exercise: 4 concurrent subscribers, 1 producer emitting 5000 events in batches of 50.
    // Measure: (a) listener-first vs head-first ordering: deliberately introduce a 10ms gap and
    //         confirm broadcast event with seq > head_at_subscribe arrives via the listener path,
    //         not the backfill path. (b) Lagged recovery: shrink broadcast bound to 32, force
    //         RecvError::Lagged, recover via load_events from last_seen.
    // Print elapsed time and counts. Target: 5000 events / 4 subscribers complete < 2s p95.
    // (a) Open in-memory store + (b) shared listener map keyed by (agent_id, session_id).
    let _store = Arc::new(nephila_store::SqliteStore::open_in_memory().await?);
    let listeners: Arc<DashMap<(String, String), broadcast::Sender<DomainEvent>>> =
        Arc::new(DashMap::new());
    let key = ("agent-spike".to_string(), "session-spike".to_string());
    let (tx, _) = broadcast::channel::<DomainEvent>(1024);
    listeners.insert(key.clone(), tx.clone());
    // (c) Spawn 4 subscriber tasks, each calling subscribe_after; record per-event recv instant.
    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let mut rx = tx.subscribe();
        handles.push(tokio::spawn(async move {
            let mut samples: Vec<(u64, Instant)> = Vec::with_capacity(5000);
            while let Ok(ev) = rx.recv().await {
                samples.push((ev.seq, Instant::now()));
                if samples.len() == 5000 { break; }
            }
            samples
        }));
    }
    // (d) Producer: 5000 events in batches of 50; record send instant per event.
    let mut send_at: Vec<Instant> = Vec::with_capacity(5000);
    for batch in 0..(5000 / 50) {
        for i in 0..50 {
            let seq = (batch * 50 + i) as u64;
            send_at.push(Instant::now());
            let _ = tx.send(DomainEvent::synth(seq));
        }
        tokio::task::yield_now().await;
    }
    // (e) Collect per-subscriber timestamps; (f) compute p50/p95/p99 over end-to-end samples.
    let mut latencies_ms: Vec<f64> = Vec::with_capacity(20_000);
    for h in handles {
        for (seq, recv_at) in h.await? {
            latencies_ms.push((recv_at - send_at[seq as usize]).as_secs_f64() * 1000.0);
        }
    }
    let elapsed = start.elapsed();
    assert!(latencies_ms.len() >= 100, "need ≥100 samples; got {}", latencies_ms.len());
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| latencies_ms[((latencies_ms.len() as f64 - 1.0) * p) as usize];
    let throughput = latencies_ms.len() as f64 / elapsed.as_secs_f64();
    // (g) Emit JSON. Target: per-subscriber end-to-end latency p95 < 50ms across 5000 events × 4
    //     subscribers (= 20000 samples). Total wall-clock <2s.
    println!(
        r#"{{ "p50_ms": {:.3}, "p95_ms": {:.3}, "p99_ms": {:.3}, "throughput_events_per_sec": {:.1} }}"#,
        pct(0.50), pct(0.95), pct(0.99), throughput
    );
    Ok(())
}
```

Implement the prototype — copy patterns from `crates/store/src/domain_event.rs` and `crates/store/src/writer.rs`. Run: `cargo run --release -p nephila-store --example spike_subscribe_after`. Target: per-subscriber end-to-end latency p95 < 50ms across 5000 events × 4 subscribers (= 20000 samples). Total wall-clock <2s. Expected output: a one-line JSON object with `p50_ms`/`p95_ms`/`p99_ms`/`throughput_events_per_sec`, and no missed events under either ordering or burst test.

- [ ] **Step 6: Document assumption 5 result and writer-thread baseline.**

Capture (a) the per-subscriber p50/p95/p99 latency numbers and total wall-clock, (b) whether listener-first / head-second ordering is correct under the prototype, (c) whether `Lagged` recovery converges. Spike outcome (raw numbers + p95 latency) is recorded in the spec appendix as a HITL artifact. The slice-1b CI bench gate (defined in Task 3) is the authoritative numeric check; spike numbers calibrate expectations only. Otherwise note the bottleneck (writer-thread fsync? broadcast bound?) and revise the spec.

- [ ] **Step 7: Verify spec assumption 3 (`McpToolResult` carries `checkpoint_id`).**

Drive the spike script through one round-trip where claude calls the `serialize_and_persist` MCP tool against a stub MCP server. Capture the `ContentBlock::McpToolResult` frame from claude's stdout. Confirm the JSON `content` field contains the tool's full payload (with `checkpoint_id`). Append the captured frame to the appendix.

- [ ] **Step 8: Write spec appendix and commit.**

Append `## Appendix: Slice 0 spike outcomes (YYYY-MM-DD)` to `docs/specs/2026-05-03-claude-session-streaming-design.md` listing each assumption, the verification artifact (frame snippet, throughput number, etc.), and PASS/FAIL. If any FAIL, raise the question to a HITL review checkpoint before slice 1.

Commit: `slice-0: spike — validate CLI flags, claude_codes pubmods, --resume id stability, subscribe_after throughput, McpToolResult shape`.

---

## Task 2: Slice 1a — Transport foundation

**Purpose:** Get a `ClaudeCodeSession` happy-path stream-json reader/writer working end-to-end against a fake-claude binary, with delta coalescing, plus a minimal `SessionPane` skeleton that renders finalized assistant messages from a single agent's pump task. **No store integration** — this slice writes to an in-memory `mpsc::Receiver<SessionEvent>`. Slice 1b plugs it into the durable store.

**Files:**
- Create: `crates/connector/src/session.rs` — public `ClaudeCodeSession` type
- Create: `crates/connector/src/stream.rs` — reader-task framing + coalescer
- Create: `crates/connector/src/event_draft.rs` — provisional `SessionEventDraft` (deleted-and-replaced by `nephila_core::SessionEvent` in slice 1b — not a pub-use rollover)
- Create: `crates/connector/tests/fixtures/fake_claude/Cargo.toml` and `src/main.rs` — fake-claude test binary
- Create: `crates/connector/tests/session_smoke.rs`
- Create: `crates/tui/src/panels/session_pane.rs` — minimal pane (read-only, no input)
- Modify: `crates/connector/src/lib.rs` — add `pub mod session; pub mod stream; pub mod event_draft;`
- Modify: `crates/tui/src/panels.rs` — add `pub mod session_pane;`
- Modify: `Cargo.toml` (root) and `crates/connector/Cargo.toml` — add `claude-codes` (`io`, `protocol` features only) and a `dev-dependencies` entry for the fake-claude path

- [x] **Step 1: Add `claude-codes` dependency to `crates/connector/Cargo.toml`.**

```toml
[dependencies]
claude-codes = { version = "<pinned-from-spike>", default-features = false, features = ["io", "protocol"] }
```

Run: `cargo build -p nephila-connector`. Expected: builds; `claude_codes::io` and `claude_codes::protocol` resolve.

- [x] **Step 2: Write the failing test — fake-claude smoke.**

`crates/connector/tests/session_smoke.rs`:

```rust
use nephila_connector::session::{ClaudeCodeSession, SessionConfig, PromptSource};
use nephila_connector::event_draft::SessionEventDraft;
use std::path::PathBuf;
use uuid::Uuid;

#[tokio::test]
async fn happy_turn_emits_assistant_then_result() {
    let fake = std::env::var("FAKE_CLAUDE_BIN")
        .unwrap_or_else(|_| target_dir().join("fake_claude").display().to_string());
    let session_id = Uuid::new_v4();
    let cfg = SessionConfig {
        claude_binary: fake.into(),
        session_id,
        agent_id: nephila_core::id::AgentId::new(),
        working_dir: tempfile::tempdir().unwrap().keep(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    };
    let mut session = ClaudeCodeSession::start(cfg).await.unwrap();
    let mut events = session.subscribe_drafts();

    let turn_id = session.send_turn(PromptSource::Human, "echo OK".into()).await.unwrap();

    let mut seen = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await {
        seen.push(ev);
        if matches!(seen.last(), Some(SessionEventDraft::TurnCompleted { .. })) { break; }
    }
    session.shutdown().await.unwrap();

    let kinds: Vec<&'static str> = seen.iter().map(SessionEventDraft::kind).collect();
    assert!(kinds.contains(&"HumanPromptQueued"), "kinds = {kinds:?}");
    assert!(kinds.contains(&"HumanPromptDelivered"));
    assert!(kinds.contains(&"AssistantMessage"));
    assert!(kinds.contains(&"TurnCompleted"));
    assert_eq!(seen.iter().filter(|e| matches!(e, SessionEventDraft::TurnCompleted{..})).count(), 1);
    let _ = turn_id;
}

fn target_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).parent().unwrap().to_owned()
}
```

- [x] **Step 3: Run test to verify it fails.**

Run: `cargo test -p nephila-connector --test session_smoke -- --nocapture`
Expected: compile error initially. Then add a stub `pub struct ClaudeCodeSession; impl ClaudeCodeSession { pub async fn start(...) -> ... { todo!() } pub fn subscribe_drafts(...) -> ... { todo!() } pub async fn send_turn(...) -> ... { todo!() } pub async fn shutdown(...) -> ... { todo!() } }` so the test compiles and panics on `todo!()`. That panic is the behavioral RED. Only after seeing the panic (not the compile error) proceed to Step 4.

- [x] **Step 4: Implement `event_draft::SessionEventDraft` (provisional).**

`crates/connector/src/event_draft.rs` — enum mirroring the spec's `SessionEvent` variants verbatim, but holding `serde_json::Value` payloads. Adds a `kind(&self) -> &'static str` for the test's introspection. Replaced wholesale in 1b by `nephila_core::SessionEvent` — note the rollover is **not** a `pub use` re-export (payload types differ: draft uses `serde_json::Value`, real uses `ToolResultPayload`). Slice 1b's Task 3 step 12 deletes this file and rewrites every callsite in one pass; slice 1a callers must therefore not depend on exact draft variant shapes beyond what step 12 will provide.

```rust
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub type TurnId = Uuid;
pub type CheckpointId = String;
pub type MessageId = String;

#[derive(Debug, Clone)]
pub enum SessionEventDraft {
    SessionStarted { resumed: bool, ts: DateTime<Utc> },
    HumanPromptQueued { turn_id: TurnId, text: String, ts: DateTime<Utc> },
    HumanPromptDelivered { turn_id: TurnId, ts: DateTime<Utc> },
    AgentPromptQueued { turn_id: TurnId, text: String, ts: DateTime<Utc> },
    AgentPromptDelivered { turn_id: TurnId, ts: DateTime<Utc> },
    PromptDeliveryFailed { turn_id: TurnId, reason: String, ts: DateTime<Utc> },
    AssistantMessage { message_id: MessageId, seq_in_message: u32, delta_text: String, is_final: bool, ts: DateTime<Utc> },
    ToolCall { tool_use_id: String, tool_name: String, input: serde_json::Value, ts: DateTime<Utc> },
    ToolResult { tool_use_id: String, output: serde_json::Value, is_error: bool, ts: DateTime<Utc> },
    CheckpointReached { checkpoint_id: CheckpointId, interrupt: Option<serde_json::Value>, ts: DateTime<Utc> },
    TurnCompleted { turn_id: TurnId, stop_reason: String, ts: DateTime<Utc> },
    TurnAborted { turn_id: TurnId, reason: String, ts: DateTime<Utc> },
    SessionCrashed { reason: String, exit_code: Option<i32>, ts: DateTime<Utc> },
    SessionEnded { ts: DateTime<Utc> },
}

impl SessionEventDraft {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "SessionStarted",
            Self::HumanPromptQueued { .. } => "HumanPromptQueued",
            Self::HumanPromptDelivered { .. } => "HumanPromptDelivered",
            Self::AgentPromptQueued { .. } => "AgentPromptQueued",
            Self::AgentPromptDelivered { .. } => "AgentPromptDelivered",
            Self::PromptDeliveryFailed { .. } => "PromptDeliveryFailed",
            Self::AssistantMessage { .. } => "AssistantMessage",
            Self::ToolCall { .. } => "ToolCall",
            Self::ToolResult { .. } => "ToolResult",
            Self::CheckpointReached { .. } => "CheckpointReached",
            Self::TurnCompleted { .. } => "TurnCompleted",
            Self::TurnAborted { .. } => "TurnAborted",
            Self::SessionCrashed { .. } => "SessionCrashed",
            Self::SessionEnded { .. } => "SessionEnded",
        }
    }
}
```

- [x] **Step 5: Implement `stream::Coalescer` for assistant deltas.**

`crates/connector/src/stream.rs`:

```rust
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::event_draft::SessionEventDraft;
use chrono::Utc;

const MAX_DELTAS: usize = 5;
const MAX_BUFFERED_BYTES: usize = 200 * 1024; // 200 KiB; leaves slack under spec's 256 KiB cap
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct Coalescer {
    buffers: HashMap<String, MessageBuffer>,
}

struct MessageBuffer {
    seq_in_message: u32,
    pending_text: String,
    pending_count: usize,
    last_flush: Instant,
}

impl Coalescer {
    pub(crate) fn push_delta(&mut self, message_id: &str, text: &str) -> Option<SessionEventDraft> {
        let buf = self.buffers.entry(message_id.to_owned()).or_insert_with(|| MessageBuffer {
            seq_in_message: 0,
            pending_text: String::new(),
            pending_count: 0,
            last_flush: Instant::now(),
        });
        buf.pending_text.push_str(text);
        buf.pending_count += 1;
        if buf.pending_count >= MAX_DELTAS
            || buf.pending_text.len() >= MAX_BUFFERED_BYTES
            || buf.last_flush.elapsed() >= FLUSH_INTERVAL
        {
            return Some(emit(buf, message_id, false));
        }
        None
    }
    pub(crate) fn finalize(&mut self, message_id: &str) -> Option<SessionEventDraft> {
        self.buffers.remove(message_id).map(|mut buf| emit(&mut buf, message_id, true))
    }
    pub(crate) fn tick(&mut self, now: Instant) -> Vec<SessionEventDraft> {
        let mut out = Vec::new();
        for (id, buf) in self.buffers.iter_mut() {
            if !buf.pending_text.is_empty() && now.duration_since(buf.last_flush) >= FLUSH_INTERVAL {
                out.push(emit(buf, id, false));
            }
        }
        out
    }
}

fn emit(buf: &mut MessageBuffer, message_id: &str, is_final: bool) -> SessionEventDraft {
    let seq = buf.seq_in_message;
    buf.seq_in_message += 1;
    buf.pending_count = 0;
    buf.last_flush = Instant::now();
    let text = std::mem::take(&mut buf.pending_text);
    SessionEventDraft::AssistantMessage {
        message_id: message_id.to_owned(),
        seq_in_message: seq,
        delta_text: text,
        is_final,
        ts: Utc::now(),
    }
}
```

- [x] **Step 6: Write Coalescer unit tests.**

In the same file, `#[cfg(test)] mod tests`:
- 4 deltas → no emission; 5th delta → emit non-final.
- Two messages interleaved → independent flushes by `message_id`.
- `finalize` empties buffer and emits with `is_final: true`.
- `tick` after 250ms with one pending delta → one emission.

Run: `cargo test -p nephila-connector stream::tests -- --nocapture`. Expected: PASS.

- [x] **Step 7: Implement `ClaudeCodeSession::start` and `send_turn` and reader/writer tasks.**

`crates/connector/src/session.rs` — sole producer. Two tasks plus the `Coalescer`. Sketch:

```rust
use crate::event_draft::SessionEventDraft;
use crate::stream::Coalescer;
use chrono::Utc;
use claude_codes::io::JsonLines;
use claude_codes::protocol::{ClaudeInput, ClaudeOutput, ContentBlock};
use nephila_core::id::AgentId;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct SessionConfig {
    pub claude_binary: PathBuf,
    pub session_id: Uuid,
    pub agent_id: AgentId,
    pub working_dir: PathBuf,
    pub mcp_endpoint: String,
    pub permission_mode: String,
}

#[derive(Copy, Clone, Debug)]
pub enum PromptSource { Agent, Human }

pub type TurnId = Uuid;

// Channel bounds (slice 1a):
//   turns_tx:    mpsc bound 16 (small — operators cannot enqueue beyond this; backpressure on send_turn)
//   drafts_tx:   broadcast bound 4096 (per spec §subscribe_after; lagged consumers handled in slice 1b)
//   stderr ring: VecDeque cap 256 lines (drop oldest)
//   stdin write: tokio::process::ChildStdin is buffered by the OS pipe; no app-level bound
pub struct ClaudeCodeSession {
    session_id: Uuid,
    agent_id: AgentId,
    turns_tx: mpsc::Sender<Turn>,
    drafts_tx: broadcast::Sender<SessionEventDraft>,
    cancel: CancellationToken,
    child: tokio::sync::Mutex<Option<Child>>,
}

struct Turn { source: PromptSource, text: String, turn_id: TurnId }

impl ClaudeCodeSession {
    pub async fn start(cfg: SessionConfig) -> Result<Self, crate::error::ConnectorError> {
        // 1. spawn claude with the spec's flag set, stdin+stdout+stderr piped, no kill_on_drop
        // 2. spawn reader task (FramedRead<ChildStdout, JsonLines<ClaudeOutput>>)
        // 3. spawn writer task (recv from turns_rx, write ClaudeInput::User to stdin)
        // 4. spawn stderr_drain task: read child.stderr line-by-line into a bounded
        //    VecDeque<String> (cap 256 lines, drop oldest); shared with reader as
        //    Arc<Mutex<VecDeque<String>>>. On SessionCrashed emission the reader calls
        //    stderr_drain.snapshot() and inlines the last ~32 lines into SessionCrashed.reason.
        //    Without this task the stderr pipe buffer fills and claude blocks.
        // 5. seed broadcast::channel(4096) for drafts; drafts_tx is shared by all tasks
        // 6. emit SessionStarted { resumed: false, ts: Utc::now() }
        todo!()
    }

    pub fn subscribe_drafts(&self) -> broadcast::Receiver<SessionEventDraft> {
        self.drafts_tx.subscribe()
    }

    pub async fn send_turn(&self, source: PromptSource, text: String) -> Result<TurnId, crate::error::ConnectorError> {
        let turn_id = Uuid::new_v4();
        self.turns_tx.send(Turn { source, text, turn_id }).await
            .map_err(|_| crate::error::ConnectorError::Process { exit_code: None, stderr: "writer task closed".into() })?;
        Ok(turn_id)
    }

    pub async fn shutdown(self) -> Result<(), crate::error::ConnectorError> {
        // SIGTERM, await tasks via cancel, append SessionEnded, await writer drain
        todo!()
    }

    pub fn id(&self) -> Uuid { self.session_id }
    pub fn agent_id(&self) -> AgentId { self.agent_id }
}
```

Reader task pseudocode (write the actual implementation):

```
loop frame in JsonLines<ClaudeOutput>::next():
  match frame:
    System/init { session_id } => no-op (slice 1b records into SessionStarted)
    Assistant   { content_blocks } =>
      for block in content_blocks:
        ContentBlock::TextDelta { message_id, text } =>
          if let Some(ev) = coalescer.push_delta(message_id, text) { send(ev) }
        ContentBlock::TextStop  { message_id } =>
          if let Some(ev) = coalescer.finalize(message_id) { send(ev) }
        ContentBlock::ToolUse { id, name, input } =>
          send(ToolCall { tool_use_id: id, tool_name: name, input, ts: now })
        ContentBlock::McpToolResult { tool_use_id, content, is_error } =>
          send(ToolResult { ..., output: content, is_error, ts: now })
          if let Some(cp) = derive_checkpoint(&content) { send(CheckpointReached { ... }) }
    Result      { stop_reason } =>
      send(TurnCompleted { turn_id: open_turn.take().unwrap(), stop_reason, ts: now })
on EOF:
  if let Some(t) = open_turn.take() { send(TurnAborted { turn_id: t, reason: "session_crashed", ts: now }) }
  let stderr_tail = stderr_drain.snapshot().join("\n");
  send(SessionCrashed { reason: format!("EOF\n--- stderr tail ---\n{stderr_tail}"), exit_code: child.exit_code(), ts: now })
on parse error:
  let stderr_tail = stderr_drain.snapshot().join("\n");
  send(SessionCrashed { reason: format!("unparseable frame: {err}\n--- stderr tail ---\n{stderr_tail}"), exit_code: None, ts: now })
  break
also on cancel.cancelled() => break
```

Writer task pseudocode:

```
loop turn in turns_rx.recv():
  let queued = match turn.source {
    Human => HumanPromptQueued { turn_id: turn.turn_id, text: turn.text.clone(), ts: now },
    Agent => AgentPromptQueued { turn_id: turn.turn_id, text: turn.text.clone(), ts: now },
  };
  drafts_tx.send(queued);
  match stdin.write_all(serde_json::to_vec(&ClaudeInput::User { content: turn.text }).then_push_newline()):
    Ok(()) => drafts_tx.send(matching *PromptDelivered),
    Err(BrokenPipe) => drafts_tx.send(PromptDeliveryFailed { ... }), // reader will append SessionCrashed shortly
```

`open_turn: Option<TurnId>` is shared between writer and reader through an `Arc<Mutex<Option<TurnId>>>` — writer sets it on `Delivered`, reader clears on `TurnCompleted`/`TurnAborted`/EOF.

- [x] **Step 8: Run smoke test until it passes.**

Run: `cargo test -p nephila-connector --test session_smoke -- --nocapture`
Expected (after implementation): PASS — `HumanPromptQueued`, `HumanPromptDelivered`, ≥1 `AssistantMessage`, exactly one `TurnCompleted`.

- [x] **Step 9: Implement `crates/connector/tests/fixtures/fake_claude/`.**

Standalone bin crate. Parses the same flag set as the real `claude` binary (the connector spawns the fake with these flags, so the fake MUST honour them or tests for resume/session-id stability will silently use a placeholder):

```rust
// crates/connector/tests/fixtures/fake_claude/src/main.rs
//! Test fixture imitating `claude --print --verbose --input-format stream-json --output-format stream-json`.
//! ALL flag parsing is hand-rolled (no `clap`) to keep dev-deps minimal.

use std::io::{BufRead, Write};

#[derive(Default)]
struct Args {
    session_id: Option<String>,
    resume: Option<String>,
    scenario: Scenario,
}

#[derive(Default, Clone, Copy)]
enum Scenario { #[default] Happy, CrashMidTurn, Malformed, OversizedToolResult, SlowWriter, SlowReader }

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--session-id" => a.session_id = it.next(),
            "--resume"     => a.resume     = it.next(),
            "--scenario"   => a.scenario   = match it.next().as_deref() {
                Some("happy") | None              => Scenario::Happy,
                Some("crash_mid_turn")            => Scenario::CrashMidTurn,
                Some("malformed")                 => Scenario::Malformed,
                Some("oversized_tool_result")     => Scenario::OversizedToolResult,
                Some("slow_writer")               => Scenario::SlowWriter,
                Some("slow_reader")               => Scenario::SlowReader,
                Some(other)                       => { eprintln!("unknown scenario: {other}"); std::process::exit(2) }
            },
            // Real-claude flags we accept but ignore (to keep the spawn flag set in lockstep):
            "--print" | "--verbose"
            | "--include-partial-messages" => {}
            "--input-format" | "--output-format"
            | "--mcp-config" | "--permission-mode"
            | "--settings" => { it.next(); /* consume value */ }
            other => eprintln!("fake_claude: ignoring unknown flag: {other}"),
        }
    }
    a
}

fn main() -> std::io::Result<()> {
    let args = parse_args();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let id = args.resume
        .as_deref()
        .or(args.session_id.as_deref())
        .unwrap_or("00000000-0000-0000-0000-000000000000");
    // init frame echoes whichever id the connector passed (resume wins over session-id,
    // matching real claude's behaviour for slice 4 fallback testing).
    writeln!(out, "{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{id}\"}}")?;
    out.flush()?;

    for line in std::io::BufReader::new(std::io::stdin()).lines() {
        let _input = line?; // drop content; scenario drives the response shape
        match args.scenario {
            Scenario::Happy => emit_happy(&mut out)?,
            Scenario::CrashMidTurn => { emit_partial(&mut out)?; std::process::exit(137); }
            Scenario::Malformed => { writeln!(out, "{{not valid json")?; out.flush()?; }
            Scenario::OversizedToolResult => emit_oversized_tool_result(&mut out)?,
            Scenario::SlowWriter => { std::thread::sleep(std::time::Duration::from_millis(200)); emit_happy(&mut out)?; }
            Scenario::SlowReader => emit_burst(&mut out, 200)?, // 200 events with no flush sleeps; consumer must keep up
        }
    }
    Ok(())
}
// `emit_happy`, `emit_partial`, `emit_oversized_tool_result`, `emit_burst`:
//   write the canned frame sequences documented below, each frame newline-terminated.
```

Canned `happy` frame sequence:

```
{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text_delta","text":"O"}]}}
{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text_delta","text":"K"}]}}
{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text_stop"}]}}
{"type":"result","stop_reason":"end_turn"}
```

For slice 1a only `happy` is exercised — the others are fixture scaffolding wired by later slices (see step 13a tracking note).

Note: `slow_reader`, `slow_writer`, `malformed` scenarios are wired by slice 1b: `slow_reader` (broadcast lag test, Task 3 step 13), `slow_writer` (writer-thread backpressure test, Task 3 step 8 ordering test), `malformed` (parse-error → SessionCrashed test, Task 3 step 1 round-trip + a new dedicated test). Slice 1a leaves them as fixture scaffolding; if they aren't wired by end of slice 1b, drop them from the fixture rather than ship dead code.

`crates/connector/Cargo.toml`:

```toml
[[bin]]
name = "fake_claude"
path = "tests/fixtures/fake_claude/src/main.rs"

[[example]]
name = "spike_stream_json"
required-features = []
```

Add a `build.rs` that exports the `fake_claude` binary path via `cargo:rustc-env=FAKE_CLAUDE_BIN=...` so tests don't need an env var, or use `env!("CARGO_BIN_EXE_fake_claude")` directly in tests (preferred — no build.rs).

- [x] **Step 10: Implement `Drop` and explicit `shutdown` correctly.**

`shutdown()` is the preferred path. `Drop` is best-effort cleanup for panic paths only — it cannot await async work.

`Drop` does **not** rely on `kill_on_drop`. Sketch:

```rust
impl Drop for ClaudeCodeSession {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Take the child synchronously without async lock (try_lock returns Err on contention,
        // which is fine — child will be reaped on its own once tasks observe cancel).
        let child_opt = self.child.try_lock().ok().and_then(|mut g| g.take());
        if let Some(mut child) = child_opt {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                // No std::thread::spawn — start_kill needs a runtime that may be torn down.
                // The reader/writer tasks observe cancel and exit; the SIGTERM above is the
                // graceful nudge. If claude ignores SIGTERM, the OS will reap on process exit.
                // Operators should always call shutdown() rather than relying on Drop.
            }
            // Plain drop is correct here: with `kill_on_drop = false`, tokio's
            // `Child::drop` does NOT kill the process and does NOT block. The runtime
            // still tracks the PID via its internal reaper, so the zombie is reaped
            // when the child exits. Do NOT call `std::mem::forget(child)` — that
            // bypasses tokio's reaper bookkeeping and leaks the zombie until the
            // nephila process itself exits, which accumulates over many crash/respawn
            // cycles. Operators should always call `shutdown()` rather than relying
            // on Drop; this path is panic-cleanup only.
            drop(child);
        }
    }
}
```

`shutdown()` is the preferred path: cancel, await reader to drain to EOF, append `SessionEnded`, await writer drain. After reader/writer drain → `child.wait().await?` to reap the process; if `wait` errors, log with `tracing::warn!`.

- [x] **Step 11: Add a TUI skeleton `SessionPane` that subscribes to drafts.**

`crates/tui/src/panels/session_pane.rs` — minimal: holds `Vec<RenderedEvent>`, accepts `SessionEventDraft` over an `mpsc::Receiver` (slice 1a does not yet plumb this in via `subscribe_after`), renders a vertical list of finalized assistant messages and prompts. Skip toolresult/markdown/syntax-highlight rendering — slice 6.

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};
use crate::layout::{focused_border_style, focused_border_type};

pub struct SessionPane {
    pub events: std::collections::VecDeque<RenderedRow>,
    pub focused: bool,
}

pub struct RenderedRow { pub glyph: &'static str, pub text: String, pub timestamp: chrono::DateTime<chrono::Utc> }

impl SessionPane {
    pub fn new() -> Self { Self { events: std::collections::VecDeque::new(), focused: false } }
    pub fn push_draft(&mut self, ev: nephila_connector::event_draft::SessionEventDraft) {
        // map to a RenderedRow per spec §SessionPane.Pane behavior glyphs
    }
}

impl Widget for &SessionPane {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL)
            .border_style(focused_border_style(self.focused))
            .border_type(focused_border_type(self.focused))
            .title("Session");
        let items: Vec<ListItem> = self.events.iter()
            .map(|r| ListItem::new(format!("{} {} {}", r.timestamp.format("%H:%M:%S"), r.glyph, r.text)))
            .collect();
        List::new(items).block(block).render(area, buf);
    }
}
```

- [x] **Step 12: Add a TUI rendering test using `ratatui::backend::TestBackend`.**

`crates/tui/tests/session_pane_render.rs`:

```rust
#[test]
fn renders_three_rows() {
    let backend = ratatui::backend::TestBackend::new(60, 6);
    let mut term = ratatui::Terminal::new(backend).unwrap();
    let mut pane = nephila_tui::panels::session_pane::SessionPane::new();
    pane.push_draft(/* HumanPromptQueued mock */);
    pane.push_draft(/* AssistantMessage final mock */);
    pane.push_draft(/* TurnCompleted mock */);
    term.draw(|f| f.render_widget(&pane, f.area())).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf.content().iter().map(|c| c.symbol().chars().next().unwrap_or(' ')).collect();
    assert!(text.contains("YOU →"));
    assert!(text.contains("ASSIST"));
    assert!(text.contains("✓"));
}
```

Run: `cargo test -p nephila-tui --test session_pane_render`. Expected PASS.

- [x] **Step 13: Wire a single-agent demo in a temporary `bin/src/bin/demo_session.rs`.**

Spawns one `ClaudeCodeSession` against the fake-claude, pipes drafts into a `SessionPane` via an mpsc bridge, runs the TUI for 5 seconds, exits. Manual verification only — delete the demo bin in slice 5. Run: `cargo run --bin demo_session`. Expected: terminal shows session pane updating live with the fake-claude's `OK` stream.

- [x] **Step 13a: Track scenarios `slow_reader`, `slow_writer`, `malformed` as TODO for slice 1b wiring; create issues or notes.**

These fixture scenarios are defined in Step 9 but not exercised by slice 1a tests. File a tracking issue (or appendix note in this plan) listing the three scenarios and pointing to the slice 1b tests that will exercise them (Task 3 steps 1, 8, 13). If slice 1b ships without wiring them, drop them from the fixture rather than ship dead code.

**Tracking note (slice-1a):** the fake-claude fixture (`crates/connector/tests/fixtures/fake_claude/src/main.rs`) currently implements the `happy`, `crash_mid_turn`, `malformed`, `oversized_tool_result`, `slow_writer`, and `slow_reader` scenarios. Slice 1a only exercises `happy`. Slice 1b owners must wire the remaining three:

- `malformed` → Task 3 step 1 (parse-error → `SessionCrashed` test)
- `slow_writer` → Task 3 step 8 (writer-thread backpressure / ordering test)
- `slow_reader` → Task 3 step 13 (broadcast-lag / `Lagged` recovery test)

If slice 1b lands without wiring all three, the unused scenario branches must be deleted from the fake binary as part of slice 1b's `cargo clippy --workspace --all-targets -- -D warnings` gate (dead-code lint will catch it).

- [x] **Step 14: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.**

Expected: green. Commit: `slice-1a: ClaudeCodeSession transport (reader/writer/coalescer) and minimal SessionPane`.

---

## Task 3: Slice 1b — Aggregate + store foundation

**Purpose:** Promote `SessionEventDraft` to a real `Session` aggregate with a reducer; add `subscribe_after`, `append_batch`, `prune_aggregate` to `DomainEventStore`; assign sequences inside the writer thread; add the blob-spillover store; wire observability; pass the throughput benchmark gate.

**Files:**
- Create: `crates/core/src/session.rs` — `Session` aggregate, `SessionState`, `SessionPhase`, `SessionCommand`, `SessionError`
- Create: `crates/core/src/session_event.rs` — `SessionEvent` enum + serde glue (replaces `SessionEventDraft` — re-exported as `pub use` so slice 1a's connector keeps compiling during rollover)
- Create: `crates/store/src/blob.rs` — `BlobStore` trait + SQLite default impl
- Create: `crates/store/src/subscribe.rs` — broadcast bookkeeping (`DashMap<(String,String), broadcast::Sender>`) + `subscribe_after` join logic
- Modify: `crates/eventsourcing/src/store.rs` — add `subscribe_after`, `append_batch`, `prune_aggregate` trait methods (default impls would be insufficient — sequence assignment changes semantics)
- Modify: `crates/eventsourcing/src/envelope.rs` — change `EventEnvelope::sequence` from caller-supplied to `Option<u64>` filled by store, OR keep `u64` but document "stamped by store"; choose the latter to minimise churn (see step 4)
- Modify: `crates/store/src/writer.rs` — `WriterHandle` gains a `next_sequence` HashMap keyed by `(aggregate_type, aggregate_id)` warmed lazily from `MAX(sequence)`; broadcast publish happens inside the writer closure post-commit
- Modify: `crates/store/src/domain_event.rs` — implement new trait methods; `append`/`append_batch` stamp sequences
- Modify: `crates/store/src/lib.rs` — add `PRAGMA synchronous = NORMAL`; add a separate read-only connection pool (`r2d2_sqlite` or hand-rolled `Vec<Connection>` behind a semaphore)
- Modify: `crates/eventsourcing/src/tracing.rs` — `subscribe_after.{lagged_recovery_total,backfill_rows,head_lag}` counters/histogram; `append_batch.size` histogram
- Modify: `crates/connector/src/session.rs` — replace `broadcast::Sender<SessionEventDraft>` with `Arc<dyn DomainEventStore>` calls; `subscribe_drafts` becomes `subscribe_events`
- Create: `crates/store/benches/subscribe_after_throughput.rs` — Criterion bench gating the slice
- Create: `crates/eventsourcing/tests/session_apply.rs` — reducer property tests
- Create: `crates/store/tests/subscribe_after_ordering.rs` — listener-first ordering, lagged recovery, dedup

- [x] **Step 1: Define `SessionEvent` and round-trip serde test.**

`crates/core/src/session_event.rs`:

```rust
use crate::id::{AgentId, CheckpointId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// NOTE: `CheckpointId` is the newtype `pub struct CheckpointId(pub Uuid)` from
// `crates/core/src/id.rs`, NOT a `String` alias. The earlier slice-1a draft
// (`event_draft.rs`) used `pub type CheckpointId = String` as a placeholder; the
// real aggregate uses the newtype. The MCP-tool-result `derive_checkpoint(...)`
// helper in the connector parses the wire `String` into `CheckpointId` via
// `Uuid::parse_str(...).map(CheckpointId)`; on parse failure the connector
// emits `SessionCrashed { reason: "malformed checkpoint_id: ..." }` rather than
// silently dropping the event.
pub type SessionId = Uuid;
pub type TurnId = Uuid;
pub type MessageId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum SessionEvent {
    SessionStarted { session_id: SessionId, agent_id: AgentId, model: Option<String>, working_dir: std::path::PathBuf, mcp_endpoint: String, resumed: bool, ts: DateTime<Utc> },
    AgentPromptQueued { turn_id: TurnId, text: String, ts: DateTime<Utc> },
    AgentPromptDelivered { turn_id: TurnId, ts: DateTime<Utc> },
    HumanPromptQueued { turn_id: TurnId, text: String, ts: DateTime<Utc> },
    HumanPromptDelivered { turn_id: TurnId, ts: DateTime<Utc> },
    PromptDeliveryFailed { turn_id: TurnId, reason: String, ts: DateTime<Utc> },
    AssistantMessage { message_id: MessageId, seq_in_message: u32, delta_text: String, is_final: bool, truncated: bool, ts: DateTime<Utc> },
    ToolCall { tool_use_id: String, tool_name: String, input: serde_json::Value, ts: DateTime<Utc> },
    ToolResult { tool_use_id: String, output: ToolResultPayload, is_error: bool, ts: DateTime<Utc> },
    CheckpointReached { checkpoint_id: CheckpointId, interrupt: Option<InterruptSnapshot>, ts: DateTime<Utc> },
    TurnCompleted { turn_id: TurnId, stop_reason: String, ts: DateTime<Utc> },
    TurnAborted { turn_id: TurnId, reason: String, ts: DateTime<Utc> },
    SessionCrashed { reason: String, exit_code: Option<i32>, ts: DateTime<Utc> },
    SessionEnded { ts: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ToolResultPayload {
    Inline(serde_json::Value),
    Spilled { hash: String, original_len: u64, snippet: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InterruptSnapshot { Hitl { question: String, options: Vec<String> }, Pause, Drain }
```

Test: round-trip every variant via `serde_json::to_string` + `from_str`, assert `==`. File: `crates/core/tests/session_event_serde.rs`.

Run: `cargo test -p nephila-core session_event_serde -- --nocapture`. Expected: FAIL (file doesn't exist) → PASS after the `serde` derives compile.

- [x] **Step 2: Define `Session` aggregate and write `apply` invariant tests.**

`crates/core/src/session.rs`:

```rust
use nephila_eventsourcing::aggregate::EventSourced;
use crate::session_event::{SessionEvent, SessionId, TurnId};
use crate::id::{AgentId, CheckpointId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase { Starting, Running, WaitingHitl, Paused, Crashed, Ended }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointRef { pub checkpoint_id: CheckpointId, pub at_sequence: u64 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub agent_id: AgentId,
    pub phase: SessionPhase,
    pub open_turn: Option<TurnId>,
    pub last_checkpoint: Option<CheckpointRef>,
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommand {
    /* slice 1b: empty — connector calls produce events directly via store.append.
       handle() exists for symmetry with Agent and the post-1b control protocol. */
    NoOp,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session in terminal state — no further commands accepted")]
    Terminal,
    #[error("invariant violated: {0}")]
    Invariant(String),
}

impl EventSourced for Session {
    type Event = SessionEvent;
    type Command = SessionCommand;
    type Error = SessionError;

    fn aggregate_type() -> &'static str { "session" }
    fn aggregate_id(&self) -> String { self.id.to_string() }
    fn default_state() -> Self {
        // SessionId is `Uuid` directly, so `Uuid::nil()` works.
        // AgentId is the newtype `pub struct AgentId(pub Uuid)` from `crates/core/src/id.rs:14`
        // — there is no `AgentId::nil()` constructor, construct from the inner uuid.
        Self { id: Uuid::nil(), agent_id: AgentId(Uuid::nil()), phase: SessionPhase::Starting, open_turn: None, last_checkpoint: None, last_seq: 0 }
    }

    fn apply(self, event: &SessionEvent) -> Self {
        // see step 3 for the full match
        unimplemented!()
    }

    fn handle(&self, _cmd: SessionCommand) -> Result<Vec<SessionEvent>, SessionError> {
        if matches!(self.phase, SessionPhase::Ended) { return Err(SessionError::Terminal); }
        Ok(Vec::new())
    }
}
```

Tests in `crates/eventsourcing/tests/session_apply.rs`:

```rust
// 1. SessionStarted -> phase=Running, id and agent_id set, last_seq=0
// 2. *PromptQueued sets open_turn=Some(turn_id); a second *PromptQueued
//    while open_turn is Some is an invariant violation (apply *should* still
//    produce a state — we don't panic in apply — but the producer must not emit it;
//    we assert via a debug_assert + a comment).
// 3. TurnCompleted with matching turn_id clears open_turn; stop_reason "interrupt" -> phase=Paused.
// 4. CheckpointReached sets last_checkpoint; interrupt=Some(Hitl{..}) -> phase=WaitingHitl.
// 5. SessionCrashed with open_turn=Some -> implicit TurnAborted: state has open_turn=None, phase=Crashed.
//    (Connector-side reader emits the explicit TurnAborted event before SessionCrashed; this test
//    checks that even if it didn't, apply doesn't leave open_turn dangling — defense in depth.)
// 6. After SessionEnded, applying further events is a no-op and apply preserves phase=Ended.
// 7. After replaying N envelopes (sequences 1..=N) via `apply_envelope`, state.last_seq == N.
//    This guards CheckpointRef::at_sequence and the prune-policy math (`last_seq - 200`)
//    against a future regression where a consumer wires `apply` directly and zeros at_sequence.
```

Implement those tests as `#[test] fn` units. Run: `cargo test -p nephila-eventsourcing session_apply`. Expected FAIL initially.

- [x] **Step 3: Implement `Session::apply`.**

```rust
fn apply(mut self, event: &SessionEvent) -> Self {
    if matches!(self.phase, SessionPhase::Ended) { return self; }
    match event {
        SessionEvent::SessionStarted { session_id, agent_id, .. } => {
            self.id = *session_id; self.agent_id = *agent_id; self.phase = SessionPhase::Running;
        }
        SessionEvent::AgentPromptQueued { turn_id, .. } | SessionEvent::HumanPromptQueued { turn_id, .. } => {
            debug_assert!(self.open_turn.is_none(), "two open turns is an invariant violation");
            self.open_turn = Some(*turn_id);
        }
        SessionEvent::AgentPromptDelivered { .. } | SessionEvent::HumanPromptDelivered { .. } => { /* state unchanged */ }
        SessionEvent::PromptDeliveryFailed { turn_id, .. } => {
            if self.open_turn == Some(*turn_id) { self.open_turn = None; }
        }
        SessionEvent::AssistantMessage { .. } | SessionEvent::ToolCall { .. } | SessionEvent::ToolResult { .. } => { /* state unchanged */ }
        SessionEvent::CheckpointReached { checkpoint_id, interrupt, .. } => {
            self.last_checkpoint = Some(CheckpointRef { checkpoint_id: checkpoint_id.clone(), at_sequence: self.last_seq });
            if matches!(interrupt, Some(crate::session_event::InterruptSnapshot::Hitl { .. })) {
                self.phase = SessionPhase::WaitingHitl;
            } else if matches!(interrupt, Some(crate::session_event::InterruptSnapshot::Pause)) {
                self.phase = SessionPhase::Paused;
            }
        }
        SessionEvent::TurnCompleted { turn_id, stop_reason, .. } => {
            if self.open_turn == Some(*turn_id) { self.open_turn = None; }
            if stop_reason == "interrupt" { self.phase = SessionPhase::Paused; }
            else if matches!(self.phase, SessionPhase::WaitingHitl) { /* stays */ }
            else { self.phase = SessionPhase::Running; }
        }
        SessionEvent::TurnAborted { turn_id, .. } => {
            if self.open_turn == Some(*turn_id) { self.open_turn = None; }
        }
        SessionEvent::SessionCrashed { .. } => {
            self.open_turn = None; self.phase = SessionPhase::Crashed;
        }
        SessionEvent::SessionEnded { .. } => { self.phase = SessionPhase::Ended; }
    }
    self
}
```

**`last_seq` update contract.** The `EventSourced::apply` trait takes only `&Event`, not the envelope, so `apply` itself cannot bump `self.last_seq` from the persisted sequence. Add a thin wrapper on `Session`:

```rust
impl Session {
    /// Replay/live-stream callers MUST use this — never call `apply` directly.
    /// Sets `last_seq` to the envelope's sequence (assigned by the store) AFTER
    /// applying the event. `CheckpointRef::at_sequence` and the prune-policy
    /// math (`last_seq - 200`) both depend on this field tracking the latest
    /// envelope sequence in the aggregate.
    pub fn apply_envelope(self, env: &nephila_eventsourcing::envelope::EventEnvelope) -> Result<Self, SessionError> {
        let event: SessionEvent = serde_json::from_value(env.payload.clone())
            .map_err(|e| SessionError::Invariant(format!("payload decode: {e}")))?;
        let mut next = self.apply(&event);
        next.last_seq = env.sequence;
        Ok(next)
    }
}
```

The replay path in step 17 (snapshot trigger) and the consumer pump tasks (slice 2 / Task 4) call `apply_envelope`, NOT `apply`. The reducer property test in step 17b also drives state through `apply_envelope`.

Add to step 2's invariant test list a new case 7: "After replaying N envelopes with sequences 1..=N, `state.last_seq == N`." This guards against a future regression where someone wires `apply` directly into the consumer loop and silently zeros out `at_sequence` / breaks the prune policy.

Run: `cargo test -p nephila-eventsourcing session_apply`. Expected: PASS.

- [x] **Step 4: Change `EventEnvelope::sequence` semantics to "stamped by store".**

Decide between the two options:

| Option | Pros | Cons |
|---|---|---|
| Change to `Option<u64>` and have store fill it | Compile-time guarantee callers can't pre-populate | Touches every existing `EventEnvelope` construction site (Agent flows, tests) — bigger diff |
| Keep `u64`, document "ignored on append; populated on reads" | Smaller diff; tests stay readable | Easy to mis-set; relies on convention |

**Pending HITL decision (must be made before slice 1b merges):** Option 2 is the proposed default, but the spec at line 186 mandates a breaking change. Slice 1b implementer raises this in the HITL review checkpoint at the top of slice 1b. Document the decision in `docs/adr/0002-eventenvelope-sequence-stamping.md`. The debug-only assertion is **insufficient** as the sole guard — under Option 2 we MUST also assert that release builds simply ignore the caller-supplied value (writer overwrites unconditionally), and the contract test at the end of step 4 verifies this with a non-zero caller value.

Edit `crates/eventsourcing/src/envelope.rs`:

```rust
pub struct EventEnvelope {
    pub id: EventId,
    pub aggregate_type: String,
    pub aggregate_id: String,
    /// CONTRACT (post slice-1b): set to 0 by the connector when constructing
    /// fresh envelopes. The store stamps the actual sequence inside the writer
    /// thread, after computing `MAX(sequence) WHERE aggregate_type=? AND aggregate_id=?`.
    /// On reads, this is the assigned sequence.
    pub sequence: u64,
    /* ... */
}
```

Update every callsite that constructs `EventEnvelope` with a non-zero sequence to pass `0`. `rg "sequence: \d+" crates/` to find them. Tests in `crates/store/src/domain_event.rs::tests` use `make_envelope(_, _, seq)` — keep the helper but document it tests-only-now.

**Upgrade-path regression test (required).** The change is forward-compatible with existing `nephila.db` rows because the writer warms `next_sequence` from `SELECT COALESCE(MAX(sequence), 0)` per aggregate (step 8). To prevent a future regression, add `crates/store/tests/sequence_stamping_upgrade.rs`:

```rust
#[tokio::test]
async fn writer_continues_from_max_when_pre_existing_rows_have_caller_stamped_sequences() {
    let store = SqliteStore::open_in_memory().await.unwrap();
    // Simulate the pre-1b world: caller stamps sequences directly via a raw INSERT.
    // (Not via the public API — we want to seed rows that the writer didn't see.)
    store.raw_seed_for_test("agent", "agent-A", &[1, 2, 3]).await.unwrap();

    // Now use the post-1b API: caller passes 0; writer must stamp 4, 5, 6.
    let appended = store.append_batch(vec![
        env_with_seq_zero("agent", "agent-A"),
        env_with_seq_zero("agent", "agent-A"),
        env_with_seq_zero("agent", "agent-A"),
    ]).await.unwrap();
    assert_eq!(appended, vec![4, 5, 6]);

    // Replay must yield 1..=6 in order.
    let all = store.load_events("agent", "agent-A", 0).await.unwrap();
    assert_eq!(all.iter().map(|e| e.sequence).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 6]);
}

#[tokio::test]
async fn writer_handles_gapped_pre_existing_rows_by_continuing_from_max() {
    // If a previous deployment left a gap (e.g. seq 1, 2, 5 — never 3 or 4),
    // the writer continues from MAX(=5) → 6, 7, 8. We do NOT backfill the gap;
    // gapped histories are left as-is and the prune policy eventually wipes them.
    // This test pins that behaviour so a future "helpful" patch doesn't try to
    // detect-and-renumber gaps and accidentally break replay.
    let store = SqliteStore::open_in_memory().await.unwrap();
    store.raw_seed_for_test("agent", "agent-B", &[1, 2, 5]).await.unwrap();
    let appended = store.append_batch(vec![env_with_seq_zero("agent", "agent-B")]).await.unwrap();
    assert_eq!(appended, vec![6]);
}
```

`raw_seed_for_test` is `#[cfg(test)]`-only and inserts rows via a direct `rusqlite::Connection::execute`, bypassing the writer. Add it to `crates/store/src/domain_event.rs` behind `#[cfg(any(test, feature = "test-seam"))]`.

- [x] **Step 5: Add the new trait methods to `DomainEventStore`.**

Edit `crates/eventsourcing/src/store.rs`:

```rust
pub trait DomainEventStore: Send + Sync {
    /* existing methods kept verbatim */

    fn append_batch(
        &self,
        envelopes: Vec<EventEnvelope>,
    ) -> impl std::future::Future<Output = Result<Vec<u64>, EventStoreError>> + Send;
    // returns the assigned sequences in input order

    fn subscribe_after(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        since_sequence: u64,
    ) -> impl std::future::Future<
        Output = Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<EventEnvelope, EventStoreError>> + Send>>,
            EventStoreError
        >
    > + Send;

    fn prune_aggregate(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        before_sequence: u64,
    ) -> impl std::future::Future<Output = Result<u64, EventStoreError>> + Send;
}
```

(The `Pin<Box<dyn Stream>>` shape is the simplest stable signature given Rust's current async-trait + RPIT-in-traits limitations. If the codebase already uses `async-trait` macro elsewhere, switch to that — `rg async_trait` to check.)

- [x] **Step 6: Implement `subscribe_after` listener-first ordering.**

`crates/store/src/subscribe.rs`:

```rust
use dashmap::DashMap;
use tokio::sync::broadcast;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct BroadcastRegistry {
    senders: DashMap<(String, String), broadcast::Sender<nephila_eventsourcing::envelope::EventEnvelope>>,
}

impl BroadcastRegistry {
    pub(crate) fn sender_for(&self, agg_type: &str, agg_id: &str) -> broadcast::Sender<nephila_eventsourcing::envelope::EventEnvelope> {
        self.senders
            .entry((agg_type.to_owned(), agg_id.to_owned()))
            .or_insert_with(|| broadcast::channel(4096).0)
            .value()
            .clone()
    }
}
```

In `domain_event.rs::subscribe_after`:

```rust
async fn subscribe_after(&self, agg_type: &str, agg_id: &str, since: u64)
    -> Result<Pin<Box<dyn Stream<Item = Result<EventEnvelope, EventStoreError>> + Send>>, EventStoreError>
{
    // 1. Attach listener FIRST.
    let mut listener = self.broadcasts.sender_for(agg_type, agg_id).subscribe();

    // 2. Snapshot head AFTER listener attached.
    let head = self.head_sequence(agg_type, agg_id).await?;

    // 3. Backfill via the read pool (NOT the writer thread). `load_events_from_pool`
    //    is the new method introduced alongside the read pool in step 7 — it
    //    checks out a read-only Connection from `self.read_pool`, runs the SELECT
    //    inline, and returns the connection to the pool. The pre-existing
    //    `DomainEventStore::load_events` continues to route through the writer
    //    thread for callers that want strict-after-write consistency (Agent
    //    aggregate replay, prune verification); `subscribe_after` is the only
    //    caller of the read-pool variant.
    //    Range: `since..=head_at_subscribe` (upper bound INCLUSIVE). The dedup
    //    rule below uses `seq > head_at_subscribe`, so the inclusive upper
    //    bound is what makes the backfill+live join lossless at the boundary.
    let backfill = self.load_events_from_pool(agg_type, agg_id, since, head).await?;

    // 4. Merge: yield backfill in order, then listener with seq > head, deduped.
    // Single state-machine stream join. State transitions:
    //   Backfill(iter) → Live  when iter is exhausted
    //   Live           → terminal on listener.recv() returning Closed
    // Dedup rule: when in Live, drop any envelope with sequence <= head_at_subscribe.
    // Lagged: surface as Err(EventStoreError::Lagged); resilient_subscribe (step 14)
    // recovers by re-subscribing from last delivered sequence.
    enum State {
        Backfill(std::vec::IntoIter<EventEnvelope>),
        Live,
    }
    let head_at_subscribe = head;
    let stream = futures::stream::unfold(
        (State::Backfill(backfill.into_iter()), listener),
        move |(state, mut listener)| async move {
            match state {
                State::Backfill(mut iter) => match iter.next() {
                    Some(env) => Some((Ok(env), (State::Backfill(iter), listener))),
                    None => {
                        // Recurse into Live: try one listener.recv on the same poll.
                        loop {
                            match listener.recv().await {
                                Ok(env) if env.sequence > head_at_subscribe => {
                                    return Some((Ok(env), (State::Live, listener)));
                                }
                                Ok(_) => continue, // dedup: drop seq <= head
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    return Some((Err(EventStoreError::Lagged(n)), (State::Live, listener)));
                                }
                                Err(broadcast::error::RecvError::Closed) => return None,
                            }
                        }
                    }
                },
                State::Live => loop {
                    match listener.recv().await {
                        Ok(env) if env.sequence > head_at_subscribe => {
                            return Some((Ok(env), (State::Live, listener)));
                        }
                        Ok(_) => continue,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            return Some((Err(EventStoreError::Lagged(n)), (State::Live, listener)));
                        }
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                },
            }
        },
    );
    Ok(Box::pin(stream))
}
```

**Dedup invariant:** in Live mode, every envelope with `sequence <= head_at_subscribe` is dropped silently — those events were already delivered via backfill. **Lagged invariant:** Lagged is impossible in Backfill (events come from disk) and recoverable in Live (resilient_subscribe restarts from last delivered seq).

- [x] **Step 7: Add a separate read-only connection pool for backfill.**

Edit `crates/store/src/lib.rs`. Today `SqliteStore` holds one writer connection. Add `read_pool: Arc<Mutex<Vec<rusqlite::Connection>>>` (size 4) opened with `OpenFlags::SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_FULL_MUTEX`. `PRAGMA synchronous = NORMAL;` set on the writer connection only — readers don't write. Set `PRAGMA journal_mode = WAL;` (likely already set — verify).

Add the new method `SqliteStore::load_events_from_pool(&self, agg_type: &str, agg_id: &str, since_exclusive: u64, head_inclusive: u64) -> Result<Vec<EventEnvelope>, EventStoreError>`:

```rust
pub(crate) async fn load_events_from_pool(
    &self,
    agg_type: &str,
    agg_id: &str,
    since_exclusive: u64,
    head_inclusive: u64,
) -> Result<Vec<EventEnvelope>, EventStoreError> {
    let pool = self.read_pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = pool.lock();
        let conn = guard.pop().ok_or(EventStoreError::PoolExhausted)?;
        let result = (|| -> rusqlite::Result<Vec<EventEnvelope>> {
            let mut stmt = conn.prepare_cached(
                "SELECT /* env cols */ FROM domain_events
                 WHERE aggregate_type = ?1 AND aggregate_id = ?2
                   AND sequence > ?3 AND sequence <= ?4
                 ORDER BY sequence ASC"
            )?;
            stmt.query_map(params![agg_type, agg_id, since_exclusive as i64, head_inclusive as i64],
                           |row| /* envelope from row */).and_then(|it| it.collect())
        })();
        guard.push(conn); // return to pool even on error
        result.map_err(EventStoreError::from)
    }).await?
}
```

Note the explicit `since_exclusive` / `head_inclusive` naming on the parameters — the upper bound is **inclusive** to match the `seq > head_at_subscribe` dedup invariant in step 6. Add a `debug_assert!(since_exclusive <= head_inclusive)` at the top.

The pre-existing `DomainEventStore::load_events` still routes through the writer thread; it is unchanged. Only `subscribe_after`'s backfill uses `load_events_from_pool`.

Test: `crates/store/tests/read_pool_concurrent.rs` — write 100 events on the writer, spin up 8 concurrent `load_events_from_pool` calls, assert each gets the same set of events and no `database is locked` error appears. Also assert that with the writer thread parked (e.g. via a fake-blocking append in flight), backfill calls still complete — the read pool is independent.

- [x] **Step 8: Implement `append_batch` with sequence stamping inside the writer thread.**

The current `WriterHandle::execute(F: FnOnce(&Connection) + Send + 'static)` API takes a one-shot closure that gets a `&Connection` but no persistent thread-local state. The lazy `next_sequence` cache cannot live as a captured local in such a closure (it would be empty on every call) and cannot live on the `WriterHandle` struct (the OS thread, not the handle, owns the connection and any cache).

The fix: replace the generic `FnOnce` API with a typed command enum so the writer **thread** owns and mutates the cache directly.

```rust
// crates/store/src/writer.rs
pub(crate) enum WriterCmd {
    Append { envelopes: Vec<EventEnvelope>, blobs: Vec<PreparedBlob>, reply: oneshot::Sender<Result<Vec<u64>, EventStoreError>> },
    HeadSequence { agg_type: String, agg_id: String, reply: oneshot::Sender<Result<u64, EventStoreError>> },
    Prune { agg_type: String, agg_id: String, before: u64, reply: oneshot::Sender<Result<u64, EventStoreError>> },
    // ... other typed commands as needed
}

pub(crate) struct WriterHandle { tx: mpsc::Sender<WriterCmd> }

fn writer_thread(conn: Connection, mut rx: mpsc::Receiver<WriterCmd>, broadcasts: Arc<BroadcastRegistry>) {
    let mut next_sequence: HashMap<(String, String), u64> = HashMap::new(); // owned by THIS thread
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WriterCmd::Append { mut envelopes, blobs, reply } => {
                let result = (|| -> Result<Vec<u64>, EventStoreError> {
                    let tx = conn.unchecked_transaction()?;
                    for blob in &blobs {
                        tx.execute("INSERT OR IGNORE INTO blobs (...) VALUES (?,?,?)", params![/* ... */])?;
                    }
                    let mut assigned = Vec::with_capacity(envelopes.len());
                    for env in envelopes.iter_mut() {
                        let key = (env.aggregate_type.clone(), env.aggregate_id.clone());
                        let next = match next_sequence.get(&key) {
                            Some(&n) => n + 1,
                            None => {
                                // Warm from disk on first touch for this aggregate.
                                let from_disk: i64 = tx.query_row(
                                    "SELECT COALESCE(MAX(sequence), 0) FROM domain_events
                                     WHERE aggregate_type = ?1 AND aggregate_id = ?2",
                                    params![&env.aggregate_type, &env.aggregate_id],
                                    |r| r.get(0),
                                )?;
                                from_disk as u64 + 1
                            }
                        };
                        env.sequence = next;
                        next_sequence.insert(key, next);
                        assigned.push(next);
                        tx.execute("INSERT INTO domain_events (...) VALUES (...)", params![/* ... */])?;
                    }
                    tx.commit()?;
                    Ok(assigned)
                })();
                if result.is_ok() {
                    // CRITICAL: publish AFTER commit, BEFORE replying to the async caller.
                    // The next WriterCmd cannot be processed until this loop iteration ends.
                    for env in &envelopes {
                        broadcasts.sender_for(&env.aggregate_type, &env.aggregate_id).send(env.clone()).ok();
                    }
                }
                let _ = reply.send(result);
            }
            // ... other arms
        }
    }
}
```

`WriterHandle::append_batch` builds a `WriterCmd::Append`, sends it over the mpsc channel, awaits the oneshot reply. Total order across appends to the same aggregate is preserved because the writer thread serializes commands and the publish runs inside the same iteration as the commit before the next command is dequeued. Append-vs-append ordering across different aggregates is not guaranteed (we don't promise that).

`broadcast::Sender::send` is non-blocking (drops on full); the writer thread is not stalled by slow consumers.

- [x] **Step 9: Implement `prune_aggregate`.**

```rust
async fn prune_aggregate(&self, agg_type: &str, agg_id: &str, before: u64) -> Result<u64, EventStoreError> {
    self.writer.execute(move |conn| {
        let n = conn.execute(
            "DELETE FROM domain_events WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3",
            params![agg_type, agg_id, before as i64],
        )?;
        Ok(n as u64)
    }).await
}
```

Test: append 10 events, prune below seq 5, assert `load_events(_, 0)` returns events 5..=10 only.

- [x] **Step 10: Add the blob spillover store.**

The blob store is split into a **read-only trait** (consumed by the pane / replay path via `Arc<dyn BlobReader>`) plus a **writer-thread-private insert path**. This is the only design that preserves "blob row INSERT and event row INSERT in the same SQLite transaction" — there is no standalone `BlobStore::put` method, because any standalone async writer would open a separate `rusqlite::Connection` and forfeit the same-transaction guarantee.

The blobs table lives in the **same SQLite database file** as `domain_events` (one db file, one writer connection). Step 11 reuses the existing `SqliteStore::WriterHandle`; no second writer is created.

`crates/store/src/blob.rs`:

```rust
/// Read-only handle for retrieving spilled tool-result payloads.
/// Implemented by `SqliteBlobReader` (uses the read-only connection pool from step 7).
pub trait BlobReader: Send + Sync {
    fn get(&self, hash: &str) -> impl std::future::Future<Output = Result<Vec<u8>, BlobError>> + Send;
}

/// Pure helper — no I/O. Hashes the bytes, computes a UTF-8-safe snippet,
/// returns the parts the connector embeds in the `Spilled` payload variant.
/// The actual `INSERT OR IGNORE INTO blobs` runs inside the writer-thread
/// closure that appends the event row (see step 11).
pub fn prepare_blob(bytes: &[u8]) -> PreparedBlob {
    let hash = blake3::hash(bytes).to_hex().to_string();
    let original_len = bytes.len() as u64;
    let snippet_end = bytes.len().min(8 * 1024);
    let snippet_end = (0..=snippet_end)
        .rev()
        .find(|&i| std::str::from_utf8(&bytes[..i]).is_ok())
        .unwrap_or(0);
    let snippet = String::from_utf8_lossy(&bytes[..snippet_end]).to_string();
    PreparedBlob { hash, original_len, snippet, payload: bytes.to_owned() }
}

#[derive(Debug, Clone)]
pub struct PreparedBlob { pub hash: String, pub original_len: u64, pub snippet: String, pub payload: Vec<u8> }

#[derive(Debug, Clone)] pub struct BlobRef { pub hash: String, pub original_len: u64 }
#[derive(Debug, thiserror::Error)] pub enum BlobError { #[error("storage error: {0}")] Storage(String), #[error("not found")] NotFound }
```

Default impl `SqliteBlobReader { pool: Arc<ReadPool> }`: schema (added to the same db file as `domain_events`) `CREATE TABLE IF NOT EXISTS blobs (hash TEXT PRIMARY KEY, payload BLOB NOT NULL, original_len INTEGER NOT NULL)`. `get` runs `SELECT payload FROM blobs WHERE hash = ?` against a read-pool connection.

Verify `blake3` workspace dep with `cargo tree -p nephila-eventsourcing | rg blake3` before adding it (Task 1 spike step `cargo tree --workspace | rg blake3` is the upstream check; this step only adds the dep if the spike confirmed it isn't already present).

Test: round-trip a 300 KiB blob through the writer (using the step 11 path, since there's no standalone `put`); idempotent re-insert via `INSERT OR IGNORE`; `get` for missing hash returns `NotFound`; `prepare_blob` is a pure function and unit-testable without any I/O.

- [x] **Step 11: Wire `ToolResult` payload spillover in the connector.**

Edit `crates/connector/src/session.rs::reader_task`. When emitting `ToolResult`, compute the `PreparedBlob` outside any closure (pure CPU), then submit it through a new `append_batch_with_blobs` writer-thread method that performs both inserts in one transaction:

```rust
let bytes = serde_json::to_vec(&output)?;
let (payload, prepared) = if bytes.len() <= 256 * 1024 {
    (ToolResultPayload::Inline(output), None)
} else {
    let p = nephila_store::blob::prepare_blob(&bytes);
    (
        ToolResultPayload::Spilled { hash: p.hash.clone(), original_len: p.original_len, snippet: p.snippet.clone() },
        Some(p),
    )
};
let event = SessionEvent::ToolResult { tool_use_id, output: payload, is_error, ts: Utc::now() };
self.store.append_batch_with_blobs(vec![envelope_for(event)], prepared.into_iter().collect()).await?;
```

`DomainEventStore::append_batch_with_blobs(envelopes, blobs)` is added in step 8 (alongside `append_batch`). Its writer-thread closure runs:

```rust
let tx = conn.transaction()?;
for blob in &blobs {
    tx.execute("INSERT OR IGNORE INTO blobs (hash, payload, original_len) VALUES (?1, ?2, ?3)",
               params![blob.hash, blob.payload, blob.original_len as i64])?;
}
for env in &mut envelopes { /* stamp sequence + INSERT INTO domain_events as in step 8 */ }
tx.commit()?;
// publish broadcasts AFTER commit (same as step 8)
```

**Atomicity contract:** blob row + event row land in one SQLite transaction; if the commit fails, neither is persisted, and the connector retries (or escalates to `SessionCrashed` if retries are exhausted). `BlobReader::get` in step 10 only reads from disk, so a half-written blob can never appear (no committed event references a missing blob).

`SessionConfig` gains `blob_reader: Arc<dyn BlobReader>` (read-only — used by `SessionPane` and replay paths to fetch spilled payloads on demand). The connector itself does NOT hold a `BlobReader` — it never reads blobs back, only writes them via `append_batch_with_blobs`.

Test in `crates/connector/tests/oversized_tool_result.rs` — drive fake-claude with `--scenario oversized_tool_result`, assert the appended event is `ToolResultPayload::Spilled { .. }` with the right `original_len`, then call `blob_reader.get(hash)` and assert the bytes match. Also test atomicity: inject a writer-closure error (test-only seam) after the blobs INSERT but before the event INSERT, assert the blob is NOT visible to a follow-up `blob_reader.get(hash)` (transaction rolled back).

- [x] **Step 12: Replace draft-broadcast with store.append_batch in `ClaudeCodeSession`.**

Replace `drafts_tx: broadcast::Sender<SessionEventDraft>` with `store: Arc<dyn DomainEventStore>` and `aggregate_id: SessionId` (= `session_id`). `subscribe_drafts()` is removed; consumers call `store.subscribe_after("session", session_id.to_string(), since_seq)` directly. The connector internally batches events emitted from a single `ClaudeOutput` frame into one `append_batch` call.

The slice-1a smoke test (`session_smoke.rs`) gets rewritten to subscribe through `store` instead of `subscribe_drafts`. Run: `cargo test -p nephila-connector --test session_smoke`. Expected: PASS.

- [x] **Step 13: Ordering test — listener-first vs head-second.**

`crates/store/tests/subscribe_after_ordering.rs`:

```rust
#[tokio::test]
async fn listener_attached_before_head_snapshot_does_not_lose_concurrent_appends() {
    let store = Arc::new(SqliteStore::open_in_memory(384).unwrap());
    // Pre-seed seq 1..=3
    for s in 1..=3 { append(store.clone(), "session", "s1", s, "before").await; }

    // Subscribe after seq 0; will backfill 1..=3 plus get live events.
    let mut stream = store.subscribe_after("session", "s1", 0).await.unwrap();

    // Concurrent append racing with the head-snapshot inside subscribe_after.
    // We have no easy hook to interleave precisely, so fire bursts: 100 concurrent
    // subscribe_after calls each followed by a single append from a separate task.
    // Assertion: every subscriber must see *its* concurrent append.
    let _ = tokio::spawn(async move { append(store.clone(), "session", "s1", 0/*stamped*/, "after").await; });

    let mut seen = Vec::new();
    while let Some(Ok(env)) = stream.next().await {
        seen.push(env.sequence);
        if seen.len() == 4 { break; }
    }
    assert_eq!(seen, vec![1, 2, 3, 4]);
}
```

Plus a `Lagged` test: shrink broadcast bound to 8, append 100 events while the consumer is paused, assert it surfaces `RecvError::Lagged` and that the recovery path (`load_events` from `last_seen` then re-subscribe) yields all 100 in order with no dupes.

**Deterministic seam.** A `SqliteStore`-level `sleep_after_listener_attach` field is the wrong injection point — the broadcast listener is attached inside `subscribe_after`, not on the store. The seam must hook the gap *between* `listener = sender.subscribe()` (step 1) and `head = self.head_sequence(...)` (step 2) inside `subscribe_after` itself.

Add a test-only argument to the impl path:

```rust
#[cfg(any(test, feature = "test-seam"))]
pub(crate) struct SubscribeAfterHooks {
    pub pre_head_snapshot: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[cfg(any(test, feature = "test-seam"))]
async fn subscribe_after_with_hooks(
    &self, agg_type: &str, agg_id: &str, since: u64, hooks: SubscribeAfterHooks,
) -> Result<...> {
    let mut listener = self.broadcasts.sender_for(agg_type, agg_id).subscribe();
    if let Some(f) = hooks.pre_head_snapshot.as_ref() { f(); } // synchronization point
    let head = self.head_sequence(agg_type, agg_id).await?;
    /* ...same as production subscribe_after... */
}

async fn subscribe_after(&self, ...) -> Result<...> {
    self.subscribe_after_with_hooks(..., SubscribeAfterHooks { pre_head_snapshot: None }).await
}
```

Test 1 (deterministic — proves listener-first ordering):

```rust
let (started_tx, started_rx) = oneshot::channel::<()>();
let (done_tx, done_rx) = oneshot::channel::<()>();
let hooks = SubscribeAfterHooks {
    pre_head_snapshot: Some(Arc::new(move || {
        // We're between listener-attach and head-snapshot. Signal the test, await done.
        let _ = started_tx.send(());
        let _ = done_rx.blocking_recv();
    })),
};
let stream_fut = store.subscribe_after_with_hooks("session", "s1", 0, hooks);
// Wait until the subscribe call has attached its listener.
started_rx.await.unwrap();
// Race window: this append must be observed by the listener (published via broadcast),
// NOT by the head-snapshot (which hasn't run yet) — and NOT both (dedup must fire).
append(store.clone(), "session", "s1", 0, "raced").await;
// Release the hook so subscribe_after takes the head snapshot and proceeds.
let _ = done_tx.send(());
let mut stream = stream_fut.await.unwrap();
let env = stream.next().await.unwrap().unwrap();
assert_eq!(env.payload_text, "raced");
// Crucially: assert this event appears EXACTLY ONCE (no dupe from backfill+listener overlap).
assert!(stream.next().now_or_never().flatten().is_none());
```

Test 2 (the existing bursts test): keeps probabilistic burst pattern as a smoke for non-pathological loads. Both tests must pass.

- [x] **Step 14: Implement Lagged recovery in the consumer helper.**

There's no good "automatic" recovery inside `subscribe_after` itself — recovering means starting a fresh subscription. Add `crates/store/src/resilient_subscribe.rs`:

```rust
pub fn resilient_subscribe<S: DomainEventStore + 'static>(
    store: Arc<S>, agg_type: String, agg_id: String, since: u64
) -> impl Stream<Item = Result<EventEnvelope, EventStoreError>> {
    // wraps subscribe_after with:
    //  - on Lagged: poll load_events every 500ms until caught up, then re-subscribe
    //  - emits a tracing event on each transition
    // returns events in order with no duplicates, keying off last seq seen
    /* ... */
}
```

Consumers (SessionPane pump task, SessionSupervisor) use `resilient_subscribe` instead of raw `subscribe_after`.

**Livelock guard.** The retry counter MUST NOT reset when the cooldown period ends — otherwise sustained burst traffic loops `subscribe → lag → cooldown → resubscribe → lag → cooldown` indefinitely with no escalation. Reset rule:

```text
state RetryState {
  lagged_retries_in_window: u32,
  window_start: Instant,
  last_clean_recv: Instant,
}

on Lagged: lagged_retries_in_window += 1; (no reset)
on clean recv: if last_clean_recv.elapsed() >= QUIET_PERIOD (10s)
                 then { lagged_retries_in_window = 0; window_start = now() }
                 update last_clean_recv = now()
on K=3 retries within 30s: switch to polling for 30s cooldown (counter NOT reset)
on cooldown end: re-subscribe (counter NOT reset; only sustained QUIET_PERIOD of clean recvs resets it)
on K=6 retries within 60s: escalate — emit `EventStoreError::PersistentLag` and surface to the consumer
                            (the SessionPane pumps to a "lagging" overlay; the supervisor logs and pauses
                            autonomy for that aggregate until operator intervention).
```

Test: `crates/store/tests/resilient_subscribe_livelock.rs` — drive a burst that sustains for 90s, assert the helper escalates to `PersistentLag` on the 6th retry rather than cycling forever.

- [x] **Step 15: Observability — counters and spans.**

In `crates/eventsourcing/src/tracing.rs` (or a new `crates/store/src/metrics.rs` module), declare:

```rust
pub static SUBSCRIBE_AFTER_LAGGED_RECOVERY: Lazy<Counter<u64>> = ...;
pub static SUBSCRIBE_AFTER_BACKFILL_ROWS: Lazy<Histogram<u64>> = ...;
pub static SUBSCRIBE_AFTER_HEAD_LAG: Lazy<Histogram<u64>> = ...;
pub static APPEND_BATCH_SIZE: Lazy<Histogram<u64>> = ...;
pub static SESSION_EVENT_PAYLOAD_TRUNCATED: Lazy<Counter<u64>> = ...;
pub static SESSION_EVENT_BLOB_SPILLED: Lazy<Counter<u64>> = ...;
pub static SESSION_RESPAWN_TOTAL: Lazy<Counter<u64>> = ...; // slice 4 wires this
pub static SESSION_FALLBACK_TO_SESSION_ID: Lazy<Counter<u64>> = ...; // slice 4 wires this
```

Backend: keep using whatever `crates/eventsourcing/src/tracing.rs` already wires (likely `metrics` or `tracing` events). `rg "Counter::|Histogram::" crates/` to find the existing pattern; match it.

In `subscribe_after`, wrap the logic in `tracing::info_span!("subscribe_after", since_sequence = since, head_at_subscribe = head, replayed_count = backfill.len(), aggregate_id = %agg_id)`.

- [x] **Step 16: Throughput benchmark gate.**

`crates/store/benches/subscribe_after_throughput.rs` (Criterion):

```rust
fn bench_one_agent_500_events_10_turns(c: &mut Criterion) {
    c.bench_function("1ag_500e_10t_4sub", |b| b.iter_custom(|iters| { /* ... */ }));
}
criterion_group!(benches, bench_one_agent_500_events_10_turns);
criterion_main!(benches);
```

The benchmark drives 1 producer × 5000 events (10 turns × 500 events) with 4 concurrent subscribers reading from seq 0, asserts every subscriber sees all 5000 in order, measures wall-clock.

Add `crates/store/Cargo.toml` `[dev-dependencies] criterion = "0.5"` and `[[bench]] name = "subscribe_after_throughput" harness = false`.

Gate: p95 < 2s. The CI bench workflow (`.github/workflows/bench.yml`) runs `cargo bench -p nephila-store --bench subscribe_after_throughput` (NOT `--quick` — `--quick` produces too few samples for a meaningful p95) and fails the workflow when p95 ≥ 2s. The merge check on this PR is blocked until the workflow passes; there is no `(or manual)` escape hatch. For local iteration, `cargo bench ... -- --quick` is fine for fast feedback but not for the gate.

- [x] **Step 17: Snapshot trigger policy (split between connector and store).**

The spec gives two triggers; they live in different layers because `subscribe_after` is consumer-facing and the connector is producer-facing.

**Trigger A — `SessionEnded` (producer-side, in connector).** In `crates/connector/src/session.rs`, when the writer/reader cooperative shutdown reaches `SessionEnded`, before returning from `shutdown()`, materialize the final `Session` state by replaying its events through `Session::apply_envelope` and call `store.save_snapshot(&Snapshot { aggregate_type: "session", aggregate_id: session_id.to_string(), sequence: last_seq, state: serde_json::to_value(&session)?, timestamp: Utc::now() })`. This guarantees terminated sessions always have a snapshot for the prune policy.

**Trigger B — first-`subscribe_after`-after-threshold (consumer-side, in store).** Inside `subscribe_after` (after the head snapshot, before returning the stream), if a snapshot for `(agg_type, agg_id)` is missing or is older than `head_at_subscribe - SNAPSHOT_INTERVAL` (`SNAPSHOT_INTERVAL = 1000`), spawn a background task that replays the aggregate from the last snapshot (or default state) and writes a fresh snapshot. The spawn is fire-and-forget — the consumer's stream is not blocked. Use a per-aggregate `dashmap::DashMap<(String,String), Mutex<()>>` to ensure only one such task runs at a time per aggregate (skip if the lock is held).

Putting the threshold trigger in the connector would be wrong: the connector has no visibility into when consumers start reading, and the connector for a still-running session never "finishes" emitting events — it would have to poll its own `last_seq` against `last_snapshot_seq`, which is exactly the work the consumer already does when `subscribe_after` is called.

Replay path (used by SessionPane on focus, supervisor on respawn): `Session::default_state() -> snapshot.state -> apply_envelope(events[snapshot.sequence + 1..])`.

Tests:

- Trigger A: drive a session through `SessionEnded`, assert a snapshot exists at `sequence == last_seq`.
- Trigger B: append 1500 events without a `SessionEnded`, call `subscribe_after`, wait for the background task to settle, assert exactly one snapshot was written and that subsequent `subscribe_after` calls do NOT spawn another (the per-aggregate Mutex prevents thundering-herd snapshotting).

- [x] **Step 17b: Replay-determinism property test.**

`crates/eventsourcing/tests/session_replay_determinism.rs` — proptest:
- generate an arbitrary `Vec<SessionEvent>` (using a `proptest::Strategy` that emits valid sequences: SessionStarted, then any prompt/turn/checkpoint events, optionally SessionEnded);
- compute `s1 = events.iter().fold(Session::default_state(), |s, e| s.apply(e))`;
- pick a random `k` in `0..events.len()`, take a snapshot of `events[..k].apply_all()`;
- compute `s2 = snapshot.apply_all(&events[k..])`;
- assert `s1 == s2` for all generated sequences (replay determinism invariant).
Run: `cargo test -p nephila-eventsourcing --test session_replay_determinism`. Expected: PASS for ≥256 random sequences.

- [x] **Step 18: Default retention policy in `SessionRegistry` (placeholder for slice 4).**

Add a stub `crates/store/src/retention.rs::session_retention_policy(store, session_id)` that on `SessionEnded`: takes a final snapshot, then `prune_aggregate("session", session_id, last_seq - 200)`. The actual scheduler (daily sweep) lands in slice 4 alongside the registry.

Unit test: simulate a 1000-event session, run the policy, assert events 0..=799 are gone and 800..=999 remain.

- [x] **Step 18a: Suppress duplicate `BusEvent::CheckpointSaved` emission from the MCP handler.**

Per spec: during the transition (slices 1b–5), `CheckpointReached` is emitted from the connector reader only — NOT from the MCP handler. The MCP handler currently emits `BusEvent::CheckpointSaved`; without this step, BOTH paths fire for every checkpoint and the TUI would render a double HITL modal (the existing modal handler doesn't deduplicate).

Find the MCP-handler emission with `rg "CheckpointSaved" crates/`. In every emission site, gate the emission:

```rust
#[allow(deprecated)]  // BusEvent::CheckpointSaved is deprecated in slice 5, removed in slice 6
let _was_emitted_in_legacy_world = (); // intentional no-op: connector reader is the sole producer post-1b
// Old code that did `bus.send(BusEvent::CheckpointSaved { .. })` is removed here.
```

The TUI's `handle_bus_event` for `BusEvent::CheckpointSaved` is left in place but becomes unreachable after this step — flagged in slice 6's deletion. Add a `tracing::warn!("legacy CheckpointSaved bus event observed — should be unreachable after slice-1b")` to that arm to catch any missed emission site.

Verification test: drive a checkpoint round-trip end-to-end against fake_claude with the MCP stub, count `BusEvent::CheckpointSaved` emissions on the bus — expected 0. Count `SessionEvent::CheckpointReached` events in the store — expected exactly 1 per checkpoint.

- [x] **Step 19: Cross-process lockfile (placeholder; full wiring in slice 4).**

Add `crates/store/src/lockfile.rs` with a `WorkdirLock` RAII type that `flock`s `~/.nephila/<workdir-hash>.lock` on construction and releases on drop. Slice 4 wires it into `bin/orchestrator.rs::main`. Slice 1b only ships the type with unit tests (two concurrent `WorkdirLock::acquire` against the same hash → second returns error).

- [x] **Step 20: Final verify and commit.**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo bench -p nephila-store --bench subscribe_after_throughput -- --quick`. Expected: green; bench p95 < 2s.

Commit: `slice-1b: Session aggregate, subscribe_after/append_batch/prune, blob spill, observability, throughput gate`.

---

## Task 4: Slice 2 — Human prompt injection

**Purpose:** Operator types into the SessionPane, message reaches claude, response renders inline. Adds the multi-line input box, vim-flavored input/normal mode, Ctrl+Enter submit, and full `HumanPromptQueued`/`HumanPromptDelivered` round-trip.

**Files:**
- Create: `crates/tui/src/panels/session_pane/input.rs` — input-box state and key routing
- Modify: `crates/tui/src/panels/session_pane.rs` — pane gains an input region; mode state; pump task
- Modify: `crates/tui/src/lib.rs` — global key dispatch routes Enter on agent-tree row to "focus session pane" instead of (or in addition to) `attach_agent_session`
- Modify: `crates/tui/src/layout.rs` — new `compute_with_session_focus(area)` returning the 20/60/20 three-column layout
- Modify: `crates/tui/src/panels/agent_tree.rs` — add `last_session_event` field and per-agent activity glyph rendering (Step 7)
- Modify: `crates/tui/Cargo.toml` — add `tui-textarea = "0.x"` (pin a recent version known to work with the workspace's `ratatui` minor)
- Create: `crates/tui/tests/session_input_e2e.rs`

- [x] **Step 1: Add `tui-textarea` and bench-test it compiles against the workspace `ratatui`.**

Run: `cargo build -p nephila-tui` after the dep add. Resolve any `ratatui` version conflict by pinning the same minor. If `tui-textarea` is incompatible, fall back to a hand-rolled multi-line buffer (a `Vec<String>` plus a cursor; ~150 lines). Document the choice in a code comment.

**Outcome:** `tui-textarea = "0.7"` pulls in `ratatui 0.29` (incompatible with workspace's `ratatui 0.30`). The `tui-textarea-2` fork at `0.11` works against `ratatui 0.30` but is a non-canonical crate name. Per the explicit fallback in this step, we hand-roll a `Vec<String>` + cursor multi-line buffer in `crates/tui/src/panels/session_pane/input.rs`. No workspace dep added.

- [x] **Step 2: Write the failing TUI e2e test.**

`crates/tui/tests/session_input_e2e.rs`:

```rust
#[tokio::test]
async fn typing_in_pane_emits_human_prompt_events() {
    // Spin up an in-memory store, ClaudeCodeSession against fake_claude,
    // a SessionPane focused on that session, and a TestBackend.
    // Drive: focus pane (Tab), enter input mode (i), type "hello", Ctrl+Enter.
    // Assert (event-driven, NOT wall-clock):
    //   Use `subscribe_after("session", session_id, 0)` to consume events in order;
    //   assert the first 4 events are exactly
    //   [HumanPromptQueued, HumanPromptDelivered, AssistantMessage{is_final:true}, TurnCompleted]
    //   regardless of wall-clock timing. Then call `pane.snapshot_text()` and assert
    //   it contains "YOU → hello". The single 5s timeout protects against hangs;
    //   per-event windows are not asserted.
    //
    // tokio::time::timeout(Duration::from_secs(5), async {
    //     let mut stream = subscribe_after(store.clone(), "session".into(), session_id, 0);
    //     let mut events = Vec::new();
    //     while events.len() < 4 {
    //         let env = stream.next().await.unwrap()?;
    //         events.push(serde_json::from_value::<SessionEvent>(env.payload)?);
    //     }
    //     assert!(matches!(events[0], SessionEvent::HumanPromptQueued { .. }));
    //     assert!(matches!(events[1], SessionEvent::HumanPromptDelivered { .. }));
    //     assert!(matches!(events[2], SessionEvent::AssistantMessage { is_final: true, .. }));
    //     assert!(matches!(events[3], SessionEvent::TurnCompleted { .. }));
    //     Ok::<_, color_eyre::Report>(())
    // }).await.expect("events did not arrive within 5s")?;
    // assert!(pane.snapshot_text().contains("YOU → hello"));
}
```

Run: `cargo test -p nephila-tui --test session_input_e2e`. Expected: FAIL.

- [x] **Step 3: Implement input/normal mode state machine.**

`crates/tui/src/panels/session_pane/input.rs`:

```rust
pub enum PaneMode { Normal, Input }

pub struct InputState {
    pub area: tui_textarea::TextArea<'static>,
    pub mode: PaneMode,
}

impl InputState {
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> InputAction {
        use crossterm::event::{KeyCode, KeyModifiers};
        match (self.mode, key.code, key.modifiers) {
            (PaneMode::Normal, KeyCode::Char('i'), _) => { self.mode = PaneMode::Input; InputAction::None }
            (PaneMode::Normal, KeyCode::Char('q'), _) => InputAction::ClosePane,
            (PaneMode::Normal, KeyCode::Char('j'), _) => InputAction::ScrollDown(1),
            (PaneMode::Normal, KeyCode::Char('k'), _) => InputAction::ScrollUp(1),
            (PaneMode::Normal, KeyCode::Esc, _) => InputAction::ReturnToGlobal,
            (PaneMode::Input, KeyCode::Esc, _) => { self.mode = PaneMode::Normal; InputAction::None }
            (PaneMode::Input, KeyCode::Enter, KeyModifiers::CONTROL) => {
                let text = self.area.lines().join("\n");
                self.area = tui_textarea::TextArea::default();
                self.mode = PaneMode::Normal;
                InputAction::Submit(text)
            }
            (PaneMode::Input, _, _) => { self.area.input(key); InputAction::None }
            _ => InputAction::None,
        }
    }
}

pub enum InputAction { None, Submit(String), ClosePane, ReturnToGlobal, ScrollUp(u16), ScrollDown(u16) }
```

Unit-test the matrix: every (mode, key) combo above with the expected `InputAction`.

- [x] **Step 4: Wire pane → session.send_turn.**

`SessionPane` holds `Arc<ClaudeCodeSession>`. On `InputAction::Submit(text)`, spawn `tokio::spawn(async move { session.send_turn(PromptSource::Human, text).await })`. Don't `await` inline — keeps the TUI tick loop responsive.

- [x] **Step 5: Per-agent pump task lifecycle.**

Spec §SessionPane.Pump-task lifecycle: one pump per agent, started when `SessionStarted` is observed by the registry, ended on `SessionEnded`. Pane reads from the per-agent buffer.

Implement `SessionRegistry::on_session_started` — slice 4 owns the registry but slice 2 stubs it: when the demo bin starts a session, manually start a pump that calls `resilient_subscribe(store, "session", session_id, 0)` and pushes events into a `Mutex<VecDeque<RenderedRow>>` shared with the pane. **No cap on the VecDeque** (per spec).

- [x] **Step 6: Layout — three-column variant.**

Edit `crates/tui/src/layout.rs`:

```rust
pub struct AppLayoutWithSession { pub agent_tree: Rect, pub session_pane: Rect, pub event_log: Rect, pub hotkey_bar: Rect }

impl AppLayoutWithSession {
    pub fn compute(area: Rect) -> Self {
        let v = Layout::vertical([Constraint::Min(10), Constraint::Length(2)]).split(area);
        let h = Layout::horizontal([Constraint::Percentage(20), Constraint::Percentage(60), Constraint::Percentage(20)]).split(v[0]);
        Self { agent_tree: h[0], session_pane: h[1], event_log: h[2], hotkey_bar: v[1] }
    }
}
```

Switch between `AppLayout::compute_with_focus` and `AppLayoutWithSession::compute` based on a new `App::session_focus: Option<AgentId>` field.

- [x] **Step 7: Per-agent activity glyph in agent tree.**

Spec §SessionPane.Layout: each agent row gets a glyph reflecting its last `SessionEvent` type. Add `last_session_event: Option<&'static str>` to `AgentTreeNode` (kind str "running", "checkpoint", "crashed", etc.). The pump task updates it on every received event before storing the row. Render glyph in `crates/tui/src/panels/agent_tree.rs` next to the agent name.

**Sync model:** the pump task does NOT mutate `AgentTreeNode` directly. Each pump task owns an `mpsc::Sender<AgentActivityUpdate>` (bound 16) consumed by the TUI tick loop on the main thread. `AgentActivityUpdate { agent_id: AgentId, glyph: char, last_event_kind: &'static str }`. The TUI's main `App` holds the `mpsc::Receiver` and drains all pending updates inside its `tick()` before each render. This prevents data races on `AgentTreeNode` and matches the existing `BusEvent` pattern. Drops on full are acceptable — only the latest glyph matters; intermediate glyphs are visual transient state.

- [x] **Step 8: HITL flow uses the existing modal.**

When the pump observes `CheckpointReached { interrupt: Some(InterruptSnapshot::Hitl{question, options}), .. }`, push a `BusEvent::HitlRequested` (or call `App::open_hitl_modal` directly) — the existing `Modal::HitlResponse` flow at `crates/tui/src/modal.rs:86-117` handles the popup. The pane shows a marker row "↳ awaiting human input" but does not try to render the question inline.

Test: drive fake_claude with a scenario that emits `CheckpointReached(Hitl)`, assert the modal opens.

- [x] **Step 9: Run e2e until green.**

Run: `cargo test -p nephila-tui --test session_input_e2e -- --nocapture`. Expected: PASS.

- [x] **Step 10: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit: `slice-2: human prompt injection — input box, vim modes, three-column layout, per-agent pump`.

---

## Task 5: Slice 3 — Checkpoint-driven autonomy

**Purpose:** Replace today's process-exit-driven respawn with `(CheckpointReached, TurnCompleted)` pair detection; supervisor calls `session.send_turn(Agent, ...)` over the live channel; `Pause` SIGSTOPs the child; `RestartTracker` recalibrated for crash-not-exit semantics.

**Files:**
- Modify: `crates/lifecycle/src/lifecycle_supervisor.rs` — rewrite as a `SessionSupervisor` driven by `subscribe_after` instead of `BusEvent`
- Modify: `crates/lifecycle/src/supervisor.rs` — `RestartTracker::record_restart` semantics unchanged, but defaults documented as "crashes, not exits"; new defaults `max_restarts=5`, `restart_window_secs=600`
- Modify: `crates/connector/src/session.rs` — `pause()` and `resume()` methods that SIGSTOP/SIGCONT the child
- Move: the `compose_prompt` body from `bin/src/orchestrator.rs:21` into `crates/lifecycle/src/lifecycle_supervisor.rs::compose_next_prompt`
- Modify: `crates/core/src/config.rs` — `SupervisionConfig` defaults change; document the breaking change
- Create: `bin/src/session_registry.rs` — Task 5 lands a **stub** `SessionRegistry` exposing only `subscribe_session_started()` (returns a `broadcast::Receiver<AgentId>` whose sender is owned by an as-yet-empty placeholder); the real registry lands in Task 6 step 3 (which fills in `ensure_session`, `on_crash`, etc.). Task 5's stub is what unblocks the supervisor's `tokio::select!` — without it, Task 5 has a forward dependency on Task 6 across batches. The stub goes in this file so Task 6 only adds methods, never moves code between files.
- Create: `crates/lifecycle/tests/checkpoint_pairing.rs` — property test for the (Checkpoint, Completed) state machine

- [ ] **Step 1: Write property test for (Checkpoint, TurnCompleted) pairing.**

`crates/lifecycle/tests/checkpoint_pairing.rs`:

```rust
// proptest: arbitrary interleavings of CheckpointReached, TurnCompleted, send_turn_failure events.
// Drive a SessionSupervisor stub against an in-memory store + fake send_turn channel.
// Assertions:
//   - send_turn(Agent, ...) is never called before TurnCompleted for the active turn.
//   - TurnAborted does not auto-trigger send_turn.
//   - PromptDeliveryFailed logs a warning but does not block the next CheckpointReached cycle.
```

Run: `cargo test -p nephila-lifecycle --test checkpoint_pairing`. Expected: FAIL.

- [ ] **Step 2: Implement the supervisor state machine.**

Per session, track:

```rust
struct PerSession {
    last_checkpoint: Option<CheckpointReachedSnapshot>,
    awaiting_turn_completion: Option<TurnId>,
    phase: SessionPhase, // mirror of aggregate
}
```

State transitions on each `SessionEvent`:

```
on CheckpointReached(ev):
  last_checkpoint = Some(ev)
  // do not act yet; wait for TurnCompleted for the same turn

on TurnCompleted(turn_id, stop_reason):
  if let Some(cp) = last_checkpoint.take() {
    match cp.interrupt {
      None | Some(Drain) => issue SessionEnded path (call session.shutdown())
      Some(Hitl)         => mark phase=WaitingHitl, do not auto-prompt
      Some(Pause)        => session.pause() (SIGSTOP), wait for resume command
      None (post-task)   => prompt = compose_next_prompt(agent_state); session.send_turn(Agent, prompt)
    }
  } else {
    // turn closed without a checkpoint — this is the steady state for autonomous turns
    // that already completed their checkpoint earlier. Compose next prompt.
    prompt = compose_next_prompt(agent_state); session.send_turn(Agent, prompt)
  }

on TurnAborted(...):  // recoverable; do not auto-prompt
  log warn; surface to operator via BusEvent
on PromptDeliveryFailed(...):
  log warn
on SessionCrashed(...):
  ask SessionRegistry to respawn (slice 4 wires it; slice 3 emits a placeholder BusEvent)
```

- [ ] **Step 3: SIGSTOP/SIGCONT in the connector.**

`crates/connector/src/session.rs`:

```rust
impl ClaudeCodeSession {
    pub async fn pause(&self) -> Result<(), ConnectorError> {
        #[cfg(unix)] {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            let pid = self.pid().ok_or(ConnectorError::Process { exit_code: None, stderr: "no pid".into() })?;
            kill(Pid::from_raw(pid), Signal::SIGSTOP).map_err(...)?;
        }
        Ok(())
    }
    pub async fn resume_paused(&self) -> Result<(), ConnectorError> { /* SIGCONT */ }
}
```

Document: Pause halts the OS process, not just the prompt loop. Any in-flight tool call from claude is also stopped — be careful invoking Pause mid-turn. Spec already covers this.

Test: spawn fake_claude, pause, observe no further events for 1s, resume, observe events resume.

- [ ] **Step 4: Move `compose_prompt` to lifecycle.**

Cut/paste `compose_prompt` from `bin/src/orchestrator.rs:21` to `crates/lifecycle/src/lifecycle_supervisor.rs::compose_next_prompt`. Update its callers in the orchestrator to call the lifecycle helper instead. The orchestrator no longer needs to know about prompt composition — it only spawns and routes commands.

- [ ] **Step 5: Recalibrate `RestartTracker` defaults and update sites.**

`crates/core/src/config.rs::SupervisionConfig::default()` — change `max_restarts` from 3 → 5 and `restart_window_secs` from 60 → 600. Document in `crates/lifecycle/src/supervisor.rs` doc-comment: "Counts crashes (not turn exits). Defaults assume each crash takes ~30s of recovery."

`SessionCrashed` → `record_restart`. `SessionEnded` (clean) → reset tracker (new method `RestartTracker::reset()`). `TurnAborted` → no-op.

- [ ] **Step 6: Migrate from BusEvent subscription to subscribe_after.**

`LifecycleSupervisor::new` takes `Arc<dyn DomainEventStore>` instead of (or in addition to) `broadcast::Receiver<BusEvent>`. The supervisor's main loop, today reading `BusEvent`, becomes:

```rust
async fn run(&self) -> Result<(), SupervisorError> {
    let mut joins = JoinSet::new();
    // Initial population from existing agents
    for agent_id in self.registry.list_active_agents().await? {
        joins.spawn(self.clone().resilient_subscribe(agent_id));
    }
    // Dynamic-add loop: subscribe to a new-agent notification stream from SessionRegistry
    let mut new_agents = self.registry.subscribe_session_started();
    loop {
        tokio::select! {
            // New agent appeared at runtime — add it to the JoinSet
            Ok(agent_id) = new_agents.recv() => {
                joins.spawn(self.clone().resilient_subscribe(agent_id));
            }
            // An existing agent's subscription ended (clean or error)
            Some(_done) = joins.join_next() => { /* log; loop */ }
            else => break,
        }
    }
    Ok(())
}
```
The `SessionRegistry::subscribe_session_started()` method returns a `broadcast::Receiver<AgentId>`. Task 5 ships the **stub** registry: a struct holding `started_tx: broadcast::Sender<AgentId>` plus a `pub fn fire_started(&self, agent_id: AgentId)` test seam that the supervisor property test calls directly. Task 6 step 3 turns the stub into the real registry by adding `ensure_session(agent)` (which calls `self.started_tx.send(agent.id)` instead of the test seam) plus `on_crash`, `sessions: DashMap`, etc. Because the stub already lives in `bin/src/session_registry.rs`, Task 6 only ADDS methods to an existing struct — it never moves code between files, so the slice-3 / slice-4 batch boundary stays clean. This preserves today's `BusEvent::AgentSessionReady` semantics under the new event-stream supervisor.

`BusEvent::CheckpointSaved` is now redundant for supervisor — keep the bus emission for slice 5 but the supervisor reads from `subscribe_after`.

- [ ] **Step 7: Run property test until green.**

Run: `cargo test -p nephila-lifecycle --test checkpoint_pairing`. Expected PASS.

- [ ] **Step 8: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit: `slice-3: checkpoint-driven autonomy — (CheckpointReached, TurnCompleted) pairing, Pause SIGSTOP, RestartTracker recalibrated`.

---

## Task 6: Slice 4 — Crash + resume

**Purpose:** Reader emits `TurnAborted` then `SessionCrashed` on EOF; `SessionRegistry` subscribes to crashes and respawns with `--resume`, falling back to `--session-id` for never-persisted sessions; cross-process lockfile prevents double-resume; consumers reconnect via `subscribe_after(last_seen)`.

**Files:**
- Create: `bin/src/session_registry.rs` — `SessionRegistry` with crash subscription + respawn loop
- Modify: `crates/connector/src/session.rs` — `resume(cfg, session_id)` method with `--resume` then fallback to `--session-id` on stderr match
- Modify: `bin/src/orchestrator.rs` — own a `SessionRegistry` instead of spawning per turn; on startup scan active agents and `resume` each; acquire `WorkdirLock` before doing anything
- Modify: `crates/connector/src/session.rs::reader_task` — on EOF, append `TurnAborted` (if open turn) then `SessionCrashed` in order; on parse error, append `SessionCrashed { reason: "unparseable frame" }`
- Modify: `crates/core/src/agent_event.rs` (or wherever AgentEvent lives) — add `AgentConfigSnapshot` and `AgentSessionAssigned` variants
- Modify: `crates/core/src/agent.rs` — add `session_id: Option<SessionId>` and `last_config_snapshot: Option<AgentConfigSnapshot>` fields to `Agent`; extend the reducer to apply `AgentSessionAssigned` (sets `session_id`) and `AgentConfigSnapshot` (sets `last_config_snapshot`)
- Create: `bin/tests/respawn_e2e.rs` — kill fake_claude mid-turn, assert respawn

- [ ] **Step 1: Implement reader-task crash sequencing.**

In `ClaudeCodeSession::reader_task`, the EOF/parse-error branches must produce events in the right order — `TurnAborted` *before* `SessionCrashed`, in a single `append_batch` call so they're ordered atomically.

```rust
async fn on_terminal_eof(&mut self) {
    let mut events = Vec::new();
    if let Some(turn_id) = self.open_turn.lock().await.take() {
        events.push(SessionEvent::TurnAborted { turn_id, reason: "session_crashed".into(), ts: Utc::now() });
    }
    events.push(SessionEvent::SessionCrashed {
        reason: "stdout EOF".into(),
        exit_code: self.child_exit_code().await,
        ts: Utc::now(),
    });
    // Crash-event append must NOT be silently dropped — the SessionRegistry's
    // crash subscriber depends on observing SessionCrashed to trigger respawn.
    if let Err(e) = self.append_batch(events).await {
        // Surface to operators and to a side channel so registry doesn't hang.
        tracing::error!(
            agent_id = %self.agent_id,
            session_id = %self.session_id,
            error = ?e,
            "failed to append crash events; registry will not see SessionCrashed via store"
        );
        metrics::counter!("session.crash_append_failed_total", "agent_id" => self.agent_id.to_string()).increment(1);
        // Best-effort fallback: send on the registry's out-of-band crash channel
        // (a `mpsc::Sender<AgentId>` owned by SessionRegistry, exposed via
        // SessionConfig.crash_fallback_tx). The registry treats this as equivalent
        // to seeing SessionCrashed in the event stream; idempotency guard in
        // on_crash deduplicates if both paths fire.
        if let Some(tx) = &self.crash_fallback_tx {
            let _ = tx.try_send(self.agent_id);
        }
    }
}
```

Test: drive fake_claude with `--scenario crash_mid_turn`, assert event log contains `[TurnAborted, SessionCrashed]` in that order.

**Shutdown disambiguation:** `ClaudeCodeSession` holds an `Arc<AtomicBool> shutting_down` (set by `shutdown()` before sending SIGTERM). The reader's EOF handler reads `shutting_down`:
- If `false`: emit `TurnAborted` (if open) + `SessionCrashed { reason: "EOF", ... }` as today.
- If `true`: emit nothing — `shutdown()` itself appends `SessionEnded` after reader drain.
This eliminates the SessionCrashed-then-SessionEnded race on clean shutdown. The slice-1a `Drop` pseudocode reference must also set `shutting_down` to `true` before SIGTERM (back-port note for the slice-1a code path; flag for Task 7's cleanup verification).

- [ ] **Step 2: Implement `ClaudeCodeSession::resume`.**

```rust
pub async fn resume(cfg: SessionConfig, session_id: SessionId) -> Result<Self, ConnectorError> {
    // 1. Try claude --resume <id>. Capture stderr to a buffer in a separate task.
    // 2. If the process exits within 2s with (exit_code != 0) AND (stderr matches
    //    RESUME_NOT_FOUND_STDERR_PATTERN), fall back to: claude --session-id <id>.
    //    The regex is a constant `RESUME_NOT_FOUND_STDERR_PATTERN` defined in
    //    `crates/connector/src/resume.rs`, populated from the spike artifact captured
    //    in Task 1 step 4b. Gate the fallback on the combination of (exit code != 0)
    //    AND (stderr matches the pattern) — neither alone — to reduce false positives.
    // 3. On fallback, increment SESSION_FALLBACK_TO_SESSION_ID counter and emit a
    //    metric `session.fallback_to_session_id_total` (Counter, label: agent_id) so
    //    a sustained spike of fallbacks alerts that the regex has drifted. Emit
    //    SessionStarted { resumed: true, ... } with a metadata flag fallback=true.
    // 4. Otherwise emit SessionStarted { resumed: true, ... }.
}
```

Test: spawn a fresh session-id with no on-disk session, confirm fallback path triggers; spawn after a real session existed, confirm `--resume` succeeds without fallback.

- [ ] **Step 3: Implement `SessionRegistry`.**

`bin/src/session_registry.rs`:

```rust
pub struct SessionRegistry {
    sessions: dashmap::DashMap<AgentId, SessionHandle>,
    store: Arc<SqliteStore>,
    blob: Arc<dyn BlobStore>,
}

struct SessionHandle {
    session: Arc<ClaudeCodeSession>,
    pump: tokio::task::AbortHandle,
}

impl SessionRegistry {
    pub async fn start(...) -> Self { /* spawn one task that subscribes to a global filter on SessionCrashed */ }
    pub async fn ensure_session(&self, agent: &Agent) -> Result<Arc<ClaudeCodeSession>, _> { ... }
    async fn on_crash(&self, agent_id: AgentId) {
        // 1. Drop old handle (graceful shutdown if possible)
        // 2. ClaudeCodeSession::resume(cfg, session_id)
        // 3. Insert new handle
        SESSION_RESPAWN_TOTAL.add(1, &[]);
    }
}
```

The "global filter on SessionCrashed" is a separate `subscribe_after` per active session aggregate — there's no cross-aggregate broadcast in the spec. The registry maintains one per-session subscription whose only job is to detect terminal events:

```rust
while let Some(env) = stream.next().await {
    let ev: SessionEvent = serde_json::from_value(env.payload)?;
    match ev {
        SessionEvent::SessionCrashed { .. } => {
            self.on_crash(agent_id).await;
            // After respawn, on_crash inserts a new handle with a fresh aggregate id;
            // this old subscription's stream is for the OLD aggregate and is no
            // longer of interest. Break and let on_crash spawn a new subscription
            // for the new session.
            break;
        }
        SessionEvent::SessionEnded { .. } => {
            // Clean shutdown — registry no longer needs this entry.
            self.sessions.remove(&agent_id);
            break;
        }
        _ => continue, // ignore non-terminal events
    }
}
```

**Termination contract:** the per-session crash-subscription task always exits on either `SessionCrashed` (then `on_crash` spawns a new task for the new session) or `SessionEnded` (then the registry drops the entry). It MUST NOT loop forever — without these arms the task would consume every event indefinitely and accumulate one-task-per-session-ever-spawned.

**Idempotency:** `SessionRegistry::on_crash` holds a per-agent `Mutex<RespawnState>` keyed by `agent_id`. The Mutex is acquired before checking-and-spawning the new session. `RespawnState` records `last_handled_crash_seq: Option<u64>`; if a duplicate crash event with `sequence <= last_handled_crash_seq` arrives (e.g., via Lagged-recovery re-subscription), it is dropped without spawning. Two concurrent crash deliveries serialize on the Mutex; only the first respawns. The cross-process lockfile (step describing it later) handles cross-process safety; this Mutex handles in-process safety.

Test: drive a crash via fake_claude `--scenario crash_mid_turn`; assert respawn happens within 2s and the new session's pump rejoins consumers.

- [ ] **Step 4: Cross-process lockfile.**

In `bin/src/orchestrator.rs::main`, before spawning anything:

```rust
let _lock = nephila_store::lockfile::WorkdirLock::acquire(&workdir)
    .map_err(|e| color_eyre::eyre::eyre!("another nephila is already running here: {e}"))?;
```

Test: spawn the orchestrator twice from a script against the same workdir; second exits non-zero with the lock error in stderr.

- [ ] **Step 5: Restart-on-nephila-restart path.**

`SessionRegistry::on_startup`:

```rust
let active = store.list_agents_in_active_phase().await?;
for agent in active {
    if let Some(session_id) = agent.session_id {
        match ClaudeCodeSession::resume(cfg_from(agent), session_id).await {
            Ok(session) => self.insert(agent.id, session),
            Err(e) => {
                tracing::error!(%e, %agent.id, "resume failed; marking agent failed");
                /* transition agent to failed via store */
            }
        }
    }
}
```

Test: append a complete session log to the store, kill the orchestrator, restart it, assert (a) lockfile released and reacquired, (b) the new orchestrator's `SessionRegistry` has a session for each previously-active agent, (c) consumers can `subscribe_after(last_seen)` and see no gaps.

**Agent fields added in this slice (Step 5a, must precede step 5).**

```rust
// crates/core/src/agent.rs
pub struct Agent {
    /* existing fields */
    pub session_id: Option<SessionId>,
    pub last_config_snapshot: Option<AgentConfigSnapshot>,
}

// crates/core/src/agent_event.rs
pub enum AgentEvent {
    /* existing variants */
    AgentSessionAssigned { session_id: SessionId, ts: DateTime<Utc> },
    AgentConfigSnapshot   { snapshot: AgentConfigSnapshot, ts: DateTime<Utc> },
}

// Reducer arms:
//   AgentSessionAssigned -> agent.session_id = Some(session_id);
//   AgentConfigSnapshot   -> agent.last_config_snapshot = Some(snapshot);
```

`SessionRegistry::ensure_session` emits `AgentSessionAssigned` to the `Agent` aggregate immediately after `ClaudeCodeSession::start` succeeds, so the `Agent` reducer reflects the binding before any other consumer reads it.

**SessionConfig persistence.** `SessionConfig`'s non-volatile fields (`working_dir`, `mcp_endpoint`, `permission_mode`, `claude_binary`) are persisted in the `Agent` aggregate as `AgentConfigSnapshot` (above), emitted on `Agent::Configure`. `cfg_from(agent: &Agent) -> SessionConfig` reads from `agent.last_config_snapshot`. If the agent predates the snapshot event (existing nephila installs upgrading), `cfg_from` falls back to defaults sourced from `bin/src/orchestrator.rs` (which seeds from CLI args). Emit a `tracing::warn!` on fallback so operators see which agents need re-configure.

Test (`crates/core/tests/agent_session_assignment.rs`): fresh agent has `session_id == None`; after applying `AgentSessionAssigned`, the field is `Some(...)`; `cfg_from` falls back gracefully when `last_config_snapshot == None`.

- [ ] **Step 6: Crash-during-respawn test.**

Inject the abort via a `#[cfg(test)] abort_after_drop_old: Option<oneshot::Sender<()>>` field on `SessionRegistry`. The `on_crash` code path runs:

```rust
drop(old_handle);
#[cfg(test)]
if let Some(tx) = self.abort_after_drop_old.take() {
    // Signal the test that the drop happened, then await a oneshot recv
    // that the test never fires — simulating a process abort between
    // `drop(old_handle)` and `insert(new_handle)`.
    let _ = tx.send(());
    let (_never_tx, never_rx) = oneshot::channel::<()>();
    let _ = never_rx.await; // never resolves; the test moves on by aborting the runtime
}
let new_handle = ClaudeCodeSession::resume(cfg, session_id).await?;
self.sessions.insert(agent_id, new_handle);
```

Why not `#[cfg(test)]` `panic!`: a panic inside `tokio::spawn` is silently absorbed by the `JoinHandle` (returns `Err(JoinError)` only if explicitly awaited), so the test would always pass regardless of recovery correctness. The oneshot rendezvous gives the test a deterministic point at which `old_handle` is gone and `new_handle` has not been inserted — at that point the test:

1. Asserts the registry has no entry for `agent_id`.
2. Drops the test runtime (forcing all spawned tasks to abort, simulating a process kill).
3. Builds a fresh runtime + `SessionRegistry`, calls `on_startup`, asserts the active agent is resumed cleanly.
4. Asserts no orphan claude processes via `pgrep -af claude` returns empty (gated to Unix and skipped on CI without `pgrep`).

- [ ] **Step 7: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit: `slice-4: crash + resume — TurnAborted+SessionCrashed sequencing, SessionRegistry, lockfile, --resume fallback`.

---

## Task 7: Slice 5 — Cleanup phase 1

**Purpose:** Remove the legacy `-p` spawn path. Hide TTY handoff behind opt-in hotkey `a`. Mark `BusEvent::CheckpointSaved` deprecated. **Do not delete `attach_agent_session`.**

**HITL gate before merging Task 7.** Task 7 deletes the `-p` connector code path — the operator's only fallback if the new stream-json path has a production bug. Before merging:
1. Two consecutive weeks with the new path running in production with no rollback.
2. A documented rollback procedure (revert the slice-7 commit; the legacy `-p` code is recoverable from git for one further release).
3. HITL signoff in the spec appendix as `Slice-5 cleanup approval (YYYY-MM-DD)`.

**Files:**
- Modify: `crates/connector/src/claude_code.rs` — delete `impl TaskConnector for ClaudeCodeConnector` and the `-p` spawn code; keep `resume_interactive` (used by `attach_agent_session`)
- Modify: `crates/connector/src/task.rs` — remove `TaskHandle::ClaudeCode` variant if unused after the deletion (likely, since the new flow uses `ClaudeCodeSession`)
- Modify: `bin/src/orchestrator.rs` — drop the `compose_prompt → spawn(prompt) → wait(exit)` path; route everything through `SessionRegistry`
- Modify: `crates/tui/src/lib.rs` — re-bind agent-tree Enter to "focus session pane"; bind `a` to `attach_agent_session`; show hotkey hint "a: TTY attach" in the hotkey bar
- Modify: `crates/core/src/event.rs` — `#[deprecated(note = "use SessionEvent::CheckpointReached via subscribe_after; removed in slice 6")] BusEvent::CheckpointSaved`
- Delete: `crates/connector/examples/spike_stream_json.rs` (Task 1 spike artifact)
- Delete: `crates/store/examples/spike_subscribe_after.rs` (Task 1 spike artifact)
- Delete: `bin/src/bin/demo_session.rs` (Task 2 demo bin)
- Create: `docs/release-notes/slice-5.md` — document the `SupervisionConfig` defaults change (slice 3) and the connector flow swap

- [ ] **Step 1: Delete `-p` spawn code.**

Remove `impl TaskConnector for ClaudeCodeConnector` from `crates/connector/src/claude_code.rs`. Keep `resume_interactive` (TTY attach) and `mcp_config_json`. Run `cargo build --workspace` to find every caller — fix them to route through `SessionRegistry`. Expected: orchestrator's old per-turn spawn site is the main caller.

- [ ] **Step 2: Trim `crates/connector/src/task.rs` and `dispatch.rs`.**

If `TaskConnector` is only used by `anthropic_api.rs` and `openai_compatible.rs` (verify with `rg "impl TaskConnector"`), keep the trait. Drop only the `ClaudeCode` variant of `TaskHandle` (and `ProcessHandle` if it was claude-only). If anything else uses it, leave alone.

- [ ] **Step 3: Rewire orchestrator.**

`bin/src/orchestrator.rs::handle_command`:
- `OrchestratorCommand::SpawnAgent` now → `SessionRegistry::ensure_session(agent)`.
- `OrchestratorCommand::Respawn` now → `SessionRegistry::on_crash(agent_id)` plus the existing agent-state transitions.
- Remove the `tokio::spawn(async move { handle.wait().await ... })` that listened for process exit per agent.

- [ ] **Step 4: TUI hotkey rebind.**

`crates/tui/src/lib.rs`:
- Enter on agent-tree row → `App::focus_session_pane(agent_id)` (slice 2 added the field).
- `a` on agent-tree row → `attach_agent_session(...)` (existing function, unchanged behavior).
- Update the hotkey bar (`crates/tui/src/panels/hotkey_bar.rs`) to show `Enter: pane | a: TTY attach`.

Test: TUI test uses `TestBackend`, presses `a` on a focused agent row, asserts the test scaffolding records that `attach_agent_session` was invoked (mock the function via a trait or feature flag — see `crates/tui/tests/hotkey_routing.rs`).

- [ ] **Step 5: Mark `BusEvent::CheckpointSaved` deprecated.**

```rust
pub enum BusEvent {
    /* ... */
    #[deprecated(since = "<slice-5-version>", note = "Use SessionEvent::CheckpointReached via DomainEventStore::subscribe_after. Removed in slice 6.")]
    CheckpointSaved { agent_id: AgentId, checkpoint_id: CheckpointId },
    /* ... */
}
```

Build will warn at every usage; fix call sites that we own (TUI: prefer reading from the durable log via the per-agent pump). Allow the deprecation warning at the bus emission site itself with `#[allow(deprecated)]` plus a comment "removed in slice 6".

- [ ] **Step 6: Release notes.**

`docs/release-notes/slice-5.md`:

- BREAKING (operator-facing): `SupervisionConfig.max_restarts` default changed from 3 to 5; `restart_window_secs` from 60 to 600. Counts session crashes, not turn exits.
- BREAKING (connector flow): per-turn `claude -p` is gone. Operators previously relying on per-turn process logs should switch to `subscribe_after("session", session_id, 0)` or read the session pane.
- DEPRECATED: `BusEvent::CheckpointSaved` (removed in slice 6).
- NEW: hotkey `a` to fall back to TTY attach for features the embedded pane doesn't yet handle (markdown rendering, slash commands beyond `/clear /compact /cost`, image paste, file-path tab completion).

- [ ] **Step 7: Delete spike + demo artifacts.**

Remove `crates/connector/examples/spike_stream_json.rs`, `crates/store/examples/spike_subscribe_after.rs`, and `bin/src/bin/demo_session.rs`. Update `Cargo.toml` if any `[[example]]` or `[[bin]]` entries reference them. Run `cargo build --workspace --examples` to verify.

- [ ] **Step 8: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit: `slice-5: cleanup — remove -p path, hotkey-a TTY attach, deprecate BusEvent::CheckpointSaved`.

---

## Task 8: Slice 6 — UX parity rollup phase 1

**Purpose:** Markdown rendering, code syntax highlight, slash-command routing for `/clear`, `/compact`, `/cost`. Removes `BusEvent::CheckpointSaved`.

**Files:**
- Create: `crates/tui/src/panels/session_pane/markdown.rs` — wrap `tui-markdown` (or hand-rolled if unsuitable) for rendered assistant message rows
- Create: `crates/tui/src/panels/session_pane/highlight.rs` — code syntax highlight via `syntect` or `ratatui-syntax-highlighting` (whichever the workspace already pulls in; otherwise pick one and document the dependency add)
- Create: `crates/tui/src/panels/session_pane/slash.rs` — parse `/clear /compact /cost`, route to claude via the control protocol (stream-json `ClaudeInput::Control` if claude-codes exposes it; otherwise via stdin sentinel as documented in claude --help)
- Modify: `crates/core/src/event.rs` — delete `BusEvent::CheckpointSaved` and every emission site (TUI no longer reads it; supervisor switched in slice 3)
- Modify: `crates/tui/Cargo.toml` — add `tui-markdown` (or fallback) and a syntax-highlight dep

- [ ] **Step 1: Add `tui-markdown` (or pick a fallback).**

`cargo add tui-markdown -p nephila-tui`. If it doesn't compile against the workspace `ratatui` minor, fall back to a minimal renderer: split assistant message text into paragraphs, detect ` ```lang ... ``` ` fences, render code blocks with the highlight module, render the rest as plain wrapped text.

- [ ] **Step 2: Wire markdown into the pane row renderer.**

`SessionPane::render` for assistant rows: `markdown::render(&row.text, area, buf, &highlight_theme)`.

Test (`crates/tui/tests/markdown_render.rs`): given an assistant message with text "Here is `code`:\n\n```rust\nfn x() {}\n```\n", assert the rendered buffer contains `fn x()` styled with the rust-keyword color.

- [ ] **Step 3: Slash-command routing.**

```rust
match input.trim() {
    "/clear" | "/compact" | "/cost" => session.send_control(input).await?,
    other if other.starts_with('/') => fall_through_to_attach(),
    text => session.send_turn(PromptSource::Human, text.to_owned()).await?,
}
```

`session.send_control` writes a `ClaudeInput::Control { command: "/clear" }` JSON line (verify the exact shape against `claude_codes::protocol`; if the protocol crate doesn't expose a Control variant, write the raw JSON `{"type":"control","command":"clear"}` and pin a comment to the spec). `/clear` resets context in claude (no client-side action needed); `/compact` triggers claude's own compaction; `/cost` returns a tool-result-like response that the pane renders inline.

Tests: drive each slash command against fake_claude (which scripts a canned response per command), assert the resulting events appear in the pane.

- [ ] **Step 4: Delete `BusEvent::CheckpointSaved`.**

Remove the variant. Build will fail at every use site → fix each one. The TUI's `handle_bus_event` has a match arm — delete it. The supervisor (slice 3) doesn't need it. Confirm the bus stops emitting it in `crates/mcp/src/handlers/`-equivalent (find with `rg "CheckpointSaved"`).

- [ ] **Step 5: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit: `slice-6: UX parity rollup phase 1 — markdown, syntax highlight, slash commands, drop BusEvent::CheckpointSaved`.

---

## Task 9: Slice 7 — UX parity rollup phase 2 (deletes attach)

**Purpose:** Interactive permission prompts via control-protocol UI; complete the parity matrix from spec §SessionPane.UX-parity-gap; **delete `attach_agent_session`** and the `a` hotkey.

**Files:**
- Create: `crates/tui/src/panels/session_pane/permission.rs` — modal that handles claude's permission requests over the control protocol
- Modify: `crates/connector/src/session.rs` — read `ContentBlock::PermissionRequest` (or whatever shape the verified protocol uses) and surface as `SessionEvent::PermissionRequested { request_id, action, resource }`; new `send_control` variant for `permission/grant {request_id, decision}`
- Modify: `crates/core/src/session_event.rs` — add `PermissionRequested` and `PermissionResolved` variants
- Modify: `crates/tui/src/lib.rs` — delete `attach_agent_session`, the `a` hotkey, and the `claude_binary` field that powered it (if unused after deletion). Update hotkey bar.
- Delete: `crates/connector/src/claude_code.rs::resume_interactive` (TTY attach's only remaining customer is gone)
- Modify: `bin/src/orchestrator.rs` — drop `claude_binary` field if it was only used by TTY attach

- [ ] **Step 1: Verify the permission-request protocol shape.**

**Permission-mode operator audit (HITL gate before slice 7 merges).**
Before any code in slice 7 lands, perform a permission-mode audit:
1. List all production agents currently using `bypassPermissions` (slices 1-6 default).
2. For each agent, document the actual tools called (extract from `ToolCall` events in the store).
3. Operators sign off on the tool list per agent; mark as `acceptEdits` or stricter going forward.
4. Slice 7 wires per-session permission-mode override into `SessionConfig`; default for new agents becomes `acceptEdits` not `bypassPermissions`.
5. Existing agents keep their current mode (no silent escalation) until operators explicitly migrate.
This step is a HITL artifact — append to the spec appendix as `Permission-mode audit (YYYY-MM-DD)`.

This is HITL because the protocol surface for permission prompts is what we're betting on. Run a manual experiment with a real `claude` binary configured with `--permission-mode default` (not `bypassPermissions`), trigger a tool that requires a permission decision (e.g., a `Bash` call), capture the stream-json frames. Document the exact frame shape in `docs/specs/2026-05-03-claude-session-streaming-design.md` as a §Slice-7 appendix. Verify before writing tests.

- [ ] **Step 2: Add `SessionEvent::PermissionRequested / PermissionResolved`.**

```rust
PermissionRequested { request_id: String, action: String, resource: String, ts: DateTime<Utc> },
PermissionResolved { request_id: String, decision: PermissionDecision, ts: DateTime<Utc> },
```

`PermissionDecision: Allow | Deny | AllowOnce`. Update `Session::apply` for invariant tracking (no state change; pure record).

- [ ] **Step 3: Reader-task integration.**

In `ClaudeCodeSession::reader_task`, match the verified frame shape and emit `PermissionRequested`. New `pub async fn grant_permission(&self, request_id: &str, decision: PermissionDecision)` on `ClaudeCodeSession` writes the corresponding `ClaudeInput::Control` line, then emits `PermissionResolved` upon successful write.

- [ ] **Step 4: Permission modal UI.**

`crates/tui/src/panels/session_pane/permission.rs`: a popup like `Modal::HitlResponse` showing action+resource, with `[Allow] [Allow once] [Deny]` buttons, calling `session.grant_permission(...)` on selection.

When the pane observes `PermissionRequested`, it opens the permission modal automatically (similar to existing HITL flow).

- [ ] **Step 5 (revised): Gate `attach_agent_session` behind a feature flag `legacy-attach` (default off) for one release.**

- Move `attach_agent_session` and `resume_interactive` behind `#[cfg(feature = "legacy-attach")]` in `crates/tui/src/lib.rs`.
- Add `legacy-attach` to `crates/tui/Cargo.toml` `[features]` (not in `default`).
- Add `legacy-attach` to the workspace's optional features so operators who depend on file-path tab-complete or image-paste can build with `cargo build --features tui/legacy-attach`.
- Add a deprecation tracing-warn at startup when the feature is enabled: "legacy-attach is deprecated; the embedded SessionPane covers normal flows. File this issue if you still need attach for tab-complete or image-paste."
- Document in the release notes: feature is removed in the release after next; collect operator feedback during the deprecation window.

**Rationale:** the spec marks tab-complete and image-paste as dropped without replacement. Unconditional deletion at slice 7 is too aggressive; a one-release deprecation window lets operators surface workflows we missed before the code is gone.

- [ ] **Step 6: Schedule final removal.**

Open a tracking issue "Remove legacy-attach feature" referencing the deprecation window. The actual deletion lands in a follow-up plan after one release of operator feedback. The Cleanup phase here only flips the feature off-by-default.

- [ ] **Step 7: Update parity matrix.**

In `docs/specs/2026-05-03-claude-session-streaming-design.md` §SessionPane.UX-parity-gap, update each row's status: "yes" / "yes via slice 6" / "yes via slice 7". The deferred/dropped statuses should now all be cleared.

- [ ] **Step 8: Verify and commit.**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Manual TUI verification: run the orchestrator against a real `claude --permission-mode default`, trigger a permission-gated tool, confirm the modal appears and the decision routes correctly.

Commit: `slice-7: UX parity rollup phase 2 — permission control protocol; delete attach_agent_session`.

---

## Polish [HITL]

After Task 9 completes, run the post-implementation-polish skill against the full diff. Owner: `rust-engineer`.

**Exit criteria (all must hold before this task can flip to `[x]`):**
- `cargo test --workspace` green
- `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo bench -p nephila-store --bench subscribe_after_throughput` p95 < 2s on the CI runner
- HITL reviewer signoff recorded in the spec appendix as `Polish review (YYYY-MM-DD): approved by <name>`
- No outstanding `TODO(slice-N)` or `unimplemented!()` markers introduced by this plan remain in the code (verify with `rg -n 'TODO\(slice-' crates/ bin/` and `rg -n 'unimplemented!\(\)' crates/ bin/`)

**Steps:**

- [ ] **CI bench gate audit.** Verify the slice-1b throughput bench is wired to a CI workflow (`.github/workflows/bench.yml` or equivalent) and is set to fail the workflow if p95 > target — no `(or manual)` escape hatch.

- [ ] **Review round 1: correctness.** Walk every new `pub fn` in `crates/connector`, `crates/core`, `crates/store`, `crates/lifecycle`. For each, confirm the function does what its doc-comment promises and that all error paths are handled. Fix any drift. Output: a one-paragraph summary committed to `docs/release-notes/polish-round-1.md`.

- [ ] **Review round 2: testability.** Audit the new test files for flakiness (no wall-clock sleeps; explicit synchronization at all rendezvous points), and audit production code for hidden test seams that should be `#[cfg(test)]` only. Fix any leaks. Output: `docs/release-notes/polish-round-2.md`.

- [ ] **Review round 3: cross-cutting concerns.** Tracing instrumentation present on every public async fn that crosses a subsystem boundary (per Conventions §Tracing). Counters wired through the same backend that the rest of the workspace uses. Permission-mode HITL gate from Task 9 step 1 documented in the spec appendix. Output: `docs/release-notes/polish-round-3.md`.

- [ ] **Idiomatic-Rust pass.** Run `cargo clippy --workspace --all-targets -- -W clippy::pedantic -W clippy::nursery` (warnings, not errors). Triage: fix the easy ones, document explicit allows for the rest in a per-crate `lib.rs` comment.

- [ ] **`/cleanup` pass.** Invoke the `/cleanup` skill across the diff. Apply suggestions where they don't conflict with the plan's explicit decisions (e.g., the `EventEnvelope::sequence` "stamped by store" semantics is intentional and must NOT be reverted by cleanup).

- [ ] **AI-comment strip.** Walk the diff one final time. Remove comments that explain WHAT the code does (the code already says that). Keep WHY comments and invariant docs. Run `rg -n '// (this|now|here|just) ' crates/ bin/` as a heuristic for noise; review each hit.

---

## Self-review notes (author)

- Spec coverage: every section of the spec maps to at least one task above. Sections without an explicit task — *Snapshotting*, *Failure modes table*, *Observability*, *Open assumptions* — are folded into specific steps in Tasks 1, 3, and 6 (snapshot trigger, observability counters, lockfile, retention sweep). Crash-during-respawn, supervisor interleaving, and TUI rendering tests are explicit steps.
- Type consistency: `SessionId = Uuid`, `TurnId = Uuid`, `MessageId = String`. `CheckpointId` is the newtype `crate::id::CheckpointId(pub Uuid)` from `crates/core/src/id.rs:14` — the slice-1a draft uses a `String` placeholder, but the real `SessionEvent::CheckpointReached` payload is the newtype (Task 3 step 1's connector path parses the wire string via `Uuid::parse_str`). The `*PromptQueued / *PromptDelivered` split matches the spec's writer-task sequence exactly (Queued before stdin write, Delivered after success, Failed on BrokenPipe). `PromptSource` enum is consistent across slice 1a and 1b.
- Open trade-offs: (a) `EventEnvelope::sequence` semantics — we picked "keep `u64`, document" (Task 3 step 4); revisit if a bug surfaces. (b) `tui-markdown` vs hand-rolled fallback — Task 8 step 1 picks the lower-friction route at compile time. (c) Read pool sizing (4 conns) is a guess; revisit after the throughput bench at Task 3 step 16.
- Known follow-ups not in this plan: persistent broadcast registry GC sweep (only if memory shows up as an issue), aggregate-snapshot cadence beyond v1 thresholds, web UI / Slack consumers (out of scope; the architecture supports them without producer changes by design).
