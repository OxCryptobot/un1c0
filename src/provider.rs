//! Provider-neutral structured-output routing for the UN1C⓪ agent kernel.
//!
//! Providers are untrusted planners. This module selects a compatible provider,
//! classifies failures, and decodes responses into the existing validated Plan
//! contract. It never executes model output.

use crate::agentic::{Capability, Plan, ToolRegistry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderManifest {
    pub provider_id: String,
    pub model_id: String,
    pub schema_versions: BTreeSet<String>,
    pub structured_output: bool,
    pub max_context_tokens: u32,
    pub max_output_tokens: u32,
    pub capabilities: BTreeSet<Capability>,
    pub quality_score: u8,
    pub cost_per_million_tokens: u64,
    pub latency_ms: u64,
    pub healthy: bool,
}

impl ProviderManifest {
    pub fn supports(&self, request: &ProviderRequest) -> bool {
        self.healthy
            && self.structured_output
            && self.schema_versions.contains(&request.schema_version)
            && self.max_context_tokens >= request.context_tokens
            && self.max_output_tokens >= request.max_output_tokens
            && self
                .capabilities
                .is_superset(&request.required_capabilities)
            && self.quality_score >= request.minimum_quality_score
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    pub label: String,
    pub content: String,
    pub estimated_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub request_id: String,
    pub goal: String,
    pub context: Vec<ContextItem>,
    pub schema_version: String,
    pub context_tokens: u32,
    pub max_output_tokens: u32,
    pub deadline_ms: u64,
    pub required_capabilities: BTreeSet<Capability>,
    pub risk: TaskRisk,
    pub minimum_quality_score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    Refusal,
    ToolCall,
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub trait ModelProvider: Send + Sync {
    fn manifest(&self) -> &ProviderManifest;
    fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse, ProviderError>;
}

#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderError {
    #[error("transport error: {message}")]
    Transport { message: String },
    #[error("rate limited{retry_suffix}")]
    RateLimited {
        retry_after_ms: Option<u64>,
        #[serde(skip)]
        retry_suffix: String,
    },
    #[error("provider context window is too small")]
    ContextTooLarge,
    #[error("malformed structured output: {message}")]
    MalformedOutput { message: String },
    #[error("provider refused the request: {reason}")]
    Refused { reason: String },
    #[error("provider unavailable: {message}")]
    Unavailable { message: String },
    #[error("provider policy violation: {message}")]
    PolicyViolation { message: String },
    #[error("provider request timed out")]
    Timeout,
    #[error("provider configuration error: {message}")]
    Configuration { message: String },
}

impl ProviderError {
    pub fn rate_limited(retry_after_ms: Option<u64>) -> Self {
        let retry_suffix = retry_after_ms
            .map(|ms| format!("; retry after {} ms", ms))
            .unwrap_or_default();
        Self::RateLimited {
            retry_after_ms,
            retry_suffix,
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport { .. } | Self::RateLimited { .. } | Self::Timeout
        )
    }

    pub fn fallbackable(&self) -> bool {
        self.retryable() || matches!(self, Self::ContextTooLarge | Self::Unavailable { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteAttempt {
    pub provider_id: String,
    pub model_id: String,
    pub attempt: u32,
    pub outcome: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RouteOutcome {
    pub response: ProviderResponse,
    pub attempts: Vec<RouteAttempt>,
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("no compatible providers: {0}")]
    NoCompatibleProviders(String),
    #[error("all provider attempts failed: {0}")]
    AllAttemptsFailed(String),
    #[error("plan decode failed: {0}")]
    PlanDecode(String),
}

#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub max_retries_per_provider: u32,
    pub max_attempts_total: u32,
    pub backoff_initial_ms: u64,
    pub backoff_max_ms: u64,
    pub cooldown_after_failures: u32,
    pub cooldown_ms: u64,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            max_retries_per_provider: 1,
            max_attempts_total: 4,
            backoff_initial_ms: 50,
            backoff_max_ms: 2_000,
            cooldown_after_failures: 3,
            cooldown_ms: 30_000,
        }
    }
}

#[derive(Debug, Default)]
struct ProviderHealth {
    consecutive_failures: u32,
    cooldown_until_ms: Option<u128>,
}

pub struct ProviderRouter {
    providers: Vec<Arc<dyn ModelProvider>>,
    health: Mutex<HashMap<String, ProviderHealth>>,
    config: RouterConfig,
}

impl ProviderRouter {
    pub fn new(providers: Vec<Arc<dyn ModelProvider>>, config: RouterConfig) -> Self {
        Self {
            providers,
            health: Mutex::new(HashMap::new()),
            config,
        }
    }

    pub fn manifests(&self) -> Vec<ProviderManifest> {
        self.providers
            .iter()
            .map(|provider| provider.manifest().clone())
            .collect()
    }

    pub fn complete(&self, request: &ProviderRequest) -> Result<RouteOutcome, RoutingError> {
        let mut candidates: Vec<_> = self
            .providers
            .iter()
            .filter(|provider| provider.manifest().supports(request))
            .filter(|provider| self.is_available(&provider.manifest().provider_id))
            .collect();
        candidates.sort_by(|left, right| {
            self.score(right.manifest(), request)
                .cmp(&self.score(left.manifest(), request))
        });
        if candidates.is_empty() {
            return Err(RoutingError::NoCompatibleProviders(format!(
                "schema={}, context_tokens={}, minimum_quality={}, risk={:?}",
                request.schema_version,
                request.context_tokens,
                request.minimum_quality_score,
                request.risk
            )));
        }

        let mut attempts = Vec::new();
        for provider in candidates {
            let provider_id = provider.manifest().provider_id.clone();
            let model_id = provider.manifest().model_id.clone();
            for attempt in 0..=self.config.max_retries_per_provider {
                if attempts.len() as u32 >= self.config.max_attempts_total {
                    break;
                }
                if attempt > 0 {
                    let backoff = self.backoff(attempt);
                    if backoff > 0 {
                        thread::sleep(Duration::from_millis(backoff));
                    }
                }
                match provider.complete(request) {
                    Ok(response) => {
                        self.record_success(&provider_id);
                        attempts.push(RouteAttempt {
                            provider_id,
                            model_id,
                            attempt,
                            outcome: "success".into(),
                            latency_ms: Some(response.latency_ms),
                        });
                        return Ok(RouteOutcome { response, attempts });
                    }
                    Err(error) => {
                        let retryable = error.retryable();
                        let fallbackable = error.fallbackable();
                        attempts.push(RouteAttempt {
                            provider_id: provider_id.clone(),
                            model_id: model_id.clone(),
                            attempt,
                            outcome: error.to_string(),
                            latency_ms: None,
                        });
                        self.record_failure(&provider_id);
                        if !retryable || !fallbackable {
                            break;
                        }
                    }
                }
            }
        }
        Err(RoutingError::AllAttemptsFailed(
            attempts
                .iter()
                .map(|attempt| format!("{}:{}", attempt.provider_id, attempt.outcome))
                .collect::<Vec<_>>()
                .join(" | "),
        ))
    }

    pub fn complete_and_decode(
        &self,
        request: &ProviderRequest,
        registry: &ToolRegistry,
    ) -> Result<(Plan, RouteOutcome), RoutingError> {
        let outcome = self.complete(request)?;
        let plan = decode_plan(&outcome.response, registry)?;
        Ok((plan, outcome))
    }

    fn score(&self, manifest: &ProviderManifest, request: &ProviderRequest) -> i64 {
        let quality = i64::from(manifest.quality_score) * 10;
        let latency_penalty = match request.risk {
            TaskRisk::Low => manifest.latency_ms as i64 / 100,
            TaskRisk::Medium => manifest.latency_ms as i64 / 200,
            TaskRisk::High => manifest.latency_ms as i64 / 400,
        };
        quality - manifest.cost_per_million_tokens as i64 / 100 - latency_penalty
    }

    fn backoff(&self, attempt: u32) -> u64 {
        let multiplier = 1_u64
            .checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u64::MAX);
        self.config
            .backoff_initial_ms
            .saturating_mul(multiplier)
            .min(self.config.backoff_max_ms)
    }

    fn is_available(&self, provider_id: &str) -> bool {
        let now = now_ms();
        let Ok(mut health) = self.health.lock() else {
            return false;
        };
        let state = health.entry(provider_id.to_string()).or_default();
        state
            .cooldown_until_ms
            .map(|until| until <= now)
            .unwrap_or(true)
    }

    fn record_success(&self, provider_id: &str) {
        if let Ok(mut health) = self.health.lock() {
            health.insert(provider_id.to_string(), ProviderHealth::default());
        }
    }

    fn record_failure(&self, provider_id: &str) {
        if let Ok(mut health) = self.health.lock() {
            let state = health.entry(provider_id.to_string()).or_default();
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            if state.consecutive_failures >= self.config.cooldown_after_failures {
                state.cooldown_until_ms =
                    Some(now_ms().saturating_add(self.config.cooldown_ms as u128));
            }
        }
    }
}

pub fn decode_plan(
    response: &ProviderResponse,
    registry: &ToolRegistry,
) -> Result<Plan, RoutingError> {
    if let Some(refusal) = &response.refusal {
        return Err(RoutingError::PlanDecode(format!(
            "provider refusal: {}",
            refusal
        )));
    }
    if response.finish_reason == FinishReason::Length {
        return Err(RoutingError::PlanDecode(
            "provider output was truncated".into(),
        ));
    }
    let value = match &response.structured_output {
        Some(value) => value.clone(),
        None => serde_json::from_str::<Value>(&response.raw_output)
            .map_err(|error| RoutingError::PlanDecode(format!("invalid JSON: {}", error)))?,
    };
    let plan: Plan = serde_json::from_value(value)
        .map_err(|error| RoutingError::PlanDecode(format!("schema mismatch: {}", error)))?;
    plan.validate(registry)
        .map_err(|error| RoutingError::PlanDecode(error.to_string()))?;
    Ok(plan)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::{built_in_registry, Action, Plan};
    use serde_json::json;

    struct MockProvider {
        manifest: ProviderManifest,
        responses: Mutex<Vec<Result<ProviderResponse, ProviderError>>>,
    }

    impl MockProvider {
        fn new(
            provider_id: &str,
            quality_score: u8,
            responses: Vec<Result<ProviderResponse, ProviderError>>,
        ) -> Self {
            Self {
                manifest: ProviderManifest {
                    provider_id: provider_id.into(),
                    model_id: format!("{}-model", provider_id),
                    schema_versions: ["plan.v1".into()].into_iter().collect(),
                    structured_output: true,
                    max_context_tokens: 16_000,
                    max_output_tokens: 2_000,
                    capabilities: BTreeSet::new(),
                    quality_score,
                    cost_per_million_tokens: 1,
                    latency_ms: 50,
                    healthy: true,
                },
                responses: Mutex::new(responses),
            }
        }
    }

    impl ModelProvider for MockProvider {
        fn manifest(&self) -> &ProviderManifest {
            &self.manifest
        }
        fn complete(&self, _request: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn request() -> ProviderRequest {
        ProviderRequest {
            request_id: "req-1".into(),
            goal: "inspect safely".into(),
            context: vec![],
            schema_version: "plan.v1".into(),
            context_tokens: 100,
            max_output_tokens: 500,
            deadline_ms: 1_000,
            required_capabilities: BTreeSet::new(),
            risk: TaskRisk::Low,
            minimum_quality_score: 70,
        }
    }

    fn valid_response() -> ProviderResponse {
        let plan = Plan {
            id: "plan-1".into(),
            goal: "inspect safely".into(),
            actions: vec![Action {
                id: "echo".into(),
                tool: "echo".into(),
                input: json!({"message":"ok"}),
                depends_on: vec![],
                capabilities: vec![],
                timeout_ms: None,
            }],
            max_steps: 4,
            max_output_bytes: 1_024,
        };
        ProviderResponse {
            provider_id: "primary".into(),
            model_id: "primary-model".into(),
            raw_output: String::new(),
            structured_output: Some(serde_json::to_value(plan).unwrap()),
            refusal: None,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
            },
            finish_reason: FinishReason::Stop,
            latency_ms: 20,
        }
    }

    #[test]
    fn routes_to_a_compatible_provider_and_decodes_plan() {
        let primary = Arc::new(MockProvider::new("primary", 95, vec![Ok(valid_response())]));
        let router = ProviderRouter::new(
            vec![primary],
            RouterConfig {
                backoff_initial_ms: 0,
                ..RouterConfig::default()
            },
        );
        let (plan, outcome) = router
            .complete_and_decode(&request(), &built_in_registry())
            .unwrap();
        assert_eq!(plan.id, "plan-1");
        assert_eq!(outcome.attempts.len(), 1);
    }

    #[test]
    fn falls_back_after_transient_failure() {
        let failing = Arc::new(MockProvider::new(
            "primary",
            99,
            vec![Err(ProviderError::Timeout), Err(ProviderError::Timeout)],
        ));
        let backup = Arc::new(MockProvider::new("backup", 80, vec![Ok(valid_response())]));
        let router = ProviderRouter::new(
            vec![failing, backup],
            RouterConfig {
                max_retries_per_provider: 1,
                backoff_initial_ms: 0,
                ..RouterConfig::default()
            },
        );
        let outcome = router.complete(&request()).unwrap();
        assert_eq!(outcome.attempts.last().unwrap().provider_id, "backup");
        assert_eq!(outcome.attempts.len(), 3);
    }

    #[test]
    fn rejects_truncated_or_refused_output() {
        let registry = built_in_registry();
        let mut truncated = valid_response();
        truncated.finish_reason = FinishReason::Length;
        assert!(
            matches!(decode_plan(&truncated, &registry), Err(RoutingError::PlanDecode(message)) if message.contains("truncated"))
        );

        let mut refused = valid_response();
        refused.refusal = Some("not allowed".into());
        assert!(
            matches!(decode_plan(&refused, &registry), Err(RoutingError::PlanDecode(message)) if message.contains("refusal"))
        );
    }

    #[test]
    fn filters_provider_by_schema_context_and_quality() {
        let provider = Arc::new(MockProvider::new("small", 50, vec![Ok(valid_response())]));
        let router = ProviderRouter::new(vec![provider], RouterConfig::default());
        let mut request = request();
        request.minimum_quality_score = 80;
        assert!(matches!(
            router.complete(&request),
            Err(RoutingError::NoCompatibleProviders(_))
        ));
    }
}
