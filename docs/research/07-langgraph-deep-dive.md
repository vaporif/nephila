# LangGraph Deep Dive: Architecture, Patterns & Relevance to Nephila

**Date:** 2026-03-31
**Source:** Local clone of `langgraph` repo (libs/langgraph, libs/prebuilt, libs/checkpoint)

---

## Executive Summary

LangGraph is a Python framework for building stateful, long-running agent workflows. It uses a **graph-based execution model** inspired by Google's Pregel (bulk synchronous parallel) and Apache Beam. Nodes are functions, edges define control flow, and state flows through typed channels with reducer functions. It is fundamentally an **API-call orchestrator** — it calls LLMs via `BaseChatModel.invoke(messages)` and manages message lists, not processes.

Nephila and LangGraph are complementary, not competing. LangGraph solves *workflow composition* (what runs when, how state flows between steps). Nephila solves *agent process lifecycle* (what happens when context fills up). LangGraph has no answer for context exhaustion; Nephila has no declarative workflow composition. Several LangGraph patterns are worth adapting into Nephila's Rust/MCP architecture.

---

## How LangGraph Uses LLMs

LangGraph treats LLMs as **stateless function calls** via LangChain's `BaseChatModel` interface:

```python
# The entire LLM interaction model
model = ChatAnthropic(model="claude-3-7-sonnet")
response = model.invoke(messages)  # -> AIMessage with optional tool_calls
```

Key characteristics:
- **No persistent process.** Each `invoke()` is a single API call (messages in, message out).
- **No filesystem/shell access.** The LLM only has tools explicitly registered via `bind_tools()`.
- **Framework manages state.** LangGraph maintains the message list, applies reducers, checkpoints between steps.
- **Incompatible with Claude Code.** Claude Code is a long-running interactive process with its own tools (Read, Write, Bash, etc.) and context management. It doesn't fit `BaseChatModel.invoke()`.

This is the fundamental architectural divide: LangGraph orchestrates API calls, Nephila orchestrates processes.

---

## Core Abstractions

### 1. StateGraph (graph/state.py)

The primary builder. Users define a typed state schema (TypedDict or Pydantic), add nodes (functions), and connect them with edges. Compiling produces a `CompiledStateGraph` (which is a `Pregel` instance).

```python
class State(TypedDict):
    messages: Annotated[list[BaseMessage], add_messages]  # reducer = add_messages
    count: int                                             # reducer = last-write-wins

graph = StateGraph(State)
graph.add_node("agent", agent_fn)
graph.add_node("tools", tool_fn)
graph.add_edge(START, "agent")
graph.add_conditional_edges("agent", should_continue, {"continue": "tools", "end": END})
graph.add_edge("tools", "agent")
compiled = graph.compile(checkpointer=MemorySaver())
```

**What matters for Nephila:** The graph is a DAG of named steps with conditional routing. This is workflow composition — something Nephila's objective tree currently lacks.

### 2. Channels (channels/)

Typed containers that hold state values between graph steps. Each channel has a reducer that determines how multiple updates are merged.

| Channel Type | Behavior | Example |
|---|---|---|
| `LastValue` | Keep most recent write | Simple fields (`count: int`) |
| `BinaryOperatorAggregate` | Apply binary operator to merge | `Annotated[list, operator.add]` — concatenate |
| `EphemeralValue` | Reset each step (not persisted) | Temporary scratch data |
| `NamedBarrierValue` | Block until all named writers have written | Synchronization point |
| `Topic` | Pub/sub — all values visible | Broadcasting |

Interface (`BaseChannel`):
- `get() -> Value` — read current value
- `update(values: Sequence[Update])` — apply updates via reducer
- `checkpoint() -> Checkpoint` — serialize for persistence
- `from_checkpoint(checkpoint)` — restore from persistence

**What matters for Nephila:** The reducer concept solves multi-agent state aggregation. When 3 agents produce findings, how do you merge them? Currently Nephila has no answer. Channels with reducers provide one.

### 3. Send (types.py:574)

Dynamically invoke a node with custom state. Used in conditional edges for fan-out (map-reduce) patterns:

```python
def continue_to_jokes(state: OverallState):
    return [Send("generate_joke", {"subject": s}) for s in state["subjects"]]

# Spawns N parallel "generate_joke" nodes, one per subject
# Results aggregated via reducer on the "jokes" channel
```

**What matters for Nephila:** This is the fan-out primitive. Nephila has `spawn_agent` but no "spawn N agents for a list of items and aggregate when all complete" pattern.

### 4. Command (types.py:653)

Allows a node to simultaneously update state AND control routing:

```python
def my_node(state):
    return Command(
        update={"result": value},     # state update
        goto="next_node",             # explicit routing
        # OR goto=[Send("a", x), Send("b", y)]  # fan-out
    )
```

Also supports:
- `resume=value` — resume from an interrupt
- `graph=Command.PARENT` — send command to parent graph (subgraph communication)

**What matters for Nephila:** Agents could return both state updates and routing decisions in a single MCP tool call, rather than having the orchestrator infer next steps.

### 5. Interrupt / interrupt() (types.py:446, 705)

Pauses graph execution, checkpoints state, and waits for external input:

