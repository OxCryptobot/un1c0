# Phase 39 Resource and Durability Audit Notes

## Baseline

The published baseline is `56a517c` on `origin/main`. Phase 38 provides signed fencing-authority heartbeats, exact consumer acknowledgements, bounded supervision state, atomic snapshots, and a 137-gate compliance artifact. The worktree was clean before this audit.

## Verified heartbeat evidence path

`FencingAuthorityHeartbeat::sign` constructs a fixed-domain/version record, binds cluster/resource, authority identity, membership and fence generations, committed log index, token/state hashes, observation tick, TTL, and the signer public key. It validates shape before signing. Ed25519 signs the canonical JSON payload, then `event_hash` commits the signed record including the signature. `verify` validates shape, expected cluster/resource, registry-pinned authority key equality, signature, and event hash before a supervisor can use the evidence.

`FencingSupervisor::ingest_heartbeat` then rejects future-dated or expired evidence, authority identity changes, generation rollback, and same-generation conflicting evidence. Exact duplicates are idempotent. A new heartbeat is installed only after cryptographic and freshness checks; if the bounded journal append fails, the authority state is restored. Conflict paths quarantine the supervisor fail-closed.

## Verified consumer acknowledgement path

`FenceConsumerAcknowledgement::sign` binds cluster/resource, authority, consumer identity and kind, token hash, owner region, membership/fence generations, observation tick, TTL, outcome, and signer public key into the canonical payload. `verify` checks shape, cluster/resource, registry-pinned consumer key equality, Ed25519 signature, and content digest before admission.

`ingest_consumer_acknowledgement` requires an active authority, an exact registered consumer kind, non-future and unexpired evidence, and equality with the current authority identity, membership epoch, fence epoch, and token hash. Existing evidence is monotonic and exact duplicate delivery is idempotent; same-fence conflicting evidence quarantines. Consumer state is rolled back if the journal append fails. Readiness requires fresh authority evidence, complete configured consumer coverage, no quarantined outcome, fresh acknowledgements, and exact generation/token binding.

## Identified hardening gap

The acknowledgement includes `owner_region_id`, but the heartbeat does not currently carry an owner-region field. Consequently, a correctly signed acknowledgement can be bound to the current authority/token generations while naming a different owner region; the supervisor has no authority-signed owner-region value against which to compare it. Phase 39 should add an authority-signed `owner_region_id` and require exact acknowledgement equality before readiness.

Snapshot restore validates cardinality, all signed records, the state hash, and the journal chain. The key registry and required-consumer policy remain supervisor configuration rather than snapshot data, which is correct for preventing a snapshot from silently changing trust policy. Future-dated evidence cannot enter through ingestion; restored evidence still requires a caller-provided current tick at evaluation time.

## Resource and fsync gaps

The Phase 37 direct benchmark measured process wall time, child user/system CPU, peak RSS, and output size. It did not measure bytes written, file-descriptor count, thread count, allocator behavior, per-operation CPU, network bytes, fsync duration, directory-sync duration, staging retries, recovery scans, or persistence failure rates. The benchmark workload was rejection-heavy and had too few valid telemetry events to characterize accepted-path latency.

Phase 39 should add a bounded, sanitized persistence/resource measurement seam. It should measure file bytes and operation counts around snapshot save/load, record sync durations through an injectable clock rather than claiming kernel-level timing in the core, expose bounded resource snapshots from `/proc` only in the benchmark adapter, and keep the cryptographic supervision core independent of host-specific resource APIs. Fsync metrics must remain observational: they cannot turn a successful local sync into a claim of replicated or managed-storage durability.

## Production boundary

The local system can prove canonical signatures, key pinning, exact bindings, bounded evidence, atomic local staging, and fail-closed readiness. It cannot prove cross-region network timing, cloud failure truth, external process termination, database admission, routing convergence, mTLS lifecycle, key custody, replicated filesystem semantics, or independent failure-domain supervision. Any Phase 39 resource report must label these as deployment boundaries and must not present local fsync timing as production durability evidence.
