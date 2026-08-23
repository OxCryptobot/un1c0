# Phase 74: Emission Diagnostic Network Transport Plan

**Author:** Manus AI
**Status:** Implemented locally; validation and commit are the remaining closeout gates.
**Scope:** Real loopback TCP streaming and bounded multi-node transport over the Phase 73 Ed25519 attestation layer.

## Objective

Phase 73 established deterministic Ed25519 attestations over canonical emission-diagnostic streams and verified aggregates. Phase 74 carries those attestations over a real length-prefixed TCP connection and introduces a bounded multi-node receiver that performs current-state semantic verification before aggregation. The phase deliberately keeps the trust primitive small: an explicitly supplied public-key registry, a handshake-bound node and connection identity, and per-connection sequence enforcement.

The network layer must reject malformed or unbounded input before it can reach the aggregation layer. It must not infer authorization, quorum, cluster membership, or production identity from a valid signature. A valid network exchange proves only that a peer presented the registered public key and sent a frame whose local framing and attestation checks passed.

## Typed design

| Boundary | Contract | Fail-closed behavior |
|---|---|---|
| Listener | `AuthenticatedDiagnosticListener::bind` accepts loopback or caller-selected TCP addresses and fixes the expected node ID and frame limit. | Reject zero node IDs, zero limits, and limits above the one-megabyte network buffer ceiling. |
| Handshake | Version, non-zero node ID, non-zero connection ID, public key, and a domain-separated SHA-256 handshake digest. | Reject unsupported versions, invalid identifiers, digest tampering, unexpected nodes, and public keys absent from the supplied verifier registry. |
| Frame | Version, node ID, connection ID, one-based sequence, Phase 73 attestation, and a domain-separated SHA-256 frame digest over the connection metadata plus canonical attestation JSON. | Reject malformed shapes, oversized payloads, digest mismatch, connection mismatch, key mismatch, replay, and gaps. |
| Connection | `AuthenticatedDiagnosticConnection` maintains independent send and receive sequence windows and a bounded 64-frame budget. | Refuse writes or reads after the per-connection frame limit; never advance a sequence counter on a rejected operation. |
| Multi-node receiver | `MultiNodeDiagnosticReceiver` registers up to eight node IDs with independent explicit verifier registries and aggregators. | Reject unregistered nodes, attestation failures, stale candidate state, downstream replay/gap errors, and any aggregation error before mutation. |

The connection verifies transport identity and framing. The receiver then calls the Phase 73 `verify_stream` path with the current semantic snapshot, target capability profile, and candidate UEG map. Only after that verification succeeds does it create a verified distributed observation and pass it to the per-node aggregator.

## Canonical wire framing

Each TCP message uses a four-byte big-endian length prefix followed by a JSON payload. The length is checked against both the configured frame limit and the one-megabyte network buffer ceiling before allocation. Empty messages, oversized frames, malformed JSON, unknown fields, invalid attestation shapes, and integrity mismatches are rejected with typed `EmissionDiagnosticNetworkError` values.

The handshake digest uses the domain `un1c0/phase74/emission-diagnostic-handshake/v1`. The frame digest uses `un1c0/phase74/emission-diagnostic-network/v1`. The attestation itself remains the Phase 73 domain-separated Ed25519 statement over the canonical stream content. This composition prevents a frame's node, connection, or sequence metadata from being silently changed without detection, while preserving Phase 73's exact content binding.

## Security and authority boundary

Phase 74 intentionally does not add TLS, certificate authorities, peer discovery, key distribution, durable queues, retries, durable acknowledgements, or cluster-level authorization. The caller must construct and supply the verifier registry. The listener does not persist trust state, and the multi-node receiver does not promote a valid attestation into quorum or authorization evidence.

The exact public-key membership check is required at handshake time. A non-empty verifier registry is insufficient: the handshake's presented key must be the specific registered key. The network code also uses a fallible public-key accessor for deserialized attestations so malformed remote key lengths become typed errors rather than panics. Current-state semantic verification remains the final gate before aggregation, which preserves Phase 72's all-or-nothing mutation behavior.

## Verification coverage

The integration suite uses only `127.0.0.1:0` and covers valid handshake/frame delivery, exact-key and node-identity mismatch, multi-node isolation, client replay/gap refusal, downstream aggregator replay/gap refusal, eight-node registration bounds, 64-frame connection bounds, invalid identifiers, unregistered-node rejection, stale candidate-state rejection, and zero frame-limit rejection.

## Benchmark plan

The benchmark uses deterministic Rust-target fixtures, twelve samples, frame counts of 1/2/4/8, and concurrent connection levels of 1/2/4/8. It reports end-to-end loopback latency, receive-stage latency, current-state stream verification latency, critical-path per-connection verification cost, aggregate frames per second, stream bytes, errors, and sanitized authority markers. It records no keys, signatures, source text, tokens, cluster mutations, or credentials.

The benchmark is a local engineering signal rather than a production capacity claim. A future optimization phase should isolate canonical JSON serialization, frame-domain hashing, semantic verification, and Ed25519 verification into separate timers before attempting SIMD or batching changes.

## Acceptance criteria

Phase 74 is complete when the network module is registered and formatted, the loopback integration suite passes, the benchmark emits all 16 frame/concurrency rows with zero errors, the plan and report are present, `AGENT_SYSTEM.md` names the phase and artifacts, and the reusable skill contains the Phase 74 reference and closeout guidance.

## References

[1]: ../src/emission_diagnostic_network.rs "Phase 74 authenticated diagnostic network implementation"
[2]: ../src/emission_diagnostic_attestation.rs "Phase 73 emission diagnostic attestation implementation"
[3]: ../src/emission_diagnostic_transport.rs "Phase 72 distributed-shaped diagnostic transport and aggregation"
[4]: ../benchmarks/phase74_emission_diagnostic_network.json "Phase 74 sanitized loopback benchmark artifact"
