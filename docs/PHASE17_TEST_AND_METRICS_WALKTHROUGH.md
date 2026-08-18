# Phase 17 Test and Security Metrics Walkthrough

## Per-stream ordering

`enqueue_is_idempotent_and_pending_replays_in_stream_order` creates two records in the same `consensus` stream, enqueues sequence 2 before sequence 1, repeats sequence 1, and then reads pending entries. The outbox returns exactly two entries ordered as source sequences 1 and 2. The repeated identical enqueue does not create a duplicate because the envelope hash is the durable filename and identical bytes are accepted idempotently.

## Gap handling

`awaiting_predecessor_retains_outbox_and_acceptance_removes_it` enqueues sequence 1 and submits a signed `AwaitingPredecessor` acknowledgement. The sink returns `false` and the pending entry remains. A second signed `Accepted` acknowledgement for the same envelope returns `true`, removes the outbox file, synchronizes the directory, and leaves no pending entries. The test therefore proves that a remote gap or retryable decision cannot silently discard audit evidence.

## Ed25519 verification

`envelope_signature_and_cluster_binding_fail_closed` constructs a source envelope using the source signing key and validates it against a trusted historical signer registry. It then mutates the record hash without recomputing the envelope signature and expects rejection. A valid envelope presented for a different cluster is also rejected. `acknowledgement_binding_and_sink_signature_are_verified` verifies that a sink acknowledgement must bind the exact envelope hash and that a signature from an untrusted sink key is rejected.

## Collision protection

`same_stream_sequence_with_different_hash_is_rejected` constructs two different records that both occupy source sequence 1 in the same stream. The first is accepted. The second has a different envelope hash and is rejected with `RemoteAuditCollision`. This prevents an alternate event from replacing an already durable stream position.

## Compliance artifact

`benchmarks/security_compliance_metrics.json` records 26 passed gates. The Phase 17 section records `envelope_signature_binding`, `per_stream_sequence_order`, `idempotent_enqueue`, `gap_and_retry_retention`, and `accepted_ack_removes_and_syncs_directory` as true. The security notes state that no secret material or cluster mutation was recorded and that the external sink remains a durable file-backed idempotent outbox rather than an asserted remote quorum.
