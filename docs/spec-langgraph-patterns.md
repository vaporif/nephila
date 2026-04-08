# Spec: LangGraph-Inspired Patterns for Nephila

**Date:** 2026-03-31
**Status:** Draft
**Prerequisite:** MVP-1 MCP tool wiring must be complete first. These features are MVP-2+.
**Research:** See `docs/research/07-langgraph-deep-dive.md`

---

## Overview

Six patterns from LangGraph adapted to Nephila's Rust/MCP/process-level architecture. Ordered by implementation priority. Each pattern specifies what changes in core types, store traits, MCP tools, orchestrator logic, and TUI.

---

## 1. Conditional Routing

**Problem:** Nephila's objective tree is a static hierarchy. When an agent completes an objective, the orchestrator has no way to dynamically choose what runs next based on results.

**LangGraph equivalent:** `add_conditional_edges(source, path_fn, path_map)`

### Design

Routing rules are defined per-objective in the goals markdown or programmatically. When an objective completes, the orchestrator evaluates its routing function to determine the next objective(s) to activate.

#### Core types

```rust
// crates/core/src/routing.rs (new file)

use crate::id::ObjectiveId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A routing rule evaluated when an objective completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Objective whose completion triggers this rule.
    pub source: ObjectiveId,
    /// Named conditions mapped to target objectives.
    /// The orchestrator evaluates conditions in order; first match wins.
    pub routes: Vec<Route>,
    /// Fallback if no condition matches.
    pub default: ObjectiveId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// Human-readable condition (evaluated against agent's L0 state).
    /// Examples: "findings.len > 0", "status == approved", "always"
    pub condition: RouteCondition,
    /// Target objective to activate.
    pub target: ObjectiveId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteCondition {
    /// Always matches (unconditional edge).
    Always,
    /// Match if L0 state field equals value.
    FieldEquals { field: String, value: serde_json::Value },
    /// Match if L0 state field exists and is non-empty.
    FieldPresent { field: String },
    /// Match if L0 state field is a list with len > threshold.
    ListMinLength { field: String, min: usize },
    /// Agent explicitly returned this route name via MCP tool.
    ExplicitRoute { name: String },
}
```

#### New MCP tool

```
complete_with_route(route_name: String, state_update: JsonValue)
```

Allows an agent to declare completion AND specify which route to take. Combines LangGraph's `Command(update=..., goto=...)` pattern. Falls back to condition evaluation if agent doesn't specify.

#### Orchestrator changes

In `orchestrator.rs`, when handling `BusEvent::ObjectiveUpdated { status: Done, .. }`:

1. Look up `RoutingRule` for the completed objective
2. If agent provided explicit route via `complete_with_route`, use it
3. Otherwise, load the completing agent's L0 state and evaluate conditions
4. Activate the matched target objective (set to `InProgress`, spawn agent)

#### Store changes

```rust
// Add to ObjectiveStore trait
fn set_routing_rule(&self, rule: RoutingRule) -> impl Future<Output = Result<()>> + Send;
fn get_routing_rule(&self, source: ObjectiveId) -> impl Future<Output = Result<Option<RoutingRule>>> + Send;
```

New SQLite table:
```sql
CREATE TABLE routing_rules (
    source_objective_id TEXT PRIMARY KEY,
    routes_json TEXT NOT NULL  -- serialized Vec<Route>
);
```

#### Goals markdown extension

```markdown
## Research the problem
- route_if: findings.len > 0 -> Implement solution
- route_if: findings.len == 0 -> Research deeper
- route_default: Research deeper

## Implement solution
- route_if: always -> Review implementation

## Review implementation
- route_if: approved -> END
- route_default: Implement solution
```

#### TUI changes

Show routing edges in the objective tree panel. When an objective completes, briefly highlight the chosen route.

---

## 2. Fan-Out / Aggregate

**Problem:** No way to spawn N agents for a list of items and gate a parent objective on all completing.

**LangGraph equivalent:** `Send("node", {"item": x})` + channel reducers for aggregation.

### Design

A new MCP tool lets an agent request fan-out. The orchestrator spawns N child agents and blocks the parent objective until all complete. Results are aggregated into shared state (see pattern 5) or into the parent's L0.

#### New MCP tool

```
fan_out(
    items: Vec<FanOutItem>,
    aggregate_strategy: AggregateStrategy,
)

struct FanOutItem {
    description: String,      // objective description for child agent
    context: JsonValue,       // initial context passed to child
    directory: Option<String>, // working dir override
}

enum AggregateStrategy {
    /// Collect all results into a list in parent's L0 under `results_key`.
    CollectAll { results_key: String },
    /// Merge all L0 states using a merge function.
    MergeL0,
    /// Don't aggregate — parent reads child checkpoints manually.
    None,
}
```

