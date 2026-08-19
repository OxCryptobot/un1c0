# Phase 31 Compliance Dashboard and Trace-Seal Overhead Analysis

## Audit result

The published Phase 31 audit artifact records **82 expected, 82 observed, and 82 passed gates**, with an empty failure list, no recorded secret material, and no cluster mutation. The interactive dashboard and PNG visualize every gate status, the control-family distribution, authenticated partition verification p95, and measured trace-seal verification overhead.

## Control-family view

All gate statuses are passed. The dashboard groups the machine-readable gate inventory into baseline controls and phase families so the distribution can be inspected without treating the family count as a risk score. The family chart is descriptive; a single high-count family does not imply greater security coverage than a lower-count family.

## Performance view

The published partition benchmark measures in-process Ed25519 verification with healthy, majority-partition, and minority-partition scenarios. Verification p95 is **34.392 µs** for healthy, **29.815 µs** for majority partition, and **30.166 µs** for minority partition. The scenarios are not TCP, TLS, kernel, or cross-machine measurements.

The dedicated trace-seal benchmark executes 2,000 in-process `ReplayTraceSeal::verify` calls over canonical payload serialization and Ed25519 verification. The observed metrics are:

| Statistic | Observed |
|---|---:|
| Mean | 6,445.259 µs |
| p50 | 6,432.468 µs |
| p95 | 6,521.683 µs |
| p99 | 6,716.035 µs |
| Verification errors | 0 |
| Private key persisted | false |

These timings are sandbox observations and should be treated as a relative regression baseline, not a production capacity claim. The measured operation includes canonical payload serialization and signature verification; it does not include network transport, TLS, key-store latency, scheduling, contention, or hardware isolation.

## Reproduction

```bash
cargo run --example phase31_trace_seal_overhead -- --output benchmarks/phase31_trace_seal_overhead.json --iterations 2000
python3 scripts/generate_phase31_compliance_dashboard.py \
  --metrics benchmarks/security_compliance_metrics.json \
  --audit benchmarks/security_compliance_audit.json \
  --output benchmarks/phase31_compliance_dashboard.html \
  --png benchmarks/phase31_compliance_dashboard.png
```

## References

[1]: ../benchmarks/security_compliance_audit.json "82-gate independent audit"
[2]: ../benchmarks/security_compliance_metrics.json "82-gate metrics and partition benchmark"
[3]: ../benchmarks/phase31_trace_seal_overhead.json "Trace-seal overhead benchmark"
[4]: ../benchmarks/phase31_compliance_dashboard.html "Interactive dashboard"
[5]: ../benchmarks/phase31_compliance_dashboard.png "Rendered dashboard image"
[6]: ../scripts/generate_phase31_compliance_dashboard.py "Dashboard generator"
