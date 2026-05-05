# Nephila MVP-1 Status

**Updated:** 2026-05-05

## What's Done

| Area | Crate | Notes |
|------|-------|-------|
| Core domain types | `core` | Agent state machine, 6 store traits, checkpoint model with channel reducers, config; `Session` aggregate (second `EventSourced` impl after `Agent`) with `SessionEvent`/`SessionPhase`/`open_turn` invariants |
| Event sourcing | `eventsourcing` | `EventEnvelope` with store-stamped sequences (ADR-0002), `DomainEventStore` trait gains `subscribe_after` / `append_batch` / `prune_aggregate` |
| SQLite persistence | `store` | All 6 trait impls (agent, checkpoint, memory, objective, event log, interrupt), sqlite-vec; dedicated writer thread + read pool, blob spillover for large payloads, lockfile, retention, resilient subscribe, schema-driven pragmas (mmap/cache/temp-store/checkpoint), `subscribe_after_throughput` bench |
| Local embeddings | `embedding` | FastEmbed ONNX, 384-dim, batch support |
| LLM connectors | `connector` | `ClaudeCodeSession` — one long-lived `claude --print --verbose --input-format stream-json` process per agent, sole producer for its session aggregate, with a reader/writer/coalescer split, stderr redaction, and `--resume` fallback. `AnthropicApi` (HTTP) and `OpenAiCompatible` keep `TaskConnector`. The legacy per-turn `claude_code.rs` is still around; slice 5 removes it |
| TUI dashboard | `tui` | Three-column layout: objective tree, agent tree, `SessionPane` (live event replay + multi-line input box with vim modes) plus event log; modals (HITL, file picker, help), goal file scanning, per-agent pump |
| Lifecycle | `lifecycle` | `LifecycleSupervisor` (restart tracking, token band management) and `SessionSupervisor`, which drives turns by waiting for a `(CheckpointReached, TurnCompleted)` pair and then calling `session.send_turn(...)` on the long-lived process instead of polling for process exit |
| Binary wiring | `bin` | Config load, store init, 4 async tasks (MCP server, orchestrator, TUI, signal handler); `SessionRegistry` handles crash detection, respawn with lockfile, and `--resume` fallback dedup |
| MCP HTTP binding | `mcp` | Axum + rmcp streamable HTTP transport, tool router |

### MCP Tools (all 13 implemented)

| Module | Tools | What they do |
|--------|-------|-------------|
| `checkpoint` | `get_session_checkpoint`, `serialize_and_persist` | Load/merge L0+L1+L2 from ancestry; save checkpoint nodes + embedded L2 chunks |
| `lifecycle` | `report_token_estimate`, `get_directive`, `request_context_reset` | Token threshold detection (60/75/85%), directive polling, reset trigger |
| `memory` | `search_graph`, `store_memory` | Vector search via embedder; store entries with embeddings and tags |
| `objective` | `get_objective_tree`, `update_objective` | Tree retrieval; status updates (pending/in_progress/done/blocked) |
| `agent` | `spawn_agent`, `get_agent_status`, `get_event_log` | Spawn with objective, status lookup, event history |
| `hitl` | `request_human_input` | Create interrupt, store in DB, broadcast to TUI |
| `fork` | `fork_agent` | Fork from checkpoint with optional branch label and directive override |

## What's Left

- End-to-end integration test for the full checkpoint/reset loop (spawn → work → report tokens → drain → checkpoint → kill → respawn → restore). `bin/tests/respawn_e2e.rs` exercises the registry crash/respawn path, but the full token-driven reset is still TODO.
- Crash summarizer (trait defined in `crates/lifecycle/src/crash_summarizer.rs`, no real implementation yet)
- Token estimation from MCP traffic (deferred — self-report only for MVP-1)
- Slice 5: remove the legacy per-turn `claude_code.rs` connector once the session pane reaches UX parity with the TTY-handoff escape hatch
- See `docs/specs/2026-05-03-claude-session-streaming-design.md` for the streaming-session design and `docs/plans/2026-05-03-claude-session-streaming.md` / `docs/plans/2026-05-05-streaming-review-fixes.md` for the rollout plans

## ADRs

- [ADR-0001](adr/0001-session-as-event-sourced-aggregate.md) — `Session` modelled as an event-sourced aggregate (proposed)
- [ADR-0002](adr/0002-eventenvelope-sequence-stamping.md) — `EventEnvelope` sequences are stamped by the store, not the caller (accepted)
