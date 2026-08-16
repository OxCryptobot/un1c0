# Phase 2 — Structured-Output Providers and Fallback Routing

## Design objective

Phase 2 adds real model providers without allowing model output to become an execution primitive. Every provider adapter returns a typed response that is decoded into the existing `Plan` schema, validated semantically, and handed to the existing runtime only after policy checks.

The critical boundary is:

```text
ProviderAdapter -> ProviderResponse -> StrictDecoder -> PlanValidator -> Runtime
```

A provider can propose work. It cannot grant itself capabilities, bypass approvals, write outside the workspace, or mark verification as passed.

## Provider contracts

```rust
pub trait ModelProvider: Send + Sync {
    fn manifest(&self) -> &ProviderManifest;
    fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse, ProviderError>;
}

pub struct ProviderRequest {
    pub request_id: String,
    pub goal: String,
    pub context: Vec<ContextItem>,
    pub schema_version: String,
    pub max_output_tokens: u32,
    pub deadline_ms: u64,
    pub required_capabilities: BTreeSet<Capability>,
}

pub struct ProviderResponse {
    pub provider_id: String,
    pub model_id: String,
    pub raw_output: String,
    pub structured_output: Option<Value>,
    pub refusal: Option<String>,
    pub usage: Usage,
    pub finish_reason: FinishReason,
    pub latency_ms: u64,
}
```

`ProviderManifest` must declare the model identifier, supported schema versions, structured-output mode, maximum context size, maximum output size, capabilities, quality tier, cost estimate, latency estimate, regional/data policy, and health state. A manifest is routing metadata, not authorization; policy remains authoritative.

## Canonical plan schema

Version the plan schema independently from providers. The first schema should require:

| Field | Constraint |
|---|---|
| `id` | Non-empty stable identifier within the run |
| `goal` | Non-empty user-derived goal |
| `actions` | Non-empty array, bounded by runtime budget |
| `actions[].id` | Unique within the plan |
| `actions[].tool` | Must exist in the registry |
| `actions[].input` | Must pass the tool schema |
| `actions[].depends_on` | Existing IDs only; acyclic graph |
| `actions[].capabilities` | Exactly matches the tool’s declared capabilities |
| `max_steps` | Positive and below policy maximum |
| `max_output_bytes` | Positive and below policy maximum |

Use a strict JSON Schema where supported, but always perform local semantic validation afterward. OpenAI’s structured-output documentation notes that strict output still has refusal and incomplete-output cases, and recommends avoiding divergence between schemas and native types.[1] Therefore, decode refusals separately, reject truncated output, and generate the provider schema from the same Rust types or a checked-in schema artifact.

## Routing algorithm

1. **Normalize the request.** Compute context token estimate, required capabilities, task risk, deadline, data-residency constraints, and schema version.
2. **Filter candidates.** Remove providers that lack the required structured-output mode, schema version, context capacity, policy approval, region, or available health state.
3. **Score candidates.** Use a weighted score:

   ```text
   score = quality_weight * quality
         + latency_weight * latency_fit
         + cost_weight * cost_fit
         + health_weight * health
         - risk_penalty * capability_risk
   ```

   High-risk plans should use a high-quality provider or fail closed. Do not silently downgrade a high-risk request to a weaker model.
4. **Select a primary and ordered fallback chain.** Persist the decision in the event journal before the call. Keep fallbacks in explicit order rather than selecting a hidden “best effort.”
5. **Execute with a deadline.** Apply per-provider timeout and the remaining request deadline. Capture attempt number, provider, model, latency, token usage, and finish reason.
6. **Classify the failure.** Retry only transient transport failures, rate limits, or provider overload. Fallback for provider outage, context-window failure, or health cooldown. Do not retry malformed structured output or policy violations without changing the request/decoder strategy.
7. **Update health state.** Record success/failure counters, latency EWMA, rate-limit cooldown, and circuit state. Use a half-open probe before restoring a provider after cooldown.
8. **Return or fail closed.** If every candidate fails, return a typed aggregate error containing attempts and causes. Never fabricate a plan or treat an unavailable verifier/provider as success.

LiteLLM’s public reliability documentation provides a useful reference pattern: ordered fallback groups, distinct context-window and content-policy fallbacks, retries before fallback, timeouts, and cooldowns.[2] UN1C⓪ should implement the same semantics inside its typed runtime rather than hide them in provider-specific code.

## Error taxonomy

| Error | Retry same provider? | Fallback? | Notes |
|---|---:|---:|---|
| Network timeout | Yes, bounded | Yes after retry budget | Preserve request deadline |
| HTTP 429 / rate limit | Yes with server hint | Yes after cooldown | Honor `Retry-After` |
| HTTP 5xx / overload | Yes, bounded | Yes | Increment circuit failure count |
| Context too large | No | Yes to larger-context model | Prefer deterministic compaction first |
| Malformed JSON | No blind retry | Maybe with repair prompt | Never execute partial output |
| Schema violation | No blind retry | Maybe | Record exact validation path |
| Model refusal | No blind retry | Policy-dependent | Preserve refusal reason; do not reinterpret |
| Tool/capability violation | No | No | This is a planner/runtime contract failure |
| Authentication/configuration | No | Only explicitly configured | Alert rather than churn through providers |

## Observability

Journal these events:

- `provider_selection`: candidates, filter reasons, chosen primary, fallback chain, schema version.
- `provider_attempt_started`: provider, model, attempt, deadline.
- `provider_attempt_finished`: status, latency, usage, finish reason, output hash.
- `provider_fallback`: source provider, target provider, typed cause.
- `plan_decode_failed`: schema path, bounded diagnostic, output hash.
- `provider_circuit_opened` and `provider_circuit_closed`.

Never journal API keys, raw secrets, or unredacted private context. Store raw model output only when the configured privacy policy permits it; otherwise retain a content hash plus redacted diagnostics.

## Acceptance tests

1. A strict valid response decodes to a plan and executes only after semantic validation.
2. A refusal is classified separately from malformed JSON.
3. Unknown tool, missing dependency, cycle, undeclared capability, and excessive budget are rejected before execution.
4. A transient timeout retries within the deadline and then falls back in declared order.
5. A context overflow selects a larger-context candidate or fails with `ContextTooLarge`.
6. A high-risk task does not downgrade to a low-quality model unless policy explicitly allows it.
7. Provider API keys never appear in events or error strings.
8. Replaying the same request with the same provider response produces the same decoded plan hash.

## References

[1]: https://developers.openai.com/api/docs/guides/structured-outputs "OpenAI Structured model outputs"
[2]: https://docs.litellm.ai/docs/proxy/reliability "LiteLLM Fallbacks and Provider Failover"
