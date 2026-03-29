# GitHub Competitive Analysis: Autonomous Agent Orchestration with Persistent Context Management

**Date:** 2026-03-29
**Scope:** Open-source projects on GitHub relevant to the proposed three-layer system (Parent Process + Rust MCP Server + Claude as stateless reasoning engine)

## Executive Summary

No single open-source project implements the exact proposed architecture -- a Rust MCP server acting as orchestration brain with explicit context-window lifecycle management (spawn/checkpoint/exit/respawn), objective trees, and Qdrant-backed semantic memory. However, the individual components exist across multiple projects, and the Claude Code ecosystem is actively crowded with overlapping solutions. **claude-mem** (42.3k stars) dominates as the most popular persistent memory plugin for Claude Code, with session capture and semantic search. **Continuous-Claude-v3** (3.6k stars) is the closest system-level competitor, implementing session continuity, handoffs, multi-agent orchestration, and persistent memory via PostgreSQL/pgvector. **GobbyAI/gobby** (13 stars) is the most architecturally similar: an MCP server daemon with agent spawning, Qdrant memory, web UI, and task graphs -- but in Python and without context-window lifecycle management. **hcom** (175 stars, Rust) handles multi-agent spawning and communication but lacks persistent semantic memory and context-budget management. The major frameworks (AutoGen, LangGraph, CrewAI, Letta) operate at a higher abstraction level and do not address the specific problem of wrapping a CLI tool's context window with external lifecycle management. The genuinely novel aspect of the proposed system is treating the LLM's context window as a managed resource with proactive lifecycle control (spawn/checkpoint/exit/respawn based on token budget).

---

## Tier 1: Highest Relevance (Direct Competitors)

### 1. Continuous-Claude-v3

| Field | Value |
|-------|-------|
| **URL** | https://github.com/parcadei/Continuous-Claude-v3 |
| **Stars** | 3,639 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** A persistent, learning, multi-agent development environment built on Claude Code. Transforms Claude Code into a continuously learning system that maintains context across sessions via YAML handoffs, continuity ledgers, and a daemon-based memory extraction system.

**Architecture Comparison:**

| Proposed System Aspect | Continuous-Claude-v3 Equivalent | Gap |
|---|---|---|
| Parent process spawns/respawns Claude | Uses Claude Code hooks (PreCompact, SessionEnd) to detect context loss | No explicit process-level lifecycle management; relies on hook events |
| Rust MCP server as orchestration brain | Python hooks + PostgreSQL + file-based ledgers | Different tech stack; no single MCP server as orchestration hub |
| Context window budget tracking | Dirty-flag counter, auto-handoff at compaction | No token-level tracking; reacts to compaction rather than preventing it |
| Checkpoint/restore on context reset | YAML handoff files + continuity ledgers | Handoff is document-based, not serialized checkpoint state |
| Objective trees | No explicit objective tree; uses plans and ledgers | Plans are flat documents, not graph structures |
| Qdrant semantic memory | PostgreSQL + pgvector + BGE embeddings | Vector search exists but different backend |
| spawn_agent / child agents | 32 specialized agents spawned via Claude Task tool | Agents exist but spawned within Claude's own subagent system |
| WebSocket monitoring UI | No built-in monitoring UI | Gap |
| Signal protocol (agent_control_directive) | Hook-based event system (SessionStart, PreCompact, etc.) | Different mechanism -- hooks vs explicit signals |

**Verdict:** Closest competitor. Solves the same core problem (context loss in Claude Code) but through a fundamentally different architectural approach -- event-driven hooks and file-based handoffs rather than process-level lifecycle management with a dedicated orchestration server.

---

### 2. hcom (multi-agent communication)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/aannoo/hcom |
| **Stars** | 175 |
| **Language** | Rust |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Multi-agent communication framework built in Rust. Lets AI agents (Claude Code, Gemini CLI, Codex, OpenCode) message, watch, and spawn each other across terminals. Includes event subscriptions, transcript access, context bundles for handoffs, and cross-device relay via MQTT.

