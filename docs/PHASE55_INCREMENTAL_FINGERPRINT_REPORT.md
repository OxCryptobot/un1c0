# Phase 55 incremental semantic fingerprint report

## Executive summary

Phase 54 identified content fingerprint construction as the dominant repeated-validation cost: the prepared cache-hit path was approximately 74–102 ns p50, while full semantic-cache key construction reached approximately 313 µs p50 at 32 functions. Phase 55 introduces `SemanticFingerprint`, which separates a target/profile digest from ordered per-function digests and composes a root cache key. A one-function replacement recomputes only the changed function digest and the bounded root composition; unchanged function digests remain stable.

The Phase 55 benchmark used deterministic 1/2/4/8/16/32-function typed UEG fixtures, four target profiles, and 128 samples per row. At 32 functions, full fingerprint construction measured **134.773–136.574 µs p50**, while replacing one function measured **10.316–10.334 µs p50**, a local p50 speedup of approximately **13.0–13.2×**. At 8 functions, the measured p50 speedup ranged from **5.78–7.11×**. All changed reports were valid with zero diagnostics; each cache row recorded two misses, one warm hit, two entries, and zero evictions.

## Implementation

`SemanticFingerprint` contains `profile_key`, ordered `function_keys`, and `root_key`. The profile key covers the target binding, capability booleans, and ordered unary/binary operator sets. Each function key covers the function name, canonical parameters and annotations, return annotation, exact function span, statement kind/source/span, and recursively typed expression kind/source/span data. The root key includes the profile key, ordered function digests, and function count, so function order and membership remain invalidation inputs.

`replace_function(index, lambda)` validates the index, computes the replacement function digest, updates only that vector slot, and recomposes the bounded root key. It returns `SemanticFingerprintError::FunctionIndexOutOfBounds` instead of growing or silently changing the vector. The semantic cache exposes `fingerprint_for` and `validate_with_fingerprint`; cache misses still run the complete Phase 53 validator. A fingerprint never authorizes a target or bypasses UEG validity or semantic diagnostics.

## Benchmark results

| Functions | Expressions | Full p50 (ns) | Incremental p50 (ns) | p50 speedup | Full p95 (ns) | Incremental p95 (ns) | Changed diagnostics |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 5 | 6,030–7,446 | 4,613–5,543 | 1.27–1.34× | 6,080–10,692 | 4,749–7,242 | 0 |
| 2 | 10 | 10,261–10,572 | 4,943–4,960 | 2.07–2.13× | 11,990–12,708 | 4,985–7,186 | 0 |
| 4 | 20 | 18,367–18,961 | 5,301–5,479 | 3.36–3.46× | 20,309–28,234 | 5,448–8,297 | 0 |
| 8 | 40 | 35,714–43,935 | 6,176–6,181 | 5.78–7.11× | 46,772–56,701 | 7,679–10,418 | 0 |
| 16 | 80 | 69,215–71,135 | 7,321–7,360 | 9.40–9.71× | 84,852–96,118 | 7,588–11,655 | 0 |
| 32 | 160 | 134,773–136,574 | 10,316–10,334 | 13.04–13.24× | 155,518–188,725 | 12,279–15,690 | 0 |

The p99 ranges were 6,925–23,346 ns for one-function updates at one function, 7,645–18,044 ns at four functions, and 16,757–27,940 ns at 32 functions. The full fingerprint p99 at 32 functions ranged from 162,123–215,870 ns. These are local release-build measurements from a sandbox and are not production compiler-throughput or multi-process scalability claims.

## Changed-input and cache evidence

The integration suite proves that changing only the second function changes only its function digest among unchanged peers and changes the composed root key. Replacing that function in place produces the same root key as recomposing the changed UEG from scratch. Reordering functions changes the root key. Changing a target capability profile changes the root key. A valid UEG followed by an undefined-name UEG produces two cache misses, two entries, zero hits, and zero evictions; the invalid report remains invalid and contains `UEG-UNDEFINED-NAME`.

The visual companion is `benchmarks/phase55_incremental_fingerprint.png`, generated from the sanitized JSON artifact by `scripts/analyze_phase55_incremental_fingerprint.py`. It shows full fingerprint cost increasing with function count while one-function replacement remains comparatively bounded.

## Verification and boundaries

The Phase 55 focused integration suite contains four tests covering function-level separation, in-place recomposition, profile/order invalidation, and fail-closed changed-input behavior. Library tests passed during implementation. Complete all-target validation, formatting, Python syntax, reusable-skill validation, whitespace checks, and publication parity are required before release.

Phase 55 does not add persistent fingerprints, distributed cache coherence, remote cache trust, generated-code caching, type inference, runtime execution, filesystem access, process execution, network access, secret reads, or cluster mutation. Compliance metadata remains at **209 gates**.
