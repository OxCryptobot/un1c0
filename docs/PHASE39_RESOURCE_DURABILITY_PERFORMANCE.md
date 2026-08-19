# Phase 39 Resource and Durability Performance Report

## Scope and method

Phase 39 closes the first local measurement gap identified in the Phase 37 report. The new bounded instrumentation measures atomic snapshot write workload, bytes written, staging recovery scans, file `fsync` p95, directory `fsync` p95, total persistence p95/max, and sanitized process resource dimensions. The benchmark uses 64 valid writes of a 4,096-byte payload into a disposable local directory. It does not mutate a cluster and records no secrets.

## Observed local result

| Metric | Observed value | Interpretation |
|---|---:|---|
| Valid persistence operations | 64 | Accepted-path workload rather than rejection-only fuzzing |
| Bytes written | 262,144 bytes | 4,096 bytes per operation, counted before any replication/network effects |
| File `fsync` p95 | 209 µs | Local filesystem flush timing for the benchmark environment |
| Directory `fsync` p95 | 69 µs | Local directory-entry durability timing after atomic rename |
| Total atomic persistence p95 | 287 µs | Write, file sync, rename, and directory sync in this local run |
| Total atomic persistence maximum | 731 µs | Maximum of 64 local operations |
| Staging recovery scans | 0 | No stale staging file was present during the benchmark |
| Staging retries | 0 | No retryable persistence-open failure occurred |
| Resource snapshot before/after | 1 thread; 5 open FDs; 1,100 KiB RSS | Process-local observation from `/proc` in the benchmark process |
| Budget status | Within budget | Budget was 256 MiB RSS, 64 threads, and 256 open FDs |

A separate host-level probe measured 64 operations at approximately 57.917 µs file-`fsync` p95, 18.047 µs directory-`fsync` p95, and 166.101 µs total p95. The difference from the instrumented example is expected from workload, process startup, filesystem cache state, and measurement placement; the numbers should not be combined into one confidence interval.

## Controls added

The implementation rejects empty or oversized payloads, zero or excessive operation counts, and oversized bounds before filesystem mutation. It removes stale staging files before each operation, writes with `create_new`, performs file sync before rename, performs directory sync after rename, counts bytes, reports bounded process dimensions, and evaluates optional RSS/thread/file-descriptor budgets fail closed. Sanitized evidence exposes counters and durations only, with explicit `secret_material_recorded=false` and `cluster_mutation_performed=false` flags.

The external-fencing contract was also hardened. `FencingAuthorityHeartbeat` is now protocol v2 and signs `owner_region_id` into both its canonical Ed25519 payload and content digest. A consumer acknowledgement is accepted only when its owner region exactly equals the authority-signed owner region in addition to matching authority identity, membership epoch, fence epoch, and token hash. A validly signed wrong-region acknowledgement is rejected before consumer state mutation.

## Remaining production gaps

These measurements remain local observations. They do not establish replicated or managed-storage durability, cloud-region failure truth, network latency or bytes, TLS/mTLS overhead, database commit latency, cross-process resource accounting, cgroup throttling, allocator fragmentation, disk queue depth, or failure-domain independence. `/proc` dimensions are Linux-process snapshots, not fleet-level accounting. The instrumentation does not yet measure per-operation CPU, write amplification, actual storage-device flush barriers, recovery scan duration under large journals, or persistence latency under concurrent writers and injected failures.

Production readiness therefore still requires an external supervisor, independent key custody, real storage and replication tests, cgroup/resource telemetry, network and TLS instrumentation, process/socket/database enforcement acknowledgements, clock-health evidence, and failure-domain chaos testing. Local `fsync` success must never be interpreted as proof that a cloud-managed volume or replicated filesystem has durably committed the record.