```python
def review_node(state):
    answer = interrupt("Please approve this action")  # pauses here
    # Resumed later via Command(resume="approved")
    return {"approved": answer}
```

The key detail: the human can **modify state** before resuming, not just provide an answer.

**What matters for Nephila:** Nephila's HITL (`request_human_input`) only supports Q&A. Allowing the operator to edit L0 state or objectives before resuming would be more powerful.

### 6. RetryPolicy (types.py:404)

Per-node retry configuration:

```python
RetryPolicy(
    initial_interval=0.5,   # seconds before first retry
    backoff_factor=2.0,     # exponential backoff multiplier
    max_interval=128.0,     # cap on backoff
    max_attempts=3,         # total attempts including first
    jitter=True,            # randomize interval
    retry_on=default_retry_on,  # exception filter
)
```

**What matters for Nephila:** Nephila's supervisor tracks restarts globally. Per-objective retry with backoff would be more granular.

### 7. Pregel Execution Engine (pregel/)

The runtime that executes compiled graphs. Follows BSP (bulk synchronous parallel):

1. Determine which nodes are triggered (channels have updates)
2. Execute triggered nodes concurrently ("superstep")
3. Collect writes, apply to channels via reducers
4. Checkpoint state
5. Repeat until no more nodes triggered or END reached

**What matters for Nephila:** Not directly applicable. Nephila's "nodes" are OS processes, not coroutines. But the checkpoint-after-every-step discipline is worth noting — Nephila checkpoints on token threshold, not on step boundaries.

### 8. Streaming (multiple modes)

```python
# "updates" — node outputs as they happen
# "values" — full state after each step
# "messages" — AI message tokens (for chat UIs)
# "custom" — user-defined via StreamWriter
for chunk in graph.stream(input, stream_mode="updates"):
    print(chunk)
```

**What matters for Nephila:** Nephila has an internal event bus but no external streaming API. When web UI is added, these stream modes are a good reference.

---

## Multi-Agent Patterns in LangGraph

### Supervisor Pattern
One "supervisor" node decides which worker to route to:
```python
graph.add_conditional_edges("supervisor", route_to_worker, {"research": "researcher", "write": "writer"})
```

### Map-Reduce (Fan-Out/Fan-In)
Use `Send` to spawn parallel workers, reducer to aggregate:
```python
def fan_out(state):
    return [Send("worker", {"task": t}) for t in state["tasks"]]
# All worker outputs merged via reducer into parent state
```

### Subgraphs
Nest a compiled graph as a node in a parent graph. State is mapped between inner/outer schemas. Communication via `Command(graph=Command.PARENT)`.

### Handoff
One agent yields control to another by returning `Command(goto="other_agent")`.

---

## What LangGraph Does NOT Do

1. **Context window management** — No concept of token tracking, context thresholds, or process lifecycle.
2. **Semantic memory** — Only a basic KV store (`BaseStore`). No embeddings, no vector search, no memory lifecycle.
3. **Process-level agents** — All "agents" are Python functions called in-process. No spawning of external processes.
4. **Supervision/restart** — Retry policies exist per-node, but no OTP-style supervision strategies.
5. **Sidecar model** — LangGraph controls everything. There's no concept of an agent that has its own tools and autonomy (like Claude Code).

---

## Comparison Matrix

| Concern | LangGraph | Nephila |
|---|---|---|
| Language | Python | Rust |
| LLM interaction | API calls (`model.invoke()`) | Process spawning (Claude CLI) |
| Workflow definition | Graph (nodes + edges) | Objective tree (static hierarchy) |
| State model | Channels with reducers | 3-layer checkpoint (L0/L1/L2) |
| Conditional routing | `add_conditional_edges` | Not implemented |
| Fan-out/fan-in | `Send` + reducers | `spawn_agent` (no aggregation) |
| Human-in-the-loop | `interrupt()` + `Command(resume=)` | `request_human_input` (Q&A only) |
| Persistence | Pluggable checkpointers | SQLite + sqlite-vec |
| Memory | Basic KV store | Semantic vector memory with lifecycle |
| Context lifecycle | None | Core feature (spawn/checkpoint/kill/respawn) |
| Supervision | Per-node retry | OTP-style strategies |
| Streaming | 4 modes (updates/values/messages/custom) | Internal event bus only |
| Agent autonomy | None (framework controls everything) | Full (Claude Code has own tools) |

---

## Key Takeaways for Nephila

1. **Conditional routing** is the highest-value, lowest-effort steal. Transform the static objective tree into a dynamic graph where completion of one objective can conditionally trigger the next.

2. **Reducer-based state aggregation** solves the multi-agent merge problem. Three agents producing findings need a merge strategy — channels with reducers provide it cleanly.

3. **Send/fan-out** is the natural multi-agent primitive. "Spawn N agents for these items, gate parent on all completing" is a pattern Nephila should support natively.

4. **Interrupt with state modification** extends HITL beyond Q&A. Let the operator edit objectives, reassign agents, or modify L0 state before resuming.

5. **RetryPolicy per objective** is a small but valuable addition to the existing supervisor.

6. **Don't steal the execution engine.** Pregel/BSP is for in-process coroutine scheduling. Nephila's agents are OS processes — the orchestrator loop is the right model.
