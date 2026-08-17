//! OpenAI-compatible structured-output provider adapter.
//!
//! The adapter targets the widely implemented `/v1/chat/completions` contract.
//! It returns model output as untrusted data; local plan validation remains the
//! authority before any runtime action can execute.

use crate::agentic::{Capability, ToolRegistry};
use crate::provider::{
    ContextItem, FinishReason, ModelProvider, ProviderError, ProviderManifest, ProviderRequest,
    ProviderResponse, Usage,
};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PLAN_SCHEMA_NAME: &str = "un1c0_plan";

pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model_id: String,
    pub schema_version: String,
    pub quality_score: u8,
    pub cost_per_million_tokens: u64,
    pub latency_ms: u64,
    pub max_context_tokens: u32,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

impl Default for OpenAiCompatibleConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: None,
            model_id: "gpt-4o-mini".into(),
            schema_version: "plan.v1".into(),
            quality_score: 80,
            cost_per_million_tokens: 0,
            latency_ms: 1_000,
            max_context_tokens: 128_000,
            max_output_tokens: 8_192,
            timeout: Duration::from_secs(60),
        }
    }
}

impl OpenAiCompatibleConfig {
    pub fn from_env() -> Result<Self, ProviderError> {
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| Self::default().base_url);
        let api_key = std::env::var("OPENAI_API_KEY").ok();
        let model_id = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| Self::default().model_id);
        if api_key.is_none() && !is_local_url(&base_url) {
            return Err(ProviderError::Configuration {
                message: "OPENAI_API_KEY is required for non-local OpenAI-compatible endpoints"
                    .into(),
            });
        }
        Ok(Self {
            base_url,
            api_key,
            model_id,
            ..Self::default()
        })
    }

    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{}/chat/completions", base)
        } else {
            format!("{}/v1/chat/completions", base)
        }
    }
}

pub struct OpenAiCompatibleProvider {
    manifest: ProviderManifest,
    config: OpenAiCompatibleConfig,
    client: Client,
    plan_schema: Value,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        config: OpenAiCompatibleConfig,
        registry: &ToolRegistry,
    ) -> Result<Self, ProviderError> {
        if config.model_id.trim().is_empty() {
            return Err(ProviderError::Configuration {
                message: "model_id is empty".into(),
            });
        }
        if config.schema_version != "plan.v1" {
            return Err(ProviderError::Configuration {
                message: format!(
                    "unsupported plan schema version '{}'",
                    config.schema_version
                ),
            });
        }
        let plan_schema = plan_schema(registry)?;
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("application/json"));
        if let Some(api_key) = &config.api_key {
            let value = HeaderValue::from_str(&format!("Bearer {}", api_key)).map_err(|_| {
                ProviderError::Configuration {
                    message: "API key contains invalid header characters".into(),
                }
            })?;
            headers.insert("authorization", value);
        }
        let client = Client::builder()
            .default_headers(headers)
            .timeout(config.timeout)
            .build()
            .map_err(|error| ProviderError::Configuration {
                message: error.to_string(),
            })?;
        let capabilities = [
            Capability::WorkspaceRead,
            Capability::WorkspaceWrite,
            Capability::ProcessExec,
            Capability::NetworkAccess,
            Capability::ApiAccess,
            Capability::WebAccess,
            Capability::McpAccess,
            Capability::SkillAccess,
            Capability::LspAccess,
            Capability::SecretRead,
            Capability::EvolutionPropose,
        ]
        .into_iter()
        .collect();
        Ok(Self {
            manifest: ProviderManifest {
                provider_id: "openai-compatible".into(),
                model_id: config.model_id.clone(),
                schema_versions: [config.schema_version.clone()].into_iter().collect(),
                structured_output: true,
                max_context_tokens: config.max_context_tokens,
                max_output_tokens: config.max_output_tokens,
                capabilities,
                quality_score: config.quality_score,
                cost_per_million_tokens: config.cost_per_million_tokens,
                latency_ms: config.latency_ms,
                healthy: true,
            },
            config,
            client,
            plan_schema,
        })
    }

    pub fn plan_schema(&self) -> &Value {
        &self.plan_schema
    }

    fn request_body(&self, request: &ProviderRequest) -> Value {
        json!({
            "model": self.config.model_id,
            "temperature": 0,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a planning component. Return only a plan matching the supplied schema. Never emit shell commands, credentials, or prose outside the schema. Tool capabilities are declarations, not permissions; the runtime enforces policy."
                },
                {
                    "role": "user",
                    "content": format_prompt(request)
                }
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": PLAN_SCHEMA_NAME,
                    "strict": true,
                    "schema": self.plan_schema
                }
            },
            "max_tokens": request.max_output_tokens
        })
    }
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse, ProviderError> {
        if request.schema_version != self.config.schema_version {
            return Err(ProviderError::Configuration {
                message: format!(
                    "request schema '{}' does not match adapter schema '{}'",
                    request.schema_version, self.config.schema_version
                ),
            });
        }
        if request.max_output_tokens > self.config.max_output_tokens {
            return Err(ProviderError::ContextTooLarge);
        }
        let started = Instant::now();
        let response = self
            .client
            .post(self.config.endpoint())
            .json(&self.request_body(request))
            .send()
            .map_err(map_request_error)?;
        let status = response.status();
        let retry_after_ms = parse_retry_after(response.headers());
        let body = response.text().map_err(|error| ProviderError::Transport {
            message: error.to_string(),
        })?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(ProviderError::Configuration {
                message: "provider rejected authentication or authorization".into(),
            });
        }
        if status.as_u16() == 408 || status.as_u16() == 504 {
            return Err(ProviderError::Timeout);
        }
        if status.as_u16() == 429 {
            return Err(ProviderError::rate_limited(retry_after_ms));
        }
        if status.as_u16() >= 500 {
            return Err(ProviderError::Unavailable {
                message: format!("provider returned HTTP {}", status.as_u16()),
            });
        }
        if !status.is_success() {
            return Err(classify_client_error(status.as_u16(), &body));
        }

        let envelope: Value =
            serde_json::from_str(&body).map_err(|error| ProviderError::MalformedOutput {
                message: format!("provider returned invalid JSON: {}", error),
            })?;
        parse_response(
            envelope,
            &self.manifest,
            started.elapsed().as_millis() as u64,
        )
    }
}

