# Phase 41 Replicated Durability and Single-Writer CAS Audit Notes

## Baseline

The published Phase 40 baseline is `0ae0744` on `origin/main`. The worktree is clean and remote parity is confirmed. Phase 40 passed 153 compliance gates and added bounded concurrent persistence verification to `resource_durability.rs`.

## Concurrent worker orchestration

`measure_concurrent_snapshot_persistence` validates payload size, worker count, operations-per-worker, checked multiplication, and total operation bounds before creating the run directory. It creates an isolated run directory, optionally seeds one stale staging file per worker, captures a pre-work resource snapshot, and uses `std::thread::scope` so worker lifetimes are joined before cleanup. Each worker receives a private payload clone and deterministic worker/operation identifiers.

The worker sequence is intentionally explicit: remove stale staging if present, open a unique staging path with `create_new`, write the payload, call file `sync_all`, drop the file, rename staging to a unique target, call directory `sync_all`, and append bounded timing samples. The parent joins every worker, converts panics into a persistence failure, removes the run directory, rejects any worker error or cleanup failure, aggregates samples, and requires completed operations and every sample vector to equal the requested operation count. Active-worker resource observations are captured at worker entry and merged by maximum optional RSS/thread/open-FD values.

The current fixture uses unique target names, so it verifies bounded concurrent accepted-path throughput but does not coordinate multiple writers for one logical snapshot. `unique_target_count` is derived from exact completion accounting rather than re-scanning the directory, which is sufficient for the deterministic naming fixture but not proof of a production ownership decision. The error path cleans the run directory after joined workers, but failures while seeding stale files before worker startup can return before cleanup; Phase 41 should close this cleanup hole.

## Contention and storage-controller findings

The Phase 40 benchmark observed 128/128 completed operations, 9,649.453 operations/s, file-sync p95 of 343 microseconds, directory-sync p95 of 132 microseconds, total persistence p95 of 523 microseconds, and active-worker resource values of 1,116 KiB RSS, 5 threads, and 7 open file descriptors. These are local sandbox observations. They exclude device queue depth, write amplification, storage-controller flush barriers, managed-volume semantics, replicated-filesystem commit, database commit latency, network/TLS, cgroup throttling, allocator behavior, and cross-failure-domain recovery.

A successful local `fsync` means the local kernel/filesystem accepted the flush request. It does not prove that a storage controller has forced stable media, that a cloud volume has replicated the write, that a remote filesystem has committed directory metadata, or that a second region can recover the record. Phase 41 must model these as explicit signed acknowledgements from independent durability replicas, not infer them from local latency.

## Phase 41 design requirements

Phase 41 should implement a bounded single-writer compare-and-swap contract over a hash-bound snapshot generation. A CAS request must bind cluster/resource, logical snapshot ID, writer identity, expected generation/hash, proposed generation/hash, payload digest, writer epoch, and request nonce. The local store must reject stale expected generations, same-generation hash conflicts, writer-epoch rollback, duplicate nonce conflicts, and unknown/rebound writers before mutation.

Replicated durability should require a quorum of independently registered replica acknowledgements. Each signed acknowledgement must bind the exact snapshot ID, generation, proposed hash, replica identity, replica durability mode, observed flush sequence, and request digest. A replica must not acknowledge before its own durable adapter reports the write committed. The coordinator may publish the new local CAS generation only after the configured quorum is reached; if quorum is lost or any conflicting acknowledgement appears, the operation fails closed and local state remains unchanged.

The local implementation can validate signatures, key pinning, generation/CAS semantics, nonce idempotence, quorum counting, and atomic local staging. It cannot prove stable media, remote controller flush, managed-volume replication, process fencing, or independent failure-domain placement. Those remain adapter and deployment gates.