**Architecture Comparison:**

| Proposed System Aspect | hcom Equivalent | Gap |
|---|---|---|
| Parent process spawns/respawns Claude | `hcom N claude` spawns agents; `hcom r` resumes stopped agents | Spawning exists but no automatic context-budget respawn |
| Orchestration brain | Distributed -- agents coordinate via messages | No central orchestration server; peer-to-peer |
| Context window budget tracking | Not implemented | Significant gap |
| Checkpoint/restore | `hcom bundle` creates context bundles for handoffs | Manual bundle creation, not automatic checkpoint |
| Persistent semantic memory | SQLite event store; no vector/semantic search | No semantic memory layer |
| Agent lifecycle management | Spawn, kill, fork, resume via PTY | Process management exists but not context-aware |
| Monitoring | TUI dashboard; `hcom status` | TUI exists, no WebSocket API |
| Signal protocol | Hook-based message delivery; event subscriptions | Rich signaling but not context-budget-aware |

**Verdict:** Strong overlap in the agent spawning/communication layer (and notably in Rust). However, hcom is fundamentally a communication framework, not an orchestration brain. It has no concept of context budgets, semantic memory, or objective trees.

---

### 3. GobbyAI/gobby

| Field | Value |
|-------|-------|
| **URL** | https://github.com/GobbyAI/gobby |
| **Stars** | 13 |
| **Language** | Python |
| **Last Updated** | 2026-03-28 (active) |

**Description:** A local-first daemon that unifies AI coding tools. Provides session tracking and handoffs, MCP proxy with progressive discovery, task management with dependencies, agent spawning with worktree/clone isolation, persistent memory (Qdrant + knowledge graph), declarative rules engine, pipeline orchestration, web UI, and OpenTelemetry observability.

**Architecture Comparison:**

| Proposed System Aspect | Gobby Equivalent | Gap |
|---|---|---|
| Parent process spawns/respawns Claude | `spawn_agent` with isolation modes (current/worktree/clone) | Agent spawning exists; no automatic respawn on context exhaustion |
| MCP server orchestration brain | MCP server exposing tools for tasks, agents, memory, pipelines | Very similar concept -- MCP server as central hub |
| Context window budget tracking | Session handoffs capture context; progressive MCP discovery reduces token waste | Context awareness exists but no explicit token budget tracking |
| Checkpoint/restore | Session handoff/pickup system | Handoff-based, not serialized checkpoint |
| Objective trees | Task system with dependency graphs, TDD expansion | Task dependencies approximate objective trees |
| Qdrant semantic memory | Memory v5: Qdrant + knowledge graph | Direct match |
| WebSocket monitoring UI | Built-in web UI (tasks, memory, sessions, chat, agents) | Direct match |
| Signal protocol | Hook dispatcher + declarative rules engine | Rules engine blocks/allows actions; different from signaling |

**Verdict:** Very high overlap. Gobby implements most proposed components (MCP server, agent spawning, Qdrant memory, web UI, task graphs). The main gaps are: (1) Python not Rust, (2) no explicit context-window lifecycle management (spawn/checkpoint/exit/respawn cycle), and (3) no token-budget-driven agent cycling. Gobby is the most architecturally similar project found.

---

### 4. claude-mem

| Field | Value |
|-------|-------|
| **URL** | https://github.com/thedotmack/claude-mem |
| **Stars** | 42,347 |
| **Language** | TypeScript |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Persistent memory compression system for Claude Code. Automatically captures tool usage observations during sessions, generates semantic summaries, and injects relevant context into future sessions. Uses SQLite + Chroma vector DB for hybrid semantic/keyword search. Provides MCP search tools, a web viewer UI (port 37777), and a "progressive disclosure" pattern (index -> timeline -> details) for token-efficient memory retrieval. Features an experimental "Endless Mode" for extended sessions.

