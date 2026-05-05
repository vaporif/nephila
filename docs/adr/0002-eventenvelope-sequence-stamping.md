# 0002. EventEnvelope sequence stamping

Date: 2026-05-04
Status: superseded by ADR-0003 (2026-05-05)
Spec: `docs/specs/2026-05-03-claude-session-streaming-design.md`
Plan: `docs/plans/2026-05-03-claude-session-streaming.md` (Task 3 step 4)

> **Superseded.** ADR-0003 keeps the writer-stamps-sequence design but replaces
> the runtime invariant (`sequence: u64` + `debug_assert!`) with a compile-time
> one (private field + `EventEnvelope::new` constructor + writer-only
> `set_sequence`). The "writer continues from MAX" behavior on reads is
> unchanged.

## Context

Slice 1b moves sequence assignment for `EventEnvelope` from caller-supplied to
store-stamped. The writer thread computes
`MAX(sequence) WHERE aggregate_type=? AND aggregate_id=?` lazily on first
touch and stamps incoming envelopes inside the same SQLite transaction as the
`INSERT INTO domain_events`. Sequence assignment was previously the caller's
responsibility, which (a) couples every producer to read-then-write
serialisation it cannot enforce on its own, and (b) makes the spec's concurrent
producer story (multiple `append_batch` calls on the same aggregate) unsound
without an external lock.

Two shapes were on the table:

| Option | Pros | Cons |
| --- | --- | --- |
| **Option 1** — change `sequence: u64` to `Option<u64>` filled by the store | Compile-time guarantee that callers can't pre-populate | Touches every existing `EventEnvelope` construction site (Agent flows, tests) — bigger diff; ergonomic regression on reads (every consumer unwraps) |
| **Option 2** — keep `sequence: u64`, document "stamped by store; caller passes 0" | Smaller diff; tests stay readable; reads stay ergonomic | Easy to mis-set — relies on convention plus a writer-side overwrite contract |

## Decision

**Option 2.** `EventEnvelope::sequence` stays `u64`. The writer thread
**unconditionally overwrites** the caller-supplied value when stamping. Callers
construct fresh envelopes with `sequence: 0`. On reads, this is the assigned
sequence.

Two guards keep the contract honest:

1. **Debug-build assertion.** The writer asserts `env.sequence == 0` before
   stamping. A non-zero caller-supplied value is a programming error and the
   writer panics in debug builds to surface the bug fast.
2. **Release-build contract test.** A unit test calls `append_batch` with
   `env.sequence = 999` and verifies the store overwrites it (the row is
   persisted with sequence 1, not 999). This pins the unconditional-overwrite
   behavior so a future "helpful" patch that early-returns when sequence is
   pre-set cannot land without breaking the test.

The combination — debug assert plus release-build contract test — covers
mis-use during development (loud failure) and the upgrade path (a stale caller
that still pre-stamps gets its value silently corrected, never silently
honored).

## Consequences

Positive:

- Concurrent producers work without external coordination — the writer
  serialises sequence assignment.
- Existing rows in `nephila.db` with caller-stamped sequences continue to
  replay correctly: the writer warms its `next_sequence` cache from
  `MAX(sequence)`, so post-1b appends start from `max + 1` per aggregate.
- Two regression tests in `crates/store/tests/sequence_stamping_upgrade.rs`
  pin both the upgrade path (writer continues from MAX) and gapped-history
  semantics (no auto-renumbering of pre-existing gaps).

Negative:

- Forgetting `sequence: 0` in a fresh envelope construction site is silently
  corrected at runtime in release builds. Code review and the debug-build
  assertion are the mitigations.
- The `make_envelope(_, _, seq)` test helper in `crates/store/src/domain_event.rs`
  takes a `sequence` parameter and passes it directly into the row INSERT
  (bypassing the writer-stamping path) — that helper is now strictly
  `#[cfg(test)]`-only, used to seed pre-1b-shape rows for upgrade tests.

## Why this is hard to reverse

Switching from Option 2 → Option 1 later would touch every `EventEnvelope`
construction site (production code and tests) — the same diff Option 1 would
incur upfront, plus a migration. Switching from Option 1 → Option 2 later
would be a smaller diff (drop the `Option`), but Option 1's `Option<u64>` would
already have leaked through every consumer-side `unwrap()`. Neither direction
is impossible; both are noisy enough to want to pick once.
