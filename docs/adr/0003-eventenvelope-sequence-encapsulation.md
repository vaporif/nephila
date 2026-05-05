# 0003. EventEnvelope sequence encapsulation

Date: 2026-05-05
Status: accepted
Supersedes: ADR-0002

## Context

ADR-0002 chose Option 2 for sequence stamping: keep `EventEnvelope::sequence`
public (`u64`), instruct callers to set `0`, and have the writer thread
overwrite the value during the INSERT transaction. Two guards backed the
contract:

1. A `debug_assert!(env.sequence == 0)` in the writer (caught misuse in
   development).
2. A release-build regression test pinning the unconditional-overwrite.

After a few months of practice, the negative consequence enumerated in
ADR-0002 — "forgetting `sequence: 0` in a fresh envelope construction site is
silently corrected at runtime in release builds" — felt large enough to revisit.
Reviewers reading code with `sequence: some_local_var` in a struct literal had
to remember the writer-stamps-sequence rule themselves; the type signature
gave them no help.

ADR-0002 framed the trade-off as Option 2 (status-quo `u64`) vs Option 1
(switch to `Option<u64>`). Option 1 was rejected because every consumer-side
read would have to `unwrap` the Option even though it is never `None` after the
writer stamps it.

A third shape exists that ADR-0002 did not consider: **encapsulate** the field.

## Decision

`EventEnvelope::sequence` becomes a **private** `u64` with a public method
surface that mirrors the original direct-field semantics minus the foot-gun:

```rust
pub struct EventEnvelope {
    pub id: EventId,
    pub aggregate_type: String,
    pub aggregate_id: String,
    sequence: u64,                       // private
    pub event_type: String,
    pub payload: serde_json::Value,
    pub trace_id: TraceId,
    pub outcome: Option<Outcome>,
    pub timestamp: DateTime<Utc>,
    pub context_snapshot: Option<ContextSnapshot>,
    pub metadata: HashMap<String, String>,
}

pub struct NewEventEnvelope {
    // mirrors EventEnvelope minus `sequence`
}

impl EventEnvelope {
    pub fn new(args: NewEventEnvelope) -> Self { /* sequence = 0 */ }
    pub fn sequence(&self) -> u64 { self.sequence }
    pub fn set_sequence(&mut self, seq: u64) { self.sequence = seq; }
}
```

The construction contract is now enforced by the type system: callers in any
crate must go through `EventEnvelope::new`, and `new` always seeds `sequence`
to `0`. There is no syntax that pre-stamps a sequence at construction.

The writer thread continues to call `env.set_sequence(next)` inside its
INSERT transaction. The release-build regression test in
`crates/store/tests/sequence_stamping_upgrade.rs` is preserved (it now uses
`set_sequence(999)` to exercise deliberate post-construction mutation, which
remains possible by design — see Consequences).

The `debug_assert!` on `env.sequence == 0` in the writer is dropped: a fresh
envelope always satisfies it, and a writer-stamped envelope re-fed through
`append` would now legitimately carry a non-zero sequence (the test path).

## Consequences

Positive:

- **Compile-time enforcement.** A new construction site that mistakenly tries
  to seed a sequence (`EventEnvelope { sequence: 99, ... }`) does not
  compile outside the eventsourcing crate. ADR-0002's "silently corrected at
  runtime" footnote is gone.
- **Reads stay ergonomic.** `env.sequence` becomes `env.sequence()` — one
  pair of parens, no `unwrap`, no `Option`-induced churn. The cost paid by
  Option 1 (consumer-side `unwrap`) is avoided.
- **Serde wire format unchanged.** The private field still carries the JSON
  name `"sequence"`; rows already in `nephila.db` deserialize unchanged.

Negative:

- **One-shot migration.** 37 construction sites and 12 read sites were
  rewritten in this ADR's commit. New code must remember to use
  `EventEnvelope::new(NewEventEnvelope { ... })` instead of struct-literal
  syntax — but the compiler enforces it.
- **`set_sequence` is `pub`.** Making it `pub(crate)` to the `eventsourcing`
  crate would fence out the store writer, which lives in `nephila-store`.
  The bug we are catching is at construction (struct-literal mistakes), not
  deliberate post-construction mutation; `set_sequence` is rare and easy to
  spot in code review.

## Why this is hard to reverse

Switching back to public `sequence` would re-open the silent-correction hole
ADR-0002 documented. There is no path forward from this state that we are
likely to want.

## Migration notes

- `EventEnvelope { ..., sequence: X, ... }` → `EventEnvelope::new(NewEventEnvelope { ... })` plus an explicit `env.set_sequence(X)` if `X != 0` (only needed in tests that pre-seed sequences).
- `env.sequence` → `env.sequence()` (12 read sites in production code; tests use the same).
- Writer-side `env.sequence = next` → `env.set_sequence(next)`.