#### Core types

```rust
// crates/core/src/fanout.rs (new file)

use crate::id::{AgentId, ObjectiveId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutGroup {
    pub id: uuid::Uuid,
    pub parent_objective: ObjectiveId,
    pub parent_agent: AgentId,
    pub child_objectives: Vec<ObjectiveId>,
    pub aggregate_strategy: AggregateStrategy,
    pub status: FanOutStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FanOutStatus {
    /// Children still running.
    InProgress,
    /// All children completed, aggregation done.
    Completed,
    /// One or more children failed.
    PartialFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AggregateStrategy {
    CollectAll { results_key: String },
    MergeL0,
    None,
}
```

#### Orchestrator changes

When `fan_out` tool is called:
1. Create N child objectives under the parent objective
2. Set parent objective to `Blocked`
3. Spawn N agents (one per child objective)
4. Track the `FanOutGroup` in store

When a child objective completes:
1. Check if all siblings in the fan-out group are `Done`
2. If yes: run aggregation strategy, unblock parent, resume parent agent with aggregated data injected
3. If a child fails: follow per-objective retry policy (pattern 4). If exhausted, mark group as `PartialFailure` and unblock parent with partial results.

#### Store changes

```rust
// New trait or extend ObjectiveStore
fn create_fan_out_group(&self, group: FanOutGroup) -> impl Future<Output = Result<()>> + Send;
fn get_fan_out_group(&self, parent: ObjectiveId) -> impl Future<Output = Result<Option<FanOutGroup>>> + Send;
fn update_fan_out_status(&self, group_id: uuid::Uuid, status: FanOutStatus) -> impl Future<Output = Result<()>> + Send;
```

New SQLite table:
```sql
CREATE TABLE fan_out_groups (
    id TEXT PRIMARY KEY,
    parent_objective_id TEXT NOT NULL,
    parent_agent_id TEXT NOT NULL,
    child_objective_ids TEXT NOT NULL,  -- JSON array
    aggregate_strategy TEXT NOT NULL,   -- JSON
    status TEXT NOT NULL DEFAULT 'in_progress'
);
```

#### TUI changes

Show fan-out groups in the agent tree — display progress as "3/5 completed". Highlight blocked parent.

---

## 3. Per-Objective Retry Policy

**Problem:** Supervision config is global (`max_restarts: 5`). Different objectives have different tolerance for failure.

**LangGraph equivalent:** `RetryPolicy(max_attempts, backoff_factor, ...)` per node.

### Design

#### Core types

```rust
// Add to crates/core/src/objective.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Max attempts including the first. 0 = no retries.
    pub max_attempts: u32,
    /// Initial delay before first retry (seconds).
    pub initial_interval_secs: u64,
    /// Multiply delay by this factor after each retry.
    pub backoff_factor: f64,
    /// Cap on retry delay (seconds).
    pub max_interval_secs: u64,
    /// What to do when retries exhausted.
    pub on_exhausted: ExhaustedPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ExhaustedPolicy {
    /// Mark objective as failed, propagate to parent.
    Fail,
    /// Mark objective as blocked, notify operator via HITL.
    Escalate,
    /// Skip this objective, continue workflow.
    Skip,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_interval_secs: 5,
            backoff_factor: 2.0,
            max_interval_secs: 300,
            on_exhausted: ExhaustedPolicy::Escalate,
        }
    }
}
```

#### Changes to ObjectiveNode

```rust
pub struct ObjectiveNode {
    // ... existing fields ...
    pub retry_policy: RetryPolicy,     // NEW
    pub attempt_count: u32,            // NEW
}
```

#### Orchestrator changes

When an agent fails (crashes, exits with error):
1. Load objective's `retry_policy`
2. If `attempt_count < max_attempts`: increment count, calculate delay, schedule respawn after delay
3. If retries exhausted: execute `on_exhausted` policy

#### Store changes

Add `retry_policy_json TEXT`, `attempt_count INTEGER DEFAULT 0` columns to `objectives` table.

#### Goals markdown extension

```markdown
## Flaky API integration
- retry: max_attempts=5, backoff=2.0, on_exhausted=escalate

## Best-effort data enrichment
- retry: max_attempts=2, on_exhausted=skip
```

---

## 4. HITL State Modification

**Problem:** `request_human_input` only supports Q&A (question + options -> response string). The operator can't edit agent state or objectives before resuming.

**LangGraph equivalent:** `interrupt(value)` + `Command(resume=modified_value)` where the human can modify state.

