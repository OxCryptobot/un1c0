//! Consent-scoped external integration adapters.
//!
//! External capabilities are never inferred from tool input. An adapter must
//! carry a registered manifest, the plan must declare the adapter's exact
//! capabilities, and approval-sensitive manifests must receive an explicit
//! runtime approval from `RunOptions` before their handler is invoked.

use crate::agentic::{AgentError, Capability, Tool, ToolSpec, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::{to_vec, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

const DEFAULT_MAX_INPUT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_MANIFEST_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationKind {
    Api,
    Lsp,
    Mcp,
    Skill,
    Web,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentManifest {
    pub id: String,
    pub version: String,
    pub kind: IntegrationKind,
    pub description: String,
    pub capabilities: BTreeSet<Capability>,
    pub allowed_tools: BTreeSet<String>,
    pub requires_approval: bool,
    #[serde(default)]
    pub network_hosts: BTreeSet<String>,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

impl ConsentManifest {
    pub fn new(
        id: &str,
        version: &str,
        kind: IntegrationKind,
        description: &str,
        capabilities: BTreeSet<Capability>,
        allowed_tools: BTreeSet<String>,
        requires_approval: bool,
    ) -> Result<Self, AgentError> {
        if id.trim().is_empty() || version.trim().is_empty() {
            return Err(AgentError::Input(
                "consent manifest requires id and version".into(),
            ));
        }
        if description.trim().is_empty() || allowed_tools.is_empty() {
            return Err(AgentError::Input(
                "consent manifest requires description and at least one tool".into(),
            ));
        }
        if allowed_tools.iter().any(|tool| tool.trim().is_empty()) {
            return Err(AgentError::Input(
                "consent manifest contains an empty tool name".into(),
            ));
        }
        Ok(Self {
            id: id.to_string(),
            version: version.to_string(),
            kind,
            description: description.to_string(),
            capabilities,
            allowed_tools,
            requires_approval,
            network_hosts: BTreeSet::new(),
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        })
    }

    pub fn with_network_hosts(mut self, hosts: BTreeSet<String>) -> Result<Self, AgentError> {
        if hosts.iter().any(|host| host.trim().is_empty()) {
            return Err(AgentError::Input(
                "consent manifest contains an empty network host".into(),
            ));
        }
        if !hosts.is_empty() && !self.capabilities.contains(&Capability::NetworkAccess) {
            return Err(AgentError::Input(
                "network host allowlists require network.access".into(),
            ));
        }
        self.network_hosts = hosts;
        Ok(self)
    }

    fn validate_host(&self, input: &Value) -> Result<(), AgentError> {
        if self.network_hosts.is_empty() {
            return Ok(());
        }
        let host = input
            .get("host")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .ok_or_else(|| {
                AgentError::ConsentDenied(format!(
                    "manifest '{}' requires an explicit host field",
                    self.id
                ))
            })?;
        if !self.network_hosts.contains(host) {
            return Err(AgentError::ConsentDenied(format!(
                "host '{}' is not allowed by manifest '{}'",
                host, self.id
            )));
        }
        Ok(())
    }

    pub fn with_payload_limits(
        mut self,
        max_input_bytes: usize,
        max_output_bytes: usize,
    ) -> Result<Self, AgentError> {
        if max_input_bytes == 0
            || max_output_bytes == 0
            || max_input_bytes > MAX_MANIFEST_PAYLOAD_BYTES
            || max_output_bytes > MAX_MANIFEST_PAYLOAD_BYTES
        {
            return Err(AgentError::Input(format!(
                "manifest payload limits must be between 1 and {} bytes",
                MAX_MANIFEST_PAYLOAD_BYTES
            )));
        }
        self.max_input_bytes = max_input_bytes;
        self.max_output_bytes = max_output_bytes;
        Ok(self)
    }

    fn authorize(
        &self,
        tool_name: &str,
        tool_capabilities: &BTreeSet<Capability>,
        approved: bool,
    ) -> Result<(), AgentError> {
        if !self.allowed_tools.contains(tool_name) {
            return Err(AgentError::ConsentDenied(format!(
                "manifest '{}' does not allow tool '{}'",
                self.id, tool_name
            )));
        }
        if !self.capabilities.is_superset(tool_capabilities) {
            return Err(AgentError::ConsentDenied(format!(
                "manifest '{}' does not grant every capability required by '{}'",
                self.id, tool_name
            )));
        }
        if self.requires_approval && !approved {
            return Err(AgentError::ConsentApprovalRequired(format!(
                "manifest '{}' requires approval",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConsentStore {
    manifests: Arc<RwLock<BTreeMap<String, ConsentManifest>>>,
}

impl ConsentStore {
    pub fn register(&self, manifest: ConsentManifest) -> Result<(), AgentError> {
        let mut manifests = self
            .manifests
            .write()
            .map_err(|_| AgentError::ConsentDenied("manifest store lock poisoned".into()))?;
        if manifests.contains_key(&manifest.id) {
            return Err(AgentError::Input(format!(
                "consent manifest '{}' is already registered",
                manifest.id
            )));
        }
        manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn revoke(&self, id: &str) -> Result<bool, AgentError> {
        let mut manifests = self
            .manifests
            .write()
            .map_err(|_| AgentError::ConsentDenied("manifest store lock poisoned".into()))?;
        Ok(manifests.remove(id).is_some())
    }

    pub fn get(&self, id: &str) -> Result<ConsentManifest, AgentError> {
        let manifests = self
            .manifests
            .read()
            .map_err(|_| AgentError::ConsentDenied("manifest store lock poisoned".into()))?;
        manifests.get(id).cloned().ok_or_else(|| {
            AgentError::ConsentDenied(format!("consent manifest '{}' is not registered", id))
        })
    }

    pub fn list(&self) -> Result<Vec<ConsentManifest>, AgentError> {
        let manifests = self
            .manifests
            .read()
            .map_err(|_| AgentError::ConsentDenied("manifest store lock poisoned".into()))?;
        Ok(manifests.values().cloned().collect())
    }
}

pub type IntegrationHandler = Arc<dyn Fn(&Value) -> Result<Value, AgentError> + Send + Sync>;

pub struct ConsentScopedTool {
    spec: ToolSpec,
    manifest_id: String,
    store: ConsentStore,
    handler: IntegrationHandler,
}

impl ConsentScopedTool {
    pub fn new<F>(
        spec: ToolSpec,
        manifest_id: &str,
        store: ConsentStore,
        handler: F,
    ) -> Result<Self, AgentError>
    where
        F: Fn(&Value) -> Result<Value, AgentError> + Send + Sync + 'static,
    {
        if manifest_id.trim().is_empty() {
            return Err(AgentError::Input("adapter manifest id is empty".into()));
        }
        if spec.name.trim().is_empty() {
            return Err(AgentError::Input("adapter tool name is empty".into()));
        }
        Ok(Self {
            spec,
            manifest_id: manifest_id.to_string(),
            store,
            handler: Arc::new(handler),
        })
    }

    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }
}

impl Tool for ConsentScopedTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, input: &Value, workspace: &Workspace) -> Result<Value, AgentError> {
        self.execute_with_approval(input, workspace, false)
    }

    fn execute_with_approval(
        &self,
        input: &Value,
        _workspace: &Workspace,
        approved: bool,
    ) -> Result<Value, AgentError> {
        let manifest = self.store.get(&self.manifest_id)?;
        manifest.authorize(&self.spec.name, &self.spec.capabilities, approved)?;
        manifest.validate_host(input)?;
        let input_size = to_vec(input)
            .map_err(|error| AgentError::Serialization(error.to_string()))?
            .len();
        if input_size > manifest.max_input_bytes {
            return Err(AgentError::BudgetExceeded(format!(
                "integration input is {} bytes, limit is {}",
                input_size, manifest.max_input_bytes
            )));
        }
        let output = (self.handler)(input)?;
        let output_size = to_vec(&output)
            .map_err(|error| AgentError::Serialization(error.to_string()))?
            .len();
        if output_size > manifest.max_output_bytes {
            return Err(AgentError::BudgetExceeded(format!(
                "integration output is {} bytes, limit is {}",
                output_size, manifest.max_output_bytes
            )));
        }
        Ok(output)
    }
}

pub type McpToolAdapter = ConsentScopedTool;
pub type SkillToolAdapter = ConsentScopedTool;
pub type ApiToolAdapter = ConsentScopedTool;
pub type WebToolAdapter = ConsentScopedTool;
pub type LspToolAdapter = ConsentScopedTool;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::{
        Action, ActionStatus, EventJournal, Plan, Policy, RunOptions, Runtime, ToolRegistry,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    fn manifest(store: &ConsentStore, requires_approval: bool) {
        let manifest = ConsentManifest::new(
            "mcp.local.echo",
            "1.0.0",
            IntegrationKind::Mcp,
            "Local test MCP server",
            [Capability::NetworkAccess, Capability::McpAccess]
                .into_iter()
                .collect(),
            ["mcp_echo".to_string()].into_iter().collect(),
            requires_approval,
        )
        .unwrap();
        store.register(manifest).unwrap();
    }

    fn adapter(store: ConsentStore, calls: Arc<AtomicUsize>) -> ConsentScopedTool {
        ConsentScopedTool::new(
            ToolSpec {
                name: "mcp_echo".into(),
                description: "Test consent-scoped MCP echo".into(),
                capabilities: [Capability::NetworkAccess, Capability::McpAccess]
                    .into_iter()
                    .collect(),
                input_schema: json!({"type":"object"}),
                default_timeout_ms: 1_000,
            },
            "mcp.local.echo",
            store,
            move |input| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"echo": input}))
            },
        )
        .unwrap()
    }

    #[test]
    fn direct_execution_fails_closed_without_approval() {
        let store = ConsentStore::default();
        manifest(&store, true);
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = adapter(store, Arc::clone(&calls));
        let workspace = Workspace::new(tempdir().unwrap().path()).unwrap();
        let error = tool.execute(&json!({"x": 1}), &workspace).unwrap_err();
        assert!(matches!(error, AgentError::ConsentApprovalRequired(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn revocation_blocks_future_calls() {
        let store = ConsentStore::default();
        manifest(&store, false);
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = adapter(store.clone(), Arc::clone(&calls));
        let workspace = Workspace::new(tempdir().unwrap().path()).unwrap();
        tool.execute(&json!({"x": 1}), &workspace).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(store.revoke("mcp.local.echo").unwrap());
        let error = tool
            .execute_with_approval(&json!({"x": 2}), &workspace, true)
            .unwrap_err();
        assert!(matches!(error, AgentError::ConsentDenied(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn network_host_allowlist_is_enforced_before_handler_execution() {
        let store = ConsentStore::default();
        let manifest = ConsentManifest::new(
            "api.local.echo",
            "1.0.0",
            IntegrationKind::Api,
            "Test API",
            [Capability::NetworkAccess, Capability::ApiAccess]
                .into_iter()
                .collect(),
            ["api_echo".to_string()].into_iter().collect(),
            false,
        )
        .unwrap()
        .with_network_hosts(["api.example.test".to_string()].into_iter().collect())
        .unwrap();
        store.register(manifest).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = ConsentScopedTool::new(
            ToolSpec {
                name: "api_echo".into(),
                description: "Test API echo".into(),
                capabilities: [Capability::NetworkAccess, Capability::ApiAccess]
                    .into_iter()
                    .collect(),
                input_schema: json!({"type":"object","required":["host"]}),
                default_timeout_ms: 1_000,
            },
            "api.local.echo",
            store,
            {
                let calls = Arc::clone(&calls);
                move |input| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(input.clone())
                }
            },
        )
        .unwrap();
        let workspace = Workspace::new(tempdir().unwrap().path()).unwrap();
        assert!(matches!(
            tool.execute(&json!({"host":"evil.example.test"}), &workspace),
            Err(AgentError::ConsentDenied(_))
        ));
        tool.execute(&json!({"host":"api.example.test"}), &workspace)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_approval_reaches_external_adapter() {
        let store = ConsentStore::default();
        manifest(&store, true);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry
            .register(adapter(store, Arc::clone(&calls)))
            .unwrap();
        let workspace_dir = tempdir().unwrap();
        let workspace = Workspace::new(workspace_dir.path()).unwrap();
        let journal = EventJournal::new(workspace_dir.path().join("events.jsonl"));
        let runtime = Runtime::new(
            workspace,
            registry,
            Policy::restricted()
                .allow(Capability::NetworkAccess)
                .allow(Capability::McpAccess)
                .require_approval(Capability::NetworkAccess)
                .require_approval(Capability::McpAccess),
            journal,
        );
        let plan = Plan {
            id: "consent-plan".into(),
            goal: "call a consent-scoped adapter".into(),
            actions: vec![Action {
                id: "call".into(),
                tool: "mcp_echo".into(),
                input: json!({"message":"hello"}),
                depends_on: vec![],
                capabilities: vec![Capability::NetworkAccess, Capability::McpAccess],
                timeout_ms: None,
            }],
            max_steps: 2,
            max_output_bytes: 1024,
        };
        let blocked = runtime.run(&plan, &RunOptions::default()).unwrap();
        assert_eq!(blocked.status, ActionStatus::Blocked);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let approved = runtime
            .run(
                &plan,
                &RunOptions {
                    approved_actions: ["call".to_string()].into_iter().collect(),
                    ..RunOptions::default()
                },
            )
            .unwrap();
        assert_eq!(approved.status, ActionStatus::Succeeded);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
