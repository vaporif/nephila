# Meridian MVP-1 Status

**Updated:** 2026-04-03

## What's Done

| Area | Crate | Notes |
|------|-------|-------|
| Core domain types | `core` | Agent state machine, 6 store traits, checkpoint model with channel reducers, config |
| SQLite persistence | `store` | All 6 trait impls (agent, checkpoint, memory, objective, event log, interrupt), sqlite-vec |
| Local embeddings | `embedding` | FastEmbed ONNX, 384-dim, batch support |
| LLM connectors | `connector` | ClaudeCode (subprocess), AnthropicApi (HTTP), OpenAiCompatible |
| TUI dashboard | `tui` | 3 panels (objective tree, agent tree, event log), modals (HITL, file picker, help), goal file scanning |
| Lifecycle | `lifecycle` | Restart tracking (sliding window), token band management |
| Binary wiring | `bin` | Config load, store init, 4 async tasks (MCP server, orchestrator, TUI, signal handler) |
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

- End-to-end integration test (spawn → work → report tokens → drain → checkpoint → kill → respawn → restore)
- Crash summarizer (trait defined, no real implementation yet)
- Token estimation from MCP traffic (deferred — self-report only for MVP-1)
