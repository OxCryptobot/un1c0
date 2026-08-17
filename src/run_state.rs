//! Durable run checkpoints for crash recovery and resumable execution.

use crate::agentic::{ActionResult, AgentError, Plan};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint I/O error: {0}")]
    Io(String),
    #[error("checkpoint serialization error: {0}")]
    Serialization(String),
    #[error("checkpoint does not match the requested plan")]
    PlanMismatch,
}

impl From<CheckpointError> for AgentError {
    fn from(error: CheckpointError) -> Self {
        AgentError::Journal(error.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub run_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub completed: Vec<ActionResult>,
    pub updated_at_ms: u128,
}

impl RunCheckpoint {
    pub fn new(plan: &Plan, run_id: impl Into<String>, completed: Vec<ActionResult>) -> Self {
        Self {
            run_id: run_id.into(),
            plan_id: plan.id.clone(),
            plan_hash: plan_hash(plan),
            completed,
            updated_at_ms: now_ms(),
        }
    }

    pub fn validate_for(&self, plan: &Plan) -> Result<(), CheckpointError> {
        if self.plan_id != plan.id || self.plan_hash != plan_hash(plan) {
            return Err(CheckpointError::PlanMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointStore {
    path: PathBuf,
}

impl CheckpointStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<RunCheckpoint>, CheckpointError> {
        match fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content)
                .map(Some)
                .map_err(|error| CheckpointError::Serialization(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(CheckpointError::Io(error.to_string())),
        }
    }

    pub fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), CheckpointError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| CheckpointError::Io(error.to_string()))?;
        }
        let content = serde_json::to_vec_pretty(checkpoint)
            .map_err(|error| CheckpointError::Serialization(error.to_string()))?;
        let temp = self.path.with_extension("checkpoint.tmp");
        fs::write(&temp, content).map_err(|error| CheckpointError::Io(error.to_string()))?;
        fs::rename(&temp, &self.path).map_err(|error| CheckpointError::Io(error.to_string()))?;
        Ok(())
    }

    pub fn clear(&self) -> Result<(), CheckpointError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CheckpointError::Io(error.to_string())),
        }
    }
}

pub fn plan_hash(plan: &Plan) -> String {
    let bytes = serde_json::to_vec(plan).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!(
        "sha256:{}",
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>()
    )
}

fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::{Action, Capability};
    use serde_json::json;
    use tempfile::tempdir;

    fn plan() -> Plan {
        Plan {
            id: "plan-checkpoint".into(),
            goal: "checkpoint".into(),
            actions: vec![Action {
                id: "echo".into(),
                tool: "echo".into(),
                input: json!({"message":"ok"}),
                depends_on: vec![],
                capabilities: Vec::<Capability>::new(),
                timeout_ms: None,
            }],
            max_steps: 4,
            max_output_bytes: 1024,
        }
    }

    #[test]
    fn saves_loads_and_validates_checkpoint_atomically() {
        let directory = tempdir().unwrap();
        let store = CheckpointStore::new(directory.path().join("run.json"));
        let checkpoint = RunCheckpoint::new(
            &plan(),
            "run-1",
            vec![ActionResult {
                action_id: "echo".into(),
                status: crate::agentic::ActionStatus::Succeeded,
                output: json!({"ok":true}),
                error: None,
            }],
        );
        store.save(&checkpoint).unwrap();
        let loaded = store.load().unwrap().unwrap();
        loaded.validate_for(&plan()).unwrap();
        assert_eq!(loaded.run_id, "run-1");
    }

    #[test]
    fn rejects_plan_mismatch() {
        let checkpoint = RunCheckpoint::new(&plan(), "run-1", vec![]);
        let mut changed = plan();
        changed.goal = "different".into();
        assert!(matches!(
            checkpoint.validate_for(&changed),
            Err(CheckpointError::PlanMismatch)
        ));
    }
}
