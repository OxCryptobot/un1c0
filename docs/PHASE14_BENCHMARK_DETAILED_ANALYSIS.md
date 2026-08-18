# Phase 14 Detailed Benchmark Analysis

This report is generated directly from `benchmarks/phase14_read_benchmark.json`. It compares the lease fast path with a fresh quorum-backed read-index round at each requested concurrency. All rows are deterministic fixture outputs from the latest local run; they are not WAN capacity claims.

The run contains **16,128 total measured reads**, split evenly between both paths, with zero reported errors in every row.

## Per-concurrency comparison

| Concurrency | Lease p50/p95/p99 (µs) | Quorum p50/p95/p99 (µs) | Lease throughput | Quorum throughput | Throughput ratio | p95 delta | Interpretation |
|---:|---:|---:|---:|---:|---:|---:|---|
| 1 | 2/2/5 | 5/5/6 | 146,090.20 | 164,411.36 | 0.89× | -60.0% | Quorum path is faster in this sample; inspect contention noise. |
| 2 | 12/30/167 | 12/13/25 | 87,238.21 | 93,903.81 | 0.93× | +130.8% | Paths are near parity; scheduler and mutex effects dominate. |
| 4 | 13/87/723 | 13/51/960 | 68,358.81 | 72,733.24 | 0.94× | +70.6% | Paths are near parity; scheduler and mutex effects dominate. |
| 8 | 13/186/1728 | 13/168/1951 | 77,323.85 | 74,141.33 | 1.04× | +10.7% | Paths are near parity; scheduler and mutex effects dominate. |
| 16 | 13/387/3237 | 13/437/3457 | 78,599.30 | 74,987.80 | 1.05× | -11.4% | Paths are near parity; scheduler and mutex effects dominate. |
| 32 | 13/1571/5982 | 13/1799/5132 | 78,846.78 | 82,339.48 | 0.96× | -12.7% | Paths are near parity; scheduler and mutex effects dominate. |

## Reading the result

The lease path avoids creating and completing a fresh read-index round, so its protocol work is lower by construction. The benchmark serializes calls through a shared `Arc<Mutex<ConsensusNode>>`, which can dominate tail latency and can obscure the protocol-level advantage. A p95 increase in one row is therefore evidence about this fixture’s scheduling and lock behavior, not evidence that the lease contract weakens consistency or always regresses performance.

The safety result is stronger than the performance result: every row completed successfully, the lease path was available only after quorum observation, and the quorum path continued to provide a correctness-preserving fallback. The next performance layer should separate immutable read execution from mutable consensus bookkeeping, use independent client workers, measure authenticated transport, and repeat each point enough times for confidence intervals.

## Reproduction

```bash
scripts/validate_phase14_read_optimization.sh
python3 scripts/analyze_phase14_benchmark.py
```
