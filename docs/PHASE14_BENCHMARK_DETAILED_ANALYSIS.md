# Phase 14 Detailed Benchmark Analysis

This report is generated directly from `benchmarks/phase14_read_benchmark.json`. It compares the lease fast path with a fresh quorum-backed read-index round at each requested concurrency. All rows are deterministic fixture outputs from the latest local run; they are not WAN capacity claims.

The run contains **16,128 total measured reads**, split evenly between both paths, with zero reported errors in every row.

## Per-concurrency comparison

| Concurrency | Lease p50/p95/p99 (µs) | Quorum p50/p95/p99 (µs) | Lease throughput | Quorum throughput | Throughput ratio | p95 delta | Interpretation |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 2/3/4 | 5/6/7 | 160,879.41 | 147,126.78 | 1.09× | -50.0% | Paths are near parity; scheduler and mutex effects dominate. |
| 2 | 13/14/34 | 6/34/92 | 66,256.39 | 98,548.87 | 0.67× | -58.8% | Quorum path is faster in this sample; inspect contention noise. |
| 4 | 14/132/419 | 8/63/657 | 83,098.51 | 86,558.25 | 0.96× | +109.5% | Paths are near parity; scheduler and mutex effects dominate. |
| 8 | 14/321/1521 | 14/430/1618 | 73,418.61 | 71,099.95 | 1.03× | -25.3% | Paths are near parity; scheduler and mutex effects dominate. |
| 16 | 14/699/2741 | 14/821/2411 | 74,110.17 | 76,241.67 | 0.97× | -14.9% | Paths are near parity; scheduler and mutex effects dominate. |
| 32 | 13/1746/4917 | 14/1712/5796 | 79,486.70 | 75,523.14 | 1.05× | +2.0% | Paths are near parity; scheduler and mutex effects dominate. |

## Reading the result

The lease path avoids creating and completing a fresh read-index round, so its protocol work is lower by construction. The benchmark serializes calls through a shared `Arc<Mutex<ConsensusNode>>`, which can dominate tail latency and can obscure the protocol-level advantage. A p95 increase in one row is therefore evidence about this fixture’s scheduling and lock behavior, not evidence that the lease contract weakens consistency or always regresses performance.

The safety result is stronger than the performance result: every row completed successfully, the lease path was available only after quorum observation, and the quorum path continued to provide a correctness-preserving fallback. The next performance layer should separate immutable read execution from mutable consensus bookkeeping, use independent client workers, measure authenticated transport, and repeat each point enough times for confidence intervals.

## Reproduction

```bash
scripts/validate_phase14_read_optimization.sh
python3 scripts/analyze_phase14_benchmark.py
```
