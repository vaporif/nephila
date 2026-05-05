# Streaming Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or `/team-feature` to implement this plan task-by-task, per the Execution Strategy below. Steps use checkbox (`- [ ]`) syntax — these are **persistent durable state**, not visual decoration. The executor edits the plan file in place: `- [ ]` → `- [x]` the instant a step verifies, before moving on. On resume (new session, crash, takeover), the executor scans existing `- [x]` marks and skips them — these steps are NOT redone. TodoWrite mirrors this state in-session; the plan file is the source of truth across sessions.

**Goal:** Fix two Critical and seven High-severity findings from the multi-reviewer audit of the slice-1..4 branch (`migration-of-wrapper`). The crash-recovery path is the load-bearing repair; security and performance fixes ride alongside.

**Architecture:** Repairs span four areas. (1) The registry crash watcher and the connector-side fallback path interact to recover from `claude` subprocess failures; today the watcher loses crashes on broadcast Lag and the dedup logic double-fires when fallback is exercised — fix by routing through `resilient_subscribe` and restructuring `on_crash` so the per-agent mutex is released around `resume()` (an `in_flight` flag plus a short fallback-replay window provide deterministic dedup that the existing always-locked design could not). (2) The supervisor and connector both need small ordering/filter fixes — `TurnCompleted`/`TurnAborted` must filter by `turn_id`. (3) Two security holes — debug logging of prompts and raw stderr persisted in events — need redaction. (4) Three SQLite-store fixes (pragmas, snapshot replay, read routing) drop the writer thread's contention.

**Tech Stack:** Rust 1.94+ (Tokio async, rusqlite SQLite + WAL, `tokio::sync::broadcast`); the existing `nephila_store::resilient_subscribe` wrapper; existing `read_pool` infrastructure; `tracing`. No new dependencies.

## Execution Strategy

**Subagents.** Default — no spec override. The brainstorming flow was not run because the spec is a validated review report rather than a feature spec. Tasks are mostly disjoint by file but Tasks 1, 2, and 3 all edit `bin/src/session_registry.rs` or `bin/src/main.rs` and must run sequentially. Tasks 4–10 touch separate files and could run in parallel after Task 3 commits.

## Task Dependency Graph

- Task 1 [AFK]: depends on `none` → batch 1 (registry crash-watch resilience)
- Task 2 [AFK]: depends on `Task 1` → batch 2 (fallback listener spawn order — same file as Task 1)
- Task 3 [AFK]: depends on `Task 2` → batch 3 (respawn-in-progress dedup — same file)
- Task 4 [AFK]: depends on `Task 3` → batch 4 (supervisor turn_id filter)
- Task 5 [AFK]: depends on `Task 3` → batch 4 (custom Debug for OrchestratorCommand, parallel with Task 4)
- Task 6 [AFK]: depends on `Task 3` → batch 4 (stderr redaction, parallel with Tasks 4–5)
- Task 7 [AFK]: depends on `Task 3` → batch 4 (tool_names leak fix, parallel with Tasks 4–6)
- Task 8 [AFK]: depends on `Task 3` → batch 4 (SQLite tuning pragmas, parallel with Tasks 4–7)
- Task 9 [AFK]: depends on `Task 8` → batch 5 (snapshot task uses read pool — wants pragmas in place first for accurate timing tests)
- Task 10 [AFK]: depends on `Task 8` → batch 5 (route store reads through read pool, parallel with Task 9)
- Polish: post-implementation-polish → final batch

## Agent Assignments

- Task 1: Registry watcher resilience → systems-programming:rust-pro
- Task 2: Fallback listener spawn order → systems-programming:rust-pro
- Task 3: Respawn-in-progress dedup → systems-programming:rust-pro
- Task 4: Supervisor turn_id filter → systems-programming:rust-pro
- Task 5: Custom Debug for OrchestratorCommand → systems-programming:rust-pro
- Task 6: Stderr redaction → systems-programming:rust-pro
- Task 7: tool_names leak fix → systems-programming:rust-pro
- Task 8: SQLite tuning pragmas → systems-programming:rust-pro
- Task 9: Snapshot task uses read pool → systems-programming:rust-pro
- Task 10: Route store reads through read pool → systems-programming:rust-pro
- Polish: post-implementation-polish → general-purpose

---

## Out of scope

These findings were validated as real but recalibrated to Medium or Low; they belong in a follow-up plan, not this one:

- **H-C1** `open_turn` race vs `Result` (narrow window, gated by sqlite append latency)
- **H-C4** Snapshot trigger A bypasses `snapshot_locks` (state-recoverable)
- **H-S1** Hardcoded `bypassPermissions` (local threat model)
- **H-A3** ADR-0001 vs reality (state-level invariants do hold)
- **H-T1** `FORCE_COMMIT_FAILURE` global (latent flake, gated by `cfg(test)`)
- **H-T2** `set_var("HOME")` in lockfile tests
- Coalescer `tick()` not wired into reader `select!`
- TUI pump `JoinHandle::abort` semantics

---

## Task 1: Registry crash-watch uses `resilient_subscribe`

**Findings**: CR-001

**Files:**
- Modify: `bin/src/session_registry.rs` — function `spawn_crash_watch` (currently around lines 274-317; grep for `fn spawn_crash_watch`).
- Test: `bin/tests/respawn_e2e.rs` (add new test)

**Background:** The current implementation calls `me.store.subscribe_after("session", &agg_id, 0)` and loops; on `Err::Lagged(n)` it logs and `continue`s, but the underlying stream stays anchored at the subscribe-head, so envelopes between the head and the next received event are permanently lost. `crates/store/src/resilient_subscribe.rs:96` already implements the re-anchor-on-Lag pattern used by `session_supervisor.rs` (grep `resilient_subscribe`) and the TUI pump. The fix is mechanical: wrap the `subscribe_after` call.

