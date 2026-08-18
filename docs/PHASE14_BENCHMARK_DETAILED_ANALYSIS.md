# Phase 14 Detailed Benchmark Analysis

This report is generated directly from `benchmarks/phase14_read_benchmark.json`. It compares the lease fast path with a fresh quorum-backed read-index round at each requested concurrency. All rows are deterministic fixture outputs from the latest local run; they are not WAN capacity claims.

The run contains **16,128 total measured reads**, split evenly between both paths, with zero reported errors in every row.

## Per-concurrency comparison

| Concurrency | Lease p50/p95/p99 (µs) | Quorum p50/p95/p99 (µs) | Lease throughput | Quorum throughput | Throughput ratio | p95 delta | Interpretation |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 2/3/4 | 5/11/12 | 177,126.59 | 129,665.69 | 1.37× | -72.7% | Lease path has a clear throughput advantage in this sample. |
| 2 | 5/13/48 | 6/14/28 | 133,669.60 | 106,714.55 | 1.25× | -7.1% | Lease path has a clear throughput advantage in this sample. |
| 4 | 13/176/599 | 13/154/514 | 85,017.97 | 75,988.94 | 1.12× | +14.3% | Lease path has a clear throughput advantage in this sample. |
| 8 | 13/437/1811 | 13/364/1786 | 71,403.98 | 70,462.88 | 1.01× | +20.1% | Paths are near parity; scheduler and mutex effects dominate. |
| 16 | 13/940/2911 | 13/605/3200 | 77,028.09 | 77,521.64 | 0.99× | +55.4% | Paths are near parity; scheduler and mutex effects dominate. |
| 32 | 13/2147/5738 | 13/1935/6004 | 76,513.65 | 74,478.93 | 1.03× | +11.0% | Paths are near parity; scheduler and mutex effects dominate. |

## Reading the result

The lease path avoids creating and completing a fresh read-index round, so its protocol work is lower by construction. The benchmark serializes calls through a shared `Arc<Mutex<ConsensusNode>>`, which can dominate tail latency and can obscure the protocol-level advantage. A p95 increase in one row is therefore evidence about this fixture’s scheduling and lock behavior, not evidence that the lease contract weakens consistency or always regresses performance.

The safety result is stronger than the performance result: every row completed successfully, the lease path was available only after quorum observation, and the quorum path continued to provide a correctness-preserving fallback. The next performance layer should separate immutable read execution from mutable consensus bookkeeping, use independent client workers, measure authenticated transport, and repeat each point enough times for confidence intervals.

## Reproduction

```bash
scripts/validate_phase14_read_optimization.sh
python3 scripts/analyze_phase14_benchmark.py
```
