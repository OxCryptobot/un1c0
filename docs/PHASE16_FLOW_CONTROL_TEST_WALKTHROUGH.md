# Phase 16 Flow-Control Test Walkthrough

## Purpose

`tests/phase16_replication_flow_control_integration.rs` exercises the complete bounded replication-window contract. The fixture builds a three-member cluster, elects `node-a`, configures two entries per batch, a 64 KiB serialized-byte budget, and a ten-tick retry backoff.

## Backpressure-window coverage

`flow_control_limits_each_peer_to_one_in_flight_batch` appends three entries and sends one batch to `node-b` at tick 0. A second preparation for the same peer at tick 1 returns `Backpressured { retry_at_tick: None }`, proving the active in-flight window blocks duplicate sends. The test then prepares a batch for `node-c` at tick 1 and receives `Send`, proving windows are independent and a slow `node-b` does not block `node-c`. The per-peer counters confirm one send for each peer.

`successful_ack_releases_window_and_preserves_quorum_commit_rules` sends a batch to a real follower, feeds the follower’s response into `ReplicationBatchAck`, and verifies the leader commits only after the quorum acknowledgement. It also proves that the window is released, the batch ID is recorded as completed, the acknowledgement counter increments, and the replicated state is applied.

`failed_ack_uses_backoff_and_is_sendable_at_exact_retry_boundary` sends at tick 5, returns a failed append acknowledgement, and expects retry tick 15. Tick 14 remains backpressured; tick 15 is eligible for `Send`. This exact-boundary test prevents both premature retry storms and unnecessary extra delay.

`invalid_batch_size_does_not_mutate_window_state` configures a one-byte budget, causes batch validation to fail, and verifies that no in-flight ID, sent counter, or completed ID is recorded. Validation therefore happens before window mutation.

`stale_or_duplicate_acknowledgements_fail_closed_without_progress_mutation` submits a wrong batch ID and verifies commit remains zero. It then accepts one valid acknowledgement and rejects the identical acknowledgement a second time. The test proves that neither stale nor duplicate responses can advance replication state.

`higher_term_response_steps_down_and_clears_in_flight_windows` injects an acknowledgement from a higher term. The leader becomes a follower and later preparation returns `NotLeader`, demonstrating that old leader flow state cannot continue after term authority changes.

`clock_uncertainty_blocks_flow_controlled_sends_until_reanchored` prepares a batch at tick 10, supplies a backward tick at 5, and expects `ClockUntrusted` for another peer. Explicit re-anchoring at tick 5 restores trusted operation and allows a new peer send.

## Invariants demonstrated

| Invariant | Test evidence |
|---|---|
| At most one batch per peer | Second `node-b` preparation returns `Backpressured`. |
| Slow peers are isolated | `node-c` sends while `node-b` is backpressured. |
| Exact retry boundary is safe | Tick 14 blocks; tick 15 sends. |
| Invalid work has no side effects | Oversized batch leaves counters and IDs unchanged. |
| Quorum remains authoritative | Successful follower acknowledgement advances commit; send admission alone does not. |
| Old leaders cannot continue | Higher-term acknowledgement forces follower state. |
| Clock uncertainty fails closed | Backward tick blocks sends until explicit re-anchoring. |
