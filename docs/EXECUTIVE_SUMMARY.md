# un1c0 Executive Summary

**Review scope:** local-first AI-programmable agentic language system, architecture hardening, controlled performance benchmark, and staging deployment readiness.

**Evidence baseline:** published repository commit `54c1fec`. Measurements were produced by `src/bin/un1c0-bench.rs` using deterministic local fixtures and a local provider mock. No live provider, external API, or Kubernetes cluster was contacted.

## Executive assessment

un1c0 has advanced from a source-to-source translator into a credible local-first agent platform with a typed UEG intermediate representation, policy-gated execution, provider-neutral routing, repository intelligence, resumable checkpoints, bounded subagents, consent-scoped integrations, fail-closed verification, and controlled evolution. The strongest production characteristics are explicit trust boundaries, bounded resource contracts, deterministic local behavior, atomic state transitions, and evidence-driven evolution rather than unconstrained self-modification.

The current benchmark shows that in-process validation and cryptographic checks are inexpensive, while repository retrieval is the dominant measured hotspot and becomes tail-latency sensitive under concurrent filesystem reads. Provider routing remains fast with a local mock but develops a larger p95 tail at concurrency eight, so real provider latency, queueing, and rate limits must be measured separately before capacity commitments. These are directional sandbox results, not production SLOs.

Staging deployment is **prepared but safely gated**. The repository initially had no Helm chart, no `kubectl` executable, and no configured Kubernetes context. A hardened chart and explicit `values-staging.yaml` are now present; Helm 3.17.3 was installed locally with checksum verification for linting and client-side rendering only. No cluster mutation was attempted. The next deployment gate requires an authorized staging context, a namespace, immutable image digests, externally managed runtime and mTLS secrets, and explicit approval for a mutating Helm command.

## Verified architecture status

| Capability | Current state | Evidence |
|---|---|---|
| Language-neutral IR | UEG remains the central translation and execution representation. | `src/walker.rs`, `src/ueg_python.rs`, translator tests |
| Agent kernel | Typed plans/actions, capability policy, workspace boundaries, event journal, runtime approvals. | `src/agentic.rs` |
| Provider intelligence | Structured-output contracts, compatibility routing, bounded retries, deadlines, fallback and cooldowns. | `src/provider.rs`, `src/provider_openai.rs` |
| Repository intelligence | Deterministic file/symbol index and bounded ranked retrieval. | `src/repository.rs` |
| Durable execution | Plan-hashed atomic checkpoints and resume semantics. | `src/run_state.rs` |
| External integrations | Consent manifests, host allowlists, bounded payloads, revocation, and approval propagation for MCP, skills, API, web, and LSP seams. | `src/integration.rs` |
| Verification | Fail-closed manifests and isolated/rootless verifier contracts with bounded evidence. | `src/verification.rs` |
| Controlled evolution | Trusted Ed25519 signers, approval/canary/apply/rollback ledger, exact report binding, workspace-derived file hashes. | `src/evolution.rs`, `SECURITY_AUDIT_ED25519.md` |
| Operations | Hardened Vault/admin/nginx Compose stack with health/readiness/metrics and disposable mTLS smoke validation. | `vault/`, `scripts/validate_compose_smoke.sh` |

## Controlled benchmark results

The harness executed **2,000 samples per operation at concurrency 1, 2, 4, and 8**, for **28 benchmark rows and zero recorded errors**. Latencies below are p95 values from the precision-corrected nanosecond harness; throughput is operations per second.

| Operation | p95 @ concurrency 1 | p95 @ concurrency 8 | Throughput @ 1 | Throughput @ 8 | Interpretation |
|---|---:|---:|---:|---:|---|
| Plan validation | 0.231 ms | 0.480 ms | 3.09M/s | 3.01M/s | Stable CPU-bound contract validation. |
| Repository search | 4.176 ms | 37.202 ms | 263/s | 249/s | Dominant tail-latency hotspot due to concurrent bounded file reads. |
| Provider routing | 0.050 ms | 0.988 ms | 26.1K/s | 31.0K/s | Local mock only; external provider latency is not represented. |
| Checkpoint save/load | 0.014 ms | 0.151 ms | 74.9K/s | 91.9K/s | Temporary-file filesystem path remained error-free in this fixture. |
| Verification manifest | 0.002 ms | 0.041 ms | 529K/s | 647K/s | Cheap policy validation before sandbox execution. |
| Ed25519 signature verification | 0.030 ms | 0.048 ms | 33.1K/s | 122.7K/s | Cryptographic verification is not the current bottleneck. |
| Canary report from workspace | 0.006 ms | 0.079 ms | 165.5K/s | 256.9K/s | Bounded hashing and path checks remain low-cost for small files. |