**Architecture Comparison:**

| Proposed System Aspect | claude-mem Equivalent | Gap |
|---|---|---|
| Parent process spawns/respawns Claude | Not addressed -- operates within Claude Code sessions | No process lifecycle management |
| MCP server orchestration brain | Worker service on port 37777 + 4 MCP search tools | Memory-focused, not orchestration |
| Context window budget tracking | Progressive disclosure reduces token use; "Endless Mode" experiments with extended sessions | No explicit token-budget tracking or proactive reset |
| Checkpoint/restore | Automatic session capture + semantic injection on next session | Captures observations, not full agent state |
| Semantic memory | SQLite + Chroma vector DB with hybrid search | Strong semantic memory, different backend from Qdrant |
| WebSocket monitoring UI | Web viewer at localhost:37777 for memory stream | Monitoring exists but memory-focused |
| Agent spawning / orchestration | Not addressed | No agent orchestration |

**Verdict:** The dominant project in the Claude Code memory space by far (42k stars). Solves the persistent memory problem comprehensively but is purely a memory/context-injection system. Does not address agent orchestration, process lifecycle, or multi-agent coordination. Relevant as the strongest prior art for the "persistent context" component and as a potential integration target rather than competitor.

---

### 5. MetaBot (xvirobotics/metabot)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/xvirobotics/metabot |
| **Stars** | 459 |
| **Language** | TypeScript |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Infrastructure for supervised, self-improving agent organizations. Runs Claude Code agents from Feishu/Telegram/WeChat. Features shared memory (MetaMemory), Agent Factory (MetaSkill), cron scheduling, inter-agent communication bus, peer federation across instances, web UI, and voice assistant mode. Built on Claude Agent SDK.

**Architecture Comparison:**

| Proposed System Aspect | MetaBot Equivalent | Gap |
|---|---|---|
| Parent process spawns/respawns Claude | Bots run as managed Claude Code instances via Agent SDK; runtime creation/deletion of bots | Process management exists but not context-budget-driven |
| MCP server orchestration brain | HTTP API server + Agent Bus for inter-bot communication | Central server exists but not MCP-native |
| Context window budget tracking | `maxTurns` / `maxBudgetUsd` per-bot limits | Budget limits exist but coarse-grained (turns/USD, not tokens) |
| Persistent memory | MetaMemory (SQLite, full-text search, knowledge base sync) | Memory exists but not vector/semantic search |
| Agent spawning / orchestration | MetaSkill agent factory; Agent Bus for cross-bot communication | Strong multi-agent support |
| Monitoring UI | Web UI with chat, task management, MetaMemory browser | Direct match |
| Objective trees | Not present | Gap |

**Verdict:** Strong overlap in the "supervised agent organization" concept. MetaBot is the closest to the proposed system's vision of a central controller managing multiple Claude instances. Key gaps: not MCP-native, no semantic/vector memory, no context-window lifecycle management, no objective trees. Uses TypeScript + Claude Agent SDK rather than Rust.

---

## Tier 2: Major Frameworks (Partial Overlap)

### 6. Letta (formerly MemGPT)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/letta-ai/letta |
| **Stars** | 21,801 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Platform for building stateful agents with advanced memory that can learn and self-improve over time. Originally MemGPT, which pioneered the concept of virtual context management for LLMs -- paging memory in and out of the context window, similar to how an OS manages virtual memory.

**Architecture Comparison:**
- **Context management:** Letta's original MemGPT concept is the closest prior art to the proposed context-window management. MemGPT explicitly manages what's in the context window, with a "main context" that pages information in/out from archival memory. However, Letta has evolved into a broader agent platform and the mechanism differs: MemGPT manages memory *within* a single long-running conversation, whereas the proposed system manages context by *terminating and restarting* the agent process.
- **Memory:** Has archival memory with embeddings, tiered memory (core/recall/archival). Strong overlap.
- **Orchestration:** Supports subagents but is primarily a single-agent-with-memory platform. Not focused on multi-agent process orchestration.
- **MCP:** Not MCP-native; has its own API.