pub fn plan_schema(registry: &ToolRegistry) -> Result<Value, ProviderError> {
    let specs = registry.specs();
    if specs.is_empty() {
        return Err(ProviderError::Configuration {
            message: "cannot generate plan schema without registered tools".into(),
        });
    }
    let action_variants: Vec<Value> = specs
        .iter()
        .map(|spec| {
            let capabilities: Vec<String> = spec.capabilities.iter().map(capability_name).collect();
            let input_schema = strict_object_schema(spec.input_schema.clone());
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "id": {"type":"string", "minLength":1},
                    "tool": {"const": spec.name},
                    "input": input_schema,
                    "depends_on": {"type":"array", "items":{"type":"string"}, "maxItems":64},
                    "capabilities": {"type":"array", "items":{"type":"string", "enum":capabilities}, "maxItems":6},
                    "timeout_ms": {"anyOf":[{"type":"integer","minimum":1},{"type":"null"}]}
                },
                "required": ["id", "tool", "input", "depends_on", "capabilities", "timeout_ms"]
            })
        })
        .collect();
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": {"type":"string", "minLength":1},
            "goal": {"type":"string", "minLength":1},
            "actions": {"type":"array", "minItems":1, "maxItems":64, "items":{"anyOf":action_variants}},
            "max_steps": {"type":"integer", "minimum":1, "maximum":10000},
            "max_output_bytes": {"type":"integer", "minimum":1, "maximum":16777216}
        },
        "required": ["id", "goal", "actions", "max_steps", "max_output_bytes"]
    }))
}

fn strict_object_schema(mut schema: Value) -> Value {
    if let Some(object) = schema.as_object_mut() {
        object
            .entry("type")
            .or_insert_with(|| Value::String("object".into()));
        object.insert("additionalProperties".into(), Value::Bool(false));
        if !object.contains_key("properties") {
            object.insert("properties".into(), json!({}));
        }
        let property_names = object
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        object
            .entry("required")
            .or_insert_with(|| json!(property_names));
    }
    schema
}

fn capability_name(capability: &Capability) -> String {
    match capability {
        Capability::WorkspaceRead => "workspace.read",
        Capability::WorkspaceWrite => "workspace.write",
        Capability::ProcessExec => "process.exec",
        Capability::NetworkAccess => "network.access",
        Capability::ApiAccess => "api.access",
        Capability::WebAccess => "web.access",
        Capability::McpAccess => "mcp.access",
        Capability::SkillAccess => "skill.access",
        Capability::LspAccess => "lsp.access",
        Capability::SecretRead => "secret.read",
        Capability::EvolutionPropose => "evolution.propose",
    }
    .into()
}

fn format_prompt(request: &ProviderRequest) -> String {
    let mut prompt = format!(
        "Goal: {}\nSchema version: {}\nRisk: {:?}\n",
        request.goal, request.schema_version, request.risk
    );
    if request.context.is_empty() {
        prompt.push_str("Context: none\n");
    } else {
        prompt.push_str("Context:\n");
        for ContextItem { label, content, .. } in &request.context {
            prompt.push_str(&format!("--- {} ---\n{}\n", label, content));
        }
    }
    prompt.push_str("Plan only safe, minimal, dependency-ordered actions. The runtime will validate tools, capabilities, paths, budgets, and approvals.");
    prompt
}

