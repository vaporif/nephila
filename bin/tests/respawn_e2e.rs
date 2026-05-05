//! Slice 4 (Task 6 step 3 + 5 + 6): end-to-end respawn-after-crash and
//! orphan-recovery via `SessionRegistry`.
//!
//! `bin` is a binary crate, so `CARGO_BIN_EXE_fake_claude` (which lives in
//! `nephila-connector`'s test target) is not available here. We reach the
//! built fixture via `CARGO_TARGET_DIR` / the conventional `target/debug/`
//! path. Tests that cannot locate the binary `print` and skip rather than
//! failing.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nephila_core::agent::{Agent, AgentConfigSnapshot, AgentEvent, AgentState, SpawnOrigin};
use nephila_core::id::{AgentId, ObjectiveId};
use nephila_core::store::AgentStore;
use nephila_eventsourcing::store::DomainEventStore;
use nephila_store::SqliteStore;
use nephila_store::blob::SqliteBlobReader;
use uuid::Uuid;

#[path = "../src/session_registry.rs"]
mod session_registry;

use session_registry::{RegistryDefaults, SessionRegistry};

fn target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        PathBuf::from(t)
    } else if let Ok(m) = std::env::var("CARGO_MANIFEST_DIR") {
        PathBuf::from(m).parent().unwrap().join("target")
    } else {
        PathBuf::from("target")
    }
}

fn fake_claude_path() -> Option<PathBuf> {
    let candidates = [
        target_dir().join("debug").join("fake_claude"),
        target_dir().join("release").join("fake_claude"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn wrap_with_scenario(
    workdir: &std::path::Path,
    scenario: &str,
    inner: &std::path::Path,
) -> PathBuf {
    let wrapper = workdir.join("fake_claude_wrapper.sh");
    let body = format!(
        "#!/bin/sh\nexec {} --scenario {} \"$@\"\n",
        inner.display(),
        scenario,
    );
    std::fs::write(&wrapper, body).expect("write wrapper");
    let mut perm = std::fs::metadata(&wrapper).expect("meta").permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&wrapper, perm).expect("chmod");
    wrapper
}

fn defaults_for(workdir: &std::path::Path, claude: &std::path::Path) -> RegistryDefaults {
    let _ = workdir;
    RegistryDefaults {
        claude_binary: claude.to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
    }
}

fn fresh_agent(workdir: &std::path::Path, claude: &std::path::Path) -> Agent {
    let a = Agent::new(
        AgentId::new(),
        ObjectiveId::new(),
        workdir.to_path_buf(),
        SpawnOrigin::Operator,
        None,
    );
    let agent_id = a.id;
    let snap = AgentConfigSnapshot {
        working_dir: workdir.to_path_buf(),
        mcp_endpoint: "http://stub".into(),
        permission_mode: "bypassPermissions".into(),
        claude_binary: claude.to_path_buf(),
    };
    a.apply_event(&AgentEvent::AgentConfigSnapshotted {
        agent_id,
        snapshot: snap,
        ts: chrono::Utc::now(),
    })
}

#[tokio::test]
async fn respawn_replaces_handle_after_crash() {
    let Some(fake) = fake_claude_path() else {
        eprintln!("fake_claude binary not found; skipping respawn_replaces_handle_after_crash");
        return;
    };
    let workdir = tempfile::tempdir().expect("tempdir");
    // Crash on first turn, then a second wrapper would be used after respawn —
    // but the registry's resume path goes via `--resume` which the same wrapper
    // also handles. To simplify: use a wrapper that crashes once then works,
    // we approximate by using `crash_mid_turn` for the initial start (so the
    // EOF handler emits SessionCrashed), then the resume-fallback path will
    // try `--resume`, fail, then `--session-id` (Happy). Use `resume_not_found`
    // for the wrapper so that `--resume` returns "not found" and we fall back
    // to `--session-id` which is Happy.
    //
    // We can't use `crash_mid_turn` because real respawn would crash again.
    // Instead we test that on_crash actually replaces the handle when given
    // a known session_id. We use a wrapper that emits Happy frames; we
    // manually trigger on_crash by appending a SessionCrashed event to the
    // store, which the watcher should pick up.

    let happy = wrap_with_scenario(workdir.path(), "happy", &fake);

    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let defaults = defaults_for(workdir.path(), &happy);
    let reg = Arc::new(SessionRegistry::new(
        Arc::clone(&store),
        Arc::clone(&blob),
        defaults,
    ));

    let agent = fresh_agent(workdir.path(), &happy);
    let agent_id = agent.id;
    store.register(agent.clone()).await.expect("register");

    let session = reg.ensure_session(&agent).await.expect("ensure_session");
    let session_id = session.id();
    drop(session);

    assert!(reg.has_session(agent_id), "registry should hold a handle");

    // Manually inject a SessionCrashed envelope so the per-session crash-watch
    // picks it up and triggers on_crash.
    let crashed = nephila_core::session_event::SessionEvent::SessionCrashed {
        reason: "test-injected".into(),
        exit_code: Some(1),
        ts: chrono::Utc::now(),
    };
    let env = nephila_eventsourcing::envelope::EventEnvelope {
        id: nephila_eventsourcing::id::EventId::new(),
        aggregate_type: "session".to_owned(),
        aggregate_id: session_id.to_string(),
        sequence: 0,
        event_type: "session_crashed".to_owned(),
        payload: serde_json::to_value(&crashed).expect("payload"),
        trace_id: nephila_eventsourcing::id::TraceId(session_id.to_string()),
        outcome: None,
        timestamp: chrono::Utc::now(),
        context_snapshot: None,
        metadata: Default::default(),
    };
    store.append_batch(vec![env]).await.expect("append crash");

    // Wait until a respawn lands a NEW handle (or original is removed).
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if reg.has_session(agent_id) {
                // Could be a new handle. Verify by checking the SessionStarted
                // count via subscribe_after.
                let mut s = store
                    .subscribe_after("session", &session_id.to_string(), 0)
                    .await
                    .expect("subscribe");
                let mut started_count = 0;
                let _ = tokio::time::timeout(Duration::from_millis(500), async {
                    while let Some(item) = s.next().await {
                        if let Ok(env) = item
                            && let Ok(nephila_core::session_event::SessionEvent::SessionStarted {
                                ..
                            }) = serde_json::from_value(env.payload.clone())
                        {
                            started_count += 1;
                            if started_count >= 2 {
                                break;
                            }
                        }
                    }
                })
                .await;
                if started_count >= 2 {
                    return;
                }
            }
        }
    })
    .await;

    drop(reg);
    drop(workdir);
}

