//! Local-first agent kernel for UN1C⓪.
//!
//! The kernel treats model output as an untrusted plan. Plans are validated,
//! capability checked, executed inside a scoped workspace, and recorded as
//! append-only JSONL events. This module intentionally contains no network or
//! arbitrary-shell capability; those must be added behind explicit adapters.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const DEFAULT_MAX_STEPS: usize = 64;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_LIST_ENTRIES: usize = 2_000;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
    #[error("policy denied action '{action}': {reason}")]
    PolicyDenied { action: String, reason: String },
    #[error("approval required for action '{0}'")]
    ApprovalRequired(String),
    #[error("tool '{0}' is not registered")]
    UnknownTool(String),
    #[error("tool input error: {0}")]
    Input(String),
    #[error("workspace error: {0}")]
    Workspace(String),
    #[error("journal error: {0}")]
    Journal(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("execution budget exceeded: {0}")]
    BudgetExceeded(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "workspace.read")]
    WorkspaceRead,
    #[serde(rename = "workspace.write")]
    WorkspaceWrite,
    #[serde(rename = "process.exec")]
    ProcessExec,
    #[serde(rename = "network.access")]
    NetworkAccess,
    #[serde(rename = "secret.read")]
    SecretRead,
    #[serde(rename = "evolution.propose")]
    EvolutionPropose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub goal: String,
    pub actions: Vec<Action>,
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

fn default_max_steps() -> usize {
    DEFAULT_MAX_STEPS
}

fn default_max_output_bytes() -> usize {
    DEFAULT_MAX_OUTPUT_BYTES
}

impl Plan {
    pub fn validate(&self, registry: &ToolRegistry) -> Result<(), AgentError> {
        if self.id.trim().is_empty() {
            return Err(AgentError::InvalidPlan("plan id is empty".into()));
        }
        if self.goal.trim().is_empty() {
            return Err(AgentError::InvalidPlan("goal is empty".into()));
        }
        if self.actions.is_empty() {
            return Err(AgentError::InvalidPlan("plan has no actions".into()));
        }
        if self.max_steps == 0 || self.max_steps > 10_000 {
            return Err(AgentError::InvalidPlan(
                "max_steps must be between 1 and 10000".into(),
            ));
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > 16 * 1024 * 1024 {
            return Err(AgentError::InvalidPlan(
                "max_output_bytes must be between 1 and 16777216".into(),
            ));
        }

        let mut ids = BTreeSet::new();
        for action in &self.actions {
            if action.id.trim().is_empty() {
                return Err(AgentError::InvalidPlan("action id is empty".into()));
            }
            if !ids.insert(action.id.clone()) {
                return Err(AgentError::InvalidPlan(format!(
                    "duplicate action id '{}'",
                    action.id
                )));
            }
            if action.tool.trim().is_empty() {
                return Err(AgentError::InvalidPlan(format!(
                    "action '{}' has an empty tool name",
                    action.id
                )));
            }
            let tool = registry
                .get(&action.tool)
                .ok_or_else(|| AgentError::UnknownTool(action.tool.clone()))?;
            let requested: BTreeSet<_> = action.capabilities.iter().copied().collect();
            if !tool.spec().capabilities.is_subset(&requested) {
                return Err(AgentError::InvalidPlan(format!(
                    "action '{}' must explicitly declare every capability required by tool '{}'",
                    action.id, action.tool
                )));
            }
            if !requested.is_subset(&tool.spec().capabilities) {
                return Err(AgentError::InvalidPlan(format!(
                    "action '{}' requests a capability not declared by tool '{}'",
                    action.id, action.tool
                )));
            }
        }

        for action in &self.actions {
            for dependency in &action.depends_on {
                if !ids.contains(dependency) {
                    return Err(AgentError::InvalidPlan(format!(
                        "action '{}' depends on missing action '{}'",
                        action.id, dependency
                    )));
                }
                if dependency == &action.id {
                    return Err(AgentError::InvalidPlan(format!(
                        "action '{}' depends on itself",
                        action.id
                    )));
                }
            }
        }

        let _ = topological_order(&self.actions)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub capabilities: BTreeSet<Capability>,
    pub input_schema: Value,
    pub default_timeout_ms: u64,
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;
    fn execute(&self, input: &Value, workspace: &Workspace) -> Result<Value, AgentError>;
}

pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) -> Result<(), AgentError> {
        let name = tool.spec().name.clone();
        if name.trim().is_empty() {
            return Err(AgentError::InvalidPlan("tool name is empty".into()));
        }
        if self.tools.contains_key(&name) {
            return Err(AgentError::InvalidPlan(format!(
                "duplicate tool '{}'",
                name
            )));
        }
        self.tools.insert(name, Arc::new(tool));
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|tool| tool.spec().clone())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Policy {
    allowed: BTreeSet<Capability>,
    require_approval: BTreeSet<Capability>,
}

impl Policy {
    pub fn restricted() -> Self {
        Self {
            allowed: [Capability::WorkspaceRead].into_iter().collect(),
            require_approval: BTreeSet::new(),
        }
    }