fn parse_response(
    envelope: Value,
    manifest: &ProviderManifest,
    latency_ms: u64,
) -> Result<ProviderResponse, ProviderError> {
    let choice = envelope
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| ProviderError::MalformedOutput {
            message: "response has no choices".into(),
        })?;
    let message = choice
        .get("message")
        .ok_or_else(|| ProviderError::MalformedOutput {
            message: "response choice has no message".into(),
        })?;
    let refusal = message
        .get("refusal")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let raw_output = match message.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(content) if !content.is_null() => content.to_string(),
        _ => String::new(),
    };
    if refusal.is_some() && raw_output.is_empty() {
        return Ok(ProviderResponse {
            provider_id: manifest.provider_id.clone(),
            model_id: manifest.model_id.clone(),
            raw_output,
            structured_output: None,
            refusal,
            usage: parse_usage(&envelope),
            finish_reason: FinishReason::Refusal,
            latency_ms,
        });
    }
    if raw_output.is_empty() {
        return Err(ProviderError::MalformedOutput {
            message: "response message has no content".into(),
        });
    }
    let structured_output = serde_json::from_str::<Value>(&raw_output).ok();
    let finish_reason = match choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCall,
        "content_filter" => FinishReason::Refusal,
        _ => FinishReason::Unknown,
    };
    Ok(ProviderResponse {
        provider_id: manifest.provider_id.clone(),
        model_id: manifest.model_id.clone(),
        raw_output,
        structured_output,
        refusal,
        usage: parse_usage(&envelope),
        finish_reason,
        latency_ms,
    })
}

fn parse_usage(envelope: &Value) -> Usage {
    Usage {
        input_tokens: envelope
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: envelope
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1_000))
}

fn classify_client_error(status: u16, body: &str) -> ProviderError {
    let lower = body.to_lowercase();
    if lower.contains("context") && (lower.contains("length") || lower.contains("token")) {
        ProviderError::ContextTooLarge
    } else {
        ProviderError::Configuration {
            message: format!("provider returned HTTP {}", status),
        }
    }
}

fn map_request_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Transport {
            message: sanitize_error(&error.to_string()),
        }
    }
}

fn sanitize_error(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("authorization") || lower.contains("bearer") || lower.contains("api_key") {
        "provider transport error".into()
    } else {
        message.chars().take(512).collect()
    }
}

fn is_local_url(base_url: &str) -> bool {
    base_url.starts_with("http://localhost")
        || base_url.starts_with("http://127.0.0.1")
        || base_url.starts_with("http://[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::built_in_registry;

    #[test]
    fn generated_schema_is_strict_and_has_tool_variants() {
        let schema = plan_schema(&built_in_registry()).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["actions"]["minItems"], 1);
        assert!(
            schema["properties"]["actions"]["items"]["anyOf"]
                .as_array()
                .unwrap()
                .len()
                >= 4
        );
    }

    #[test]
    fn endpoint_normalization_supports_root_v1_and_full_paths() {
        let mut config = OpenAiCompatibleConfig {
            base_url: "http://localhost:9000".into(),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint(),
            "http://localhost:9000/v1/chat/completions"
        );
        config.base_url = "http://localhost:9000/v1".into();
        assert_eq!(
            config.endpoint(),
            "http://localhost:9000/v1/chat/completions"
        );
        config.base_url = "http://localhost:9000/v1/chat/completions".into();
        assert_eq!(
            config.endpoint(),
            "http://localhost:9000/v1/chat/completions"
        );
    }

    #[test]
    fn advertises_external_planner_capabilities_without_granting_runtime_authority() {
        let config = OpenAiCompatibleConfig {
            base_url: "http://127.0.0.1:9000".into(),
            api_key: None,
            ..Default::default()
        };
        let provider = OpenAiCompatibleProvider::new(config, &built_in_registry()).unwrap();
        for capability in [
            Capability::ApiAccess,
            Capability::WebAccess,
            Capability::McpAccess,
            Capability::SkillAccess,
            Capability::LspAccess,
        ] {
            assert!(provider.manifest().capabilities.contains(&capability));
        }
    }

    #[test]
    fn local_endpoint_does_not_require_an_api_key() {
        let config = OpenAiCompatibleConfig {
            base_url: "http://127.0.0.1:9000".into(),
            api_key: None,
            ..Default::default()
        };
        let provider = OpenAiCompatibleProvider::new(config, &built_in_registry()).unwrap();
        assert!(provider.manifest().structured_output);
    }
}