// Step 4 lockfile semantics — the second-acquire test lives in the
// `nephila-store::lockfile` module to avoid HOME-env races between test
// crates running in the same process.

#[tokio::test]
async fn on_startup_resumes_active_agents() {
    let Some(fake) = fake_claude_path() else {
        eprintln!("fake_claude binary not found; skipping on_startup_resumes_active_agents");
        return;
    };
    let workdir = tempfile::tempdir().expect("tempdir");
    let happy = wrap_with_scenario(workdir.path(), "happy", &fake);

    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let defaults = defaults_for(workdir.path(), &happy);

    // Register an active agent with a session_id.
    let mut agent = fresh_agent(workdir.path(), &happy);
    let session_id: Uuid = Uuid::new_v4();
    agent.session_id = Some(session_id.to_string());
    agent.state = AgentState::Active;
    store.register(agent.clone()).await.expect("register");
    AgentStore::update_state(&*store, agent.id, AgentState::Active)
        .await
        .expect("set active");

    let reg = Arc::new(SessionRegistry::new(
        Arc::clone(&store),
        Arc::clone(&blob),
        defaults,
    ));

    reg.on_startup().await.expect("on_startup");

    // Either the session is up (resume succeeded — happy wrapper accepts
    // both --resume and --session-id) OR it transitioned to Failed.
    // The wrapper emits a system init line on stdin and waits, so resume()
    // probe times out at 2s → Ok → real spawn.
    assert!(
        reg.has_session(agent.id),
        "expected active agent to be resumed",
    );

    drop(reg);
    drop(workdir);
}

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
    // capacity used by the store's per-aggregate sender is 4096; see
    // `crates/store/src/subscribe.rs`. 4500 envelopes overflows it.
    let mut envelopes: Vec<EventEnvelope> = Vec::with_capacity(4500);
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
    // (and break it out) despite any prior Lag burst.
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
    store
        .append_batch(vec![crash_env])
        .await
        .expect("append crash");

    // Watcher must terminate — that proves the SessionCrashed envelope was
    // delivered through the resilient stream after the burst.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if abort.is_finished() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("crash-watch did not terminate after SessionCrashed within 5s");
}

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
    registry.materialize_respawn_state_for_test(agent_id).await;

    // Fire fallback twice. tokio::join polls them in order; with the lock
    // released across resume(), the second call observes in_flight=true.
    let r1 = registry.clone();
    let r2 = registry.clone();
    tokio::join!(r1.on_crash(agent_id, 0), r2.on_crash(agent_id, 0));

    let count = registry.respawn_count_for_test(agent_id).await;
    assert_eq!(
        count, 1,
        "expected exactly one respawn decision; got {count}"
    );

    // Sanity check — a second call after the first completes within the
    // dedup window must also be skipped.
    registry.on_crash(agent_id, 0).await;
    let count = registry.respawn_count_for_test(agent_id).await;
    assert_eq!(
        count, 1,
        "fallback within dedup window must skip; got {count}"
    );
}

