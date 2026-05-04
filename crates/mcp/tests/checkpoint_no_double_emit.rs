//! Verification test for slice-1b step 18a: checkpoint round-trip against
//! fake_claude must produce exactly 0 `BusEvent::CheckpointSaved` emissions
//! and exactly 1 `SessionEvent::CheckpointReached` per checkpoint.
//!
//! This test depends on the slice-1a connector wiring (`fake_claude` driver
//! and `ClaudeCodeSession::connect_test`) which lands in the slice-1b
//! integration window — see Task 3 step 12 in the plan. Until then this
//! test is `#[ignore]`-d.

#[ignore = "depends on slice-1a connector wiring landing in integration window"]
#[test]
fn checkpoint_round_trip_emits_zero_bus_events() {
    // Outline (to be filled in during integration):
    //   1. spin up fake_claude with a scenario that hits CheckpointReached
    //   2. connect ClaudeCodeSession with the MCP stub wired to a
    //      `broadcast::Sender<BusEvent>` we control
    //   3. drive the session to a checkpoint
    //   4. count BusEvent::CheckpointSaved on the receiver — assert 0
    //   5. count SessionEvent::CheckpointReached in the store — assert 1
    unreachable!("see #[ignore] reason");
}
