# Meridian MVP-1 Status

**Date:** 2026-03-30

## Implementation Progress (~75% complete)

### Complete
| Area | Crate | Lines | Notes |
|------|-------|-------|-------|
| Core domain types | `core` | ~1,600 | All types, traits, state machine, config |
| SQLite persistence | `store` | ~2,200 | Full schema, all 6 trait impls, sqlite-vec |
| Local embeddings | `embedding` | ~120 | FastEmbed ONNX, 384-dim, offline |
| Agent state machine | `core` | — | `Agent::handle(AgentCommand)` with full transition table |
| TUI dashboard | `tui` | ~1,800 | 4 panels, modals, keyboard nav, HITL UI |
| Binary wiring | `bin` | ~300 | Config, store init, TUI launch, AgentService |
| Agent process mgmt | `lifecycle` | ~510 | spawn, resume, restart tracking, crash summarizer |

### Incomplete — MCP Tool Wiring (the critical gap)
| Tool | Status | What's missing |
|------|--------|---------------|
| `get_directive` | Stub | Read directive from AgentStore |
| `report_token_estimate` | Stub | Update agent token state, check threshold, set directive |
| `get_session_checkpoint` | Stub | Load latest checkpoint from CheckpointStore |
| `serialize_and_persist` | Stub | Parse L0/L1/L2, embed L2 chunks, save to CheckpointStore |
| `search_graph` | Stub | Embed query, search MemoryStore |
| `store_memory` | Stub | Embed content, save to MemoryStore, auto-link |
| `get_objective_tree` | Stub | Load tree from ObjectiveStore |
| `update_objective` | Stub | Parse status, update via ObjectiveStore |
| `request_context_reset` | Stub | Transition agent to Exited, signal lifecycle |
| `request_human_input` | Stub | Register oneshot, wait for TUI response |

### Not Started
- End-to-end integration test (MVP-1 Task 25)
- MCP server HTTP binding (server created but not listening)
- Token estimation from MCP traffic (deferred, self-report only for MVP-1)

## Key Structural Note

The MCP server (`MeridianMcpServer<S>`) holds `Arc<S>` (store) and `broadcast::Sender<BusEvent>` but **no embedder**. Tools that need embeddings (`search_graph`, `store_memory`, `serialize_and_persist`) need an `Arc<dyn EmbeddingProvider>` added to the server struct or passed through another mechanism.

## What Comes After MCP Wiring
1. Bind MCP server to HTTP (Axum + rmcp streamable HTTP)
2. Wire agent spawn to write `.mcp.json` pointing to the running server
3. End-to-end test: spawn agent -> work -> report tokens -> drain -> checkpoint -> kill -> respawn -> restore
