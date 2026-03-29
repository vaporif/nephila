# Meridian: Architecture Overview & Design Rationale

**Date:** 2026-03-29

This document captures the full context behind Meridian's design — the problem, the research, the decisions made and why, the alternatives rejected, and the inspirations drawn from existing systems. The companion spec (`docs/superpowers/specs/2026-03-29-meridian-design.md`) is the implementation reference; this document is the story behind it.

---

## The Problem

LLM agents hit a hard wall: the context window. A Claude instance working on a complex task accumulates tool calls, file reads, reasoning traces, and error logs. At ~150-200k tokens, the context is full. The agent either gets compacted (lossy summary), starts hallucinating, or stops being useful. Long-running autonomous tasks — the kind that take hours or days — are impossible with a single context window.

Existing solutions either:
- **Page data within a running session** (MemGPT/Letta) — works but the agent still degrades as the session ages
- **Use file-based handoffs** (Stoneforge, Continuous-Claude-v3) — works but loses semantic richness
- **Delegate to sub-agents** (Claude Code Agent Teams) — helps but doesn't survive the parent's context exhaustion

Nobody does the obvious thing: **kill the agent, save what it knows to a semantic memory store, and spawn a fresh one that picks up where the old one left off.** That's what Meridian does.

---

## Core Insight

Treat the LLM like a stateless process in an operating system. It runs, it does work, it accumulates state in "RAM" (context window). When RAM is full, checkpoint to "disk" (semantic memory), terminate the process, and start a fresh one that loads the checkpoint. The OS (Meridian) manages the lifecycle. The process (Claude) just reasons.

This is directly inspired by the OS virtual memory analogy from MemGPT (2023), extended from memory management to full process lifecycle management. The Quine paper (arXiv:2603.18030, March 2026) independently arrived at the same idea using POSIX primitives (fork/exec/exit), confirming the concept is sound.

---

## Research Summary

Four independent research agents investigated novelty across web, GitHub, academic papers, and community discussions. Full reports in `docs/research/`.

### Novelty Assessment

**Genuinely novel — no public implementation exists:**
1. Process-level context lifecycle management (spawn/checkpoint/kill/respawn based on token budget)
2. MCP as the sole orchestration backbone between a persistent server and stateless LLM instances
3. Semantic-only persistence (no SQL for memory retrieval, only vector similarity)
4. The three-layer combination as a unified system

**Partially novel — concept exists, this specific form doesn't:**
5. Signal protocol via MCP tool for orchestrator-to-agent lifecycle control
6. Objective tree maintained externally and restored into fresh instances
7. Active context management (diff output vs existing memories, persist only novel info)

**Well-established — building blocks exist and are mature:**
8. Multi-agent orchestration (CrewAI, AutoGen, LangGraph)
9. Vector database memory (Qdrant, Chroma, pgvector)
10. Human-in-the-loop patterns
11. Checkpoint/restore concepts (LangGraph checkpointers, Castor)
12. Sub-agent isolation

### Closest Competitors

