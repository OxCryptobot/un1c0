# Phase 19: Durable Compaction Manifests and Crash-Safe Recovery

## Objective

Make Phase 18 compaction durable across process failure without moving persistence or scheduling authority into the consensus state machine. The caller writes a validated configuration-bound snapshot and a compaction manifest through an approval-controlled durable store, then performs an atomic cutover. Startup recovery either finalizes a fully validated staged cutover or removes incomplete staging while preserving the last committed snapshot.

## Manifest contract

`CompactionManifest` binds cluster identity, source node, source term, last-included index and term, retained suffix start/end, snapshot state hash, configuration hash, serialized snapshot hash, and manifest hash. It records a bounded lifecycle: `Staged`, `Committed`, or `Aborted`. The manifest is not accepted when any hash or frontier is inconsistent with the staged snapshot.

## Durable sequence

1. Validate the configuration-bound snapshot and retained suffix metadata in memory.
2. Write the snapshot bytes to a unique staging file and fsync the file.
3. Write the manifest to a separate staging file and fsync the manifest.
4. Atomically rename the snapshot into the durable snapshot path.
5. Atomically rename the manifest into the durable manifest path.
6. Fsync the containing directory and expose the cutover as committed.

The implementation never deletes the prior durable snapshot before the new snapshot and manifest are validated. A process abort between steps leaves recoverable staging artifacts.

## Recovery semantics

`recover_compaction` validates any staged snapshot and manifest pair. A complete pair with matching hashes and valid metadata can be finalized. A missing pair, malformed pair, mismatched hash, wrong cluster, or invalid frontier is removed as aborted staging, while the previous durable snapshot remains authoritative. Recovery is deterministic and idempotent.

## Follower catch-up

The manifest exposes the exact last-included frontier and configuration hash used by Phase 18 snapshot-required catch-up. A follower must reject a manifest that would move backward, change configuration without a valid snapshot, or claim a retained suffix inconsistent with its snapshot frontier.

## Production boundary

The durable store owns files, fsync, atomic rename, and startup recovery. The consensus core still owns only bounded validation and typed catch-up decisions. Deployment code remains responsible for scheduling compaction, backup retention, storage errors, and transport delivery.
