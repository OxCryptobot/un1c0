# Phase 11 Failure-Injection and Partition Report

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** joint-consensus membership changes, dynamic re-voting, durable snapshot crash recovery, rollback safety, and authenticated transport partition evidence
**Author:** Manus AI

## Executive assessment

Phase 11 adds a bounded joint-consensus protocol rather than replacing the active voter set in one step. A `ConfigurationJoint` entry carries both old and new membership sets and requires a majority in each set. A `ConfigurationFinal` entry is admitted only after the joint entry commits. Existing and late-joining nodes apply the same configuration payload, so they reconstruct the old/new quorum relationship deterministically.

The failure-injection suite crosses a process boundary: a dedicated helper writes a partial snapshot staging file, flushes it, and aborts before rename. Startup recovery removes the incomplete staging artifact, then a valid snapshot is saved and reopened. A separate invalid-hash installation test confirms that term, commit index, and state remain unchanged after rejection.

## Membership and recovery evidence

| Control | Evidence | Result |
|---|---|---|
| Double-majority joint consensus | `phase11_consensus_integration.rs` | Passed |
| Finalization only after joint commit | Phase 11 integration test | Passed |
| Existing follower adoption | Phase 11 integration test | Passed |
| Late-node adoption | Phase 11 integration test | Passed |
| Dynamic re-voting after final membership | Phase 11 integration test | Passed |
| Single-flight and unchanged-membership rejection | Phase 11 integration test | Passed |
| Process abort before snapshot rename | `un1c0-failure-injector` | Passed |
| Temporary-file cleanup and rewrite | `DurableSnapshotStore::recover_staging` | Passed |
| Invalid snapshot rollback | `failure_injection_integration.rs` | Passed |

## Authenticated transport partition metrics

The benchmark uses **2,000 samples per scenario**, deterministic Ed25519 keys, and in-process envelope verification. Messages crossing the simulated partition are dropped before signature verification. The benchmark is therefore evidence about envelope verification and partition filtering, not socket, TLS, kernel, storage, or cross-machine latency.

| Scenario | Connected members | Attempted | Verified | Dropped | Quorum available | Verification p95 | Verification throughput |
|---|---:|---:|---:|---:|:---:|---:|---:|
| Healthy | 5/5 | 10,000 | 10,000 | 0 | Yes | 29.315 µs | 34,964 ops/s |
| Majority partition | 3/5 | 10,000 | 3,600 | 6,400 | Yes | 28.303 µs | 34,840 ops/s |
| Minority partition | 2/5 | 10,000 | 1,600 | 8,400 | No | 28.253 µs | 34,820 ops/s |

The minority component must not be treated as healthy simply because its local signature verification remains fast. The state machine should refuse quorum-dependent commits there. The majority partition retains quorum availability, but the measured verification throughput is not a claim that inter-node replication continues without transport or application-level backpressure.

## Remaining production gates

Production promotion still requires a real authenticated transport with replay windows and a cluster/configuration ID, failure detectors and election timers, durable membership configuration backup/restore, cross-process and cross-machine partition tests, log compaction coordination, snapshot transfer backpressure, and remote audit-sink durability. No Kubernetes cluster was mutated, and no live secret or Vault operation was invoked by this validation.

## Reproduction

Run `scripts/validate_security_compliance.sh` from the repository root. It validates the reusable skill, Rust/Python/CLI surfaces, Helm fail-closed security, Phase 10 security integration, Phase 11 membership integration, crash recovery, partition benchmark generation, and isolated rootful Podman mTLS smoke. The non-secret JSON evidence is written to `benchmarks/security_compliance_metrics.json`.
