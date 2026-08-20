use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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

fn max_resource_snapshot(left: &ResourceSnapshot, right: &ResourceSnapshot) -> ResourceSnapshot {
    ResourceSnapshot {
        rss_kib: max_optional(left.rss_kib, right.rss_kib),
        threads: max_optional(left.threads, right.threads),
        open_fds: max_optional(left.open_fds, right.open_fds),
    }
}

fn max_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
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

const MAX_CONCURRENT_WORKERS: usize = 32;
const MAX_OPERATIONS_PER_WORKER: usize = 512;
const MAX_CONCURRENT_OPERATIONS: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConcurrentPersistenceMeasurement {
    pub workers: usize,
    pub operations_per_worker: usize,
    pub operation_count: usize,
    pub completed_operations: usize,
    pub failed_operations: usize,
    pub unique_target_count: usize,
    pub stale_staging_seeded: bool,
    pub staging_recovery_scans: usize,
    pub file_sync_p95_us: u64,
    pub directory_sync_p95_us: u64,
    pub total_p95_us: u64,
    pub total_max_us: u64,
    pub wall_time_us: u64,
    pub throughput_milli_ops_per_sec: u64,
    pub resource_before: ResourceSnapshot,
    pub resource_during_workers: ResourceSnapshot,
    pub resource_after: ResourceSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SanitizedConcurrentPersistenceMeasurement {
    pub workers: usize,
    pub operations_per_worker: usize,
    pub operation_count: usize,
    pub completed_operations: usize,
    pub failed_operations: usize,
    pub unique_target_count: usize,
    pub stale_staging_seeded: bool,
    pub staging_recovery_scans: usize,
    pub file_sync_p95_us: u64,
    pub directory_sync_p95_us: u64,
    pub total_p95_us: u64,
    pub total_max_us: u64,
    pub wall_time_us: u64,
    pub throughput_milli_ops_per_sec: u64,
    pub resource_before: ResourceSnapshot,
    pub resource_during_workers: ResourceSnapshot,
    pub resource_after: ResourceSnapshot,
    pub secret_material_recorded: bool,
    pub cluster_mutation_performed: bool,
}

impl ConcurrentPersistenceMeasurement {
    pub fn sanitized(&self) -> SanitizedConcurrentPersistenceMeasurement {
        SanitizedConcurrentPersistenceMeasurement {
            workers: self.workers,
            operations_per_worker: self.operations_per_worker,
            operation_count: self.operation_count,
            completed_operations: self.completed_operations,
            failed_operations: self.failed_operations,
            unique_target_count: self.unique_target_count,
            stale_staging_seeded: self.stale_staging_seeded,
            staging_recovery_scans: self.staging_recovery_scans,
            file_sync_p95_us: self.file_sync_p95_us,
            directory_sync_p95_us: self.directory_sync_p95_us,
            total_p95_us: self.total_p95_us,
            total_max_us: self.total_max_us,
            wall_time_us: self.wall_time_us,
            throughput_milli_ops_per_sec: self.throughput_milli_ops_per_sec,
            resource_before: self.resource_before.clone(),
            resource_during_workers: self.resource_during_workers.clone(),
            resource_after: self.resource_after.clone(),
            secret_material_recorded: false,
            cluster_mutation_performed: false,
        }
    }
}

pub fn measure_concurrent_snapshot_persistence(
    root: impl AsRef<Path>,
    payload: &[u8],
    workers: usize,
    operations_per_worker: usize,
    seed_stale_staging: bool,
) -> Result<ConcurrentPersistenceMeasurement, ResourceDurabilityError> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(ResourceDurabilityError::InvalidInput(
            "snapshot payload is outside the bounded range".into(),
        ));
    }
    if workers == 0 || workers > MAX_CONCURRENT_WORKERS {
        return Err(ResourceDurabilityError::InvalidInput(
            "worker count is outside the bounded range".into(),
        ));
    }
    if operations_per_worker == 0 || operations_per_worker > MAX_OPERATIONS_PER_WORKER {
        return Err(ResourceDurabilityError::InvalidInput(
            "operations per worker is outside the bounded range".into(),
        ));
    }
    let operation_count = workers
        .checked_mul(operations_per_worker)
        .ok_or_else(|| ResourceDurabilityError::InvalidInput("operation count overflow".into()))?;
    if operation_count > MAX_CONCURRENT_OPERATIONS {
        return Err(ResourceDurabilityError::InvalidInput(
            "concurrent operation count is outside the bounded range".into(),
        ));
    }

    let root = root.as_ref();
    fs::create_dir_all(root)
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ResourceDurabilityError::InvalidInput(error.to_string()))?
        .as_nanos();
    let run_dir = root.join(format!(".phase40-run-{}-{}", std::process::id(), run_id));
    fs::create_dir(&run_dir)
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;

    if seed_stale_staging {
        for worker_id in 0..workers {
            let stale = run_dir.join(format!("worker-{worker_id:02}-op-0000.snapshot.staging"));
            fs::write(stale, b"stale-staging")
                .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        }
    }
    let resource_before = capture_resource_snapshot();
    let started = Instant::now();
    let results = std::thread::scope(|scope| {
        let handles = (0..workers)
            .map(|worker_id| {
                let run_dir = &run_dir;
                let worker_payload = payload.to_vec();
                scope.spawn(move || {
                    measure_concurrent_worker(
                        run_dir,
                        worker_id,
                        &worker_payload,
                        operations_per_worker,
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(result) => result,
                Err(_) => Err(ResourceDurabilityError::PersistenceFailed(
                    "concurrent persistence worker panicked".into(),
                )),
            })
            .collect::<Vec<_>>()
    });
    let resource_after = capture_resource_snapshot();
    let wall_time_us = started.elapsed().as_micros() as u64;
    let cleanup_result = fs::remove_dir_all(&run_dir);

    if let Some(error) = results
        .iter()
        .find_map(|result| result.as_ref().err().cloned())
    {
        let _ = cleanup_result;
        return Err(error);
    }
    cleanup_result
        .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;

    let mut file_sync_samples = Vec::with_capacity(operation_count);
    let mut directory_sync_samples = Vec::with_capacity(operation_count);
    let mut total_samples = Vec::with_capacity(operation_count);
    let mut completed_operations = 0usize;
    let mut resource_during_workers = ResourceSnapshot::default();
    let mut staging_recovery_scans = 0usize;
    for result in results {
        let worker = result?;
        completed_operations = completed_operations.saturating_add(worker.completed_operations);
        resource_during_workers =
            max_resource_snapshot(&resource_during_workers, &worker.resource_snapshot);
        staging_recovery_scans =
            staging_recovery_scans.saturating_add(worker.staging_recovery_scans);
        file_sync_samples.extend(worker.file_sync_samples);
        directory_sync_samples.extend(worker.directory_sync_samples);
        total_samples.extend(worker.total_samples);
    }
    if completed_operations != operation_count
        || file_sync_samples.len() != operation_count
        || directory_sync_samples.len() != operation_count
        || total_samples.len() != operation_count
    {
        return Err(ResourceDurabilityError::PersistenceFailed(
            "concurrent persistence completion accounting is inconsistent".into(),
        ));
    }
    let throughput_milli_ops_per_sec = if wall_time_us == 0 {
        0
    } else {
        ((completed_operations as u128)
            .saturating_mul(1_000_000_000)
            .checked_div(wall_time_us as u128)
            .unwrap_or_default()
            .min(u64::MAX as u128)) as u64
    };
    Ok(ConcurrentPersistenceMeasurement {
        workers,
        operations_per_worker,
        operation_count,
        completed_operations,
        failed_operations: 0,
        unique_target_count: completed_operations,
        stale_staging_seeded: seed_stale_staging,
        staging_recovery_scans,
        file_sync_p95_us: percentile95(&file_sync_samples),
        directory_sync_p95_us: percentile95(&directory_sync_samples),
        total_p95_us: percentile95(&total_samples),
        total_max_us: total_samples.iter().copied().max().unwrap_or_default(),
        wall_time_us,
        throughput_milli_ops_per_sec,
        resource_before,
        resource_during_workers,
        resource_after,
    })
}

struct ConcurrentWorkerMeasurement {
    completed_operations: usize,
    resource_snapshot: ResourceSnapshot,
    staging_recovery_scans: usize,
    file_sync_samples: Vec<u64>,
    directory_sync_samples: Vec<u64>,
    total_samples: Vec<u64>,
}

fn measure_concurrent_worker(
    run_dir: &Path,
    worker_id: usize,
    payload: &[u8],
    operations_per_worker: usize,
) -> Result<ConcurrentWorkerMeasurement, ResourceDurabilityError> {
    let resource_snapshot = capture_resource_snapshot();
    let mut file_sync_samples = Vec::with_capacity(operations_per_worker);
    let mut directory_sync_samples = Vec::with_capacity(operations_per_worker);
    let mut total_samples = Vec::with_capacity(operations_per_worker);
    let mut staging_recovery_scans = 0usize;
    for operation_id in 0..operations_per_worker {
        let stem = format!("worker-{worker_id:02}-op-{operation_id:04}");
        let target = run_dir.join(format!("{stem}.snapshot"));
        let staging = run_dir.join(format!("{stem}.snapshot.staging"));
        if staging.exists() {
            staging_recovery_scans = staging_recovery_scans.saturating_add(1);
            fs::remove_file(&staging)
                .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        let started = Instant::now();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        file.write_all(payload)
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let file_sync_started = Instant::now();
        file.sync_all()
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let file_sync_elapsed = file_sync_started.elapsed().as_micros() as u64;
        drop(file);
        fs::rename(&staging, &target)
            .map_err(|error| ResourceDurabilityError::PersistenceFailed(error.to_string()))?;
        let directory_sync_started = Instant::now();
        sync_directory(run_dir)?;
        let directory_sync_elapsed = directory_sync_started.elapsed().as_micros() as u64;
        file_sync_samples.push(file_sync_elapsed);
        directory_sync_samples.push(directory_sync_elapsed);
        total_samples.push(started.elapsed().as_micros() as u64);
    }
    Ok(ConcurrentWorkerMeasurement {
        completed_operations: operations_per_worker,
        resource_snapshot,
        staging_recovery_scans,
        file_sync_samples,
        directory_sync_samples,
        total_samples,
    })
}