**Decision — wrapper choice:** Use `resilient_subscribe` with `RetryConfig::DEFAULT`. Pros: identical to TUI pump and supervisor; pre-tested by `crates/store/tests/resilient_subscribe_livelock.rs`. Cons: budget exhaustion (`PersistentLag(n)`) ends the watcher, leaving the agent in registry but uncrashed-watched. Mitigated by the fallback channel (Task 2 ensures it's listening). Auto-selected — no downsides compared to alternatives.

**Test design:** rather than threading a `last_handled_crash_seq_for_test` seam (which couples to internal respawn state and would also require `resume()` to actually run, which needs a fake_claude binary), the test uses the same termination-signal pattern as the existing `crash_watch_terminates_on_session_crashed` test (`bin/src/session_registry.rs` `mod tests`): the watcher's `AbortHandle::is_finished()` flips true once the `SessionCrashed` arm breaks out of the loop. That signal verifies the envelope was delivered through the resilient stream — independent of whether `on_crash` succeeded downstream. No new test seam is needed.

- [x] **Step 1: Write the failing test**

Add to `bin/tests/respawn_e2e.rs`. Note: this file uses `#[path = "../src/session_registry.rs"] mod session_registry;` (see lines 26-29), so private items like `spawn_crash_watch` ARE callable. Use the real `SessionRegistry::new(store, blob, defaults)` constructor; there is no `new_for_test`. The `EventEnvelope` field names are `event_type` (not `kind`), `timestamp` (not `ts`), with `trace_id: TraceId(...)`, `outcome: None`, `context_snapshot: None`, `metadata: HashMap`-typed (use `Default::default()`). `id` is `EventId::new()` (not `Uuid::new_v4()`).

```rust
#[tokio::test]
async fn crash_watch_recovers_session_crashed_after_broadcast_lag() {
    use nephila_core::session_event::SessionEvent;
    use nephila_eventsourcing::envelope::EventEnvelope;
    use nephila_eventsourcing::id::{EventId, TraceId};

    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let defaults = RegistryDefaults {
        claude_binary: PathBuf::from("/usr/bin/false"),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    };
    let reg = Arc::new(SessionRegistry::new(
        Arc::clone(&store),
        Arc::clone(&blob),
        defaults,
    ));

    let agent_id = AgentId::new();
    let session_id = Uuid::new_v4();
    // Bind session_id so on_crash can find it (still bails when the agent is
    // not in the store — but we only need the watcher itself to break out,
    // not for the respawn to succeed).
    reg.bind_session_id_for_test(agent_id, session_id);

    let abort = reg.spawn_crash_watch(agent_id, session_id);

    // Saturate the broadcast channel past Lagged. The default broadcast
    // capacity used by the store's per-aggregate sender is well under 4500;
    // see `crates/store/src/subscribe.rs`. The exact bound doesn't matter —
    // we just need to overflow it.
    let mut envelopes: Vec<EventEnvelope> = Vec::with_capacity(4500);
    // TurnId is a type alias for uuid::Uuid (see crates/core/src/session_event.rs:8).
    let noop = SessionEvent::HumanPromptQueued {
        turn_id: Uuid::new_v4(),
        text: String::new(),
        ts: chrono::Utc::now(),
    };
    let payload = serde_json::to_value(&noop).expect("payload");
    for _ in 0..4500u64 {
        envelopes.push(EventEnvelope {
            id: EventId::new(),
            aggregate_type: "session".to_owned(),
            aggregate_id: session_id.to_string(),
            sequence: 0,
            event_type: "human_prompt_queued".to_owned(),
            payload: payload.clone(),
            trace_id: TraceId(session_id.to_string()),
            outcome: None,
            timestamp: chrono::Utc::now(),
            context_snapshot: None,
            metadata: Default::default(),
        });
    }
    store.append_batch(envelopes).await.expect("append batch");

    // Now append the crash — must reach the watcher's SessionCrashed arm
    // (and break it out) despite the prior Lag.
    let crashed = SessionEvent::SessionCrashed {
        reason: "test".into(),
        exit_code: Some(1),
        ts: chrono::Utc::now(),
    };
    let crash_env = EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: session_id.to_string(),
        sequence: 0,
        event_type: "session_crashed".to_owned(),
        payload: serde_json::to_value(&crashed).expect("payload"),
        trace_id: TraceId(session_id.to_string()),
        outcome: None,
        timestamp: chrono::Utc::now(),
        context_snapshot: None,
        metadata: Default::default(),
    };
    store.append_batch(vec![crash_env]).await.expect("append crash");

    // Watcher must terminate — that proves the SessionCrashed envelope was
    // delivered through the resilient stream after the Lag burst.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if abort.is_finished() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("crash-watch did not terminate after SessionCrashed within 5s");
}
```

Pick whichever no-op `SessionEvent` variant is cheapest to construct — the body is irrelevant; only the volume matters.

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila --test respawn_e2e crash_watch_recovers -- --nocapture`
Expected: PANIC `crash-watch did not terminate after SessionCrashed within 5s` — the raw `subscribe_after` loop loses the crash envelope after the Lag burst.

NOTE: The bug is real but the test as designed cannot deterministically reproduce it on a fast multi-core system — the watcher's tokio task keeps up with the writer thread, so the broadcast buffer never overflows enough to evict the crash event. The test was retained as a smoke test that verifies the resilient stream path doesn't break the happy path. See task report for details.

- [x] **Step 3: Replace the raw `subscribe_after` with `resilient_subscribe`**

`resilient_subscribe` is already re-exported via `pub mod resilient_subscribe;` in `crates/store/src/lib.rs:12`. Add at the top of `bin/src/session_registry.rs`:

```rust
use nephila_store::resilient_subscribe::resilient_subscribe;
```

Then replace the body of `spawn_crash_watch` (currently `fn spawn_crash_watch(...) -> AbortHandle`):

```rust
let handle = tokio::spawn(async move {
    let mut stream = Box::pin(resilient_subscribe(
        Arc::clone(&me.store),
        "session".to_owned(),
        agg_id,
        0,
    ));

    while let Some(item) = stream.next().await {
        let env = match item {
            Ok(e) => e,
            Err(nephila_eventsourcing::store::EventStoreError::PersistentLag(n)) => {
                tracing::error!(
                    %agent_id, %session_id, retries = n,
                    "crash-watch persistent lag; relying on connector fallback"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(%agent_id, %session_id, %e, "crash-watch stream error");
                continue;
            }
        };
        let ev: SessionEvent = match serde_json::from_value(env.payload.clone()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match ev {
            SessionEvent::SessionCrashed { .. } => {
                me.on_crash(agent_id, env.sequence).await;
                break;
            }
            SessionEvent::SessionEnded { .. } => {
                me.sessions.remove(&agent_id);
                me.session_ids.remove(&agent_id);
                break;
            }
            _ => {}
        }
    }
});
```

The `Box::pin` is required because `resilient_subscribe` returns `impl Stream` — pinning lets `StreamExt::next` work on a `let mut` binding. The `PersistentLag(_)` arm uses the variant's `u32` payload (see `crates/eventsourcing/src/store.rs:24`) — bare `PersistentLag` (no parens) won't compile.

- [x] **Step 4: Run the new test and the existing respawn suite**

Run: `cargo test -p nephila --test respawn_e2e -- --nocapture`
Expected: PASS for `crash_watch_recovers_session_crashed_after_broadcast_lag` AND all existing respawn tests (some of which require the `fake_claude` fixture and will skip with a printed message if it's missing — that's expected, not a failure).

- [x] **Step 5: Run cargo check + clippy on touched crates**

Run: `cargo check -p nephila && cargo clippy -p nephila -- -D warnings`
Expected: clean.

- [x] **Step 6: Commit**

```
git add bin/src/session_registry.rs bin/tests/respawn_e2e.rs
git commit -m "fix(registry): crash-watch uses resilient_subscribe (CR-001)"
```

---

## Task 2: Spawn fallback listener before `on_startup`

**Findings**: H-C3

**Files:**
- Modify: `bin/src/main.rs` — the block that constructs `session_registry`, calls `on_startup()`, then calls `start_crash_fallback_listener()` (currently lines ~186-199; grep `start_crash_fallback_listener`).

**Background:** Currently `on_startup` runs BEFORE `start_crash_fallback_listener`. If a resume during boot itself triggers a crash, the connector's reader will `try_send` the agent_id into the bounded (cap 64) fallback channel — the send succeeds, but no listener is draining yet, so the message just sits in the buffer until later. If the bound is exceeded before the listener spawns, sends are silently dropped. Reorder so the listener is up before any session can produce events. The existing `start_crash_fallback_listener` is idempotent (it `take()`s the receiver under a mutex), so a single early call is sufficient.

- [x] **Step 1: Read the current sequence in `bin/src/main.rs`**

Grep for `start_crash_fallback_listener` to find the current location. Expected order today: `let session_registry = ...; session_registry.on_startup().await; ...; session_registry.clone().start_crash_fallback_listener().await;`.

- [x] **Step 2: Reorder**

Move the `start_crash_fallback_listener()` call to BEFORE `on_startup()`. The listener's `JoinHandle` must be retained for the lifetime of `main` (the existing code binds it to `_crash_fallback_handle`; preserve that). The `on_startup` call already wraps its error in `if let Err(e) = ... { tracing::warn!(...) }` — preserve that too. The new order:

```rust
let _crash_fallback_handle = session_registry
    .clone()
    .start_crash_fallback_listener()
    .await;
// Resume any agents in an active phase from a previous orchestrator run.
if let Err(e) = session_registry.on_startup().await {
    tracing::warn!(%e, "SessionRegistry::on_startup failed");
}
```

- [x] **Step 3: Run the orchestrator boot test**

Run: `cargo test -p nephila --test respawn_e2e -- --nocapture`
Expected: PASS (no behavioral test specifically covers the listener-before-startup race; this regression check is just to ensure the reorder didn't break boot).

- [x] **Step 4: Manual smoke**

Run: `cargo build -p nephila`
Expected: clean build.

- [x] **Step 5: Commit**

```
git add bin/src/main.rs
git commit -m "fix(boot): start crash-fallback listener before on_startup (H-C3)"
```

---

## Task 3: Respawn-in-progress dedup replaces `crash_seq == 0` bypass

**Findings**: CR-002

**Files:**
- Modify: `bin/src/session_registry.rs` — `struct RespawnState` (currently around line 73), `pub async fn on_crash` (currently around line 323).
- Test: `bin/tests/respawn_e2e.rs`

**Background:** The current dedup says "if `crash_seq != 0` and `crash_seq <= prev`, skip." This bypasses the guard entirely when `crash_seq == 0`, which is the value the connector's fallback path always sends (`crates/connector/src/session.rs:1064-1066`). Two consecutive fallback sends, or fallback racing with a store-observed crash, both run `resume()`.

**Important:** the per-agent `Mutex<RespawnState>` is currently held across `resume().await` (line 329 acquires, the guard is alive through the function's end). Two concurrent `on_crash` calls therefore *serialize* on this mutex — the second one runs after the first finishes. So a naive `respawn_in_flight: bool` that is set TRUE on entry and FALSE on exit, *all inside the lock*, is observed FALSE by the second caller and provides zero dedup. The fix must (a) release the lock before calling `resume()`, so an in-flight flag is observable by waiters, AND (b) handle the sequential case (the second call arrives after the first completes) with a separate signal.

**Decision — dedup design (chosen):**
1. Add `respawn_in_flight: bool` AND `last_respawn_at: Option<std::time::Instant>` to `RespawnState`.
2. Restructure `on_crash` into three lock cycles:
   - Lock cycle 1 (decision): acquire mutex; check `respawn_in_flight` (concurrent-call dedup); check `crash_seq <= prev` (store-path replay dedup); check `crash_seq == 0 && last_respawn_at within window` (fallback-replay dedup). If any fires, return. Otherwise set `respawn_in_flight = true` and drop the guard.
   - Work phase (no lock held): drop old handle, look up agent, call `resume()`, install new handle.
   - Lock cycle 2 (commit): re-acquire mutex; set `respawn_in_flight = false`; on success update `last_handled_crash_seq` and `last_respawn_at`.
3. Use a small dedup window (suggest 2s) for the fallback-replay case. Two real crashes 2s apart that BOTH fall back via the channel will collapse into one respawn — acceptable; the connector's `record_restart()` budget would have killed the agent anyway under that crash rate.

Why not Option B (monotonic counter routed through fallback): the fallback channel is `mpsc::Sender<AgentId>` (`bin/src/session_registry.rs` line 93) — adding a sequence would require changing the connector's `crash_fallback_tx` type and every send site. Out of proportion to the bug.

Why not Option C (drop dedup; rely on `RestartTracker` budget): the budget lives in the supervisor, not the registry. Each duplicate respawn would still spawn a process before the supervisor noticed.

- [x] **Step 1: Write the failing test**

Add to `bin/tests/respawn_e2e.rs`. The test counts "decisions to respawn" (incremented just after the dedup gate passes) rather than "successful respawns" (which would require `fake_claude` and a real `resume()` to succeed). The counter increments inside lock cycle 1 — atomic with the dedup decision, deterministic without timing.

```rust
#[tokio::test]
async fn double_fallback_only_respawns_once() {
    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let defaults = RegistryDefaults {
        claude_binary: PathBuf::from("/usr/bin/false"),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    };
    let registry = Arc::new(SessionRegistry::new(
        Arc::clone(&store),
        Arc::clone(&blob),
        defaults,
    ));

    let agent_id = AgentId::new();
    let session_id = Uuid::new_v4();
    registry.bind_session_id_for_test(agent_id, session_id);
    // Materialise the respawn_states entry so both callers race the SAME mutex.
    registry.install_respawn_counter_for_test(agent_id).await;

    // Fire fallback twice. tokio::join polls them in order; with the lock
    // released across resume(), the second call observes in_flight=true.
    let r1 = registry.clone();
    let r2 = registry.clone();
    tokio::join!(r1.on_crash(agent_id, 0), r2.on_crash(agent_id, 0));

    let count = registry.respawn_count_for_test(agent_id).await;
    assert_eq!(count, 1, "expected exactly one respawn decision; got {count}");

    // Sanity check — a second call after the first completes within the
    // dedup window must also be skipped.
    registry.on_crash(agent_id, 0).await;
    let count = registry.respawn_count_for_test(agent_id).await;
    assert_eq!(count, 1, "fallback within dedup window must skip; got {count}");
}
```

This requires test seams `install_respawn_counter_for_test` and `respawn_count_for_test` (added in Step 3). Both are async because they touch `tokio::sync::Mutex`.

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila --test respawn_e2e double_fallback -- --nocapture`
Expected: compile error pending Step 3 seams; once seams are in place, PANIC `expected exactly one respawn decision; got 2`.

- [x] **Step 3: Add new fields to `RespawnState` and counter test seams**

Find `struct RespawnState` in `bin/src/session_registry.rs` (currently around line 73). Replace with:

```rust
#[derive(Default)]
struct RespawnState {
    last_handled_crash_seq: Option<u64>,
    respawn_in_flight: bool,
    last_respawn_at: Option<std::time::Instant>,
    #[cfg(test)]
    respawn_count: u64,
}
```

Use `#[cfg(test)]` to match the file's existing `#[cfg(test)]` test-only items (e.g. `abort_after_drop_old`, `bind_session_id_for_test`). The file does not currently use a `test-seam` cargo feature; adding one is out of scope.

Add test seams in the existing `impl SessionRegistry` block, alongside `bind_session_id_for_test`:

```rust
#[cfg(test)]
pub async fn install_respawn_counter_for_test(self: &Arc<Self>, agent_id: AgentId) {
    let _ = self
        .respawn_states
        .entry(agent_id)
        .or_insert_with(|| Arc::new(Mutex::new(RespawnState::default())))
        .clone();
}

#[cfg(test)]
pub async fn respawn_count_for_test(&self, agent_id: AgentId) -> u64 {
    let Some(lock) = self.respawn_states.get(&agent_id).map(|r| r.clone()) else {
        return 0;
    };
    lock.lock().await.respawn_count
}
```

- [x] **Step 4: Restructure `on_crash` — release the lock around `resume()`**

Find `pub async fn on_crash(self: &Arc<Self>, agent_id: AgentId, crash_seq: u64)` (currently around line 323). The current shape acquires `let mut state = lock.lock().await;` and holds the guard through the entire function. Restructure into three phases. Replacement skeleton (preserve existing `tracing::instrument` attribute and surrounding test seam at `#[cfg(test)]` `abort_after_drop_old`):

```rust
const FALLBACK_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

#[tracing::instrument(level = "debug", skip(self), fields(%agent_id, crash_seq))]
pub async fn on_crash(self: &Arc<Self>, agent_id: AgentId, crash_seq: u64) {
    let lock = self
        .respawn_states
        .entry(agent_id)
        .or_insert_with(|| Arc::new(Mutex::new(RespawnState::default())))
        .clone();

    // Phase 1: dedup decision.
    {
        let mut state = lock.lock().await;
        if state.respawn_in_flight {
            tracing::debug!(%agent_id, crash_seq, "respawn already in flight; skipping");
            return;
        }
        if crash_seq != 0
            && let Some(prev) = state.last_handled_crash_seq
            && crash_seq <= prev
        {
            tracing::debug!(%agent_id, crash_seq, prev, "duplicate crash sequence; skipping");
            return;
        }
        if crash_seq == 0
            && let Some(last) = state.last_respawn_at
            && last.elapsed() < FALLBACK_DEDUP_WINDOW
        {
            tracing::debug!(
                %agent_id,
                last_respawn_ago_ms = last.elapsed().as_millis() as u64,
                "fallback within dedup window; skipping"
            );
            return;
        }
        state.respawn_in_flight = true;
        #[cfg(test)]
        {
            state.respawn_count += 1;
        }
    } // drop(state) — lock released before any awaits below.

    // Phase 2: do the work without holding the lock.
    let result = self.do_respawn_work(agent_id).await;

    // Phase 3: commit + clear in-flight, regardless of result.
    let mut state = lock.lock().await;
    state.respawn_in_flight = false;
    if result.is_ok() {
        state.last_handled_crash_seq = Some(crash_seq);
        state.last_respawn_at = Some(std::time::Instant::now());
    }
}

/// Inner respawn work — drop the old handle, resume, install the new
/// handle. Holds NO `RespawnState` lock; the caller manages the in-flight
/// flag.
async fn do_respawn_work(
    self: &Arc<Self>,
    agent_id: AgentId,
) -> Result<(), RegistryError> {
    if let Some((_id, old)) = self.sessions.remove(&agent_id) {
        old.pump.abort();
        drop(old.session);
    }

    #[cfg(test)]
    {
        let mut tx_guard = self.abort_after_drop_old.lock().await;
        if let Some(tx) = tx_guard.take() {
            drop(tx_guard);
            let _ = tx.send(());
            let (_never_tx, never_rx) = tokio::sync::oneshot::channel::<()>();
            let _ = never_rx.await;
            return Err(RegistryError::Store("test-only abort path".into()));
        }
    }

    let session_id = self
        .session_ids
        .get(&agent_id)
        .map(|r| *r)
        .ok_or_else(|| RegistryError::Store("no session_id known".into()))?;

    let agent = match nephila_core::store::AgentStore::get(&*self.store, agent_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Err(RegistryError::Store("agent not found".into())),
        Err(e) => return Err(RegistryError::Store(e.to_string())),
    };

    let cfg = self.cfg_from(&agent);
    let session = ClaudeCodeSession::resume(cfg, session_id).await?;
    let session_arc = Arc::new(session);
    let pump = self.spawn_crash_watch(agent_id, session_id);
    self.sessions.insert(
        agent_id,
        SessionHandle {
            session: Arc::clone(&session_arc),
            pump,
        },
    );
    nephila_store::metrics::record_session_respawn(&session_id.to_string());
    tracing::info!(%agent_id, %session_id, "session respawned after crash");
    let _ = self.started_tx.send(agent_id);
    Ok(())
}
```

Key invariants:
- The `respawn_in_flight = true` and `respawn_count += 1` happen inside Phase 1's lock; both are atomic with the dedup decision.
- Phase 3 ALWAYS resets `respawn_in_flight = false`. No early-return path between Phase 1 and Phase 3 — Phase 2's work is in `do_respawn_work` which returns a `Result`, never panics through. (If it could panic, wrap with `tokio::task::spawn`'s catch-unwind or `AssertUnwindSafe` + `FuturesExt::catch_unwind`. For now, treat resume failures as `Err`, not panic.)
- The `#[cfg(test)] abort_after_drop_old` test path returns `Err`, so Phase 3 still runs and clears `respawn_in_flight`. The existing `crash_during_respawn_orphan_recovery` test must still pass.

- [x] **Step 5: Run the test**

Run: `cargo test -p nephila --test respawn_e2e double_fallback -- --nocapture`
Expected: PASS — both assertions hit `count == 1`.

- [x] **Step 6: Run the full respawn suite**

Run: `cargo test -p nephila --test respawn_e2e -- --nocapture`
Expected: PASS — including `crash_during_respawn_orphan_recovery` (which exercises the test-only abort path; verify Phase 3 still runs after the simulated abort by reading the test).

- [x] **Step 7: Run cargo check + clippy**

Run: `cargo check -p nephila && cargo clippy -p nephila -- -D warnings`
Expected: clean.

- [x] **Step 8: Commit**

```
git add bin/src/session_registry.rs bin/tests/respawn_e2e.rs
git commit -m "fix(registry): release lock around resume; add fallback-window dedup (CR-002)"
```

---

## Task 4: Supervisor `TurnCompleted` filters by `turn_id`

**Findings**: H-C2

**Files:**
- Modify: `crates/lifecycle/src/session_supervisor.rs` — `handle_event` `match event` arm for `TurnCompleted` (currently around line 233; grep `SessionEvent::TurnCompleted`).
- Test: `crates/lifecycle/tests/checkpoint_pairing.rs`

**Background:** The current arm `SessionEvent::TurnCompleted { .. } => { state.awaiting_turn_completion = None; ... }` does not bind `turn_id` and does not check it against `state.awaiting_turn_completion`. Any spurious or replayed `TurnCompleted` (e.g. resume re-anchoring on a different consumer's last_seq) clears the await and auto-fires `send_agent_prompt`. The fix is to bind the field and gate the body on a match.

The function is `pub fn handle_event(&mut self, session_id: SessionId, event: &SessionEvent) -> SupervisorAction` (`session_supervisor.rs` line ~206). The TurnCompleted arm sits inside the function's outer `match event { ... }`, so the early-out is `return SupervisorAction::Idle;` — NOT `return;` (the function returns a value) and NOT `continue` (no enclosing loop).

The actual `SessionEvent::TurnCompleted` carries `{turn_id: TurnId, stop_reason: String, ts: DateTime<Utc>}` (see `crates/core/src/session_event.rs:71`). Tests must include `stop_reason`.

- [x] **Step 1: Read the test harness in `crates/lifecycle/tests/checkpoint_pairing.rs`**

Read the file first to identify the existing setup pattern: how a `SessionSupervisor` is constructed in tests, how events are delivered (call `handle_event` directly, or via a helper), how `awaiting_turn_completion` is observed, and how to count `send_agent_prompt` calls (likely via a fake `SessionDriver` impl). The Step 2 test must use the existing helpers — no new `SupervisorHarness` type from scratch.

- [x] **Step 2: Write the failing test**

Add to `crates/lifecycle/tests/checkpoint_pairing.rs`. Adapt names to match the harness discovered in Step 1; the sketch below assumes a fake driver records prompts in a `Vec<String>` shared via `Arc<Mutex<...>>`, and the supervisor exposes `awaiting_turn_completion(session_id)` (or the test introspects via a public field — choose whichever the file already uses).

```rust
#[test]
fn supervisor_ignores_turn_completed_for_unrelated_turn() {
    use nephila_core::session_event::SessionEvent;
    use uuid::Uuid;
    // ... reuse harness construction from existing tests ...

    let mut supervisor = build_supervisor_with_attached_session(/* ... */);
    let session_id = /* the attached session's id */;
    // TurnId is a type alias for uuid::Uuid.
    let turn_a = Uuid::new_v4();
    let turn_b = Uuid::new_v4();

    // Open turn A.
    supervisor.handle_event(
        session_id,
        &SessionEvent::AgentPromptDelivered {
            turn_id: turn_a,
            ts: chrono::Utc::now(),
        },
    );

    let prompts_before = prompt_count();

    // Inject TurnCompleted for an unrelated turn B — must be ignored.
    let action = supervisor.handle_event(
        session_id,
        &SessionEvent::TurnCompleted {
            turn_id: turn_b,
            stop_reason: "end_turn".into(),
            ts: chrono::Utc::now(),
        },
    );

    assert!(matches!(action, SupervisorAction::Idle));
    assert_eq!(prompt_count(), prompts_before, "no auto-prompt should fire for unrelated TurnCompleted");
    // Optional: assert `awaiting_turn_completion(session_id) == Some(turn_a)` if the
    // harness exposes that introspection; otherwise rely on the prompt-count check
    // and on the next event firing correctly:
    let action = supervisor.handle_event(
        session_id,
        &SessionEvent::TurnCompleted {
            turn_id: turn_a,
            stop_reason: "end_turn".into(),
            ts: chrono::Utc::now(),
        },
    );
    assert!(matches!(action, SupervisorAction::PromptedAgent), "TurnCompleted for the awaited turn must clear and prompt");
}
```

- [x] **Step 3: Run test to verify it fails**

Run: `cargo test -p nephila-lifecycle --test checkpoint_pairing supervisor_ignores -- --nocapture`
Expected: FAIL — the unrelated `TurnCompleted` triggers a prompt and the second one returns `Idle` (because awaiting was already cleared).

- [x] **Step 4: Bind `turn_id` and gate on match**

In `crates/lifecycle/src/session_supervisor.rs`, replace the existing `SessionEvent::TurnCompleted { .. } =>` arm with:

```rust
SessionEvent::TurnCompleted { turn_id, .. } => {
    if state.awaiting_turn_completion != Some(*turn_id) {
        tracing::debug!(
            %session_id,
            %turn_id,
            awaiting = ?state.awaiting_turn_completion,
            "TurnCompleted for unrelated turn; ignoring"
        );
        return SupervisorAction::Idle;
    }
    state.awaiting_turn_completion = None;
    let cp = state.last_checkpoint.take();
    let agent_id = state.agent_id;
    // ... rest of the existing body unchanged
}
```

Notes:
- `event` is `&SessionEvent` (see the function signature at line ~206-210), so the bound `turn_id` is `&TurnId` — dereference for the `Some(*turn_id)` comparison.
- The early-out returns `SupervisorAction::Idle` — the outer function returns `SupervisorAction`. `return;` and `continue` are wrong.
- Apply the same `turn_id` filter to the `TurnAborted` arm (line ~269-281) — it currently also clears `awaiting_turn_completion` unconditionally and is vulnerable to the same replay bug. Match on `turn_id`, compare to `state.awaiting_turn_completion`, ignore if mismatched. Add a sub-test if the existing test doesn't already cover this.

- [x] **Step 5: Run the test**

Run: `cargo test -p nephila-lifecycle --test checkpoint_pairing -- --nocapture`
Expected: PASS for the new test AND all existing tests.

- [x] **Step 6: Run cargo check + clippy**

Run: `cargo check -p nephila-lifecycle && cargo clippy -p nephila-lifecycle -- -D warnings`
Expected: clean.

- [x] **Step 7: Commit**

```
git add crates/lifecycle/src/session_supervisor.rs crates/lifecycle/tests/checkpoint_pairing.rs
git commit -m "fix(supervisor): TurnCompleted/TurnAborted filter by turn_id (H-C2)"
```

---

## Task 5: Custom `Debug` for `OrchestratorCommand` redacts content

**Findings**: H-S2

**Files:**
- Modify: `crates/core/src/command.rs`
- Test: `crates/core/tests/command_debug.rs` (new)

**Background:** `tracing::debug!(?cmd, "...")` at `bin/src/orchestrator.rs:77` invokes the derived `Debug` for `OrchestratorCommand`, which renders `content: String` and `response: String` verbatim. With the default `nephila=debug` filter (set in `bin/src/main.rs`; grep `nephila=debug`), every prompt and HITL response lands in stderr or the TUI log buffer.

The actual enum (`crates/core/src/command.rs`) has 11 variants: `Spawn { objective_id, content, dir }`, `SpawnAgent { objective_id, content, dir, spawned_by }`, `Kill { agent_id }`, `Pause { agent_id }`, `Resume { agent_id }`, `Suspend { agent_id }`, `HitlRespond { agent_id, response }`, `TokenThreshold { agent_id, directive }`, `AgentExited { agent_id, success }`, `Respawn { objective_id, content, dir, restore_checkpoint_id }`. The sensitive fields are `content` (in `Spawn`/`SpawnAgent`/`Respawn`) and `response` (in `HitlRespond`).

- [ ] **Step 1: Write the failing test**

Create `crates/core/tests/command_debug.rs`:

```rust
use nephila_core::command::OrchestratorCommand;
use nephila_core::id::{AgentId, ObjectiveId};

#[test]
fn debug_redacts_spawn_content() {
    let cmd = OrchestratorCommand::Spawn {
        objective_id: ObjectiveId::new(),
        content: "MY_API_KEY=sk-secret-xyz".into(),
        dir: std::path::PathBuf::from("/tmp"),
    };
    let s = format!("{cmd:?}");
    assert!(!s.contains("sk-secret-xyz"), "Debug must not leak Spawn.content; got: {s}");
    assert!(s.contains("Spawn"), "Debug should still name the variant; got: {s}");
    assert!(s.contains("bytes") || s.contains("redacted"), "Debug should indicate redaction; got: {s}");
}

#[test]
fn debug_redacts_spawn_agent_content() {
    let cmd = OrchestratorCommand::SpawnAgent {
        objective_id: ObjectiveId::new(),
        content: "INNER_SECRET_TOKEN".into(),
        dir: std::path::PathBuf::from("/tmp"),
        spawned_by: AgentId::new(),
    };
    let s = format!("{cmd:?}");
    assert!(!s.contains("INNER_SECRET_TOKEN"), "Debug must not leak SpawnAgent.content; got: {s}");
}

#[test]
fn debug_redacts_respawn_content() {
    use nephila_core::id::CheckpointId;
    let cmd = OrchestratorCommand::Respawn {
        objective_id: ObjectiveId::new(),
        content: "RESPAWN_PAYLOAD_SECRET".into(),
        dir: std::path::PathBuf::from("/tmp"),
        restore_checkpoint_id: CheckpointId::new(),
    };
    let s = format!("{cmd:?}");
    assert!(!s.contains("RESPAWN_PAYLOAD_SECRET"), "Debug must not leak Respawn.content; got: {s}");
}

#[test]
fn debug_redacts_hitl_response() {
    let cmd = OrchestratorCommand::HitlRespond {
        agent_id: AgentId::new(),
        response: "BEARER_TOKEN_HERE".into(),
    };
    let s = format!("{cmd:?}");
    assert!(!s.contains("BEARER_TOKEN_HERE"), "Debug must not leak HitlRespond.response; got: {s}");
}

#[test]
fn debug_passes_through_pure_id_variants() {
    let id = AgentId::new();
    let cmd = OrchestratorCommand::Kill { agent_id: id };
    let s = format!("{cmd:?}");
    assert!(s.contains("Kill"), "Debug should name the variant: {s}");
    assert!(s.contains(&format!("{id}")), "Debug should pass through agent_id: {s}");
}
```

Verify the constructor names — `ObjectiveId::new()`, `CheckpointId::new()`, `AgentId::new()` — exist by grepping `crates/core/src/id.rs`. If any uses a different constructor (e.g. `default()` or `from_uuid`), adjust.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila-core --test command_debug -- --nocapture`
Expected: FAIL on each redaction assertion — `derive(Debug)` prints the strings.

- [ ] **Step 3: Replace `derive(Debug)` with a manual impl**

In `crates/core/src/command.rs`, change `#[derive(Debug)]` to nothing on the enum (remove only `Debug`; keep any other derives if added later). Below the enum, add the impl with ALL 11 variants enumerated — Rust requires exhaustiveness, no `_` arm:

```rust
impl std::fmt::Debug for OrchestratorCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { objective_id, content, dir } => f
                .debug_struct("Spawn")
                .field("objective_id", objective_id)
                .field("content", &format_args!("<{} bytes>", content.len()))
                .field("dir", dir)
                .finish(),
            Self::SpawnAgent { objective_id, content, dir, spawned_by } => f
                .debug_struct("SpawnAgent")
                .field("objective_id", objective_id)
                .field("content", &format_args!("<{} bytes>", content.len()))
                .field("dir", dir)
                .field("spawned_by", spawned_by)
                .finish(),
            Self::Kill { agent_id } => f
                .debug_struct("Kill")
                .field("agent_id", agent_id)
                .finish(),
            Self::Pause { agent_id } => f
                .debug_struct("Pause")
                .field("agent_id", agent_id)
                .finish(),
            Self::Resume { agent_id } => f
                .debug_struct("Resume")
                .field("agent_id", agent_id)
                .finish(),
            Self::Suspend { agent_id } => f
                .debug_struct("Suspend")
                .field("agent_id", agent_id)
                .finish(),
            Self::HitlRespond { agent_id, response } => f
                .debug_struct("HitlRespond")
                .field("agent_id", agent_id)
                .field("response", &format_args!("<{} bytes>", response.len()))
                .finish(),
            Self::TokenThreshold { agent_id, directive } => f
                .debug_struct("TokenThreshold")
                .field("agent_id", agent_id)
                .field("directive", directive)
                .finish(),
            Self::AgentExited { agent_id, success } => f
                .debug_struct("AgentExited")
                .field("agent_id", agent_id)
                .field("success", success)
                .finish(),
            Self::Respawn { objective_id, content, dir, restore_checkpoint_id } => f
                .debug_struct("Respawn")
                .field("objective_id", objective_id)
                .field("content", &format_args!("<{} bytes>", content.len()))
                .field("dir", dir)
                .field("restore_checkpoint_id", restore_checkpoint_id)
                .finish(),
        }
    }
}
```

If any variant has been added since this plan was written, the missing match arm will be a compile error — add it with the same passthrough shape (or with content-redaction if it carries a sensitive field).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nephila-core --test command_debug -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run cargo check + clippy across crates that import the enum**

Run: `cargo check -p nephila-core -p nephila && cargo clippy -p nephila-core -- -D warnings`
Expected: clean. (If `nephila-mcp` or other crates also import `OrchestratorCommand`, add them — grep for `OrchestratorCommand` first.)

- [ ] **Step 6: Commit**

```
git add crates/core/src/command.rs crates/core/tests/command_debug.rs
git commit -m "fix(security): redact OrchestratorCommand Debug to prevent prompt leak (H-S2)"
```

---

## Task 6: Redact stderr tail in `SessionCrashed`

**Findings**: H-S4

**Files:**
- Modify: `crates/connector/src/session.rs` — function `snapshot_stderr_tail` (currently around line 652) and the crash-event emission site (currently around line 1040; grep `snapshot_stderr_tail`).
- Test: `crates/connector/tests/stderr_redaction.rs` (new)

**Background:** `snapshot_stderr_tail` joins the last 32 stderr lines verbatim into the `SessionCrashed.reason` field, which is persisted. Claude CLI stderr can include API errors with bearer tokens, file paths, env-derived diagnostics. Apply a regex pass before embedding.

**Decision — redaction strategy:** Replace known-prefix and key=value tokens with `<redacted>`. Patterns:
- `sk-[A-Za-z0-9_-]{20,}` (Anthropic / OpenAI keys)
- `ghp_[A-Za-z0-9]{30,}` (GitHub PATs)
- `Bearer\s+[A-Za-z0-9._~+/=-]{20,}` (HTTP bearer)
- `(?i)(api[_-]?key|secret|token|password)[=:]\s*\S+` (key=value pairs)

**Excluded:** the original draft also redacted any `[A-Za-z0-9_-]{40,}` opaque token. That false-positives on blake3 digests, sha256 hashes, container IDs, and long path components — exactly the human-debugging signal the tail exists to preserve. Drop it; the prefix and key=value patterns cover the credential cases that matter. If high-entropy tokens without a recognizable prefix become a real leak source, add a Shannon-entropy filter rather than a length filter.

Pros: cheap (single regex pass on ~32 lines); catches the common cases.
Cons: a credential without a recognizable prefix and not in `key=value` form would still leak. Acceptable trade-off.

- [ ] **Step 1: Write the failing test**

Create `crates/connector/tests/stderr_redaction.rs`:

```rust
use nephila_connector::redact_stderr;

#[test]
fn redact_strips_anthropic_key() {
    let s = "auth failed: sk-ant-api03-1234567890abcdef1234567890abcdef";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-ant-api03"), "got: {r}");
    assert!(r.contains("<redacted>"), "got: {r}");
}

#[test]
fn redact_strips_bearer_token() {
    let s = "Authorization: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9";
    let r = redact_stderr(s);
    assert!(!r.contains("eyJ0eXAi"), "got: {r}");
}

#[test]
fn redact_strips_keyvalue_pair() {
    let s = "ANTHROPIC_API_KEY=sk-secret123 something else";
    let r = redact_stderr(s);
    assert!(!r.contains("sk-secret123"), "got: {r}");
}

#[test]
fn redact_keeps_file_paths() {
    let s = "failed to read /home/user/project/file.rs: permission denied";
    let r = redact_stderr(s);
    assert!(r.contains("/home/user/project/file.rs"), "should keep paths; got: {r}");
}

#[test]
fn redact_keeps_long_hashes() {
    // blake3/sha256 digests embedded in error messages must survive — they're
    // diagnostic, not credentials.
    let s = "blob hash mismatch: a3f5e8c9b1d2f4a6b8c0e2f4a6b8d0c2e4f6a8b0d2";
    let r = redact_stderr(s);
    assert!(r.contains("a3f5e8c9b1d2f4a6b8c0e2f4a6b8d0c2e4f6a8b0d2"), "should keep long hashes; got: {r}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila-connector --test stderr_redaction -- --nocapture`
Expected: compile failure — `redact_stderr` not defined.

- [ ] **Step 3: Add `regex` to `crates/connector/Cargo.toml`**

Check first — `regex` may already be a workspace dep. If not, add to `crates/connector/Cargo.toml`:

```toml
[dependencies]
regex = { workspace = true }
```

(Or add a direct version pin if no workspace entry exists. `regex = "1.10"` is fine.)

- [ ] **Step 4: Implement `redact_stderr` in `crates/connector/src/session.rs`**

Add at the top of the file (after existing `use` lines):

```rust
use std::sync::LazyLock;

static REDACTION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{30,}|Bearer\s+[A-Za-z0-9._~+/=-]{20,}|(api[_-]?key|secret|token|password)[=:]\s*\S+)",
    )
    .expect("redaction regex compiles")
});

pub fn redact_stderr(s: &str) -> String {
    REDACTION_RE.replace_all(s, "<redacted>").into_owned()
}
```

Then in `snapshot_stderr_tail` (line 652), call `redact_stderr` on each line before joining, OR call it once on the joined output. Recommended: redact on the joined output (one regex pass):

```rust
fn snapshot_stderr_tail(ring: &StderrRing) -> String {
    // ... existing logic to build `joined` ...
    redact_stderr(&joined)
}
```

Re-export from `crates/connector/src/lib.rs` so the test can import it:

```rust
pub use session::redact_stderr;
```

- [ ] **Step 5: Run the redaction tests**

Run: `cargo test -p nephila-connector --test stderr_redaction -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run the full connector test suite**

Run: `cargo test -p nephila-connector -- --nocapture`
Expected: PASS — including session_smoke, resume_e2e, session_pause.

- [ ] **Step 7: Run cargo check + clippy**

Run: `cargo check -p nephila-connector && cargo clippy -p nephila-connector -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```
git add crates/connector/src/session.rs crates/connector/src/lib.rs crates/connector/Cargo.toml crates/connector/tests/stderr_redaction.rs
git commit -m "fix(security): redact stderr tail in SessionCrashed.reason (H-S4)"
```

---

## Task 7: Fix `tool_names` map leak in plain `ToolResult` arm

**Findings**: H-S3 (recalibrated to Low/Medium during validation, but cheap to fix)

**Files:**
- Modify: `crates/connector/src/session.rs` — the plain `ContentBlock::ToolResult` match arm (currently around line 813), the McpToolResult arm (currently around line 823) which calls `lookup_tool_name`, and the helpers `record_tool_use` / `lookup_tool_name` / `build_tool_result_event` (currently around lines 907, 913, 922).

**Background:** `record_tool_use` inserts on every `ToolUse` and `McpToolUse`; `lookup_tool_name` (which removes) is called only in the `McpToolResult` arm. Plain non-MCP tool results (Read/Edit/Bash/etc.) leave the entry in the map for the session's lifetime.

**Test design:** `tool_names: ToolNames` is created locally inside the reader-task spawn (around `session.rs:359`); it is NOT a field on `ClaudeCodeSession`. Adding a `&self` test seam would require restructuring the type. Instead, write an in-crate unit test directly on the cleanup behavior of the helpers and/or the result-handling path. Tests in `#[cfg(test)] mod tests { ... }` inside `session.rs` can call module-private items directly without any `pub` change.

- [ ] **Step 1: Write the failing unit test**

Append (or extend the existing) `#[cfg(test)] mod tests { ... }` inside `crates/connector/src/session.rs`:

```rust
#[cfg(test)]
mod tool_names_cleanup_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn fresh_tool_names() -> ToolNames {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn plain_tool_result_drains_tool_names_entry() {
        let names = fresh_tool_names();
        record_tool_use(&names, "tool-1", "Read");
        assert_eq!(names.lock().unwrap().len(), 1);

        // Simulate the plain (non-MCP) ToolResult arm.
        let raw = serde_json::Value::String("ok".into());
        let mut events: Vec<SessionEvent> = Vec::new();
        let _prepared = build_tool_result_event(&names, "tool-1", &raw, false, &mut events);

        assert_eq!(
            names.lock().unwrap().len(),
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

        assert_eq!(names.lock().unwrap().len(), 0);
    }
}
```

The signature of `build_tool_result_event` shown matches the current source (it already takes `&ToolNames` after the refactor — see Step 3); read the current signature at session.rs:922 first to confirm the parameter list before writing the call sites in the test.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila-connector tool_names_cleanup_tests -- --nocapture`
Expected: FAIL — `assertion failed: len == 0` for the plain ToolResult path. (If `build_tool_result_event` does not yet take `&ToolNames`, the test won't compile until Step 3 adds the parameter.)

- [ ] **Step 3: Push `lookup_tool_name` into `build_tool_result_event`**

In `crates/connector/src/session.rs`:
- Add `tool_names: &ToolNames` as the first parameter of `fn build_tool_result_event(...)`. Inside the function, call `let name = lookup_tool_name(tool_names, tool_use_id);` exactly once (this drains the entry for both arms).
- Update both call sites:
  - Plain `ContentBlock::ToolResult` arm (around line 813-822): pass `tool_names`.
  - `ContentBlock::McpToolResult` arm (around line 823-859): pass `tool_names` AND remove the now-redundant `lookup_tool_name(tool_names, &r.tool_use_id)` call inside the arm — instead, return the resolved `name` from `build_tool_result_event` (change the return type to `(Option<PreparedBlob>, Option<String>)` if needed, or have the function emit the name into a `&mut Option<String>` out-parameter, or expose it via an alternative inspector). The simplest is: have `build_tool_result_event` return `(Option<String>, Option<PreparedBlob>)` and let the MCP arm use `(name, prepared)`.

The post-refactor MCP arm:

```rust
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
    if !is_error
        && let Some(name) = name
        && matches!(name.as_str(), "serialize_and_persist" | "request_human_input")
    {
        // ... existing checkpoint-dispatch logic unchanged ...
    }
}
```

The plain arm becomes:

```rust
ContentBlock::ToolResult(r) => {
    let raw = serde_json::to_value(&r.content).unwrap_or(serde_json::Value::Null);
    let is_error = r.is_error.unwrap_or(false);
    let (_name, prepared) =
        build_tool_result_event(tool_names, &r.tool_use_id, &raw, is_error, &mut events);
    if let Some(p) = prepared {
        blobs.push(p);
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p nephila-connector tool_names_cleanup_tests -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the full connector suite**

Run: `cargo test -p nephila-connector -- --nocapture`
Expected: PASS — including `session_smoke`, which exercises the MCP arm's checkpoint-dispatch behavior end-to-end.

- [ ] **Step 6: Run cargo check + clippy**

Run: `cargo check -p nephila-connector && cargo clippy -p nephila-connector -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```
git add crates/connector/src/session.rs
git commit -m "fix(connector): drain tool_names on plain ToolResult arm (H-S3)"
```

---

## Task 8: Add SQLite tuning pragmas

**Findings**: H-P3

**Files:**
- Modify: `crates/store/src/lib.rs` — `SqliteStore::open` (currently lines 81-95) AND `SqliteStore::open_in_memory` (currently lines 97-128).
- Modify: `crates/store/src/writer.rs` — connection setup in `WriterHandle::new_with_broadcasts` (currently around line 65).
- Modify: `crates/store/src/read_pool.rs` — `ReadPool::open_file` (currently around line 31) and `ReadPool::open_in_memory_shared` (currently around line 49).
- Test: `crates/store/tests/pragma_settings.rs` (new)

**Background:** Currently only `journal_mode=WAL`, `foreign_keys=ON` (lib.rs `open`), and `synchronous=NORMAL` (writer.rs) are set. Default `cache_size` is 2MB, `mmap_size` is 0, `temp_store=FILE`, no explicit `wal_autocheckpoint` tuning. Apply a uniform pragma set on the writer connection AND every read-pool connection — but only for file-backed paths. The in-memory paths set `read_uncommitted=ON` for cross-connection visibility (`lib.rs:114`, `read_pool.rs:59`); avoid disturbing that.

**Decision — pragma values:**

| Pragma | Value | Rationale |
|---|---|---|
| `mmap_size` | 268435456 (256MB) | Multi-GB event log; mmap reads avoid syscall overhead |
| `cache_size` | -65536 (64MB) | Negative = KiB; covers backfill working sets |
| `temp_store` | MEMORY | Avoid tempfile creation for in-memory sorts |
| `wal_autocheckpoint` | 1000 | Default 1000 already; explicit sets it back if WAL grew |
| `synchronous` | NORMAL | Already set on writer; needed on read pool for consistent WAL view |

Auto-selected — these are conservative SQLite tuning defaults from sqlite.org's "Performance Tuning" guide. No downsides for the workload.

- [ ] **Step 1: Write the failing test**

Create `crates/store/tests/pragma_settings.rs`:

```rust
use nephila_store::SqliteStore;
use rusqlite::Connection;

#[test]
fn database_file_has_persistent_pragmas() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let _store = SqliteStore::open(&path, 384).unwrap();

    // Open a fresh connection to inspect database-level pragmas (`auto_vacuum`,
    // `application_id`, etc. survive open/close because they live in the file
    // header). For per-connection pragmas (`cache_size`, `temp_store`,
    // `mmap_size`, `wal_autocheckpoint`, `synchronous`) the only test is that
    // the open succeeded — they're applied per-connection by the helper.
    let conn = Connection::open(&path).unwrap();

    // journal_mode is persistent (WAL is sticky once set).
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode should be WAL");
}
```

Per-connection pragmas (`cache_size`, `temp_store`, `mmap_size`, `wal_autocheckpoint`, `synchronous`) are not observable across a fresh `Connection::open` because they reset to defaults on each open. The test above only verifies that the open path succeeds AND persistent pragmas land. The Step 4 verification — running the existing store suite — covers per-connection pragma application via behavioral tests.

If a stronger test is wanted, expose `apply_tuning_pragmas` as `pub` (not `pub(crate)`) and unit-test it directly: open a `Connection`, call `apply_tuning_pragmas(&conn)`, then `pragma_query_value` to confirm each one. That's lightly tighter coupling for a clearer signal.

- [ ] **Step 2: Run test to verify it fails (or trivially pass for the persistent-only assertion)**

Run: `cargo test -p nephila-store --test pragma_settings -- --nocapture`
The journal_mode assertion already passes with current code (WAL is set in `lib.rs:84`). This is intentional — the test exists to lock in the open-path contract; the meaningful regression coverage comes from running the full store suite after the pragma helper is in place.

- [ ] **Step 3: Add a shared `apply_tuning_pragmas` helper**

Add to `crates/store/src/schema.rs` (or a new helper module):

```rust
pub(crate) fn apply_tuning_pragmas(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;
    conn.pragma_update(None, "cache_size", -65_536_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}
```

- [ ] **Step 4: Call the helper from FILE-BACKED connection sites only**

File paths (apply tuning):
- `crates/store/src/lib.rs` — inside `SqliteStore::open` (after the existing `journal_mode=WAL` + `foreign_keys=ON` calls): `crate::schema::apply_tuning_pragmas(&conn)?;`
- `crates/store/src/writer.rs` — inside `WriterHandle::new_with_broadcasts` (currently sets `synchronous=NORMAL` at line 65). Call `let _ = crate::schema::apply_tuning_pragmas(&conn);` and remove the now-redundant `synchronous` set. Note: this path runs for BOTH file-backed AND in-memory writers; pragmas like `mmap_size` and `wal_autocheckpoint` are no-ops on `:memory:` / in-memory shared, so applying them universally is safe.
- `crates/store/src/read_pool.rs` — inside `ReadPool::open_file` (currently around line 31): after the `Connection::open_with_flags` call inside the loop, call `let _ = crate::schema::apply_tuning_pragmas(&conn);` before pushing into the pool.

In-memory paths (DO NOT add tuning pragmas):
- `crates/store/src/lib.rs::open_in_memory` — leaves `read_uncommitted=ON` as the only post-open pragma. The writer thread still gets the universal apply via `writer.rs`, which is acceptable.
- `crates/store/src/read_pool.rs::open_in_memory_shared` — same: leaves `read_uncommitted=ON` only.

This keeps the in-memory cross-connection visibility behavior intact for tests.

- [ ] **Step 5: Run the test**

Run: `cargo test -p nephila-store --test pragma_settings -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Run the full store suite**

Run: `cargo test -p nephila-store -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Run cargo check + clippy**

Run: `cargo check -p nephila-store && cargo clippy -p nephila-store -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```
git add crates/store/src/lib.rs crates/store/src/writer.rs crates/store/src/read_pool.rs crates/store/tests/pragma_settings.rs
git commit -m "fix(store): apply mmap/cache/temp-store/checkpoint pragmas (H-P3)"
```

---

## Task 9: Snapshot task uses `load_events_from_pool`

**Findings**: H-P4

**Files:**
- Modify: `crates/store/src/domain_event.rs` — `run_session_snapshot_task` (currently around line 449; the load-events call is currently at line 471).
- Test: `crates/store/tests/snapshot_does_not_block_writer.rs` (new)

**Background:** `run_session_snapshot_task` calls `store.load_events(...)` which dispatches into `writer.execute(...)` — the snapshot replay blocks every concurrent append for the duration of the SELECT + row decode. The pool variant `load_events_from_pool` (currently at line 342) uses `spawn_blocking` on the read pool. The same function also calls `store.load_latest_snapshot(...)` at the start (currently line 460), which ALSO routes through the writer; Task 10 covers refactoring `load_latest_snapshot` to use the pool. Together those two changes free the snapshot path from the writer.

**Important:** `load_events_from_pool` takes FOUR arguments — `(agg_type, agg_id, since_exclusive, head_inclusive)` — not three. The `head` parameter already in scope inside `run_session_snapshot_task` is the right value for `head_inclusive`. The plan's earlier "one-line swap" was wrong; the substitution adds a fourth argument.

- [ ] **Step 1: Write the failing test**

Create `crates/store/tests/snapshot_does_not_block_writer.rs`. Use the real `EventEnvelope` shape (`event_type` not `kind`, `timestamp` not `ts`, `id: EventId::new()`, `metadata: HashMap`-shaped via `Default::default()`, plus `trace_id`, `outcome`, `context_snapshot` fields). `SqliteStore::open` takes `&Path` — pass `&tmp.path().join("db.sqlite")` so the borrow checker doesn't complain.

```rust
use nephila_eventsourcing::envelope::EventEnvelope;
use nephila_eventsourcing::id::{EventId, TraceId};
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_test_envelope(agg: &str, kind: &str) -> EventEnvelope {
    EventEnvelope {
        id: EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: agg.to_owned(),
        sequence: 0,
        event_type: kind.to_owned(),
        payload: serde_json::json!({}),
        trace_id: TraceId(agg.to_owned()),
        outcome: None,
        timestamp: chrono::Utc::now(),
        context_snapshot: None,
        metadata: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_not_starved_during_snapshot_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let store = Arc::new(SqliteStore::open(&path, 384).unwrap());

    // Seed >SNAPSHOT_INTERVAL (1000) events on one aggregate so the snapshot
    // task actually runs (it skips when head < last_snap_seq + 1000).
    let agg = "test-agg".to_string();
    let mut envs = Vec::with_capacity(2000);
    for _ in 0..2000 {
        envs.push(make_test_envelope(&agg, "noop"));
    }
    store.append_batch(envs).await.unwrap();

    // Trigger a snapshot task in the background via the test seam.
    let store_clone = Arc::clone(&store);
    let agg_clone = agg.clone();
    let snap_task = tokio::spawn(async move {
        store_clone.run_session_snapshot_task_for_test(&agg_clone, 2000).await
    });

    // Race a small append against the snapshot. Measure latency.
    let start = Instant::now();
    store.append_batch(vec![make_test_envelope(&agg, "concurrent")]).await.unwrap();
    let append_latency = start.elapsed();

    snap_task.await.unwrap().unwrap();

    assert!(
        append_latency < Duration::from_millis(50),
        "append blocked by snapshot: {append_latency:?}"
    );
}
```

This requires a public test seam `run_session_snapshot_task_for_test(agg_id, head)`. Add it to `crates/store/src/domain_event.rs`:

```rust
#[cfg(test)]
impl SqliteStore {
    pub async fn run_session_snapshot_task_for_test(
        &self,
        agg_id: &str,
        head: u64,
    ) -> Result<(), nephila_eventsourcing::store::EventStoreError> {
        run_session_snapshot_task(self, "session", agg_id, head).await
    }
}
```

(Use `#[cfg(test)]` to match the rest of the crate's test-seam style. If a non-test consumer needs this, switch to a feature flag — out of scope for this task.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila-store --test snapshot_does_not_block_writer -- --nocapture`
Expected: FAIL — append latency over the 50ms threshold.

- [ ] **Step 3: Swap `load_events` for `load_events_from_pool` (with the head argument)**

In `run_session_snapshot_task`, replace:

```rust
let events = store.load_events(agg_type, agg_id, last_snap_seq).await?;
```

with:

```rust
let events = store
    .load_events_from_pool(agg_type, agg_id, last_snap_seq, head)
    .await?;
```

Confirm the signature (currently `load_events_from_pool(&self, agg_type: &str, agg_id: &str, since_exclusive: u64, head_inclusive: u64) -> Result<Vec<EventEnvelope>, EventStoreError>`). The `head` value comes from the function's existing `head: u64` parameter (line 453). The semantic match is `since_exclusive = last_snap_seq`, `head_inclusive = head`.

- [ ] **Step 4: Run the test**

Run: `cargo test -p nephila-store --test snapshot_does_not_block_writer -- --nocapture`
Expected: PASS — append latency under threshold.

(If the test still fails marginally, check whether `load_latest_snapshot` at line 460 is the remaining culprit. Task 10 fixes that; you can ALSO inline a temporary pool-based version of `load_latest_snapshot` here, but that's redundant once Task 10 lands. Prefer to commit Task 9 even if it only partially closes the gap, then complete the picture in Task 10.)

- [ ] **Step 5: Run the full store suite**

Run: `cargo test -p nephila-store -- --nocapture`
Expected: PASS — especially the snapshot-trigger and `subscribe_after_ordering` tests.

- [ ] **Step 6: Commit**

```
git add crates/store/src/domain_event.rs crates/store/tests/snapshot_does_not_block_writer.rs
git commit -m "perf(store): snapshot replay uses read pool, not writer thread (H-P4)"
```

---

## Task 10: Route store reads through `read_pool`

**Findings**: H-P1

**Files:**
- Modify: `crates/store/src/domain_event.rs` — `load_events`, `load_by_trace_id`, `load_by_time_range`, `load_latest_snapshot` (grep each by `async fn`).
- Modify: `crates/store/src/agent.rs` — `AgentStore::get` (around line 57), `AgentStore::list` (around line 76), `AgentStore::get_directive`, `AgentStore::list_agents_in_active_phase` (grep each).
- Test: extend `crates/store/tests/snapshot_does_not_block_writer.rs` from Task 9 OR add a dedicated test.

**Background:** Reads currently route through `self.writer.execute(...)`, which serializes them with appends on the single writer thread. The `read_pool` infrastructure exists and is used by `load_events_from_pool` and `BlobReader::read`. Each read site needs the pattern. Note: `acquire_guarded` is **synchronous** (`crates/store/src/read_pool.rs:83`); it returns `Result<PooledConn, ReadPoolError>` directly, no `.await`. The pattern is:

```rust
let pool = self.read_pool.clone();
let agg_type = agg_type.to_owned();
let agg_id = agg_id.to_owned();
tokio::task::spawn_blocking(move || -> Result<_, EventStoreError> {
    let conn = pool
        .acquire_guarded()
        .map_err(|_| EventStoreError::PoolExhausted)?;
    let mut stmt = conn.prepare_cached("SELECT ...")
        .map_err(|e| EventStoreError::Storage(e.to_string()))?;
    // ... existing rusqlite query body using `&conn` via Deref ...
}).await
.map_err(|e| EventStoreError::Storage(format!("join: {e}")))?
```

This mirrors `load_events_from_pool` (around line 342). For the `agent.rs` methods, the error type is `nephila_core::Result<...>` (i.e. `Result<_, NephilaError>`), not `EventStoreError`; map errors via `nephila_core::NephilaError::Storage(...)` instead.

**Scope cap:** Tracing-store reads (`tracing_store.rs`) and `save_snapshot` (a write) are out of scope. Cover the eight read methods listed above. Tracing reads are a follow-up.

- [ ] **Step 1: Write the failing test**

Add to `crates/store/tests/snapshot_does_not_block_writer.rs` (the `make_test_envelope` helper from Task 9 is already in scope). Use `nephila_core::store::AgentStore` to call `list` (the actual method name; `store.list_agents()` does not exist).

```rust
use nephila_core::store::AgentStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_list_not_starved_during_long_append() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("db.sqlite");
    let store = Arc::new(SqliteStore::open(&path, 384).unwrap());

    // Kick off a slow append in the background (large batch to keep writer busy).
    let store_clone = Arc::clone(&store);
    let big_batch: Vec<_> = (0..5000).map(|_| make_test_envelope("agg", "noop")).collect();
    let writer_task = tokio::spawn(async move {
        store_clone.append_batch(big_batch).await.unwrap()
    });

    // Give the writer a head-start so the read fires while it's still busy.
    tokio::time::sleep(Duration::from_millis(2)).await;

    // Issue AgentStore::list while the writer is busy. Should not be blocked.
    let start = Instant::now();
    let _ = AgentStore::list(&*store).await.unwrap();
    let read_latency = start.elapsed();

    writer_task.await.unwrap();

    assert!(
        read_latency < Duration::from_millis(20),
        "read blocked by writer: {read_latency:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nephila-store --test snapshot_does_not_block_writer agent_list_not_starved -- --nocapture`
Expected: FAIL — read blocked behind the writer's append batch.

- [ ] **Step 3: Refactor each read method**

For each method, swap the `self.writer.execute(...)` body for the `spawn_blocking` + `acquire_guarded` pattern shown in the Background section. Read each method's existing body BEFORE editing — some take owned args (good — clone them once at top), some borrow `&self` deeply (use `let pool = self.read_pool.clone()` and `to_owned()` the borrowed args before moving into `spawn_blocking`).

The eight methods:

`crates/store/src/domain_event.rs`:
- `load_events`
- `load_by_trace_id`
- `load_by_time_range`
- `load_latest_snapshot`

`crates/store/src/agent.rs`:
- `AgentStore::get`
- `AgentStore::list`
- `AgentStore::get_directive`
- `AgentStore::list_agents_in_active_phase`

For domain_event.rs, error type stays `EventStoreError`; for agent.rs, the trait return is `nephila_core::Result<T>` (alias for `Result<T, nephila_core::NephilaError>`) — map errors via `NephilaError::Storage(format!("..."))`.

Quick worked example for `agent.rs::list` (currently lines 76-91):

```rust
async fn list(&self) -> nephila_core::Result<Vec<Agent>> {
    let pool = self.read_pool.clone();
    let records = tokio::task::spawn_blocking(move || -> nephila_core::Result<Vec<Agent>> {
        let conn = pool
            .acquire_guarded()
            .map_err(|e| nephila_core::NephilaError::Storage(format!("read pool: {e}")))?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, state, directory, objective_id, checkpoint_id, spawned_by, injected_message, created_at, updated_at, directive, session_id, spawn_origin_type, source_checkpoint_id, restore_checkpoint_id, last_config_snapshot
                 FROM agents ORDER BY created_at",
            )
            .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_agent)
            .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| nephila_core::NephilaError::Storage(e.to_string()))?;
        Ok(rows)
    })
    .await
    .map_err(|e| nephila_core::NephilaError::Storage(format!("join: {e}")))??;
    Ok(records)
}
```

Note the `??` at the end — outer `?` for the `JoinError`, inner `?` for the inner `nephila_core::Result`. Apply the same template to the other seven.

For `load_latest_snapshot` (returns `Result<Option<Snapshot>, EventStoreError>`), the inner closure returns `Result<Option<Snapshot>, EventStoreError>`; preserve the `Option` via `query_row(...).optional()` or by matching on `QueryReturnedNoRows` exactly as the current code does.

- [ ] **Step 4: Run the read-blocking test**

Run: `cargo test -p nephila-store --test snapshot_does_not_block_writer -- --nocapture`
Expected: PASS for both the snapshot test (Task 9) and the new agent_list test.

- [ ] **Step 5: Run the full store + downstream suite**

Run: `cargo test -p nephila-store -p nephila-lifecycle -p nephila --tests -- --nocapture`
Expected: PASS — especially `read_pool_concurrent`, `read_pool_panic_safety`, `subscribe_after_ordering`, `respawn_e2e`.

- [ ] **Step 6: Run cargo check + clippy**

Run: `cargo check --workspace && cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```
git add crates/store/src/domain_event.rs crates/store/src/agent.rs crates/store/tests/snapshot_does_not_block_writer.rs
git commit -m "perf(store): route reads through read_pool, not writer thread (H-P1)"
```

---

## Polish

After all 10 tasks pass review, run `post-implementation-polish` over the diff. Expected polish items:

- Strip any AI-generated comments (skill auto-runs)
- Verify no dead code introduced (e.g. unused test seams)
- Verify CHANGELOG / commit history reads coherently
- Final `cargo clippy --workspace --all-targets -- -D warnings` clean run

The polish phase does NOT make architectural changes; if a finding from the review surfaces during polish, file a follow-up plan rather than expanding scope here.

---

## Verification — full suite

After Polish, run:

```
cargo test --workspace -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
```

All three must succeed before declaring this plan complete. The `respawn_e2e`, `checkpoint_pairing`, `session_smoke`, `resume_e2e`, and `subscribe_after_ordering` suites are the load-bearing regression coverage for this work.