**Verdict:** Letta/MemGPT pioneered virtual context management and is the intellectual ancestor of the proposed approach. However, it operates as an API platform, not as a CLI wrapper/MCP server, and does not manage agent processes.

---

### 7. Microsoft AutoGen

| Field | Value |
|-------|-------|
| **URL** | https://github.com/microsoft/autogen |
| **Stars** | 56,388 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** A programming framework for agentic AI. Supports multi-agent conversations, flexible conversation patterns, and tool use. Agents can be backed by LLMs, humans, or tools.

**Architecture Comparison:**
- **Multi-agent:** Strong multi-agent support with GroupChat, sequential/nested chats. Agents can spawn and communicate.
- **Context management:** No explicit context-window lifecycle management. Agents run within a single conversation session.
- **Memory:** No built-in persistent semantic memory (can be added via extensions).
- **MCP:** Not MCP-native.
- **Rust:** Python only.

**Verdict:** Powerful multi-agent framework but operates at a different level. No context lifecycle management, no MCP integration, no process-level orchestration.

---

### 8. LangGraph

| Field | Value |
|-------|-------|
| **URL** | https://github.com/langchain-ai/langgraph |
| **Stars** | 27,853 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Build resilient language agents as graphs. Supports state machines, conditional edges, checkpointing, human-in-the-loop, and multi-agent coordination.

**Architecture Comparison:**
- **Graph-based workflows:** Strong graph/state machine model for agent workflows. Closest to "objective trees" in concept.
- **Checkpointing:** Built-in checkpointing and state persistence. Can pause, save, and resume agent execution.
- **Context management:** No context-window lifecycle management. Manages *application state*, not LLM context budget.
- **Memory:** State persistence via checkpointing; no built-in vector semantic memory.
- **MCP:** Not MCP-native.

**Verdict:** LangGraph's checkpointing and graph-based workflows are directly relevant patterns. However, it is a workflow framework, not a process manager for CLI tools.

---

### 9. CrewAI

| Field | Value |
|-------|-------|
| **URL** | https://github.com/crewAIInc/crewAI |
| **Stars** | 47,493 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Framework for orchestrating role-playing, autonomous AI agents. Agents with defined roles, goals, and backstories collaborate on tasks.

**Architecture Comparison:**
- **Multi-agent:** Role-based agent orchestration with task delegation.
- **No context management:** No concept of context-window budgets or lifecycle.
- **No MCP:** Not MCP-native.
- **No persistent memory:** Session-scoped by default.

**Verdict:** High-level agent orchestration framework. Relevant for the "agent roles and objectives" concept but does not address any of the core technical problems (context lifecycle, MCP, persistent memory).

---

### 10. mem0 (Memory Layer for AI Agents)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/mem0ai/mem0 |
| **Stars** | 51,390 |
| **Language** | Python |
| **Last Updated** | 2026-03-29 (active) |

**Description:** Universal memory layer for AI agents. Provides persistent, semantic memory that can be shared across agents and sessions.

**Architecture Comparison:**
- **Semantic memory:** Direct overlap with the Qdrant/semantic memory component. Supports multiple vector backends including Qdrant.
- **No orchestration:** Memory layer only; no agent spawning, no context management, no MCP.

**Verdict:** Strong match for the persistent semantic memory component specifically. Could be used as a building block but covers only one aspect of the proposed system.

---

## Tier 3: Relevant Niche Projects

### 11. ZeptoPM (qhkm/zeptopm)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/qhkm/zeptopm |
| **Stars** | 4 |
| **Language** | Rust |
| **Last Updated** | 2026-03-19 |

