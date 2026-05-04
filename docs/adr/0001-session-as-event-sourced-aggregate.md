# 0001. Session as event-sourced aggregate

Date: 2026-05-03
Status: proposed
Spec: `docs/specs/2026-05-03-claude-session-streaming-design.md`

## Context

When we replace the current `claude -p` per-turn spawn with a persistent stream-json claude process per agent, several consumers need to observe what claude is doing: the TUI session pane (live render), the lifecycle supervisor (react to checkpoints), the orchestrator (handle crashes), and likely future consumers (audit, metrics, Slack notifier).

Two shapes were on the table:

- **Trait split with in-memory broadcast.** Add a `SessionConnector` trait alongside `TaskConnector`. `ClaudeCodeSession` exposes `subscribe() -> broadcast::Receiver<ClaudeEvent>`. Consumers register at runtime. Events are ephemeral.
- **Event-sourced aggregate.** Define `Session` as a new aggregate type in `nephila-eventsourcing` with a real `EventSourced` impl (state, command, `apply`, `handle`). The connector appends events to the durable store. Consumers subscribe through the store via a new `subscribe_after(seq)` primitive that joins backfill and live updates.

## Decision

Use the event-sourced aggregate, with a **real `EventSourced` impl** (not just an event taxonomy persisted into the same table). The connector is the sole producer per session aggregate; everything else is a consumer reading from the persisted log.

The aggregate has a reducer (`apply`) that enforces session invariants — turn boundaries, terminal-state lockout — so that consumers can rely on the state machine rather than re-deriving it.

## Consequences

Positive:

- Consumers join and leave without coordinating with the producer. Adding a new consumer (audit log, Slack alert) requires no change to `ClaudeCodeSession`.
- Session history survives nephila process restarts. The TUI pane can replay full history on resume; the supervisor can resume reasoning from `last_seen_seq` instead of starting blank.
- Aligns with the existing `EventSourced` aggregate model used by `Agent` (`crates/core/src/agent.rs:367`). `Session` will be the second `EventSourced` impl in the codebase — the first non-`Agent` aggregate. A previous draft of this ADR claimed `CheckpointNode` was also event-sourced; that was wrong (`crates/core/src/checkpoint.rs:7-16` is a plain serde struct).
- Time-travel and forking come naturally — same aggregate-tree machinery used for checkpoints.

Negative:

- More upfront work than a `tokio::sync::broadcast`: aggregate reducer, the `subscribe_after` primitive on `DomainEventStore`, snapshotting policy, retention primitive (no `prune` exists today; we add one).
- Event log grows on disk. Tool-heavy agents emit hundreds of events per turn. Snapshotting (focus-demand + a coarse periodic floor) plus retention keeps disk usage bounded.
- Two event taxonomies coexist transitionally: `SessionEvent` (durable, session-scoped) and the existing in-memory `BusEvent`. `BusEvent::CheckpointSaved` overlaps with `SessionEvent::CheckpointReached`. We keep both for slice 5; after, `BusEvent::CheckpointSaved` is deprecated in favor of consumers subscribing to the durable log.

## Why this is hard to reverse

If we ship the trait-split version and later want persistence, every consumer (TUI, supervisor, orchestrator, plus any third-party additions by then) has to migrate to a different subscription API and learn replay semantics. Doing that under load with multiple consumers in flight is far more expensive than building it once now.

Conversely, if we ship the event-sourced version and decide later we don't need persistence, we can keep the same shape and stop persisting (or persist with a short retention) without touching consumer code.

## Alternatives considered

- **Trait split with in-memory broadcast.** Smaller initial diff, but every advantage (replay, multi-consumer fan-out without producer changes, restart resilience) would need to be added later as a second migration. The savings now don't pay for the migration later.
- **Event log without a reducer (event-logging-as-event-sourcing).** Simpler — skip `Session` state and `apply` — but loses the ability to enforce invariants like "no events after `SessionEnded`" and "every prompt closes with `TurnCompleted` or `TurnAborted`." Since consumers (supervisor) already need that state machine, building it once in the aggregate is cheaper than redoing it in every consumer.
- **Unified `SessionConnector` for all connectors.** Forces a stream shape on `anthropic_api.rs` and `openai_compatible.rs`, which are genuinely request/response. Leaky abstraction at the trait layer with no benefit at the runtime layer.