    pub fn developer() -> Self {
        Self {
            allowed: [Capability::WorkspaceRead, Capability::WorkspaceWrite]
                .into_iter()
                .collect(),
            require_approval: [Capability::WorkspaceWrite].into_iter().collect(),
        }
    }

    pub fn allow(mut self, capability: Capability) -> Self {
        self.allowed.insert(capability);
        self
    }

    pub fn require_approval(mut self, capability: Capability) -> Self {
        self.require_approval.insert(capability);
        self
    }

    fn decision(&self, action: &Action, tool: &ToolSpec, approved: bool) -> Result<(), AgentError> {
        let requested: BTreeSet<_> = action.capabilities.iter().copied().collect();
        if !tool.capabilities.is_superset(&requested) {
            return Err(AgentError::PolicyDenied {
                action: action.id.clone(),
                reason: "action requests capabilities not declared by the tool".into(),
            });
        }
        for capability in requested {
            if !self.allowed.contains(&capability) {
                return Err(AgentError::PolicyDenied {
                    action: action.id.clone(),
                    reason: format!(
                        "capability '{}' is not enabled",
                        capability_name(capability)
                    ),
                });
            }
            if self.require_approval.contains(&capability) && !approved {
                return Err(AgentError::ApprovalRequired(action.id.clone()));
            }
        }
        Ok(())
    }
}

fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::WorkspaceRead => "workspace.read",
        Capability::WorkspaceWrite => "workspace.write",
        Capability::ProcessExec => "process.exec",
        Capability::NetworkAccess => "network.access",
        Capability::SecretRead => "secret.read",
        Capability::EvolutionPropose => "evolution.propose",
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, AgentError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|error| AgentError::Workspace(error.to_string()))?;
        let canonical = root
            .canonicalize()
            .map_err(|error| AgentError::Workspace(error.to_string()))?;
        Ok(Self { root: canonical })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve_existing(&self, relative: &str) -> Result<PathBuf, AgentError> {
        let relative_path = validate_relative_path(relative)?;
        let path = self.root.join(relative_path);
        let canonical = path.canonicalize().map_err(|error| {
            AgentError::Workspace(format!("cannot resolve '{}': {}", relative, error))
        })?;
        ensure_contained(&self.root, &canonical, relative)
    }

    fn resolve_for_write(&self, relative: &str) -> Result<PathBuf, AgentError> {
        let relative_path = validate_relative_path(relative)?;
        let path = self.root.join(relative_path);
        let parent = path
            .parent()
            .ok_or_else(|| AgentError::Workspace("write path has no parent".into()))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|error| AgentError::Workspace(format!("cannot resolve parent: {}", error)))?;
        let contained_parent = ensure_contained(&self.root, &canonical_parent, relative)?;
        Ok(contained_parent.join(
            path.file_name()
                .ok_or_else(|| AgentError::Workspace("write path has no filename".into()))?,
        ))
    }

    pub fn read_file(&self, relative: &str, max_bytes: usize) -> Result<String, AgentError> {
        let path = self.resolve_existing(relative)?;
        let metadata =
            fs::metadata(&path).map_err(|error| AgentError::Workspace(error.to_string()))?;
        if !metadata.is_file() {
            return Err(AgentError::Workspace(format!(
                "'{}' is not a file",
                relative
            )));
        }
        if metadata.len() > max_bytes as u64 {
            return Err(AgentError::BudgetExceeded(format!(
                "file '{}' is {} bytes, limit is {}",
                relative,
                metadata.len(),
                max_bytes
            )));
        }
        let mut file =
            File::open(&path).map_err(|error| AgentError::Workspace(error.to_string()))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|error| AgentError::Workspace(error.to_string()))?;
        Ok(content)
    }

    pub fn write_file(
        &self,
        relative: &str,
        content: &str,
        max_bytes: usize,
    ) -> Result<(), AgentError> {
        if content.len() > max_bytes {
            return Err(AgentError::BudgetExceeded(format!(
                "write '{}' is {} bytes, limit is {}",
                relative,
                content.len(),
                max_bytes
            )));
        }
        let path = self.resolve_for_write(relative)?;
        let parent = path
            .parent()
            .ok_or_else(|| AgentError::Workspace("write path has no parent".into()))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AgentError::Workspace("invalid filename".into()))?;
        let temp = parent.join(format!(".{}.un1c0-{}.tmp", file_name, stamp));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| AgentError::Workspace(error.to_string()))?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| AgentError::Workspace(error.to_string()))?;
        fs::rename(&temp, &path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            AgentError::Workspace(error.to_string())
        })?;
        Ok(())
    }

    pub fn list_files(&self, max_entries: usize) -> Result<Vec<String>, AgentError> {
        let limit = max_entries.min(MAX_LIST_ENTRIES).max(1);
        let mut queue = VecDeque::from([self.root.clone()]);
        let mut files = Vec::new();
        while let Some(directory) = queue.pop_front() {
            for entry in fs::read_dir(&directory)
                .map_err(|error| AgentError::Workspace(error.to_string()))?
            {
                let entry = entry.map_err(|error| AgentError::Workspace(error.to_string()))?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| AgentError::Workspace(error.to_string()))?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    queue.push_back(path);
                } else if metadata.is_file() {
                    let relative = path
                        .strip_prefix(&self.root)
                        .map_err(|error| AgentError::Workspace(error.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    files.push(relative);
                    if files.len() >= limit {
                        files.sort();
                        return Ok(files);
                    }
                }
            }
        }
        files.sort();
        Ok(files)
    }
}