**Description:** Process Manager for AI agents, inspired by Erlang/OTP supervision trees. Each agent runs as a separate OS process (~7 MB) with its own crash domain. Supports orchestrated runs, agent channels, pipelines, and session persistence.

**Architecture Comparison:**
- **Process lifecycle:** Very close to the proposed parent-process layer. Erlang-inspired supervisor that spawns, monitors, and restarts agent processes.
- **Rust:** Written in Rust, using Tokio.
- **Session persistence:** Agents remember conversations across restarts via session files.
- **Channels:** TurnBased and Stream communication between agents.
- **No context-budget management:** Does not track token usage or trigger respawns based on context exhaustion.
- **No MCP:** Not an MCP server; uses HTTP API.
- **No semantic memory:** No vector/semantic memory.

**Verdict:** The closest match to the parent-process supervisor layer. Same language (Rust), same OTP-inspired philosophy. Missing MCP integration, context-budget tracking, and semantic memory.

---

### 12. Regula (OmarTheGrey/Regula)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/OmarTheGrey/Regula |
| **Stars** | 5 |
| **Language** | Rust |
| **Last Updated** | 2026-01-12 |

**Description:** High-performance, type-safe framework for building stateful, multi-agent LLM applications in Rust. Implements a Pregel-style graph computational model with state channels, checkpointing, and human-in-the-loop support.

**Architecture Comparison:**
- **Graph execution:** State graph model with nodes, edges, conditional routing -- relevant to objective trees.
- **Checkpointing:** Built-in checkpointing with thread-based state recovery.
- **Rust:** Native Rust, Tokio-based.
- **No MCP:** Not MCP-native.
- **No context lifecycle:** Manages application state, not LLM context budgets.
- **No semantic memory:** No vector DB integration.

**Verdict:** Useful Rust-based graph execution engine with checkpointing. Could serve as a component for objective tree management but does not address the core problem.

---

### 13. Agentic-Workflow (agentralabs/agentic-workflow)

| Field | Value |
|-------|-------|
| **URL** | https://github.com/agentralabs/agentic-workflow |
| **Stars** | 1 |
| **Language** | Rust |
| **Last Updated** | 2026-03-24 |

**Description:** Universal orchestration engine for AI agents. Rust core with 124 MCP tools. Supports DAGs, state machines, batch processing, approval gates, rollback, and circuit breakers. Uses a custom `.awf` binary file format.

**Architecture Comparison:**
- **Rust + MCP:** Direct match for tech stack (Rust MCP server).
- **Workflow orchestration:** Comprehensive DAG/FSM engine exposed as MCP tools.
- **No context lifecycle:** Does not manage LLM context windows.
- **No semantic memory:** No vector DB integration.
- **No agent spawning:** Orchestrates *workflows*, not agent *processes*.

**Verdict:** Demonstrates Rust + MCP server pattern well. Relevant as an architecture reference for building MCP tools in Rust but solves a different problem (workflow orchestration vs agent lifecycle management).

---

### 14. Swarms / swarms-rs

| Field | Value |
|-------|-------|
| **URL** | https://github.com/kyegomez/swarms (Python, 6.1k stars) / https://github.com/The-Swarm-Corporation/swarms-rs (Rust, 138 stars) |
| **Language** | Python / Rust |

**Description:** Enterprise multi-agent orchestration frameworks. swarms-rs is the Rust port.

**Architecture Comparison:**
- **Multi-agent:** Focused on swarm-based multi-agent patterns.
- **No context management:** No context-window lifecycle.
- **No MCP:** Not MCP-native.
- **swarms-rs in Rust:** Exists but is a direct port of the Python patterns, not architecturally novel.

**Verdict:** General multi-agent framework. swarms-rs provides a Rust reference but lacks the specific features proposed.

---

### 15. Other Notable Projects

