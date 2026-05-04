//! Verification test for slice-1b step 18a: checkpoint round-trip against
//! fake_claude must produce exactly 0 `BusEvent::CheckpointSaved` emissions
//! and exactly 1 `SessionEvent::CheckpointReached` per checkpoint.
//!
//! Slice-1b integration window status: kept `#[ignore]`. The connector
//! call site (Task A) wires `CheckpointReached` from the
//! `ContentBlock::McpToolResult` payload, but the end-to-end harness this
//! test would need (an MCP stub server the connector can talk to via the
//! HTTP MCP transport, plus a test seam on `ClaudeCodeSession` that
//! exposes the bus the MCP handler writes to) does not exist yet.
//!
//! Slice 4 (crash + resume) introduces the `SessionRegistry` plumbing
//! that gives this test a real harness; the un-ignore lands there. Until
//! then the assertion is preserved as `unreachable!()` to fail loudly if
//! the file is run directly without the harness.

#[ignore = "needs MCP stub harness; un-ignored in slice 4 after SessionRegistry lands"]
#[test]
fn checkpoint_round_trip_emits_zero_bus_events() {
    // Outline (to be filled in during slice 4 integration):
    //   1. spin up fake_claude with a scenario that hits CheckpointReached
    //   2. connect ClaudeCodeSession with the MCP stub wired to a
    //      `broadcast::Sender<BusEvent>` we control
    //   3. drive the session to a checkpoint
    //   4. count BusEvent::CheckpointSaved on the receiver — assert 0
    //   5. count SessionEvent::CheckpointReached in the store — assert 1
    unreachable!("see #[ignore] reason");
}