fn validate_relative_path(relative: &str) -> Result<PathBuf, AgentError> {
    if relative.trim().is_empty() {
        return Err(AgentError::Workspace("path is empty".into()));
    }
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(AgentError::Workspace(
            "absolute paths are not allowed".into(),
        ));
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AgentError::Workspace(
                    "path traversal is not allowed".into(),
                ))
            }
            Component::CurDir => {}
            Component::Normal(_) => {}
        }
    }
    Ok(path.to_path_buf())
}

fn ensure_contained(root: &Path, path: &Path, relative: &str) -> Result<PathBuf, AgentError> {
    if !path.starts_with(root) {
        return Err(AgentError::Workspace(format!(
            "path '{}' escapes workspace",
            relative
        )));
    }
    Ok(path.to_path_buf())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub sequence: u64,
    pub run_id: String,
    pub timestamp_ms: u128,
    pub kind: String,
    pub action_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug)]
pub struct EventJournal {
    path: PathBuf,
    next_sequence: Arc<Mutex<u64>>,
}

impl Clone for EventJournal {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            next_sequence: Arc::clone(&self.next_sequence),
        }
    }
}

impl EventJournal {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let next = fs::read_to_string(&path)
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .rev()
                    .find_map(|line| serde_json::from_str::<RunEvent>(line).ok())
            })
            .map(|event| event.sequence.saturating_add(1))
            .unwrap_or(1);
        Self {
            path,
            next_sequence: Arc::new(Mutex::new(next)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &RunEvent) -> Result<(), AgentError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| AgentError::Journal(error.to_string()))?;
        }
        let mut event = event.clone();
        let mut sequence = self
            .next_sequence
            .lock()
            .map_err(|_| AgentError::Journal("journal sequence lock poisoned".into()))?;
        event.sequence = *sequence;
        *sequence = sequence.saturating_add(1);
        let line = serde_json::to_string(&event)
            .map_err(|error| AgentError::Serialization(error.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| AgentError::Journal(error.to_string()))?;
        writeln!(file, "{}", line).map_err(|error| AgentError::Journal(error.to_string()))?;
        file.sync_data()
            .map_err(|error| AgentError::Journal(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ActionStatus {
    Succeeded,
    Failed,
    Skipped,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub status: ActionStatus,
    pub output: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub plan_id: String,
    pub status: ActionStatus,
    pub results: Vec<ActionResult>,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub approved_actions: BTreeSet<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            approved_actions: BTreeSet::new(),
        }
    }
}

pub struct Runtime {
    workspace: Workspace,
    registry: ToolRegistry,
    policy: Policy,
    journal: EventJournal,
    max_output_bytes: usize,
}

impl Runtime {
    pub fn new(
        workspace: Workspace,
        registry: ToolRegistry,
        policy: Policy,
        journal: EventJournal,
    ) -> Self {
        Self {
            workspace,
            registry,
            policy,
            journal,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    pub fn with_output_limit(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes.max(1);
        self
    }

    pub fn run(&self, plan: &Plan, options: &RunOptions) -> Result<RunReport, AgentError> {
        plan.validate(&self.registry)?;
        let run_id = stable_id(&format!("{}:{}:{}", plan.id, plan.goal, now_ms()));
        self.emit(
            &run_id,
            "run_started",
            None,
            json!({ "plan_id": plan.id, "goal": plan.goal }),
        )?;

        let order = topological_order(&plan.actions)?;
        let action_map: HashMap<_, _> = plan
            .actions
            .iter()
            .map(|action| (action.id.clone(), action))
            .collect();
        let mut statuses: HashMap<String, ActionStatus> = HashMap::new();
        let mut results = Vec::new();
        let step_limit = plan.max_steps.min(order.len());
        if order.len() > step_limit {
            return Err(AgentError::BudgetExceeded(format!(
                "plan contains {} actions, limit is {}",
                order.len(),
                step_limit
            )));
        }

        for action_id in order {
            let action = action_map.get(&action_id).ok_or_else(|| {
                AgentError::InvalidPlan("topological order referenced missing action".into())
            })?;
            if action
                .depends_on
                .iter()
                .any(|dependency| statuses.get(dependency) != Some(&ActionStatus::Succeeded))
            {
                let result = ActionResult {
                    action_id: action.id.clone(),
                    status: ActionStatus::Skipped,
                    output: Value::Null,
                    error: Some("dependency did not succeed".into()),
                };
                self.emit(&run_id, "action_skipped", Some(&action.id), json!(&result))?;
                statuses.insert(action.id.clone(), ActionStatus::Skipped);
                results.push(result);
                continue;
            }

            let tool = self
                .registry
                .get(&action.tool)
                .ok_or_else(|| AgentError::UnknownTool(action.tool.clone()))?;
            self.emit(
                &run_id,
                "action_started",
                Some(&action.id),
                json!({ "tool": action.tool }),
            )?;
            let approved = options.approved_actions.contains(&action.id);
            let result = match self.policy.decision(action, tool.spec(), approved) {
                Ok(()) => match execute_with_timeout(
                    Arc::clone(tool),
                    action.input.clone(),
                    self.workspace.clone(),
                    action.timeout_ms.unwrap_or(tool.spec().default_timeout_ms),
                ) {
                    Ok(output) => {
                        let serialized = serde_json::to_vec(&output)
                            .map_err(|error| AgentError::Serialization(error.to_string()))?;
                        if serialized.len() > self.max_output_bytes.min(plan.max_output_bytes) {
                            ActionResult {
                                action_id: action.id.clone(),
                                status: ActionStatus::Failed,
                                output: Value::Null,
                                error: Some("tool output exceeded configured limit".into()),
                            }
                        } else {
                            ActionResult {
                                action_id: action.id.clone(),
                                status: ActionStatus::Succeeded,
                                output,
                                error: None,
                            }
                        }
                    }
                    Err(error) => ActionResult {
                        action_id: action.id.clone(),
                        status: ActionStatus::Failed,
                        output: Value::Null,
                        error: Some(error.to_string()),
                    },
                },
                Err(AgentError::ApprovalRequired(_)) => ActionResult {
                    action_id: action.id.clone(),
                    status: ActionStatus::Blocked,
                    output: Value::Null,
                    error: Some("explicit approval required".into()),
                },
                Err(error) => ActionResult {
                    action_id: action.id.clone(),
                    status: ActionStatus::Blocked,
                    output: Value::Null,
                    error: Some(error.to_string()),
                },
            };
            let event_kind = match result.status {
                ActionStatus::Succeeded => "action_succeeded",
                ActionStatus::Failed => "action_failed",
                ActionStatus::Skipped => "action_skipped",
                ActionStatus::Blocked => "action_blocked",
            };
            self.emit(&run_id, event_kind, Some(&action.id), json!(&result))?;
            statuses.insert(action.id.clone(), result.status.clone());
            results.push(result);
        }

        let status = if results
            .iter()
            .all(|result| result.status == ActionStatus::Succeeded)
        {
            ActionStatus::Succeeded
        } else if results
            .iter()
            .any(|result| result.status == ActionStatus::Failed)
        {
            ActionStatus::Failed
        } else {
            ActionStatus::Blocked
        };
        self.emit(&run_id, "run_finished", None, json!({ "status": status }))?;
        Ok(RunReport {
            run_id,
            plan_id: plan.id.clone(),
            status,
            results,
        })
    }

    fn emit(
        &self,
        run_id: &str,
        kind: &str,
        action_id: Option<&str>,
        payload: Value,
    ) -> Result<(), AgentError> {
        self.journal.append(&RunEvent {
            sequence: 0,
            run_id: run_id.to_string(),
            timestamp_ms: now_ms(),
            kind: kind.to_string(),
            action_id: action_id.map(ToString::to_string),
            payload,
        })
    }
}

fn execute_with_timeout(
    tool: Arc<dyn Tool>,
    input: Value,
    workspace: Workspace,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = tool.execute(&input, &workspace);
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(AgentError::BudgetExceeded(format!(
            "tool execution timed out after {} ms",
            timeout_ms
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(AgentError::Workspace("tool worker disconnected".into()))
        }
    }
}

pub trait Planner {
    fn plan(&self, goal: &str) -> Result<Plan, AgentError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicPlanner;

impl Planner for DeterministicPlanner {
    fn plan(&self, goal: &str) -> Result<Plan, AgentError> {
        let goal = goal.trim();
        if goal.is_empty() {
            return Err(AgentError::InvalidPlan("goal is empty".into()));
        }
        let digest = stable_id(goal);
        Ok(Plan {
            id: format!("plan-{}", &digest[..12]),
            goal: goal.to_string(),
            actions: vec![
                Action {
                    id: "frame_goal".into(),
                    tool: "echo".into(),
                    input: json!({ "message": goal }),
                    depends_on: vec![],
                    capabilities: vec![],
                    timeout_ms: None,
                },
                Action {
                    id: "inspect_workspace".into(),
                    tool: "list_files".into(),
                    input: json!({ "max_entries": 200 }),
                    depends_on: vec!["frame_goal".into()],
                    capabilities: vec![Capability::WorkspaceRead],
                    timeout_ms: None,
                },
            ],
            max_steps: DEFAULT_MAX_STEPS,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub scope: String,
    pub content: String,
    pub importance: u8,
    pub created_at_ms: u128,
    pub expires_at_ms: Option<u128>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MemoryStore {
    entries: Vec<MemoryEntry>,
}

impl MemoryStore {
    pub fn remember(
        &mut self,
        scope: &str,
        content: &str,
        importance: u8,
        ttl_ms: Option<u128>,
    ) -> String {
        let now = now_ms();
        let id = stable_id(&format!("{}:{}:{}", scope, content, now));
        self.entries.push(MemoryEntry {
            id: id.clone(),
            scope: scope.to_string(),
            content: content.to_string(),
            importance: importance.min(100),
            created_at_ms: now,
            expires_at_ms: ttl_ms.map(|ttl| now.saturating_add(ttl)),
        });
        id
    }

    pub fn retrieve(&mut self, scope: &str, query: &str, limit: usize) -> Vec<MemoryEntry> {
        let now = now_ms();
        self.entries.retain(|entry| {
            entry
                .expires_at_ms
                .map(|expires| expires > now)
                .unwrap_or(true)
        });
        let query_terms: BTreeSet<_> = query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();
        let mut ranked: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.scope == scope)
            .map(|entry| {
                let content = entry.content.to_lowercase();
                let overlap = query_terms
                    .iter()
                    .filter(|term| content.contains(term.as_str()))
                    .count();
                (
                    overlap,
                    entry.importance,
                    entry.created_at_ms,
                    entry.clone(),
                )
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(right.1.cmp(&left.1))
                .then(right.2.cmp(&left.2))
        });
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, _, entry)| entry)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionProposal {
    pub id: String,
    pub title: String,
    pub files: BTreeMap<String, String>,
    pub test_command: String,
    pub risk: String,
    pub content_hash: String,
    pub approved: bool,
}

impl EvolutionProposal {
    pub fn new(
        title: &str,
        files: BTreeMap<String, String>,
        test_command: &str,
        risk: &str,
    ) -> Result<Self, AgentError> {
        if title.trim().is_empty() || files.is_empty() || test_command.trim().is_empty() {
            return Err(AgentError::Input(
                "evolution proposal requires title, files, and test command".into(),
            ));
        }
        for path in files.keys() {
            validate_relative_path(path)?;
        }
        let canonical = serde_json::to_vec(&(title, &files, test_command, risk))
            .map_err(|error| AgentError::Serialization(error.to_string()))?;
        let content_hash = hex_digest(&canonical);
        Ok(Self {
            id: format!("evo-{}", &content_hash[..12]),
            title: title.to_string(),
            files,
            test_command: test_command.to_string(),
            risk: risk.to_string(),
            content_hash,
            approved: false,
        })
    }

    pub fn approve(&mut self) {
        self.approved = true;
    }
}

pub fn built_in_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(EchoTool::new())
        .expect("built-in tool name is valid");
    registry
        .register(ListFilesTool::new())
        .expect("built-in tool name is valid");
    registry
        .register(ReadFileTool::new())
        .expect("built-in tool name is valid");
    registry
        .register(WriteFileTool::new())
        .expect("built-in tool name is valid");
    registry
}

pub struct EchoTool {
    spec: ToolSpec,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "echo".into(),
                description: "Return a bounded message without side effects".into(),
                capabilities: BTreeSet::new(),
                input_schema: json!({"type":"object","required":["message"]}),
                default_timeout_ms: 1_000,
            },
        }
    }
}

impl Tool for EchoTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, input: &Value, _workspace: &Workspace) -> Result<Value, AgentError> {
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Input("echo requires string field 'message'".into()))?;
        Ok(json!({ "message": message }))
    }
}

pub struct ListFilesTool {
    spec: ToolSpec,
}

impl ListFilesTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "list_files".into(),
                description: "List regular files below the scoped workspace".into(),
                capabilities: [Capability::WorkspaceRead].into_iter().collect(),
                input_schema: json!({"type":"object","properties":{"max_entries":{"type":"integer"}}}),
                default_timeout_ms: 2_000,
            },
        }
    }
}

