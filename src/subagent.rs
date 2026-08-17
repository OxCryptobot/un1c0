//! Bounded subagent orchestration and deterministic merge gates.
//!
//! Subagents are parallel planners/executors, not independent authorities. Each
//! task must use a distinct workspace, declare its budget, and produce evidence
//! before a merge gate can accept its result.

use crate::verification::{VerificationResult, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub id: String,
    pub goal: String,
    pub workspace: PathBuf,
    pub max_steps: usize,
    pub max_output_bytes: usize,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubagentStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub task_id: String,
    pub status: SubagentStatus,
    pub changed_files: BTreeSet<String>,
    pub verification: Option<VerificationStatus>,
    pub verification_evidence: Vec<VerificationResult>,
    pub summary: String,
}

pub trait SubagentWorker: Send + Sync + 'static {
    fn run(&self, task: SubagentTask) -> SubagentResult;
}

#[derive(Debug, Error)]
pub enum SubagentError {
    #[error("invalid subagent schedule: {0}")]
    InvalidSchedule(String),
    #[error("subagent worker failed to return a result")]
    WorkerDisconnected,
}

#[derive(Debug, Clone)]
pub struct SubagentCoordinator {
    max_parallel: usize,
}

impl SubagentCoordinator {
    pub fn new(max_parallel: usize) -> Result<Self, SubagentError> {
        if max_parallel == 0 || max_parallel > 64 {
            return Err(SubagentError::InvalidSchedule(
                "max_parallel must be between 1 and 64".into(),
            ));
        }
        Ok(Self { max_parallel })
    }

    pub fn max_parallel(&self) -> usize {
        self.max_parallel
    }

