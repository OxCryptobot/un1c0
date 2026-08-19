use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

const MAX_SAMPLES: usize = 512;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResourceDurabilityError {
    #[error("resource durability input is invalid: {0}")]
    InvalidInput(String),
    #[error("resource durability persistence failed: {0}")]
    PersistenceFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ResourceSnapshot {
    pub rss_kib: Option<u64>,
    pub threads: Option<u64>,
    pub open_fds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceBudget {
    pub max_rss_kib: Option<u64>,
    pub max_threads: Option<u64>,
    pub max_open_fds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceBudgetDecision {
    pub within_budget: bool,
    pub violations: Vec<String>,
}

impl ResourceBudget {
    pub fn evaluate(&self, snapshot: &ResourceSnapshot) -> ResourceBudgetDecision {
        let mut violations = Vec::new();
        if let (Some(limit), Some(value)) = (self.max_rss_kib, snapshot.rss_kib) {
            if value > limit {
                violations.push("rss_kib".to_string());
            }
        }
        if let (Some(limit), Some(value)) = (self.max_threads, snapshot.threads) {
            if value > limit {
                violations.push("threads".to_string());
            }
        }
        if let (Some(limit), Some(value)) = (self.max_open_fds, snapshot.open_fds) {
            if value > limit {
                violations.push("open_fds".to_string());
            }
        }
        ResourceBudgetDecision {
            within_budget: violations.is_empty(),
            violations,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistenceMeasurement {
    pub operation_count: usize,
    pub bytes_written: u64,
    pub staging_recovery_scans: usize,
    pub staging_retries: usize,
    pub file_sync_p95_us: u64,
    pub directory_sync_p95_us: u64,
    pub total_p95_us: u64,
    pub total_max_us: u64,
    pub resource_before: ResourceSnapshot,
    pub resource_after: ResourceSnapshot,
}

impl PersistenceMeasurement {
    pub fn sanitized(&self) -> SanitizedPersistenceMeasurement {
        SanitizedPersistenceMeasurement {
            operation_count: self.operation_count,
            bytes_written: self.bytes_written,
            staging_recovery_scans: self.staging_recovery_scans,
            staging_retries: self.staging_retries,
            file_sync_p95_us: self.file_sync_p95_us,
            directory_sync_p95_us: self.directory_sync_p95_us,
            total_p95_us: self.total_p95_us,
            total_max_us: self.total_max_us,
            resource_before: self.resource_before.clone(),
            resource_after: self.resource_after.clone(),
            secret_material_recorded: false,
            cluster_mutation_performed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SanitizedPersistenceMeasurement {
    pub operation_count: usize,
    pub bytes_written: u64,
    pub staging_recovery_scans: usize,
    pub staging_retries: usize,
    pub file_sync_p95_us: u64,
    pub directory_sync_p95_us: u64,
    pub total_p95_us: u64,
    pub total_max_us: u64,
    pub resource_before: ResourceSnapshot,
    pub resource_after: ResourceSnapshot,
    pub secret_material_recorded: bool,
    pub cluster_mutation_performed: bool,
}

pub fn capture_resource_snapshot() -> ResourceSnapshot {
    ResourceSnapshot {
        rss_kib: proc_status_value("VmRSS"),
        threads: proc_status_value("Threads"),
        open_fds: fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| entries.filter_map(Result::ok).count() as u64),
    }
}

pub fn measure_atomic_snapshot_persistence(
    root: impl AsRef<Path>,
    payload: &[u8],
    operation_count: usize,
) -> Result<PersistenceMeasurement, ResourceDurabilityError> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(ResourceDurabilityError::InvalidInput(
            "snapshot payload is outside the bounded range".into(),
        ));
    }
    if operation_count == 0 || operation_count > MAX_SAMPLES {
        return Err(ResourceDurabilityError::InvalidInput(
            "operation count is outside the bounded range".into(),
        ));
    }
    let root = root.as_ref();
    fs::create_dir_all(root)
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
    let target = root.join("supervision.snapshot");
    let staging = root.join("supervision.snapshot.staging");
    let resource_before = capture_resource_snapshot();
    let mut file_sync_samples = Vec::with_capacity(operation_count);
    let mut directory_sync_samples = Vec::with_capacity(operation_count);
    let mut total_samples = Vec::with_capacity(operation_count);
    let mut staging_recovery_scans = 0usize;

    for _ in 0..operation_count {
        if staging.exists() {
            staging_recovery_scans = staging_recovery_scans.saturating_add(1);
            fs::remove_file(&staging)
                .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        let started = Instant::now();
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => file,
            Err(error) => {
                return Err(ResourceDurabilityError::PersistenceFailed(
                    error.to_string(),
                ));
            }
        };
        let write_started = Instant::now();
        file.write_all(payload)
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let write_elapsed = write_started.elapsed();
        file.sync_all()
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let file_sync_elapsed = write_started.elapsed().saturating_sub(write_elapsed);
        drop(file);
        fs::rename(&staging, &target)
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let directory_started = Instant::now();
        sync_directory(root)?;
        let directory_sync_elapsed = directory_started.elapsed();
        file_sync_samples.push(file_sync_elapsed.as_micros() as u64);
        directory_sync_samples.push(directory_sync_elapsed.as_micros() as u64);
        total_samples.push(started.elapsed().as_micros() as u64);
    }
    let resource_after = capture_resource_snapshot();
    Ok(PersistenceMeasurement {
        operation_count,
        bytes_written: payload.len() as u64 * operation_count as u64,
        staging_recovery_scans,
        staging_retries: 0,
        file_sync_p95_us: percentile95(&file_sync_samples),
        directory_sync_p95_us: percentile95(&directory_sync_samples),
        total_p95_us: percentile95(&total_samples),
        total_max_us: total_samples.iter().copied().max().unwrap_or_default(),
        resource_before,
        resource_after,
    })
}

fn sync_directory(root: &Path) -> Result<(), ResourceDurabilityError> {
    let directory = OpenOptions::new()
        .read(true)
        .open(root)
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
    directory
        .sync_all()
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))
}

fn percentile95(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() * 95).saturating_add(99) / 100).saturating_sub(1);
    sorted[index.min(sorted.len() - 1)]
}

fn proc_status_value(key: &str) -> Option<u64> {
    let contents = fs::read_to_string("/proc/self/status").ok()?;
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key {
            return None;
        }
        value.split_whitespace().next()?.parse().ok()
    })
}

#[allow(dead_code)]
fn _bounded_identifier(value: &str) -> Result<(), ResourceDurabilityError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(ResourceDurabilityError::InvalidInput(
            "resource identifier is outside the bounded range".into(),
        ));
    }
    Ok(())
}

#[allow(dead_code)]
fn _path_for_report(root: &Path) -> PathBuf {
    root.join("supervision.snapshot")
}