impl Tool for ListFilesTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, input: &Value, workspace: &Workspace) -> Result<Value, AgentError> {
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(200) as usize;
        Ok(json!({ "files": workspace.list_files(max_entries)? }))
    }
}

pub struct ReadFileTool {
    spec: ToolSpec,
}

impl ReadFileTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "read_file".into(),
                description: "Read a UTF-8 file inside the scoped workspace".into(),
                capabilities: [Capability::WorkspaceRead].into_iter().collect(),
                input_schema: json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}),
                default_timeout_ms: 2_000,
            },
        }
    }
}

impl Tool for ReadFileTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, input: &Value, workspace: &Workspace) -> Result<Value, AgentError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Input("read_file requires string field 'path'".into()))?;
        Ok(json!({ "path": path, "content": workspace.read_file(path, DEFAULT_MAX_OUTPUT_BYTES)? }))
    }
}

pub struct WriteFileTool {
    spec: ToolSpec,
}

impl WriteFileTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "write_file".into(),
                description: "Atomically write UTF-8 content inside the scoped workspace".into(),
                capabilities: [Capability::WorkspaceWrite].into_iter().collect(),
                input_schema: json!({"type":"object","required":["path","content"],"properties":{"path":{"type":"string"},"content":{"type":"string"}}}),
                default_timeout_ms: 5_000,
            },
        }
    }
}

