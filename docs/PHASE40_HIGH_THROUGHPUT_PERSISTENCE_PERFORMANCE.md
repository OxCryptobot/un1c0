# Phase 40 High-Throughput Persistence Performance Report

## Scope and workload

Phase 40 verifies the accepted persistence path under bounded concurrency rather than measuring only a serial workload. The sanitized benchmark used **4 workers**, **32 operations per worker**, and a **4,096-byte payload**, for **128 concurrent local atomic persistence operations**. Each operation used a unique bounded staging/target pair. Four stale staging files were seeded before workers started, and no cluster mutation or secret-bearing material was used.

## Observed local results

| Metric | Observed value | Interpretation |
|---|---:|---|
| Requested operations | 128 | 4 workers × 32 operations |
| Completed operations | 128 | Accepted-path completion was lossless in this run |
| Failed operations | 0 | No worker or persistence failure occurred |
| Unique target count | 128 | Every successful operation had a distinct target |
| Stale staging recovery scans | 4 | One seeded stale file recovered per worker |
| Wall time | 13,265 µs | End-to-end concurrent benchmark interval |
| Throughput | 9,649.453 operations/s | Sanitized integer milli-operations-per-second field converted to operations/s |
| File `fsync` p95 | 343 µs | Contention-observed local file sync timing |
| Directory `fsync` p95 | 132 µs | Post-rename directory sync timing |
| Total persistence p95 | 523 µs | Write, file sync, close, rename, and directory sync |
| Total persistence maximum | 761 µs | Maximum observed operation timing |
| RSS before / active workers / after | 1,116 / 1,116 / 1,312 KiB | Active-worker snapshot is captured separately from post-join state |
| Threads before / active workers / after | 1 / 5 / 1 | Four workers plus the process thread were observed during execution |
| Open FDs before / active workers / after | 5 / 7 / 5 | Active-worker resource snapshot captured concurrent descriptor pressure |
| Resource budget | Within budget | 256 MiB RSS, 64 threads, and 256 open FDs |

The throughput number is derived from the bounded local run and is not a capacity promise. It includes concurrent filesystem operations in the sandbox environment, not network, TLS, database, replication, or external-fencing overhead.

## Implementation verification

The concurrent path validates payload, worker, per-worker operation, and total-operation bounds before filesystem mutation. It creates an isolated run directory, seeds bounded stale staging files when requested, and uses deterministic worker/operation names. Each worker removes its stale file if present, opens a unique staging file with `create_new`, writes the payload, calls file `sync_all`, closes the file, renames staging to a unique target, and calls directory `sync_all`. Worker panics and persistence failures propagate as failed measurements rather than being hidden in a partial throughput result. Completion, timing-sample, and unique-target counts must all equal the requested operation count.

The run directory is removed after worker joins, including the error path. The report records active-worker RSS, thread, and file-descriptor observations in addition to pre/post snapshots. Sanitized output contains counters, durations, bounded resource dimensions, and explicit `secret_material_recorded=false` and `cluster_mutation_performed=false` flags.

## Criteria and remaining gaps

Phase 40 closes the local gaps around concurrent accepted-path throughput, unique staging/target accounting, seeded staging recovery, contention timing, and active-worker resource observation. It still does not establish cross-process coordination for multiple independent callers sharing one logical target, device queue depth, write amplification, storage-controller flush barriers, managed-volume semantics, replicated-filesystem durability, network/TLS cost, database commit latency, cgroup throttling, allocator fragmentation, or cross-failure-domain recovery.

The unique per-operation staging design is intentionally a verification fixture. A production adapter that writes one logical supervision snapshot must use an explicit ownership/serialization policy or a durable compare-and-swap protocol; unique filenames alone do not establish single-writer authority. A local successful `fsync` and rename must never be treated as proof that a remote or managed storage system has durably replicated the record.