### Design

#### Extended HITL modal in TUI

When an agent pauses (via HITL or manual pause), the TUI modal shows:

1. **Current question** (if HITL request) — existing behavior
2. **Editable L0 state** — JSON editor showing current L0, operator can modify fields
3. **Objective reassignment** — change which objective the agent is working on
4. **Directive override** — force a specific directive (Continue, PrepareReset, Abort)

#### New OrchestratorCommand

```rust
// Add to command.rs
HitlRespondWithState {
    agent_id: AgentId,
    response: String,
    l0_override: Option<serde_json::Value>,
    objective_override: Option<ObjectiveId>,
    directive_override: Option<Directive>,
}
```

#### Orchestrator changes

When `HitlRespondWithState` is received:
1. If `l0_override` is Some: save modified L0 to checkpoint store, set agent's injected_message to include it
2. If `objective_override` is Some: reassign agent to new objective
3. If `directive_override` is Some: set directive
4. Resume agent with response

#### TUI changes

Extend the HITL modal (`modal.rs`) with:
- A JSON text editor widget for L0 state (using `tui-textarea` or similar)
- A dropdown for objective selection
- A dropdown for directive override
- Tab navigation between the panels

---

## 5. Shared State with Reducers

**Problem:** Multiple agents working on related objectives have no way to share intermediate state. Each agent has isolated L0/L1/L2. If three agents produce findings, there's no merge mechanism.

**LangGraph equivalent:** Channels with `BinaryOperatorAggregate` reducers.

### Design

A shared key-value state that agents read/write via MCP tools. Each key has a configured reducer that merges concurrent writes.

#### Core types

```rust
// crates/core/src/shared_state.rs (new file)

use serde::{Deserialize, Serialize};

/// Strategy for merging concurrent writes to the same key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reducer {
    /// Replace with latest write. Default for scalar values.
    LastWriteWins,
    /// Append to list. For collecting results from multiple agents.
    Append,
    /// Merge JSON objects (shallow). Later keys override earlier.
    MergeObject,
    /// Numeric addition.
    Sum,
    /// Keep the maximum value.
    Max,
    /// Keep the minimum value.
    Min,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedStateKey {
    /// Namespace (usually the root objective ID).
    pub namespace: String,
    /// Key name.
    pub key: String,
    /// How to merge concurrent writes.
    pub reducer: Reducer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedStateEntry {
    pub namespace: String,
    pub key: String,
    pub value: serde_json::Value,
    pub version: u64,
    pub last_writer: crate::id::AgentId,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

#### New store trait

```rust
pub trait SharedStateStore: Send + Sync {
    /// Write a value. The store applies the configured reducer.
    fn write(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        writer: AgentId,
        reducer: Reducer,
    ) -> impl Future<Output = Result<SharedStateEntry>> + Send;

    /// Read current merged value.
    fn read(
        &self,
        namespace: &str,
        key: &str,
    ) -> impl Future<Output = Result<Option<SharedStateEntry>>> + Send;

    /// Read all keys in a namespace.
    fn read_namespace(
        &self,
        namespace: &str,
    ) -> impl Future<Output = Result<Vec<SharedStateEntry>>> + Send;
}
```

#### New MCP tools

```
write_shared_state(
    key: String,
    value: JsonValue,
    reducer: Option<String>,  // "append" | "merge" | "sum" | "last_write_wins" | ...
                              // Defaults to "last_write_wins"
)

read_shared_state(key: String) -> JsonValue

