# Phase 12 Transport and Recovery Report

**Project:** un1c0 local-first AI-programmable agent runtime

**Scope:** authenticated socket framing, cluster configuration IDs, replay windows, sender binding, joint-consensus transport delivery, and simulated power-loss snapshot recovery

**Author:** Manus AI

## Joint-consensus mechanics reviewed

`ConfigurationJoint` is appended with both `old_members` and `new_members`, preserving the exact quorum relationship for existing and late-joining nodes. During the joint phase, election and commit checks require a majority intersection with both sets. A second transition is rejected while joint mode is active.

`ConfigurationFinal` is admitted only after the joint entry has committed. Its membership set becomes authoritative after the final entry commits. Followers apply the same command through the replicated log, clear the previous set, rebuild replication progress, and remain deterministic with the leader. A removed leader steps down rather than continuing to propose under an obsolete configuration.

## Authenticated socket transport

The Phase 12 transport uses a bounded four-byte big-endian length prefix followed by a serialized `AuthenticatedConsensusEnvelope`. The envelope signature binds the cluster configuration ID, sender ID, positive term, nonce, consensus message bytes, and public key. The receiver looks up the expected public key from trusted configuration before accepting the frame.

The transport rejects zero or oversized frames before payload allocation, malformed JSON, unknown senders, cluster mismatch, sender impersonation, public-key rebinding, term/message mismatch, invalid keys, invalid signatures, and duplicate nonces. The transport object itself is bound to one local node ID and cannot send on behalf of another trusted node.

The local implementation authenticates application frames over TCP. It does not claim confidentiality or peer certificate authentication; production deployment must layer mTLS or the approved zero-trust mesh and retain connection-level authorization.

## Simulated power-loss recovery

The dedicated `un1c0-failure-injector` process writes and fsyncs a partial snapshot staging file, then calls `abort` before rename. The integration test confirms the child exits unsuccessfully, `recover_staging` removes the incomplete temporary file, the target snapshot was never published by the interrupted write, and a subsequent valid snapshot saves and reloads successfully. A separate invalid-hash installation test confirms no term, commit-index, or state mutation occurs on rejection.

| Control | Result |
|---|---|
| Joint configuration double majority | Passed |
| ConfigurationFinal commit gate | Passed |
| Existing and late-node membership adoption | Passed |
| Loopback TCP valid delivery | Passed |
| Cluster-ID mismatch rejection | Passed |
| Duplicate replay rejection | Passed |
| Oversized-frame rejection | Passed |
| Untrusted-key rejection | Passed |
| Process-boundary power-loss staging recovery | Passed |
| Invalid snapshot rollback | Passed |

## Production boundaries

A production transport still needs mTLS or mesh confidentiality, durable replay epochs across restart, election/failure-detector timers, socket deadlines, bounded connection concurrency, backpressure, configuration-ID persistence tied to membership state, cross-machine partition tests, snapshot transfer recovery, and remote audit/transport metrics. The test suite is deterministic and local; it does not substitute for kernel, TLS, network, storage, or multi-host chaos evidence.