| Project | URL | Stars | Relevance |
|---------|-----|-------|-----------|
| **mem0-mcp-selfhosted** | https://github.com/elvismdev/mem0-mcp-selfhosted | 57 | Self-hosted mem0 MCP server with Qdrant + Neo4j + Ollama. Demonstrates MCP + Qdrant pattern for Claude Code. |
| **tokio-prompt-orchestrator** | https://github.com/Mattbusel/tokio-prompt-orchestrator | 50 | Rust/Tokio LLM pipeline orchestration with backpressure, circuit breakers. No context lifecycle. |
| **mcp-qdrant-memory** | https://github.com/delorenj/mcp-qdrant-memory | 25 | MCP server providing knowledge graph + semantic search via Qdrant. TypeScript. |
| **CloudLLM** | https://github.com/CloudLLM-ai/cloudllm | 26 | Rust LLM toolkit with multi-agent orchestration. No context lifecycle or MCP. |
| **centurion** | https://github.com/spacelobster88/centurion | 5 | Agent orchestration engine with auto-scaling. Python/MCP. No context lifecycle. |
| **madrox** | https://github.com/barkain/madrox | 4 | MCP server spawning parallel Claude Code agents via tmux. Python. Simple task delegation. |
| **agentctl** | https://github.com/jordanpartridge/agentctl | 3 | Go-based Claude Code agent container orchestrator. Spawns isolated agents. No context budget. |
| **rusty-mcp** | https://github.com/CaliLuke/rusty-mcp | 1 | Minimal Rust MCP server for Qdrant vector memory. Demonstrates Rust+MCP+Qdrant pattern. Appears inactive. |

---

## Feature Matrix

| Feature | Proposed | CC-v3 | hcom | Gobby | claude-mem | MetaBot | Letta | AutoGen | LangGraph | CrewAI | mem0 | ZeptoPM | Regula |
|---------|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| **Context-window lifecycle (spawn/checkpoint/exit/respawn)** | Y | Partial | N | N | N | N | Partial | N | N | N | N | Partial | N |
| **Token budget tracking & proactive reset** | Y | N | N | N | Partial | Partial | Y (MemGPT) | N | N | N | N | N | N |
| **MCP server as orchestration brain** | Y | N | N | Y | N | N | N | N | N | N | N | N | N |
| **Rust implementation** | Y | N | Y | N | N | N | N | N | N | N | N | Y | Y |
| **Parent-child agent spawning** | Y | Y | Y | Y | N | Y | N | Y | N | Y | N | Y | N |
| **Objective trees / goal graphs** | Y | N | N | Partial | N | N | N | N | Y | N | N | N | Y |
| **Qdrant semantic memory** | Y | N (pgvec) | N | Y | N (Chroma) | N (SQLite) | N (own) | N | N | N | Y | N | N |
| **Checkpoint/restore for conversations** | Y | Partial | Partial | Partial | Partial | N | N | N | Y | N | N | Y | Y |
| **WebSocket monitoring UI** | Y | N | TUI | Y | Y | Y | N | N | N | N | N | N | N |
| **Signal protocol (orchestrator-agent)** | Y | Partial | Y | Y | N | Partial | N | N | N | N | N | N | N |
| **Process-level agent isolation** | Y | N | Y (PTY) | Y (worktree) | N | Y (per-bot) | N | N | N | N | N | Y | N |
| **Persistent semantic memory** | Y | Y (pgvec) | N | Y | Y (Chroma) | Y (SQLite) | Y | N | N | N | Y | N | N |

Legend: Y = Implemented, N = Not present, Partial = Partially implemented or different mechanism

---

## Novelty Assessment

### Genuinely Novel Aspects of the Proposed System

1. **Explicit context-window lifecycle management as a first-class architectural concern.** No existing project treats the LLM's context window as a managed resource with a budget, threshold detection, and process-level restart cycle. Continuous-Claude-v3 reacts to compaction events; Letta/MemGPT pages memory within a session; but nobody spawns/kills/respawns an LLM CLI process based on token budget exhaustion.