    pub fn run<W: SubagentWorker>(
        &self,
        tasks: Vec<SubagentTask>,
        worker: Arc<W>,
    ) -> Result<Vec<SubagentResult>, SubagentError> {
        validate_schedule(&tasks)?;
        let mut results = Vec::with_capacity(tasks.len());
        for chunk in tasks.chunks(self.max_parallel) {
            let (sender, receiver) = mpsc::channel();
            thread::scope(|scope| {
                for (index, task) in chunk.iter().cloned().enumerate() {
                    let sender = sender.clone();
                    let worker = Arc::clone(&worker);
                    scope.spawn(move || {
                        let result = worker.run(task);
                        let _ = sender.send((index, result));
                    });
                }
                drop(sender);
                let mut chunk_results = HashMap::new();
                for received in receiver {
                    chunk_results.insert(received.0, received.1);
                }
                for index in 0..chunk.len() {
                    if let Some(result) = chunk_results.remove(&index) {
                        results.push(result);
                    }
                }
            });
        }
        if results.len() != tasks.len() {
            return Err(SubagentError::WorkerDisconnected);
        }
        Ok(results)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeDecision {
    pub accepted: bool,
    pub task_ids: Vec<String>,
    pub reason: String,
}

pub struct MergeGate;

impl MergeGate {
    pub fn evaluate(results: &[SubagentResult]) -> MergeDecision {
        if results.is_empty() {
            return MergeDecision {
                accepted: false,
                task_ids: vec![],
                reason: "no subagent results".into(),
            };
        }
        let mut changed_files = BTreeSet::new();
        let mut task_ids = Vec::with_capacity(results.len());
        for result in results {
            task_ids.push(result.task_id.clone());
            if result.status != SubagentStatus::Succeeded {
                return MergeDecision {
                    accepted: false,
                    task_ids,
                    reason: format!("task '{}' did not succeed", result.task_id),
                };
            }
            if result.verification != Some(VerificationStatus::Passed) {
                return MergeDecision {
                    accepted: false,
                    task_ids,
                    reason: format!("task '{}' lacks passing verification", result.task_id),
                };
            }
            for file in &result.changed_files {
                if !changed_files.insert(file.clone()) {
                    return MergeDecision {
                        accepted: false,
                        task_ids,
                        reason: format!("changed-file conflict on '{}'", file),
                    };
                }
            }
        }
        MergeDecision {
            accepted: true,
            task_ids,
            reason: "all subagents passed verification with disjoint changes".into(),
        }
    }
}

fn validate_schedule(tasks: &[SubagentTask]) -> Result<(), SubagentError> {
    if tasks.is_empty() {
        return Err(SubagentError::InvalidSchedule(
            "at least one task is required".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut workspaces = BTreeSet::new();
    for task in tasks {
        if task.id.trim().is_empty() || !ids.insert(task.id.clone()) {
            return Err(SubagentError::InvalidSchedule(
                "task IDs must be unique and non-empty".into(),
            ));
        }
        if task.goal.trim().is_empty() || task.max_steps == 0 || task.max_output_bytes == 0 {
            return Err(SubagentError::InvalidSchedule(format!(
                "task '{}' has invalid goal or budget",
                task.id
            )));
        }
        if task.workspace.is_absolute()
            && task
                .workspace
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(SubagentError::InvalidSchedule(format!(
                "task '{}' has an invalid workspace",
                task.id
            )));
        }
        if !workspaces.insert(task.workspace.clone()) {
            return Err(SubagentError::InvalidSchedule(format!(
                "task '{}' shares a workspace",
                task.id
            )));
        }
        if task
            .dependencies
            .iter()
            .any(|dependency| dependency == &task.id || !ids.contains(dependency))
        {
            // Dependencies must point to an earlier task in the declared order.
            return Err(SubagentError::InvalidSchedule(format!(
                "task '{}' has an invalid dependency",
                task.id
            )));
        }
        if task
            .workspace
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(SubagentError::InvalidSchedule(format!(
                "task '{}' workspace escapes its configured root",
                task.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockWorker {
        calls: AtomicUsize,
        status: SubagentStatus,
    }

    impl SubagentWorker for MockWorker {
        fn run(&self, task: SubagentTask) -> SubagentResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            SubagentResult {
                task_id: task.id,
                status: self.status.clone(),
                changed_files: [format!("{}.rs", task.goal)].into_iter().collect(),
                verification: Some(VerificationStatus::Passed),
                verification_evidence: vec![],
                summary: "mocked".into(),
            }
        }
    }

    fn task(id: &str, workspace: &str) -> SubagentTask {
        SubagentTask {
            id: id.into(),
            goal: id.into(),
            workspace: Path::new(workspace).to_path_buf(),
            max_steps: 4,
            max_output_bytes: 1024,
            dependencies: vec![],
        }
    }

    #[test]
    fn runs_bounded_tasks_in_input_order_and_merge_accepts_disjoint_verified_results() {
        let worker = Arc::new(MockWorker {
            calls: AtomicUsize::new(0),
            status: SubagentStatus::Succeeded,
        });
        let coordinator = SubagentCoordinator::new(2).unwrap();
        let results = coordinator
            .run(
                vec![task("a", "work/a"), task("b", "work/b")],
                Arc::clone(&worker),
            )
            .unwrap();
        assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
        assert_eq!(results[0].task_id, "a");
        assert!(MergeGate::evaluate(&results).accepted);
    }

    #[test]
    fn rejects_workspace_sharing_and_merge_conflicts() {
        let worker = Arc::new(MockWorker {
            calls: AtomicUsize::new(0),
            status: SubagentStatus::Succeeded,
        });
        let coordinator = SubagentCoordinator::new(2).unwrap();
        assert!(coordinator
            .run(
                vec![task("a", "work/shared"), task("b", "work/shared")],
                Arc::clone(&worker)
            )
            .is_err());
        let mut first = worker.run(task("a", "work/a"));
        let mut second = worker.run(task("b", "work/b"));
        second.changed_files = first.changed_files.clone();
        first.verification = Some(VerificationStatus::Passed);
        assert!(!MergeGate::evaluate(&[first, second]).accepted);
    }

    #[test]
    fn rejects_failed_or_unverified_subagents() {
        let failed = SubagentResult {
            task_id: "failed".into(),
            status: SubagentStatus::Failed,
            changed_files: BTreeSet::new(),
            verification: Some(VerificationStatus::Failed),
            verification_evidence: vec![],
            summary: "failed".into(),
        };
        let decision = MergeGate::evaluate(&[failed]);
        assert!(!decision.accepted);
    }
}
