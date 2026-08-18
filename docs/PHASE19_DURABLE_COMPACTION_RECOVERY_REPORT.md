# Phase 19 Durable Compaction and Crash-Safe Recovery

**Project:** un1c0 local-first AI-programmable agent runtime
**Scope:** durable compaction manifests, staged snapshot cutover, startup recovery, and follower frontier validation
**Author:** Manus AI

## Architecture

Phase 19 extends Phase 18 in-memory compaction with a bounded file-backed durability boundary. `CompactionManifest` binds cluster and source identity, term, last-included index and term, retained suffix frontier, snapshot state hash, configuration hash, serialized snapshot digest, lifecycle, and manifest hash.

`DurableCompactionStore` writes a validated configuration-bound snapshot and manifest to separate staging files. Each file is fsynced before cutover. The snapshot is atomically renamed into its durable path, the manifest lifecycle is changed to `Committed`, the manifest is fsynced and atomically renamed, and the containing directory is synchronized. The prior durable pair is not removed before the new staged pair validates.

## Recovery

`recover_compaction` distinguishes three outcomes. `NoStaging` means no staged artifact exists. A complete snapshot/manifest pair with matching hashes is finalized through the same commit path and returns `Finalized`. Missing, malformed, mismatched, wrong-frontier, or tampered staging is removed and returns `Aborted`; the prior durable snapshot remains authoritative.

Recovery is deterministic and idempotent. Re-running recovery after an abort finds no staging. Re-running recovery after a committed cutover also finds no staging. The consensus core does not schedule compaction, choose retention policy, or open network connections.

## Evidence matrix

| Control | Evidence |
|---|---|
| Manifest and snapshot hash binding | Manifest creation and validation test. |
| Staged artifacts are not durable before commit | `load_latest` returns none after `stage`. |
| Atomic committed load | Commit test reloads identical snapshot and committed manifest. |
| Partial staging safety | Missing manifest staging returns `Aborted` and preserves the prior durable pair. |
| Snapshot-rename recovery | Recovery finalizes a valid snapshot when the process stops before manifest rename. |
| Previous-pair rollback | Invalid staged manifest after snapshot promotion restores the previous durable pair. |
| Tamper rejection | Altered staged snapshot digest is aborted without promotion. |
| Idempotent recovery | Repeated recovery returns `NoStaging` after cleanup or finalization. |
| Invalid retained frontier | Manifest creation fails before any file is staged. |

## Production boundary

The Phase 19 integration suite now passes seven tests covering manifest binding, staged visibility, atomic commit/load, partial staging, snapshot-rename finalization, previous-pair rollback, tampered manifests, idempotent recovery, and invalid frontiers. The local tests prove file-backed ordering and recovery behavior in one process. Production still requires storage quotas, backup retention, encryption-at-rest policy, fsync error telemetry, cross-device rename handling, operator approval for compaction, and multi-host recovery testing.