2. **Rust MCP server combining orchestration + semantic memory + monitoring in a single binary.** Gobby comes closest (MCP server + Qdrant + web UI) but is Python. Agentic-workflow demonstrates Rust + MCP but lacks memory and agent management. No Rust project combines all three.

3. **Objective tree as persistent, searchable graph structure.** While task dependency graphs exist (Gobby, LangGraph), no project maintains a persistent, semantically searchable objective tree that survives across agent restarts and drives agent behavior.

4. **Signal protocol (agent_control_directive) for orchestrator-to-agent communication.** While hooks (Continuous-Claude, Gobby) and messages (hcom) exist for inter-agent signaling, no project implements a formal directive protocol between an external orchestrator and a stateless LLM reasoning engine, designed specifically for context-lifecycle management.

### Already Well-Implemented Elsewhere

1. **Persistent memory for Claude Code sessions** -- claude-mem (42k stars) dominates this space comprehensively with automatic session capture, semantic search, and context injection. Multiple MCP+Qdrant memory servers also exist (mem0-mcp-selfhosted, mcp-qdrant-memory, shared-agent-memory).
2. **Multi-agent spawning and communication** -- hcom (Rust), Continuous-Claude-v3, Gobby, MetaBot, AutoGen, CrewAI all handle this well.
3. **Persistent semantic memory with vector search** -- Letta, mem0, Gobby (Qdrant), Continuous-Claude-v3 (pgvector), claude-mem (Chroma) all implement this.
4. **MCP server implementations in Rust** -- agentic-workflow (124 tools), tokio-prompt-orchestrator show the pattern.
5. **Session handoffs and context preservation** -- Continuous-Claude-v3 and Gobby handle this well.
6. **Process isolation for agents** -- ZeptoPM (Rust, OTP-style) and hcom (PTY-based) both implement this.
7. **Graph-based workflow execution** -- LangGraph and Regula (Rust) both implement this with checkpointing.
8. **Web monitoring UI for agents** -- Gobby, claude-mem, and MetaBot all ship web UIs.
9. **Supervised agent organizations** -- MetaBot implements agent factories, inter-agent buses, cron scheduling, and per-bot budget limits.

---

## Most Relevant Competitors (Ranked by Similarity)

1. **GobbyAI/gobby** (13 stars) -- Highest architectural overlap: MCP server daemon, agent spawning, Qdrant memory, web UI, task dependency graphs, session handoffs, pipeline orchestration. Main gaps: Python not Rust, no explicit context-window lifecycle management. If Gobby were rewritten in Rust with context-budget management added, it would be nearly identical to the proposed system.

2. **parcadei/Continuous-Claude-v3** (3.6k stars) -- Closest in problem domain (making Claude Code persistent and multi-agent). Different architectural approach (hooks + handoffs vs process lifecycle). Largest dedicated community. Covers memory, agents, session continuity, but through Python scripts and file-based coordination rather than a dedicated orchestration server.

3. **xvirobotics/metabot** (459 stars) -- Closest to the "supervised agent organization" vision. Central server managing multiple Claude Code instances with shared memory, agent factory, inter-agent communication bus, cron scheduling, web UI. Main gaps: TypeScript not Rust, not MCP-native, no context-window lifecycle, no semantic/vector memory.

4. **thedotmack/claude-mem** (42.3k stars) -- Dominant player for persistent Claude Code memory. Solves session context preservation comprehensively. Not an orchestration system -- pure memory layer. Relevant as prior art and potential integration target, not direct competitor.

5. **aannoo/hcom** (175 stars) -- Closest in tech stack for agent communication (Rust). Strong spawning/messaging/event system. Missing orchestration brain, semantic memory, and context-budget management.

