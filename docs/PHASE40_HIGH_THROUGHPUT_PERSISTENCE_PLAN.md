# Phase 40 High-Throughput Persistence Verification Plan

## Objective

Phase 40 extends the Phase 39 local persistence seam from a serial valid-path measurement to a bounded concurrent verification workload. The goal is to measure directory contention and accepted-path throughput while preserving atomic staging, file-sync-before-rename, directory-sync-after-rename, stale-staging recovery, and sanitized resource evidence.

The batch is an **observability and verification layer**, not a new durability authority. It must not claim that a local filesystem benchmark proves replicated storage, managed-volume barriers, or cross-region durability.

## Existing sequence under review

The Phase 39 path validates payload and operation bounds before creating the root directory. For each operation it removes a stale fixed-name staging file, opens that staging file with `create_new`, writes the bounded payload, calls file `sync_all`, closes the file, renames staging to the fixed target, opens the parent directory, and calls directory `sync_all`. It records timing only after the entire sequence succeeds. A failure returns before a `PersistenceMeasurement` is emitted.

This sequence is atomic for the single caller’s staging/rename order, but the fixed staging and target names are not a concurrent writer protocol. Two independent callers using the same root can race on stale-staging cleanup, `create_new`, rename, and directory synchronization. Phase 40 therefore uses unique per-worker/per-operation staging and target names for high-throughput verification rather than pretending that a process-local mutex proves cross-process serialization.

## Phase 40 acceptance criteria

| Criterion | Required evidence |
|---|---|
| Bounded concurrency | Worker count and operations-per-worker are validated against explicit maxima before any filesystem mutation |
| Accepted-path completeness | Completed operations equal the requested worker-operation product; failed operations are zero for the valid benchmark |
| Atomic ordering | Every successful sample performs write, file sync, close, atomic rename, and directory sync in that order |
| Unique staging/targets | Each worker-operation has a deterministic bounded unique pair; no shared fixed staging name is used in the concurrent path |
| Recovery | Seeded stale staging files are removed before their worker’s first write and recovery count is reported |
| Throughput | Wall duration and completed operations produce a sanitized operations-per-second value; p95/max operation timing is reported |
| Contention visibility | File-sync, directory-sync, and total-persistence p95/max values are reported separately under concurrent load |
| Resource bounds | Before/after RSS, thread count, open FDs, and optional fail-closed budget decisions are retained without raw process logs |
| No partial success claim | Any worker panic or persistence error fails the measurement rather than being silently counted as a passing throughput run |
| Production boundary | Reports explicitly state that local fsync, rename, and directory sync do not establish managed or replicated storage durability |

## Proposed valid benchmark profile

The default benchmark uses 4 workers and 32 operations per worker with a 4 KiB bounded payload and one seeded stale staging file per worker. A second profile may use 8 workers and 64 operations per worker if the runtime budget permits. The first profile is the compliance fixture; the second is a performance observation, not a gate.

## Planned implementation surface

Add a `ConcurrentPersistenceMeasurement` contract and `measure_concurrent_snapshot_persistence` function to `resource_durability.rs`. Add integration tests for bound rejection, stale-staging recovery, zero-loss completion, unique target accounting, worker panic/error propagation, resource-budget evaluation, and sanitized output. Add a sanitized Phase 40 benchmark, an eight-gate compliance section, a validator, a performance report, an architecture roadmap entry, and a reusable skill reference.

## Remaining production gaps

The local batch will not measure real disk queue depth, write amplification, storage-controller flush behavior, managed-database commit latency, network/TLS cost, cgroup throttling, allocator fragmentation, cross-process lock coordination, cross-host replication, process fencing, or failure-domain independence. Those remain deployment gates requiring production-like storage, independent supervision, and authorized failure injection.
