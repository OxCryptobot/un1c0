//! Phase 4 verification contracts for compilers, linters, and test suites.
//!
//! This module intentionally performs manifest validation and evidence modeling
//! only. Actual process execution must be supplied by a trusted sandbox adapter.
//! The fail-closed default prevents model-generated commands from reaching the
//! host shell.

use crate::agentic::{Capability, Workspace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MAX_GATES: usize = 128;
const MAX_ARGS: usize = 64;
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationBudget {
    pub wall_clock_ms: u64,
    pub output_bytes: u64,
    pub max_processes: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub network: NetworkPolicy,
}

impl Default for VerificationBudget {
    fn default() -> Self {
        Self {
            wall_clock_ms: 600_000,
            output_bytes: 1024 * 1024,
            max_processes: 128,
            memory_bytes: 4 * 1024 * 1024 * 1024,
            disk_bytes: 8 * 1024 * 1024 * 1024,
            network: NetworkPolicy::Disabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkPolicy {
    Disabled,
    Allowlist(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationGate {
    pub id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub class: GateClass,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub required_capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GateClass {
    Format,
    Lint,
    Compile,
    Test,
    Security,
    Golden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationManifest {
    pub id: String,
    pub language: String,
    pub gates: Vec<VerificationGate>,
    #[serde(default)]
    pub budget: VerificationBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerificationStatus {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Unavailable,
    PolicyBlocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: Option<String>,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub run_id: String,
    pub gate_id: String,
    pub status: VerificationStatus,
    pub program: String,
    pub args_hash: String,
    pub workspace_tree_before: String,
    pub workspace_tree_after: String,
    pub toolchain_digest: Option<String>,
    pub started_at_ms: u128,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairTask {
    pub run_id: String,
    pub gate_id: String,
    pub failure_class: FailureClass,
    pub diagnostic_digest: String,
    pub checkpoint: String,
    pub remaining_iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailureClass {
    Syntax,
    Type,
    Compile,
    TestAssertion,
    Environment,
    Timeout,
    Dependency,
    Policy,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ValidatedGate {
    pub gate: VerificationGate,
    pub workspace_root: PathBuf,
    pub working_directory: PathBuf,
    pub timeout_ms: u64,
}

#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("invalid verification manifest: {0}")]
    InvalidManifest(String),
    #[error("verification policy blocked: {0}")]
    PolicyBlocked(String),
    #[error("verification unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub struct VerifierCatalog {
    allowed_programs: BTreeSet<String>,
    allowed_capabilities: BTreeSet<Capability>,
    allow_network: bool,
}

impl VerifierCatalog {
    pub fn safe_local() -> Self {
        Self {
            allowed_programs: [
                "cargo", "rustc", "rustfmt", "pytest", "python3", "npm", "pnpm", "go", "zig",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            allowed_capabilities: [Capability::WorkspaceRead, Capability::ProcessExec]
                .into_iter()
                .collect(),
            allow_network: false,
        }
    }

    pub fn allow_program(mut self, program: &str) -> Self {
        self.allowed_programs.insert(program.to_string());
        self
    }

    pub fn validate_manifest(
        &self,
        manifest: &VerificationManifest,
        workspace: &Workspace,
    ) -> Result<Vec<ValidatedGate>, VerificationError> {
        if manifest.id.trim().is_empty() || manifest.language.trim().is_empty() {
            return Err(VerificationError::InvalidManifest(
                "id and language are required".into(),
            ));
        }
        if manifest.gates.is_empty() || manifest.gates.len() > MAX_GATES {
            return Err(VerificationError::InvalidManifest(format!(
                "gate count must be between 1 and {}",
                MAX_GATES
            )));
        }
        if manifest.budget.output_bytes == 0 || manifest.budget.output_bytes > MAX_OUTPUT_BYTES {
            return Err(VerificationError::InvalidManifest(
                "output budget is out of bounds".into(),
            ));
        }
        if manifest.budget.network != NetworkPolicy::Disabled && !self.allow_network {
            return Err(VerificationError::PolicyBlocked(
                "network is disabled by verifier policy".into(),
            ));
        }

        let mut ids = BTreeSet::new();
        let mut validated = Vec::with_capacity(manifest.gates.len());
        for gate in &manifest.gates {
            if gate.id.trim().is_empty() || !ids.insert(gate.id.clone()) {
                return Err(VerificationError::InvalidManifest(format!(
                    "duplicate or empty gate id: {}",
                    gate.id
                )));
            }
            if !self.allowed_programs.contains(&gate.program) {
                return Err(VerificationError::PolicyBlocked(format!(
                    "program '{}' is not allowlisted",
                    gate.program
                )));
            }
            if gate.args.len() > MAX_ARGS {
                return Err(VerificationError::InvalidManifest(format!(
                    "gate '{}' has too many arguments",
                    gate.id
                )));
            }
            if gate
                .required_capabilities
                .iter()
                .any(|capability| !self.allowed_capabilities.contains(capability))
            {
                return Err(VerificationError::PolicyBlocked(format!(
                    "gate '{}' requests a disallowed capability",
                    gate.id
                )));
            }
            if gate.network && !self.allow_network {
                return Err(VerificationError::PolicyBlocked(format!(
                    "gate '{}' requests network access",
                    gate.id
                )));
            }
            for arg in &gate.args {
                if arg.len() > 8 * 1024 || contains_path_escape(arg) {
                    return Err(VerificationError::PolicyBlocked(format!(
                        "gate '{}' contains an unsafe argument",
                        gate.id
                    )));
                }
            }
            let working_directory =
                resolve_working_directory(workspace, gate.working_directory.as_deref())?;
            let timeout_ms = gate.timeout_ms.unwrap_or(manifest.budget.wall_clock_ms);
            if timeout_ms == 0 || timeout_ms > manifest.budget.wall_clock_ms {
                return Err(VerificationError::InvalidManifest(format!(
                    "gate '{}' timeout exceeds manifest budget",
                    gate.id
                )));
            }
            validated.push(ValidatedGate {
                gate: gate.clone(),
                workspace_root: workspace.root().to_path_buf(),
                working_directory,
                timeout_ms,
            });
        }
        Ok(validated)
    }
}

pub trait SandboxVerifier: Send + Sync {
    fn execute(
        &self,
        gate: &ValidatedGate,
        budget: &VerificationBudget,
    ) -> Result<VerificationResult, VerificationError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FailClosedVerifier;

impl SandboxVerifier for FailClosedVerifier {
    fn execute(
        &self,
        _gate: &ValidatedGate,
        _budget: &VerificationBudget,
    ) -> Result<VerificationResult, VerificationError> {
        Err(VerificationError::Unavailable(
            "no sandbox adapter is configured; refusing to execute a process on the host".into(),
        ))
    }
}

pub fn classify_failure(result: &VerificationResult) -> FailureClass {
    match result.status {
        VerificationStatus::TimedOut => FailureClass::Timeout,
        VerificationStatus::PolicyBlocked => FailureClass::Policy,
        VerificationStatus::Unavailable => FailureClass::Environment,
        VerificationStatus::Cancelled => FailureClass::Environment,
        VerificationStatus::Passed => FailureClass::Unknown,
        VerificationStatus::Failed => {
            let text = format!("{} {}", result.stdout, result.stderr).to_lowercase();
            if text.contains("syntax") || text.contains("parse") {
                FailureClass::Syntax
            } else if text.contains("type") {
                FailureClass::Type
            } else if text.contains("test") || text.contains("assert") {
                FailureClass::TestAssertion
            } else if text.contains("not found") || text.contains("no such file") {
                FailureClass::Dependency
            } else if text.contains("compile") || text.contains("error[e") {
                FailureClass::Compile
            } else {
                FailureClass::Unknown
            }
        }
    }
}

fn resolve_working_directory(
    workspace: &Workspace,
    relative: Option<&str>,
) -> Result<PathBuf, VerificationError> {
    let relative = relative.unwrap_or(".");
    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(VerificationError::PolicyBlocked(
            "absolute working directories are forbidden".into(),
        ));
    }
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(VerificationError::PolicyBlocked(
                "working directory escapes workspace".into(),
            ));
        }
    }
    let full = workspace.root().join(path);
    fs::create_dir_all(&full).map_err(|error| VerificationError::Unavailable(error.to_string()))?;
    let canonical = full
        .canonicalize()
        .map_err(|error| VerificationError::Unavailable(error.to_string()))?;
    if !canonical.starts_with(workspace.root()) {
        return Err(VerificationError::PolicyBlocked(
            "working directory escapes workspace".into(),
        ));
    }
    Ok(canonical)
}

fn contains_path_escape(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
}

#[derive(Debug, Clone)]
pub struct RootlessContainerConfig {
    pub runtime: String,
    pub image: String,
    pub require_rootless: bool,
    pub tmpfs_bytes: u64,
}

impl RootlessContainerConfig {
    pub fn new(
        runtime: impl Into<String>,
        image: impl Into<String>,
    ) -> Result<Self, VerificationError> {
        let runtime = runtime.into();
        let image = image.into();
        if runtime.trim().is_empty() || image.trim().is_empty() {
            return Err(VerificationError::InvalidManifest(
                "container runtime and image are required".into(),
            ));
        }
        Ok(Self {
            runtime,
            image,
            require_rootless: true,
            tmpfs_bytes: 256 * 1024 * 1024,
        })
    }
}

pub struct RootlessContainerVerifier {
    config: RootlessContainerConfig,
}

impl RootlessContainerVerifier {
    pub fn new(config: RootlessContainerConfig) -> Self {
        Self { config }
    }

    pub fn command_preview(
        &self,
        gate: &ValidatedGate,
        budget: &VerificationBudget,
    ) -> Vec<String> {
        let workdir = gate
            .working_directory
            .strip_prefix(&gate.workspace_root)
            .ok()
            .map(|path| format!("/workspace/{}", path.display()))
            .unwrap_or_else(|| "/workspace".into());
        let memory = format!("{}b", budget.memory_bytes);
        let tmpfs = format!("/tmp:rw,nosuid,size={}b", self.config.tmpfs_bytes);
        let mount = format!(
            "type=bind,src={},dst=/workspace,rw",
            gate.workspace_root.display()
        );
        let mut args = vec![
            self.config.runtime.clone(),
            "run".into(),
            "--rm".into(),
            "--init".into(),
            "--network=none".into(),
            "--cap-drop=ALL".into(),
            "--security-opt=no-new-privileges".into(),
            "--security-opt=seccomp=default".into(),
            format!("--pids-limit={}", budget.max_processes),
            format!("--memory={}", memory),
            format!("--cpus={}", cpu_quota(budget.max_processes)),
            "--read-only".into(),
            format!("--tmpfs={}", tmpfs),
            format!("--mount={}", mount),
            format!("--workdir={}", workdir),
            self.config.image.clone(),
            gate.gate.program.clone(),
        ];
        args.extend(gate.gate.args.clone());
        args
    }

    fn ensure_rootless_runtime(&self) -> Result<(), VerificationError> {
        let runtime_name = Path::new(&self.config.runtime)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let output = if runtime_name == "podman" {
            Command::new(&self.config.runtime)
                .args(["info", "--format", "{{.Host.Security.Rootless}}"])
                .output()
        } else if runtime_name == "docker" {
            Command::new(&self.config.runtime)
                .args(["info", "--format", "{{json .SecurityOptions}}"])
                .output()
        } else {
            return Err(VerificationError::Unavailable(format!(
                "unsupported container runtime '{}'; use docker or podman",
                self.config.runtime
            )));
        }
        .map_err(|error| {
            VerificationError::Unavailable(format!("container runtime unavailable: {}", error))
        })?;
        if !output.status.success() {
            return Err(VerificationError::Unavailable(
                "container runtime info failed".into(),
            ));
        }
        if self.config.require_rootless {
            let info = String::from_utf8_lossy(&output.stdout).to_lowercase();
            let rootless = if runtime_name == "podman" {
                info.trim() == "true" || info.contains("rootless: true")
            } else {
                info.contains("rootless")
            };
            if !rootless {
                return Err(VerificationError::PolicyBlocked(
                    "container runtime is not configured for rootless execution".into(),
                ));
            }
        }
        Ok(())
    }
}

impl SandboxVerifier for RootlessContainerVerifier {
    fn execute(
        &self,
        gate: &ValidatedGate,
        budget: &VerificationBudget,
    ) -> Result<VerificationResult, VerificationError> {
        if gate.gate.network || budget.network != NetworkPolicy::Disabled {
            return Err(VerificationError::PolicyBlocked(
                "rootless verifier only permits network-disabled gates".into(),
            ));
        }
        self.ensure_rootless_runtime()?;
        let started_at_ms = now_ms();
        let before = hash_tree(&gate.workspace_root)?;
        let args = self.command_preview(gate, budget);
        let args_hash = hash_bytes(
            serde_json::to_vec(&args)
                .map_err(|error| VerificationError::Unavailable(error.to_string()))?
                .as_slice(),
        );
        let mut command = Command::new(&args[0]);
        command
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            VerificationError::Unavailable(format!("failed to start container runtime: {}", error))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            VerificationError::Unavailable("container stdout pipe unavailable".into())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            VerificationError::Unavailable("container stderr pipe unavailable".into())
        })?;
        let output_limit = budget.output_bytes / 2;
        let stdout_thread = spawn_bounded_reader(stdout, output_limit);
        let stderr_thread = spawn_bounded_reader(stderr, output_limit);
        let timeout = Duration::from_millis(gate.timeout_ms.min(budget.wall_clock_ms));
        let deadline = Instant::now() + timeout;
        let (status, timed_out) = wait_with_timeout(&mut child, deadline)?;
        let stdout = join_reader(stdout_thread)?;
        let stderr = join_reader(stderr_thread)?;
        let after = hash_tree(&gate.workspace_root)?;
        let final_status = if timed_out {
            VerificationStatus::TimedOut
        } else if status.success() {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };
        Ok(VerificationResult {
            run_id: format!("verify-{}", started_at_ms),
            gate_id: gate.gate.id.clone(),
            status: final_status,
            program: gate.gate.program.clone(),
            args_hash,
            workspace_tree_before: before,
            workspace_tree_after: after,
            toolchain_digest: Some(hash_bytes(self.config.image.as_bytes())),
            started_at_ms,
            duration_ms: now_ms().saturating_sub(started_at_ms) as u64,
            exit_code: status.code(),
            stdout,
            stderr,
            diagnostics: Vec::new(),
        })
    }
}

fn wait_with_timeout(
    child: &mut Child,
    deadline: std::time::Instant,
) -> Result<(std::process::ExitStatus, bool), VerificationError> {
    loop {
        match child
            .try_wait()
            .map_err(|error| VerificationError::Unavailable(error.to_string()))?
        {
            Some(status) => return Ok((status, false)),
            None if std::time::Instant::now() >= deadline => {
                child
                    .kill()
                    .map_err(|error| VerificationError::Unavailable(error.to_string()))?;
                let status = child
                    .wait()
                    .map_err(|error| VerificationError::Unavailable(error.to_string()))?;
                return Ok((status, true));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn spawn_bounded_reader<R: Read + Send + 'static>(mut reader: R, limit: u64) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut truncated = false;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if (bytes.len() as u64) < limit {
                        let remaining = (limit - bytes.len() as u64) as usize;
                        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
                        if read > remaining {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
                Err(_) => break,
            }
        }
        let mut output = String::from_utf8_lossy(&bytes).into_owned();
        if truncated {
            output.push_str("\n...[output truncated by verifier budget]");
        }
        output
    })
}

fn join_reader(handle: JoinHandle<String>) -> Result<String, VerificationError> {
    handle
        .join()
        .map_err(|_| VerificationError::Unavailable("output reader panicked".into()))
}

fn hash_tree(path: &Path) -> Result<String, VerificationError> {
    let mut entries = Vec::new();
    collect_tree(path, path, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, bytes) in entries {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }
    Ok(format!(
        "sha256:{}",
        hex_digest(hasher.finalize().as_slice())
    ))
}

fn collect_tree(
    root: &Path,
    path: &Path,
    entries: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), VerificationError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| VerificationError::Unavailable(error.to_string()))?;
    if metadata.file_type().is_symlink() {
        entries.push((
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
            b"symlink".to_vec(),
        ));
        return Ok(());
    }
    if metadata.is_file() {
        let bytes =
            fs::read(path).map_err(|error| VerificationError::Unavailable(error.to_string()))?;
        entries.push((
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
            bytes,
        ));
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(path).map_err(|error| VerificationError::Unavailable(error.to_string()))?
        {
            collect_tree(
                root,
                &entry
                    .map_err(|error| VerificationError::Unavailable(error.to_string()))?
                    .path(),
                entries,
            )?;
        }
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_digest(hasher.finalize().as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

fn cpu_quota(max_processes: u32) -> String {
    if max_processes <= 1 {
        "1.0".into()
    } else {
        "2.0".into()
    }
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
    use tempfile::tempdir;

    fn manifest() -> VerificationManifest {
        VerificationManifest {
            id: "rust-default".into(),
            language: "rust".into(),
            gates: vec![VerificationGate {
                id: "check".into(),
                program: "cargo".into(),
                args: vec!["check".into(), "--all-targets".into()],
                class: GateClass::Compile,
                working_directory: None,
                required_capabilities: [Capability::WorkspaceRead, Capability::ProcessExec]
                    .into_iter()
                    .collect(),
                timeout_ms: Some(10_000),
                network: false,
            }],
            budget: VerificationBudget::default(),
        }
    }

    #[test]
    fn validates_trusted_local_manifest() {
        let dir = tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let gates = VerifierCatalog::safe_local()
            .validate_manifest(&manifest(), &workspace)
            .unwrap();
        assert_eq!(gates[0].gate.program, "cargo");
        assert_eq!(gates[0].working_directory, workspace.root());
    }

    #[test]
    fn blocks_unknown_program_network_and_path_escape() {
        let dir = tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let mut unknown = manifest();
        unknown.gates[0].program = "sh".into();
        assert!(matches!(
            VerifierCatalog::safe_local().validate_manifest(&unknown, &workspace),
            Err(VerificationError::PolicyBlocked(_))
        ));

        let mut network = manifest();
        network.gates[0].network = true;
        assert!(matches!(
            VerifierCatalog::safe_local().validate_manifest(&network, &workspace),
            Err(VerificationError::PolicyBlocked(_))
        ));

        let mut escape = manifest();
        escape.gates[0].args = vec!["--manifest-path".into(), "../Cargo.toml".into()];
        assert!(matches!(
            VerifierCatalog::safe_local().validate_manifest(&escape, &workspace),
            Err(VerificationError::PolicyBlocked(_))
        ));
    }

    #[test]
    fn fail_closed_verifier_never_executes_without_adapter() {
        let dir = tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let gate = VerifierCatalog::safe_local()
            .validate_manifest(&manifest(), &workspace)
            .unwrap()
            .remove(0);
        assert!(matches!(
            FailClosedVerifier.execute(&gate, &VerificationBudget::default()),
            Err(VerificationError::Unavailable(_))
        ));
    }

    #[test]
    fn rootless_command_uses_least_privilege_defaults() {
        let dir = tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let gate = VerifierCatalog::safe_local()
            .validate_manifest(&manifest(), &workspace)
            .unwrap()
            .remove(0);
        let verifier = RootlessContainerVerifier::new(
            RootlessContainerConfig::new("docker", "rust:1.85-slim").unwrap(),
        );
        let command = verifier.command_preview(&gate, &VerificationBudget::default());
        assert!(command.contains(&"--network=none".into()));
        assert!(command.contains(&"--cap-drop=ALL".into()));
        assert!(command.contains(&"--security-opt=no-new-privileges".into()));
        assert!(command.contains(&"--security-opt=seccomp=default".into()));
        assert!(command.contains(&"--read-only".into()));
        assert!(command.iter().any(|arg| arg.starts_with("--memory=")));
        assert!(command.iter().any(|arg| arg.starts_with("--pids-limit=")));
    }

    #[test]
    fn rootless_backend_rejects_unsupported_runtime_before_execution() {
        let dir = tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let gate = VerifierCatalog::safe_local()
            .validate_manifest(&manifest(), &workspace)
            .unwrap()
            .remove(0);
        let verifier = RootlessContainerVerifier::new(
            RootlessContainerConfig::new("untrusted-runtime", "rust:1.85-slim").unwrap(),
        );
        assert!(matches!(
            verifier.execute(&gate, &VerificationBudget::default()),
            Err(VerificationError::Unavailable(message)) if message.contains("unsupported container runtime")
        ));
    }

    #[test]
    fn classifies_diagnostics_for_repair_routing() {
        let result = VerificationResult {
            run_id: "run".into(),
            gate_id: "test".into(),
            status: VerificationStatus::Failed,
            program: "cargo".into(),
            args_hash: "hash".into(),
            workspace_tree_before: "a".into(),
            workspace_tree_after: "b".into(),
            toolchain_digest: None,
            started_at_ms: 0,
            duration_ms: 1,
            exit_code: Some(101),
            stdout: String::new(),
            stderr: "assertion failed in test fib".into(),
            diagnostics: vec![],
        };
        assert_eq!(classify_failure(&result), FailureClass::TestAssertion);
    }
}