6. **letta-ai/letta** (21.8k stars) -- Intellectual ancestor for context management (MemGPT virtual context). Different problem framing (API platform vs CLI wrapper). Relevant for the concept of treating context as a managed resource.

7. **qhkm/zeptopm** (4 stars) -- Closest to parent-process supervisor pattern (Rust, OTP-inspired, per-agent process isolation). Missing MCP, semantic memory, and context-budget awareness.

---

## Review Notes

### Review 1 (Challenge Findings)

**Additional searches conducted:** Searched for "agent_control_directive" (0 results), "context_threshold token_count checkpoint" (0 results), "spawn_agent objective_tree" (0 results), OpenAI Symphony (0 results). Searched for Rust LLM agent frameworks (found Regula, swarms-rs, CloudLLM, nine). Searched specifically for BerriAI/litellm -- confirmed it is an LLM proxy, not an orchestration system, and excluded from main analysis. Searched for MCP + Qdrant memory servers (found mem0-mcp-selfhosted at 57 stars, mcp-qdrant-memory at 25 stars, rusty-mcp at 1 star -- a Rust Qdrant MCP server).

**Assessment corrections:**
- Elevated Gobby from Tier 2 to Tier 1 after reading full README. It is more architecturally similar than initially assessed -- it implements Qdrant, web UI, MCP tools, and agent spawning. Previously underweighted due to low star count.
- Confirmed that no project implements the specific spawn/checkpoint/exit/respawn cycle driven by token-budget thresholds. This is genuinely novel.
- Added ZeptoPM as Tier 3 -- initially missed its strong alignment with the parent-process supervisor pattern.

### Review 2 (Gaps & Inconsistencies)

**Major omissions found and corrected:**
- **claude-mem (42.3k stars)** was missing entirely. This is the most popular Claude Code memory project by an order of magnitude. Added as Tier 1 entry. While not an orchestration system, its dominance in the persistent memory space is critical context.
- **MetaBot (459 stars)** was missing. This "supervised agent organization" is architecturally relevant -- central server managing Claude Code bots with shared memory, agent factory, communication bus. Added as Tier 1 entry.
- Added "Persistent semantic memory" as a separate row in the feature matrix (was conflated with Qdrant specifically).

**Feature matrix corrections:**
- Changed Letta "Token budget tracking" to "Y (MemGPT)" -- MemGPT's original paper explicitly tracks context window usage.
- Added claude-mem "Token budget tracking" as "Partial" -- progressive disclosure and Endless Mode address token efficiency.
- Added MetaBot "Token budget tracking" as "Partial" -- supports maxTurns/maxBudgetUsd per bot.
- Changed Gobby "Objective trees" to "Partial" -- its task dependency graphs with TDD expansion approximate this pattern.
- Verified all repos are not archived and have recent activity.

### Review 3 (Final Quality Pass)

**Final adjustments:**
- Reranked competitors: Gobby remains #1 for architectural similarity; MetaBot added at #3 for "supervised agent organization" concept; claude-mem added at #4 for market dominance in persistent memory.
- Ensured every repo URL, star count, and language is accurate based on GitHub API responses from 2026-03-29.
- All assessments are based on actual README content and code structure review via GitHub API, not speculation.
- Verified the novelty assessment is defensible: the specific combination of (1) process-level context lifecycle with token-budget-driven respawning, (2) Rust MCP server as orchestration brain, (3) persistent objective tree as searchable graph, and (4) formal signal protocol for orchestrator-to-agent communication is not found in any existing project.
- The ecosystem is much more crowded than expected -- there are dozens of Claude Code orchestration projects. But the crowding is concentrated around hooks/handoffs and multi-agent communication. The "context window as managed resource" concept remains novel at the process level.
- Note: rusty-mcp (CaliLuke/rusty-mcp) is a minimal Rust MCP server for Qdrant vector memory. Only 1 star and appears inactive, but demonstrates that the Rust + MCP + Qdrant pattern has been attempted (albeit minimally).