read_all_shared_state() -> Map<String, JsonValue>
```

Namespace is derived from the agent's root objective (so agents working on the same objective tree share state).

#### SQLite table

```sql
CREATE TABLE shared_state (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,       -- JSON
    reducer TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    last_writer TEXT NOT NULL,  -- agent_id
    updated_at TEXT NOT NULL,
    PRIMARY KEY (namespace, key)
);
```

#### Reducer application (in store crate)

```rust
fn apply_reducer(
    current: Option<&serde_json::Value>,
    update: &serde_json::Value,
    reducer: &Reducer,
) -> serde_json::Value {
    match (reducer, current) {
        (Reducer::LastWriteWins, _) => update.clone(),
        (Reducer::Append, Some(Value::Array(arr))) => {
            let mut result = arr.clone();
            match update {
                Value::Array(new) => result.extend(new.iter().cloned()),
                other => result.push(other.clone()),
            }
            Value::Array(result)
        }
        (Reducer::Append, _) => Value::Array(vec![update.clone()]),
        (Reducer::MergeObject, Some(Value::Object(existing))) => {
            let mut merged = existing.clone();
            if let Value::Object(new) = update {
                merged.extend(new.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            Value::Object(merged)
        }
        (Reducer::Sum, Some(Value::Number(n))) => {
            // numeric addition
            let current_f = n.as_f64().unwrap_or(0.0);
            let update_f = update.as_f64().unwrap_or(0.0);
            json!(current_f + update_f)
        }
        _ => update.clone(),
    }
}
```

#### Usage example

Three agents researching different modules, writing findings to shared state:

```
Agent A: write_shared_state("findings", [{"module": "auth", "issues": [...]}], reducer="append")
Agent B: write_shared_state("findings", [{"module": "api", "issues": [...]}], reducer="append")
Agent C: write_shared_state("findings", [{"module": "db", "issues": [...]}], reducer="append")

# Aggregation agent reads:
read_shared_state("findings")
# -> [{"module": "auth", ...}, {"module": "api", ...}, {"module": "db", ...}]
```

---

## 6. External Streaming API

**Problem:** Nephila's event bus is internal only (`broadcast::Sender<BusEvent>`). No way for external consumers (web UI, CI pipelines, monitoring) to observe agent activity.

**LangGraph equivalent:** `stream_mode=["updates", "values", "messages", "custom"]`

### Design

Expose filtered event streams over Server-Sent Events (SSE) on the existing Axum HTTP server.

#### Stream modes

| Mode | Events included | Use case |
|---|---|---|
| `events` | All `BusEvent` variants | Debugging, full observability |
| `state` | `AgentStateChanged`, `ObjectiveUpdated`, `CheckpointSaved` | Dashboard / web UI |
| `hitl` | `HitlRequested`, `HitlResponded` | External HITL responders (Slack bot, web form) |
| `tokens` | `TokenReport` | Monitoring, alerting |

#### HTTP endpoints

```
GET /stream?mode=events&agent_id=optional_filter
GET /stream?mode=state
GET /stream?mode=hitl
GET /stream?mode=tokens
```

Returns `text/event-stream` (SSE). Each event is JSON:

```
event: agent_state_changed
data: {"agent_id": "abc", "old_state": "active", "new_state": "draining", "timestamp": "..."}

event: token_report
data: {"agent_id": "abc", "used": 150000, "remaining": 50000}
```

#### Implementation

Add an SSE handler to the Axum router in `bin/src/main.rs`:

```rust
async fn stream_events(
    Query(params): Query<StreamParams>,
    State(event_rx): State<broadcast::Receiver<BusEvent>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(event_rx)
        .filter_map(move |event| {
            let event = event.ok()?;
            filter_by_mode(&event, &params.mode, params.agent_id.as_deref())
        })
        .map(|event| Ok(Event::default().event(event.name).data(event.json)));
    Sse::new(stream)
}
```

#### Future: WebSocket for bidirectional

SSE is one-directional (server -> client). For external HITL responders that need to *send* responses, add a WebSocket endpoint later:

```
WS /ws/hitl
```

This is deferred — SSE + existing MCP tools are sufficient for MVP-2.

---

## Implementation Order

| # | Pattern | Effort | Depends on | Crate changes |
|---|---|---|---|---|
| 1 | Conditional routing | Small | MVP-1 complete | core, store, orchestrator |
| 2 | Per-objective retry | Small | MVP-1 complete | core, store, orchestrator |
| 3 | HITL state modification | Small | MVP-1 complete | core, orchestrator, tui |
| 4 | Fan-out / aggregate | Medium | Patterns 1, 2 | core, store, mcp, orchestrator, tui |
| 5 | Shared state with reducers | Medium | MVP-1 complete | core, store, mcp |
| 6 | External streaming | Medium | Axum HTTP binding | bin (axum routes) |

Patterns 1-3 can land together as a batch. Pattern 4 builds on 1 and 2. Patterns 5 and 6 are independent of each other.

---

## What We're NOT Taking from LangGraph

| LangGraph concept | Why not |
|---|---|
| StateGraph builder DSL | Over-engineered for process-level agents. Routing rules + fan-out cover the same ground with less abstraction. |
| Pregel/BSP execution | Solves in-process coroutine scheduling. Our agents are OS processes managed by the orchestrator loop. |
| Channels as primary state | Too complex. Shared state with reducers (pattern 5) gets 80% of the value. Agent-local L0/L1/L2 remains the primary state model. |
| Functional API (`@task`/`@entrypoint`) | Python convenience. No equivalent need in a Rust binary that manages external processes. |
| LangChain integration | Different ecosystem entirely. We use Claude Code + MCP, not `BaseChatModel.invoke()`. |