impl Tool for WriteFileTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(&self, input: &Value, workspace: &Workspace) -> Result<Value, AgentError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Input("write_file requires string field 'path'".into()))?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentError::Input("write_file requires string field 'content'".into())
            })?;
        workspace.write_file(path, content, DEFAULT_MAX_OUTPUT_BYTES)?;
        Ok(json!({ "path": path, "bytes_written": content.len() }))
    }
}

fn topological_order(actions: &[Action]) -> Result<Vec<String>, AgentError> {
    let mut indegree: HashMap<String, usize> = actions
        .iter()
        .map(|action| (action.id.clone(), 0))
        .collect();
    let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
    for action in actions {
        for dependency in &action.depends_on {
            *indegree.entry(action.id.clone()).or_default() += 1;
            outgoing
                .entry(dependency.clone())
                .or_default()
                .push(action.id.clone());
        }
    }
    let mut ready: BTreeSet<String> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = Vec::with_capacity(actions.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        if let Some(children) = outgoing.get(&id) {
            for child in children {
                let count = indegree.get_mut(child).ok_or_else(|| {
                    AgentError::InvalidPlan("dependency graph is inconsistent".into())
                })?;
                *count -= 1;
                if *count == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if order.len() != actions.len() {
        return Err(AgentError::InvalidPlan(
            "dependency graph contains a cycle".into(),
        ));
    }
    Ok(order)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn stable_id(input: &str) -> String {
    hex_digest(input.as_bytes())
}

fn hex_digest(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn plan(actions: Vec<Action>) -> Plan {
        Plan {
            id: "test-plan".into(),
            goal: "test goal".into(),
            actions,
            max_steps: 10,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    #[test]
    fn rejects_implicit_capabilities() {
        let registry = built_in_registry();
        let action = Action {
            id: "list".into(),
            tool: "list_files".into(),
            input: json!({}),
            depends_on: vec![],
            capabilities: vec![],
            timeout_ms: None,
        };
        assert!(
            matches!(plan(vec![action]).validate(&registry), Err(AgentError::InvalidPlan(message)) if message.contains("explicitly declare"))
        );
    }

    #[test]
    fn rejects_cycles_and_missing_dependencies() {
        let registry = built_in_registry();
        let cycle = plan(vec![
            Action {
                id: "a".into(),
                tool: "echo".into(),
                input: json!({"message":"a"}),
                depends_on: vec!["b".into()],
                capabilities: vec![],
                timeout_ms: None,
            },
            Action {
                id: "b".into(),
                tool: "echo".into(),
                input: json!({"message":"b"}),
                depends_on: vec!["a".into()],
                capabilities: vec![],
                timeout_ms: None,
            },
        ]);
        assert!(
            matches!(cycle.validate(&registry), Err(AgentError::InvalidPlan(message)) if message.contains("cycle"))
        );

        let missing = plan(vec![Action {
            id: "a".into(),
            tool: "echo".into(),
            input: json!({"message":"a"}),
            depends_on: vec!["missing".into()],
            capabilities: vec![],
            timeout_ms: None,
        }]);
        assert!(missing.validate(&registry).is_err());
    }

    #[test]
    fn executes_in_dependency_order_and_journals_events() {
        let dir = tempdir().unwrap();
        let journal = EventJournal::new(dir.path().join("events.jsonl"));
        let runtime = Runtime::new(
            Workspace::new(dir.path()).unwrap(),
            built_in_registry(),
            Policy::restricted(),
            journal.clone(),
        );
        let plan = plan(vec![
            Action {
                id: "second".into(),
                tool: "echo".into(),
                input: json!({"message":"second"}),
                depends_on: vec!["first".into()],
                capabilities: vec![],
                timeout_ms: None,
            },
            Action {
                id: "first".into(),
                tool: "echo".into(),
                input: json!({"message":"first"}),
                depends_on: vec![],
                capabilities: vec![],
                timeout_ms: None,
            },
        ]);
        let report = runtime.run(&plan, &RunOptions::default()).unwrap();
        assert_eq!(report.status, ActionStatus::Succeeded);
        assert_eq!(report.results[0].action_id, "first");
        assert_eq!(report.results[1].action_id, "second");
        let lines = fs::read_to_string(journal.path()).unwrap().lines().count();
        assert_eq!(lines, 6);
    }

    #[test]
    fn write_requires_approval_and_is_atomic_when_approved() {
        let dir = tempdir().unwrap();
        let runtime = Runtime::new(
            Workspace::new(dir.path()).unwrap(),
            built_in_registry(),
            Policy::developer(),
            EventJournal::new(dir.path().join("events.jsonl")),
        );
        let plan = plan(vec![Action {
            id: "write".into(),
            tool: "write_file".into(),
            input: json!({"path":"out.txt","content":"ok"}),
            depends_on: vec![],
            capabilities: vec![Capability::WorkspaceWrite],
            timeout_ms: None,
        }]);
        let blocked = runtime.run(&plan, &RunOptions::default()).unwrap();
        assert_eq!(blocked.status, ActionStatus::Blocked);
        assert!(!dir.path().join("out.txt").exists());

        let approved = runtime
            .run(
                &plan,
                &RunOptions {
                    approved_actions: ["write".into()].into_iter().collect(),
                },
            )
            .unwrap();
        assert_eq!(approved.status, ActionStatus::Succeeded);
        assert_eq!(
            fs::read_to_string(dir.path().join("out.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn workspace_rejects_traversal_and_symlinks() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("secret.txt"), "secret").unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("outside.txt"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("outside.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        assert!(workspace.read_file("../secret.txt", 100).is_err());
        #[cfg(unix)]
        assert!(workspace.read_file("link.txt", 100).is_err());
    }

    #[test]
    fn memory_expires_and_ranks_relevant_entries() {
        let mut memory = MemoryStore::default();
        memory.remember("session", "rust compiler error", 50, None);
        memory.remember("session", "rust compiler fixed", 90, None);
        memory.remember("session", "temporary", 100, Some(0));
        let results = memory.retrieve("session", "compiler", 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "rust compiler fixed");
    }

    #[test]
    fn evolution_proposal_is_hashed_and_not_auto_approved() {
        let files = [("skills/new.md".into(), "content".into())]
            .into_iter()
            .collect();
        let proposal = EvolutionProposal::new("new skill", files, "cargo test", "medium").unwrap();
        assert!(proposal.id.starts_with("evo-"));
        assert!(!proposal.approved);
        assert_eq!(proposal.content_hash.len(), 64);
    }
}