| Project | Similarity | Key Difference |
|---------|-----------|---------------|
| [Quine](https://arxiv.org/abs/2603.18030) (paper, Mar 2026) | Same spawn/checkpoint/respawn pattern | Uses POSIX primitives (env vars), no MCP, no vector DB, no objective tree |
| [GobbyAI/gobby](https://github.com/GobbyAI/gobby) (13 stars) | MCP server + Qdrant + agent spawning + web UI | Python, no context-window lifecycle management |
| [Continuous-Claude-v3](https://github.com/parcadei/Continuous-Claude-v3) (3.6k stars) | Same problem domain, hooks + YAML handoffs | No MCP orchestration, file-based, no vector DB |
| [Letta/MemGPT](https://github.com/letta-ai/letta) (21.8k stars, $10M funded) | Intellectual ancestor, tiered memory hierarchy | Pages data within a running session, doesn't kill/respawn |
| [Gas Town](https://github.com/steveyegge/gastown) (Steve Yegge) | Persistent identity + ephemeral sessions, 20-30 agents | Git-backed coordination, no MCP, no semantic graph |
| [hcom](https://github.com/aannoo/hcom) (175 stars, Rust) | Multi-agent spawning/communication in Rust | Communication framework, no orchestration brain, no semantic memory |
| [claude-mem](https://github.com/thedotmack/claude-mem) (42.3k stars) | Dominant persistent memory for Claude Code | Memory-only, no orchestration, no lifecycle management |
| [MetaBot](https://github.com/xvirobotics/metabot) (459 stars) | Central server managing Claude bots | TypeScript, not MCP-native, no context lifecycle |
| [ZeptoPM](https://github.com/qhkm/zeptopm) (4 stars, Rust) | OTP-style agent process supervision in Rust | No MCP, no semantic memory, no context-budget awareness |
| [Stoneforge](https://stoneforge.ai) | Context limit handoffs, git worktree isolation | TypeScript, file-based handoffs, SQLite not vector DB |
| [ccswarm](https://github.com/nwiizo/ccswarm) (Rust) | Rust-based Claude orchestration with PTY sessions | No MCP, no vector DB, orchestrator loop incomplete |
| [Regula](https://github.com/OmarTheGrey/Regula) (5 stars, Rust) | Pregel-style graph execution with checkpointing in Rust | No MCP, no context lifecycle, application state only |
| [agentic-workflow](https://github.com/agentralabs/agentic-workflow) (1 star, Rust) | 124 MCP tools in Rust | Workflow orchestration, not agent lifecycle |

### Academic Foundations

| Paper | Relevance | Link |
|-------|-----------|------|
| **MemGPT** (Packer et al., 2023) — virtual context management via OS memory paging analogy | CRITICAL — intellectual ancestor, extended from memory to process lifecycle | [arXiv:2310.08560](https://arxiv.org/abs/2310.08560) |
| **Quine** (Ke, 2026) — LLM agents as POSIX processes with exec-based context renewal | CRITICAL — independently arrived at same spawn/checkpoint/respawn pattern | [arXiv:2603.18030](https://arxiv.org/abs/2603.18030) |
| **CoALA** (Sumers et al., 2023) — cognitive architecture framework for language agents | CRITICAL — theoretical foundation, maps to Meridian's memory architecture | [arXiv:2309.02427](https://arxiv.org/abs/2309.02427) |
| **Generative Agents** (Park et al., 2023) — memory stream + reflection + retrieval | HIGH — foundational memory consolidation pattern | [arXiv:2304.03442](https://arxiv.org/abs/2304.03442) |
| **Reflexion** (Shinn et al., 2023) — verbal reinforcement via episodic memory across trials | HIGH — persistent episodic buffer maps to checkpoint mechanism | [arXiv:2303.11366](https://arxiv.org/abs/2303.11366) |
| **Voyager** (Wang et al., 2023) — lifelong learning agent with persistent skill library | HIGH — external persistent memory, treats LLM as blackbox stateless engine | [arXiv:2305.16291](https://arxiv.org/abs/2305.16291) |
| **A-MEM** (Xu et al., 2025) — Zettelkasten-inspired interconnected memory notes | HIGH — inspiration for linked memories, 192% improvement over MemGPT | [arXiv:2502.12110](https://arxiv.org/abs/2502.12110) |
| **ReAct** (Yao et al., 2022) — interleaved reasoning and acting | MODERATE — establishes stateless reasoning engine pattern | [arXiv:2210.03629](https://arxiv.org/abs/2210.03629) |
| **InfiAgent** (2026) — hierarchical agents with periodic state consolidation | HIGH — periodic consolidation + context refresh parallels checkpoint/respawn | [arXiv:2601.03204](https://arxiv.org/abs/2601.03204) |
| **CaveAgent** (2026) — decouples state from context via persistent runtime | HIGH — variable reference indirection, context contains pointers not data | [arXiv:2601.01569](https://arxiv.org/abs/2601.01569) |
| **SagaLLM** (Chang & Geng, 2025) — transactional checkpoints with compensating rollback | HIGH — inspiration for transactional checkpoint integrity | [arXiv:2503.11951](https://arxiv.org/abs/2503.11951) |
| **Focus** (Verma, 2026) — agent-autonomous context compression | HIGH — agent-initiated consolidation into persistent knowledge blocks | [arXiv:2601.07190](https://arxiv.org/abs/2601.07190) |
| **ACON** (2025) — context compression for long-horizon agents | MODERATE — subtask-to-brief compression parallels checkpoint summarization | [arXiv:2510.00615](https://arxiv.org/abs/2510.00615) |
| **Memory survey** (2026) — 218 papers, identifies summarization drift pathology | HIGH — validates layered storage, warns about repeated compression loss | [arXiv:2603.07670](https://arxiv.org/abs/2603.07670) |
| **Rethinking Memory** (2026) — five cognitive memory systems taxonomy | MODERATE — theoretical grounding for what vector DB stores vs objective tree | [arXiv:2602.06052](https://arxiv.org/abs/2602.06052) |
| **AgentOrchestra** (2025) — TCP extending MCP with lifecycle management | MODERATE — directly relevant protocol design patterns | [arXiv:2506.12508](https://arxiv.org/abs/2506.12508) |
| **KubeIntellect** (2025) — PostgreSQL-backed checkpointing with LangGraph | HIGH — database-backed checkpoint/restore for agent state | [arXiv:2509.02449](https://arxiv.org/abs/2509.02449) |
| **CTA** (2026) — tree-structured conversation management with context isolation | MODERATE — context flow primitives between tree nodes | [arXiv:2603.21278](https://arxiv.org/abs/2603.21278) |

### Community Sentiment

The community is split. The "orchestrate now" camp builds multi-agent systems with sub-agent isolation and external memory. The "wait for better models" camp argues that 10M+ token windows with improved attention will make orchestration unnecessary.

The 2026 memory survey settles this: **bigger windows don't eliminate the need for external memory.** Cost scales linearly with context size, attention scales quadratically. Targeted retrieval from external memory outperforms brute-force long context. External orchestration is complementary, not obsoleted by larger windows.

Notable community resources:
- [Anthropic's Context Engineering Guide](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) — validates the sub-agent + compaction + external memory pattern
- [StackOne: Agent Suicide by Context](https://www.stackone.com/blog/agent-suicide-by-context) — 6 patterns by which agents destroy their own context
- [Google ADK Context Architecture](https://developers.googleblog.com/architecting-efficient-context-aware-multi-agent-framework-for-production/) — "context as first-class system with its own lifecycle"
- [LangChain Deep Agents Context Management](https://blog.langchain.com/context-management-for-deepagents/) — filesystem-based context compression
- [Eunomia C/R Survey](https://eunomia.dev/blog/2025/05/11/checkpointrestore-systems-evolution-techniques-and-applications-in-ai-agents/) — checkpoint/restore evolution for AI agents
- [Zylos: Multi-Agent Memory Architectures](https://zylos.ai/research/2026-03-09-multi-agent-memory-architectures-shared-isolated-hierarchical) — "token usage explains 80% of performance variance"
- [DEV: Running 10+ Claude Instances](https://dev.to/bredmond1019/multi-agent-orchestration-running-10-claude-instances-in-parallel-part-3-29da) — subprocess.Popen + Redis approach
- [Chris Hay: Infinite Context via Hierarchical Memory](https://www.youtube.com/watch?v=3mSbR7fJX9A) — session chaining on token exhaustion (closest video demo)
- [Using MCP to Orchestrate Agents with Qdrant](https://www.youtube.com/watch?v=mZK2a_4jgwo) — Qdrant + MCP integration demo
- [Qdrant MCP Server](https://github.com/qdrant/mcp-server-qdrant) (800+ stars) — official semantic memory MCP server
- [mem0](https://github.com/mem0ai/mem0) (51k stars) — universal memory layer for AI agents
- [Castor](https://www.reddit.com/r/LLMDevs/comments/1s0qgxt/) — kernel-level agent checkpoint/resume
- [MemOS](https://www.reddit.com/r/LocalLLaMA/comments/1qkrhec/) — memory lifecycle management with next-scene prediction

---

## Architecture Decisions

### ADR-1: Single Binary (MCP Server + Lifecycle Manager)

**Context:** The original proposal described three layers: parent process, MCP server, and Claude. The question was whether to keep them separate or merge.

**Options:**
- **(A) Thin parent + separate MCP server** — Parent just spawns/watches processes. MCP server is the brain. Two binaries, IPC between them.
- **(B) Proper CLI daemon + MCP server** — Parent is a full daemon managing agent trees. MCP server is a tool provider. Distributed responsibility.
- **(C) Single binary** — Parent IS the MCP server. One process does everything.

**Decision:** C — single binary.

**Rationale:** Eliminates an entire coordination layer. No IPC between parent and MCP, no split-brain risk where parent thinks agent is alive but MCP thinks it's dead, one thing to deploy and debug. The MCP server already needs to know about agent lifecycle (to serve directives, track tokens), and the lifecycle manager already needs to know about MCP state (to reconstruct checkpoints from call logs). Separating them forces both to maintain synchronized views of the same state.

**Consequences:** Larger binary. If the MCP server crashes, lifecycle management also dies. Accepted trade-off — the alternative (two processes that must stay in sync) is worse.

---

### ADR-2: Streamable HTTP Transport (Not stdio)

**Context:** How does Meridian (MCP server) communicate with Claude (MCP client)?

**Options:**
- **(A) stdio** — Claude spawns Meridian as a child process via stdin/stdout. Standard MCP pattern.
- **(B) Streamable HTTP** — Meridian listens on a port, Claude connects via HTTP. Each agent gets a unique URL path.
- **(C) stdio bridge** — Meridian spawns a thin shim per agent that tunnels back to the main process via Unix socket.

**Decision:** B — streamable HTTP.

**Rationale:** stdio has a fundamental direction problem. In stdio MCP, the *client* (Claude) spawns the *server* as a child process. But Meridian's architecture needs the opposite — Meridian spawns Claude, and Meridian is the long-lived server. If Claude spawns Meridian via stdio, you get a fresh Meridian per agent instance, defeating the purpose of a single orchestrator managing multiple agents.

Streamable HTTP solves this cleanly: one Meridian process, many Claude connections. Agent identity is baked into the URL path (`/agent/{agent_id}`). No custom bridging needed.

Option C (stdio bridge) works but adds a shim process per agent for no real benefit.

**Consequences:** Requires Meridian to run an HTTP server (Axum, already needed for future web UI). Claude must be configured with an MCP endpoint URL rather than a command. Minor: streamable HTTP is the current MCP standard (SSE is being phased out).

---

### ADR-3: Orchestration Sidecar (Not Tool Proxy)

**Context:** Should Meridian be Claude's only interface to the world, or should Claude keep its own tools?

**Options:**
- **(A) Full proxy** — Claude only talks to Meridian. Meridian proxies filesystem, bash, git, everything.
- **(B) Sidecar** — Claude keeps its own tools (Read, Write, Bash, Git). Meridian handles orchestration only (checkpoint, memory, objectives, directives).

**Decision:** B — sidecar.

**Rationale:** If Meridian proxies every tool, it becomes a massive tool proxy with no benefit. Claude needs to read files, run commands, and use git for actual work. Routing all of that through Meridian adds latency, complexity, and a massive API surface for no orchestration value. Meridian doesn't need to know what Claude does with files — it only cares about lifecycle, memory, and objectives.

MCP config is additive in Claude Code. Meridian drops a `.mcp.json` in the agent's working directory pointing to its endpoint. Claude picks it up alongside its standard tools.

**Consequences:** Meridian has incomplete visibility into what Claude does (can't see filesystem reads, bash output). This is acceptable — Meridian tracks token usage via self-reporting, not by observing all activity.

---

### ADR-4: Flat Agent Ownership (Not Hierarchical)

**Context:** When agents spawn child agents, who "owns" the children? Should there be a parent-child lifecycle relationship?

**Options:**
- **(A) Flat** — Meridian owns all agents. No parent controls children. `spawned_by` is metadata, not authority.
- **(B) Hierarchical** — Parent agent owns children. If parent resets, respawned version inherits children. If parent dies, children are orphaned.
- **(C) Hierarchical with promotion** — Like B, but orphaned agents get promoted to Meridian or adopted.

**Decision:** A — flat ownership.

**Rationale:** The context reset kills hierarchy. When Agent A (parent of B and C) resets, it loses all awareness of B and C. The checkpoint must include child state, but B and C have progressed since the checkpoint — respawned A has stale info immediately. Multiply this by every agent resetting independently and you get cascading staleness.

Deep nesting is a footgun. A spawns B, B spawns C, C spawns D. B resets. Now C and D are orphans nested two levels deep. Promotion logic adds complexity for every edge case.

Claude doesn't need lifecycle control — it needs coordination. "Spawn something to write tests" and later "are the tests done?" That's a query to Meridian, not parental authority.

With flat ownership: if A resets, respawned A queries Meridian for fresh child status. If A dies, B and C continue — Meridian reassigns or lets them finish. No orphan crisis. This is the Kubernetes model: the control plane owns all pods, logical groupings track relationships.

**Consequences:** No cascading lifecycle control. An agent can't kill its children directly — it requests Meridian to do it. This is a feature, not a limitation.

---

### ADR-5: Layered Checkpoint (L0/L1/L2) to Semantic Memory

**Context:** What gets serialized when an agent context-resets?

**Options:**
- **(A) Minimal** — Just objective state. ~500 tokens. Lean but loses nuance.
- **(B) Structured summary** — Objective state + compressed narrative. ~2-5k tokens. Richer but trusts summarization.
- **(C) Layered** — L0 (objective state, always injected), L1 (session summary, always injected), L2 (detailed findings, retrieved on demand via semantic search).

**Decision:** C — layered.

**Rationale:** Matches the "active context management" principle. Fresh Claude gets L0+L1 automatically (~3-5k tokens, lean) and pulls L2 on demand via `search_graph()`. This doesn't dump everything into context, lets Claude pull what it needs. L2 entries are individually embedded and semantically searchable — Claude can find "that error I saw in auth.rs" without loading the entire checkpoint.

**Consequences:** More complex storage model. L2 requires embedding each chunk. But this is exactly what the A-MEM linking pattern is designed for.

---

### ADR-6: SQLite Default (Not Qdrant)

**Context:** Where does persistent state live?

**Options:**
- **(A) Qdrant** — dedicated vector database, requires running a separate server.
- **(B) SQLite + sqlite-vec** — embedded database with vector extension, single file.

**Decision:** B — SQLite default, with `MemoryStore` trait for swappability.

**Rationale:**
- Zero additional infrastructure — no Qdrant process to run. Single binary + one `.db` file.
- ACID transactions for free — checkpoint integrity (write L2, then L1, then L0 atomically) becomes a real SQL transaction. No partial checkpoints possible.
- A-MEM linking is simpler — foreign keys, JOINs to follow links, vs managing point ID arrays in Qdrant payloads.
- Backup is `cp meridian.db meridian.db.bak`.
- Latency — no network hop, everything in-process.

For MVP-1 (single agent, hundreds to low thousands of memory entries), SQLite vector search is perfectly adequate. Qdrant becomes relevant at 100k+ vectors with concurrent writes from many agents.

The `MemoryStore` trait means Qdrant can be swapped in via config without changing any application code.

**Consequences:** SQLite is single-writer (WAL mode helps). Multiple agents checkpointing simultaneously may see write contention. Addressed via dedicated writer thread (actor model). Acceptable for MVP-1/MVP-2. Qdrant recommended for production multi-agent deployments at scale.

---

### ADR-7: Claude Summarizes (Configurable Backend)

**Context:** When an agent checkpoints, who writes the summary — Claude before it exits, or a local LLM after?

**Options:**
- **(A) Claude before exit** — highest quality, but costs context tokens at the worst time.
- **(B) Local LLM after exit** — zero cost to Claude, lower quality, can retry.
- **(C) Hybrid** — Claude writes L0 (structured, cheap), local LLM handles L1/L2.
- **(D) Configurable** — `Summarizer` trait, any backend, pick via config.

**Decision:** D — configurable, with Claude as default for MVP-1.

**Rationale:** Different deployments have different constraints. Solo developer on Claude Max wants Claude quality. Team running 20 agents wants local LLM to avoid API costs. The trait boundary is clean and the backends are independent.

For MVP-1, `ClaudeSummarizer` is simplest — Claude writes all three layers as tool arguments. No local LLM infrastructure needed. `CrashSummarizer` (heuristic-only, no LLM) handles the crash recovery path.

**Consequences:** System prompt must tell Claude what to write based on configured backend. Adds a coupling between config and system prompt generation.

---

### ADR-8: Token Tracking via Self-Report + Directive Polling

**Context:** How does Meridian know when Claude is approaching the context threshold?

**Options:**
- **(A) Claude self-reports** — Claude calls `report_token_estimate(used, remaining)` periodically.
- **(B) Meridian estimates** — Track MCP traffic, apply a multiplier for unseen activity.
- **(C) Directive-based polling** — Claude checks `get_directive()` each cycle. Meridian sets threshold flags.

**Decision:** A+C combined.

**Rationale:** Meridian sees only its own MCP traffic — it doesn't see filesystem reads, bash output, or Claude's internal reasoning. It can estimate a lower bound but not the full picture. Claude knows its own context best.

The two are paired: Claude calls `report_token_estimate` and `get_directive` together, at matching frequencies that increase near the threshold (every 10 calls → every 3 → every 1). This ensures Meridian has fresh data when it needs to decide on `prepare_reset`, and Claude reads the directive immediately after reporting.

Safety buffer: if Meridian's conservative estimate hits 85% before Claude starts draining, force-kill and use CrashSummarizer.

**Consequences:** Relies on Claude following the system prompt instructions. If Claude ignores reporting instructions, Meridian falls back to its own estimate with a conservative multiplier.

---

### ADR-9: `get_directive` as Tool (Not MCP Resource)

**Context:** How does Meridian communicate lifecycle directives to Claude?

**Options:**
- **(A) MCP resource** — Claude reads `meridian://agent/{id}/directive` as a resource. Elegant, declarative.
- **(B) MCP tool** — Claude calls `get_directive()`. Explicit, proven.

**Decision:** B — tool.

**Rationale:** MCP resource polling support in Claude Code is unverified. Building the entire lifecycle management flow on an unverified behavior is unacceptable risk. `get_directive()` as a tool is guaranteed to work — it's a standard MCP tool call.

If resource polling is later verified, it can be added as an optional optimization.

**Consequences:** One extra tool call per reasoning cycle. At critical polling frequency (every tool call), this roughly doubles MCP traffic. Acceptable — the calls are tiny.

---

### ADR-10: TUI First (Not Web UI)

**Context:** How does the human operator interact with Meridian?

**Decision:** ratatui TUI for MVP-1. Web UI (Axum + WebSocket) as a future addition.

**Rationale:** The operator needs to be at the terminal anyway (to see agent output, manage code). A TUI keeps everything in one place. It also avoids the complexity of an HTTP server for the monitoring interface in MVP-1 (the MCP server already uses HTTP, but a monitoring UI is a different concern).

The TUI is the human-in-the-loop interface: agent status, objective tree, task board, event stream, HITL prompts, and direct commands. WhatsApp/web UI become alternative frontends later — same event bus, different renderer.

---

## Patterns Adopted from Prior Art

| Pattern | Source | Why we adopted it |
|---------|--------|-------------------|
| Progressive MCP tool discovery | [GobbyAI/gobby](https://github.com/GobbyAI/gobby) | Don't expose all tools at once. Surface tools per agent phase. Saves ~30-40% tokens in system prompt. |
| Sleep-time compute | [Letta/MemGPT](https://github.com/letta-ai/letta) | Use idle time (HITL wait, child agent wait) for background memory consolidation, embedding, dedup. Free work. |
| OTP supervision strategies | [ZeptoPM](https://github.com/qhkm/zeptopm) (Rust/Erlang) | one_for_one, one_for_all, rest_for_one restart strategies. Battle-tested patterns from Erlang/OTP. |
| Memory lifecycle states | [MemOS](https://www.reddit.com/r/LocalLLaMA/comments/1qkrhec/) | Generated → Active → Merged → Archived → Forgotten. Prevents unbounded graph growth. Also: next-scene prediction for pre-loading context. |
| Transactional checkpoints with rollback | [SagaLLM](https://arxiv.org/abs/2503.11951) | SQLite transactions for atomic checkpoints. Operator can roll back to any prior version. Cheap insurance. |
| Interconnected memory notes | Inspired by [A-MEM](https://arxiv.org/abs/2502.12110) (NeurIPS 2025) | Bidirectional links between related memories. Zettelkasten-style. Only 1,200-2,500 tokens vs 16,900 for MemGPT. |
| Progressive disclosure for retrieval | [claude-mem](https://github.com/thedotmack/claude-mem) (42k stars) | Index → summary → detail. Initial retrieval cheap (~200-500 tokens), full detail on demand. |
| Append-only event log | [Gas Town](https://github.com/steveyegge/gastown) (beads ledger) | Full event history for debugging, crash recovery, and TUI display. Every MCP call logged synchronously. |
| Novelty filter | Original (validated by research gap analysis) | Compare output vs existing memories, persist only new information. Prevents unbounded growth across checkpoint cycles. |

---

## Risk Assessment

### Technical Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Checkpoint quality degrades over many cycles (summarization drift) | Medium | High | Layered storage (raw L2 preserved). Rollback available. Memory survey (arXiv:2603.07670) warns about this specifically. |
| Cold start tax — respawned agent performs worse than original | Medium | Medium | Measure empirically in MVP-1. L1 summary quality is key. If too degraded, switch to HybridSummarizer. |
| sqlite-vec Rust bindings not production-ready | Low | High | Fallback: usearch or hora for vector index alongside SQLite metadata. |
| Claude CLI doesn't support required spawn arguments | Low | High | Validate early in MVP-1. Open Question in spec. |
| Token self-reporting is inaccurate | Medium | Medium | Safety buffer (force-kill at 85%). MCP traffic estimate as cross-check. |
| Better models make this unnecessary | Low | Medium | External orchestration is complementary to larger windows (validated by 2026 memory survey). Even with 10M context, managed retrieval outperforms brute-force. |

### Scope Risks

| Risk | Mitigation |
|------|------------|
| MVP-1 is too large | The single-agent loop is the minimal viable proof. Cut TUI features if needed (text-only log first, panels later). |
| Integration glue dominates effort | This was identified as the main risk upfront. Meridian is fundamentally an integration project. The spec is detailed precisely to reduce glue-layer ambiguity. |

---

## What Meridian Is Not

- **Not a framework** — it's a runtime. You don't import it, you run it.
- **Not an API wrapper** — it uses Claude Code CLI, not the Anthropic API. No per-token costs (Claude Max subscription).
- **Not a replacement for Claude Code** — it's a supervisor that manages Claude Code instances.
- **Not a general-purpose agent framework** — it's specifically designed for the context-reset lifecycle pattern.
- **Not a database** — it uses SQLite/Qdrant for persistence, but the value is in the lifecycle management, not the storage.