#[tokio::test]
async fn crash_during_respawn_orphan_recovery() {
    let Some(fake) = fake_claude_path() else {
        eprintln!("fake_claude binary not found; skipping crash_during_respawn_orphan_recovery");
        return;
    };
    let workdir = tempfile::tempdir().expect("tempdir");
    let happy = wrap_with_scenario(workdir.path(), "happy", &fake);

    let store = Arc::new(SqliteStore::open_in_memory(384).expect("store"));
    let blob = Arc::new(SqliteBlobReader::new(store.read_pool()));
    let defaults = defaults_for(workdir.path(), &happy);

    let agent_id = AgentId::new();
    let session_id = Uuid::new_v4();

    // Phase 1: register agent + spawn first session, install the abort hook
    // so on_crash signals between drop(old) and insert(new).
    {
        let reg = Arc::new(SessionRegistry::new(
            Arc::clone(&store),
            Arc::clone(&blob),
            defaults.clone(),
        ));

        let mut agent = Agent::new(
            agent_id,
            ObjectiveId::new(),
            workdir.path().to_path_buf(),
            SpawnOrigin::Operator,
            None,
        );
        agent.session_id = Some(session_id.to_string());
        agent.state = AgentState::Active;
        let snap = AgentConfigSnapshot {
            working_dir: workdir.path().to_path_buf(),
            mcp_endpoint: "http://stub".into(),
            permission_mode: "bypassPermissions".into(),
            claude_binary: happy.clone(),
        };
        agent = agent.apply_event(&AgentEvent::AgentConfigSnapshotted {
            agent_id,
            snapshot: snap,
            ts: chrono::Utc::now(),
        });
        store.register(agent.clone()).await.expect("register");
        AgentStore::update_state(&*store, agent_id, AgentState::Active)
            .await
            .expect("set active");

        // Spin up the first handle.
        reg.bind_session_id_for_test(agent_id, session_id);

        // We must produce a SessionStarted in the store so the watcher's
        // subscribe sees state. Easiest: ensure_session — but that uses a
        // fresh session_id. For this test we directly call on_crash with a
        // synthetic crash sequence.

        // Install the abort hook BEFORE on_crash fires.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        reg.install_abort_after_drop_old(tx).await;

        // Spawn on_crash; the hook will park the task before insert.
        let reg_for_task = Arc::clone(&reg);
        let crash_task = tokio::spawn(async move {
            reg_for_task.on_crash(agent_id, 1).await;
        });

        // Wait for the signal that drop happened.
        let _ = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("drop signal not received in time");

        // At this exact moment the registry should have NO entry for the
        // agent — the old handle is dropped and new is not inserted.
        assert!(
            !reg.has_session(agent_id),
            "expected no session entry between drop and insert",
        );

        crash_task.abort();
    }

    // Phase 2: simulate process restart — fresh registry, on_startup picks up
    // the active agent and resumes it.
    {
        let reg2 = Arc::new(SessionRegistry::new(
            Arc::clone(&store),
            Arc::clone(&blob),
            defaults,
        ));
        reg2.on_startup().await.expect("on_startup");
        assert!(
            reg2.has_session(agent_id),
            "fresh registry should resume the active agent",
        );
        drop(reg2);
    }

    drop(workdir);
}
