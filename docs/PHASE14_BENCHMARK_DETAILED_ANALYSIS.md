# Phase 14 Detailed Benchmark Analysis

This report is generated directly from `benchmarks/phase14_read_benchmark.json`. It compares the lease fast path with a fresh quorum-backed read-index round at each requested concurrency. All rows are deterministic fixture outputs from the latest local run; they are not WAN capacity claims.

The run contains **16,128 total measured reads**, split evenly between both paths, with zero reported errors in every row.

## Per-concurrency comparison

| Concurrency | Lease p50/p95/p99 (µs) | Quorum p50/p95/p99 (µs) | Lease throughput | Quorum throughput | Throughput ratio | p95 delta | Interpretation |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 2/4/6 | 5/6/7 | 159,589.66 | 138,110.67 | 1.16× | -33.3% | Lease path has a clear throughput advantage in this sample. |
| 2 | 12/18/24 | 14/16/222 | 77,459.26 | 75,259.83 | 1.03× | +12.5% | Paths are near parity; scheduler and mutex effects dominate. |
| 4 | 13/100/612 | 13/17/318 | 70,449.17 | 72,964.02 | 0.97× | +488.2% | Paths are near parity; scheduler and mutex effects dominate. |
| 8 | 14/47/2075 | 14/27/1536 | 69,326.73 | 70,998.52 | 0.98× | +74.1% | Paths are near parity; scheduler and mutex effects dominate. |
| 16 | 14/565/3652 | 14/661/4095 | 68,703.67 | 70,843.65 | 0.97× | -14.5% | Paths are near parity; scheduler and mutex effects dominate. |
| 32 | 14/1588/6883 | 13/2132/6645 | 70,101.82 | 77,216.03 | 0.91× | -25.5% | Paths are near parity; scheduler and mutex effects dominate. |

## Reading the result

The lease path avoids creating and completing a fresh read-index round, so its protocol work is lower by construction. The benchmark serializes calls through a shared `Arc<Mutex<ConsensusNode>>`, which can dominate tail latency and can obscure the protocol-level advantage. A p95 increase in one row is therefore evidence about this fixture’s scheduling and lock behavior, not evidence that the lease contract weakens consistency or always regresses performance.

The safety result is stronger than the performance result: every row completed successfully, the lease path was available only after quorum observation, and the quorum path continued to provide a correctness-preserving fallback. The next performance layer should separate immutable read execution from mutable consensus bookkeeping, use independent client workers, measure authenticated transport, and repeat each point enough times for confidence intervals.

## Reproduction

```bash
scripts/validate_phase14_read_optimization.sh
python3 scripts/analyze_phase14_benchmark.py
```