The benchmark harness records nanoseconds to avoid reporting sub-microsecond operations as zero. The fixture contains 128 small Rust files and is intentionally deterministic. Results should be repeated on staging hardware with realistic repository sizes, provider latency distributions, disk classes, and verifier workloads before setting operational SLOs.

A targeted repository-search profile showed that bounded content snapshots reduce p95 latency from 37.202 ms to 13.454 ms at concurrency eight and raise measured throughput from 249 to 923 operations per second, with zero errors in both runs. The optimization is documented in [`docs/REPOSITORY_SEARCH_PROFILE.md`](REPOSITORY_SEARCH_PROFILE.md); the before/after data is preserved in [`benchmarks/repository_search_profile.json`](../benchmarks/repository_search_profile.json) and [`benchmarks/agent_benchmark_optimized.json`](../benchmarks/agent_benchmark_optimized.json).

## Prioritized stakeholder decisions

| Priority | Decision | Why it matters |
|---:|---|---|
| 1 | Authorize and identify a staging Kubernetes context, namespace, image digests, Vault address, runtime secret, and mTLS secret. | Without these deployment inputs, a Helm release cannot be safely or truthfully executed. |
| 2 | Decide whether repository retrieval should move from per-query filesystem reads to a content cache or bounded in-memory snippet store. | Search is the only measured operation with a pronounced concurrency p95 increase. |
| 3 | Establish the production trust-store distribution, key rotation, revocation, and append-only audit sink for evolution signers. | The in-process trusted signer store separates authentication from authorization but is not yet an organizational key-management service. |

## Staging deployment gate

The prepared chart is under `deploy/helm/un1c0`. It uses an external Vault address, an existing secret for `VAULT_TOKEN` and `ADMIN_API_KEY`, an existing mTLS secret containing `server.crt`, `server.key`, and `ca.crt`, non-root/read-only containers, probes, resource budgets, PodDisruptionBudgets, and NetworkPolicies. `values-staging.yaml` deliberately contains a placeholder image digest and an example Vault address; it must not be deployed unchanged.

At the time of target discovery, the sandbox reported no `kubectl`, no current Kubernetes context, and no context list. Helm was later installed locally with checksum verification and used for client-side lint/template validation; no Kubernetes API was contacted. Therefore, the deployment was not attempted. Once the user supplies or connects an authorized staging target, the required sequence is chart lint/template, server-side dry run, explicit approval, bounded rollout, mTLS/health/readiness/metrics probes, and end-to-end fixture verification.

## Evidence artifacts

The raw benchmark data is available in [`benchmarks/agent_benchmark.json`](../benchmarks/agent_benchmark.json), with tabular output in [`benchmarks/agent_benchmark_summary.csv`](../benchmarks/agent_benchmark_summary.csv) and analysis metadata in [`benchmarks/benchmark_analysis.json`](../benchmarks/benchmark_analysis.json). The generated plots are [`latency_p95_by_concurrency.png`](../benchmarks/latency_p95_by_concurrency.png), [`throughput_by_concurrency.png`](../benchmarks/throughput_by_concurrency.png), and the combined [`performance_scaling_analysis.png`](../benchmarks/performance_scaling_analysis.png). The self-contained interactive dashboard is [`benchmark_dashboard.html`](../benchmarks/benchmark_dashboard.html). The component architecture is rendered in [`un1c0-system.png`](architecture/rendered/un1c0-system.png), and the deployment boundary is rendered in [`staging-deployment.png`](architecture/rendered/staging-deployment.png). The detailed chart data and scaling ratios are in [`performance_scaling_analysis.json`](../benchmarks/performance_scaling_analysis.json), while the Helm security findings are in [`HELM_PRODUCTION_READINESS_AUDIT.md`](HELM_PRODUCTION_READINESS_AUDIT.md).

The reusable agent-system engineering skill includes the repeatable benchmark, stakeholder-delivery, Helm-gating, GitHub publication, mTLS validation, and evolution-security procedures. It is delivered separately as a skill attachment.
