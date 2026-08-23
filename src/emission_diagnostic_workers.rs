use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use thiserror::Error;

use crate::emission_diagnostic_attestation::{
    DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
    EmissionDiagnosticAttestationError, VerifiedDiagnosticEvidence,
};
use crate::emission_diagnostic_cache::DiagnosticEvidenceCache;
use crate::emission_diagnostic_instrumentation::DiagnosticInstrumentation;
use crate::emission_diagnostic_stream::EmissionDiagnosticStream;
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_DIAGNOSTIC_VERIFICATION_WORKERS: usize = 32;
pub const MAX_DIAGNOSTIC_VERIFICATION_QUEUE: usize = 256;
pub const MAX_DIAGNOSTIC_NODE_IN_FLIGHT: usize = 64;
pub const MAX_DIAGNOSTIC_WORKER_LATENCY_SAMPLES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiagnosticVerificationWorkerError {
    #[error("diagnostic verification worker count must be greater than zero")]
    ZeroWorkers,
    #[error("diagnostic verification worker count {count} exceeds maximum {maximum}")]
    TooManyWorkers { count: usize, maximum: usize },
    #[error("diagnostic verification queue capacity must be greater than zero")]
    ZeroQueueCapacity,
    #[error("diagnostic verification queue capacity {capacity} exceeds maximum {maximum}")]
    QueueCapacityTooLarge { capacity: usize, maximum: usize },
    #[error("diagnostic verification per-node limit must be greater than zero")]
    ZeroPerNodeLimit,
    #[error("diagnostic verification per-node limit {limit} exceeds maximum {maximum}")]
    PerNodeLimitTooLarge { limit: usize, maximum: usize },
    #[error("diagnostic verification job node ID must be non-zero")]
    InvalidNodeId,
    #[error("diagnostic verification worker queue is full")]
    QueueFull,
    #[error("diagnostic verification per-node fairness limit reached for node {node_id}")]
    FairnessLimit { node_id: u64, limit: usize },
    #[error("diagnostic verification worker pool is shut down")]
    Shutdown,
    #[error("diagnostic verification result channel closed")]
    ResultChannelClosed,
    #[error("diagnostic verification failed: {0}")]
    Verification(EmissionDiagnosticAttestationError),
    #[error("diagnostic verification worker cancelled")]
    Cancelled,
    #[error("diagnostic verification worker panicked during shutdown")]
    WorkerPanicked,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiagnosticVerificationWorkerMetrics {
    pub worker_count: usize,
    pub queue_capacity: usize,
    pub per_node_limit: usize,
    pub submitted_jobs: u64,
    pub completed_jobs: u64,
    pub failed_jobs: u64,
    pub cancelled_jobs: u64,
    pub queue_full_rejections: u64,
    pub fairness_rejections: u64,
    pub shutdown_rejections: u64,
    pub ordered_dispatches: u64,
    pub out_of_order_buffered: u64,
    pub latency_sample_count: usize,
    pub latency_sample_cap: usize,
    pub queue_wait_p50_us: u64,
    pub queue_wait_p95_us: u64,
    pub queue_wait_max_us: u64,
    pub verification_service_p50_us: u64,
    pub verification_service_p95_us: u64,
    pub verification_service_max_us: u64,
}

#[derive(Debug, Default)]
struct WorkerMetricsState {
    submitted_jobs: u64,
    completed_jobs: u64,
    failed_jobs: u64,
    cancelled_jobs: u64,
    queue_full_rejections: u64,
    fairness_rejections: u64,
    shutdown_rejections: u64,
    ordered_dispatches: u64,
    out_of_order_buffered: u64,
    queue_wait_us: Vec<u64>,
    verification_service_us: Vec<u64>,
}

impl WorkerMetricsState {
    fn record_submission(&mut self) {
        self.submitted_jobs = self.submitted_jobs.saturating_add(1);
    }

    fn record_queue_full(&mut self) {
        self.queue_full_rejections = self.queue_full_rejections.saturating_add(1);
    }

    fn record_fairness_rejection(&mut self) {
        self.fairness_rejections = self.fairness_rejections.saturating_add(1);
    }

    fn record_shutdown(&mut self) {
        self.shutdown_rejections = self.shutdown_rejections.saturating_add(1);
    }

    fn record_completion(
        &mut self,
        queue_wait_us: u64,
        service_us: u64,
        success: bool,
        cancelled: bool,
    ) {
        self.completed_jobs = self.completed_jobs.saturating_add(1);
        if cancelled {
            self.cancelled_jobs = self.cancelled_jobs.saturating_add(1);
        } else if !success {
            self.failed_jobs = self.failed_jobs.saturating_add(1);
        }
        if self.queue_wait_us.len() < MAX_DIAGNOSTIC_WORKER_LATENCY_SAMPLES {
            self.queue_wait_us.push(queue_wait_us);
            self.verification_service_us.push(service_us);
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticVerificationCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl DiagnosticVerificationCancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Default for DiagnosticVerificationCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticVerificationTicket {
    job_id: u64,
    cancellation: DiagnosticVerificationCancellationToken,
}

impl DiagnosticVerificationTicket {
    pub fn job_id(&self) -> u64 {
        self.job_id
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticVerificationJob {
    pub node_id: u64,
    pub connection_id: u64,
    pub sequence: u64,
    pub attestation: EmissionDiagnosticAttestation,
    pub stream: EmissionDiagnosticStream,
    pub envelope: SemanticSnapshotEnvelope,
    pub profile: TargetCapabilityProfile,
    pub units: BTreeMap<SemanticUnitId, Ueg>,
    pub verifier: Arc<DiagnosticAttestationVerifier>,
    pub cache: DiagnosticEvidenceCache,
    pub instrumentation: DiagnosticInstrumentation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticWorkerError {
    Verification(EmissionDiagnosticAttestationError),
    Cancelled,
}

impl std::fmt::Display for EmissionDiagnosticWorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verification(error) => write!(formatter, "{error}"),
            Self::Cancelled => formatter.write_str("diagnostic verification worker cancelled"),
        }
    }
}

impl std::error::Error for EmissionDiagnosticWorkerError {}

#[derive(Debug, Clone)]
pub struct DiagnosticVerifiedResult {
    pub job_id: u64,
    pub node_id: u64,
    pub connection_id: u64,
    pub sequence: u64,
    pub evidence: Result<VerifiedDiagnosticEvidence, EmissionDiagnosticWorkerError>,
    cancellation: DiagnosticVerificationCancellationToken,
}

impl DiagnosticVerifiedResult {
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
            || matches!(self.evidence, Err(EmissionDiagnosticWorkerError::Cancelled))
    }
}

#[derive(Debug)]
struct WorkerJob {
    job_id: u64,
    submitted_at: Instant,
    input: DiagnosticVerificationJob,
    cancellation: DiagnosticVerificationCancellationToken,
}

#[derive(Debug)]
struct WorkerResult {
    job_id: u64,
    node_id: u64,
    connection_id: u64,
    sequence: u64,
    cancellation: DiagnosticVerificationCancellationToken,
    evidence: Result<VerifiedDiagnosticEvidence, EmissionDiagnosticWorkerError>,
}

#[derive(Debug, Default)]
struct AdmissionState {
    in_flight: usize,
    per_node: BTreeMap<u64, usize>,
}

#[derive(Debug)]
pub struct DiagnosticVerificationWorkerPool {
    worker_count: usize,
    queue_capacity: usize,
    per_node_limit: usize,
    sender: Option<SyncSender<WorkerJob>>,
    result_receiver: Receiver<WorkerResult>,
    next_job_id: Mutex<u64>,
    next_result_id: u64,
    pending_results: BTreeMap<u64, WorkerResult>,
    admission: Arc<Mutex<AdmissionState>>,
    metrics: Arc<Mutex<WorkerMetricsState>>,
    workers: Vec<JoinHandle<()>>,
}

impl DiagnosticVerificationWorkerPool {
    pub fn new(
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, DiagnosticVerificationWorkerError> {
        let per_node_limit = queue_capacity.div_ceil(2).max(1);
        Self::new_with_limits(worker_count, queue_capacity, per_node_limit)
    }

    pub fn new_with_limits(
        worker_count: usize,
        queue_capacity: usize,
        per_node_limit: usize,
    ) -> Result<Self, DiagnosticVerificationWorkerError> {
        Self::new_internal(worker_count, queue_capacity, per_node_limit, None)
    }

    pub fn new_with_start_gate(
        worker_count: usize,
        queue_capacity: usize,
        start_gate: Arc<Barrier>,
    ) -> Result<Self, DiagnosticVerificationWorkerError> {
        let per_node_limit = queue_capacity.div_ceil(2).max(1);
        Self::new_with_start_gate_and_limits(
            worker_count,
            queue_capacity,
            per_node_limit,
            start_gate,
        )
    }

    pub fn new_with_start_gate_and_limits(
        worker_count: usize,
        queue_capacity: usize,
        per_node_limit: usize,
        start_gate: Arc<Barrier>,
    ) -> Result<Self, DiagnosticVerificationWorkerError> {
        Self::new_internal(
            worker_count,
            queue_capacity,
            per_node_limit,
            Some(start_gate),
        )
    }

    fn new_internal(
        worker_count: usize,
        queue_capacity: usize,
        per_node_limit: usize,
        start_gate: Option<Arc<Barrier>>,
    ) -> Result<Self, DiagnosticVerificationWorkerError> {
        if worker_count == 0 {
            return Err(DiagnosticVerificationWorkerError::ZeroWorkers);
        }
        if worker_count > MAX_DIAGNOSTIC_VERIFICATION_WORKERS {
            return Err(DiagnosticVerificationWorkerError::TooManyWorkers {
                count: worker_count,
                maximum: MAX_DIAGNOSTIC_VERIFICATION_WORKERS,
            });
        }
        if queue_capacity == 0 {
            return Err(DiagnosticVerificationWorkerError::ZeroQueueCapacity);
        }
        if queue_capacity > MAX_DIAGNOSTIC_VERIFICATION_QUEUE {
            return Err(DiagnosticVerificationWorkerError::QueueCapacityTooLarge {
                capacity: queue_capacity,
                maximum: MAX_DIAGNOSTIC_VERIFICATION_QUEUE,
            });
        }
        if per_node_limit == 0 {
            return Err(DiagnosticVerificationWorkerError::ZeroPerNodeLimit);
        }
        if per_node_limit > MAX_DIAGNOSTIC_NODE_IN_FLIGHT {
            return Err(DiagnosticVerificationWorkerError::PerNodeLimitTooLarge {
                limit: per_node_limit,
                maximum: MAX_DIAGNOSTIC_NODE_IN_FLIGHT,
            });
        }
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let (result_sender, result_receiver) = mpsc::sync_channel(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let metrics = Arc::new(Mutex::new(WorkerMetricsState::default()));
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let result_sender = result_sender.clone();
            let metrics = Arc::clone(&metrics);
            let worker_gate = start_gate.clone();
            let worker = thread::Builder::new()
                .name(format!("un1c0-diagnostic-verifier-{index}"))
                .spawn(move || run_worker(receiver, result_sender, metrics, worker_gate))
                .map_err(|_| DiagnosticVerificationWorkerError::Shutdown)?;
            workers.push(worker);
        }
        drop(result_sender);
        Ok(Self {
            worker_count,
            queue_capacity,
            per_node_limit,
            sender: Some(sender),
            result_receiver,
            next_job_id: Mutex::new(1),
            next_result_id: 1,
            pending_results: BTreeMap::new(),
            admission: Arc::new(Mutex::new(AdmissionState::default())),
            metrics,
            workers,
        })
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    pub fn per_node_limit(&self) -> usize {
        self.per_node_limit
    }

    pub fn submit(
        &self,
        input: DiagnosticVerificationJob,
    ) -> Result<u64, DiagnosticVerificationWorkerError> {
        Ok(self.submit_with_cancellation(input)?.job_id())
    }

    pub fn submit_with_cancellation(
        &self,
        input: DiagnosticVerificationJob,
    ) -> Result<DiagnosticVerificationTicket, DiagnosticVerificationWorkerError> {
        if input.node_id == 0 {
            return Err(DiagnosticVerificationWorkerError::InvalidNodeId);
        }
        let node_id = input.node_id;
        let cancellation = DiagnosticVerificationCancellationToken::new();
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if admission.in_flight >= self.queue_capacity {
            self.metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record_queue_full();
            return Err(DiagnosticVerificationWorkerError::QueueFull);
        }
        let node_in_flight = admission.per_node.get(&node_id).copied().unwrap_or(0);
        if node_in_flight >= self.per_node_limit {
            self.metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record_fairness_rejection();
            return Err(DiagnosticVerificationWorkerError::FairnessLimit {
                node_id,
                limit: self.per_node_limit,
            });
        }
        let mut next_job_id = self
            .next_job_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let job_id = *next_job_id;
        let job = WorkerJob {
            job_id,
            submitted_at: Instant::now(),
            input,
            cancellation: cancellation.clone(),
        };
        let Some(sender) = self.sender.as_ref() else {
            self.metrics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .record_shutdown();
            return Err(DiagnosticVerificationWorkerError::Shutdown);
        };
        match sender.try_send(job) {
            Ok(()) => {
                *next_job_id = next_job_id.saturating_add(1);
                admission.in_flight = admission.in_flight.saturating_add(1);
                admission
                    .per_node
                    .entry(node_id)
                    .and_modify(|count| *count = count.saturating_add(1))
                    .or_insert(1);
                self.metrics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .record_submission();
                Ok(DiagnosticVerificationTicket {
                    job_id,
                    cancellation,
                })
            }
            Err(TrySendError::Full(_)) => {
                self.metrics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .record_queue_full();
                Err(DiagnosticVerificationWorkerError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.metrics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .record_shutdown();
                Err(DiagnosticVerificationWorkerError::Shutdown)
            }
        }
    }

    pub fn next_ordered(
        &mut self,
    ) -> Result<Option<DiagnosticVerifiedResult>, DiagnosticVerificationWorkerError> {
        while !self.pending_results.contains_key(&self.next_result_id) {
            match self.result_receiver.recv() {
                Ok(result) => {
                    if result.job_id > self.next_result_id {
                        let mut metrics = self
                            .metrics
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        metrics.out_of_order_buffered =
                            metrics.out_of_order_buffered.saturating_add(1);
                    }
                    self.pending_results.insert(result.job_id, result);
                }
                Err(_) if self.pending_results.is_empty() => return Ok(None),
                Err(_) => return Err(DiagnosticVerificationWorkerError::ResultChannelClosed),
            }
        }
        let result = self
            .pending_results
            .remove(&self.next_result_id)
            .expect("ordered result exists");
        self.next_result_id = self.next_result_id.saturating_add(1);
        self.release_admission(result.node_id);
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        metrics.ordered_dispatches = metrics.ordered_dispatches.saturating_add(1);
        drop(metrics);
        Ok(Some(DiagnosticVerifiedResult {
            job_id: result.job_id,
            node_id: result.node_id,
            connection_id: result.connection_id,
            sequence: result.sequence,
            evidence: result.evidence,
            cancellation: result.cancellation,
        }))
    }

    pub fn metrics(&self) -> DiagnosticVerificationWorkerMetrics {
        let state = self
            .metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        DiagnosticVerificationWorkerMetrics {
            worker_count: self.worker_count,
            queue_capacity: self.queue_capacity,
            per_node_limit: self.per_node_limit,
            submitted_jobs: state.submitted_jobs,
            completed_jobs: state.completed_jobs,
            failed_jobs: state.failed_jobs,
            cancelled_jobs: state.cancelled_jobs,
            queue_full_rejections: state.queue_full_rejections,
            fairness_rejections: state.fairness_rejections,
            shutdown_rejections: state.shutdown_rejections,
            ordered_dispatches: state.ordered_dispatches,
            out_of_order_buffered: state.out_of_order_buffered,
            latency_sample_count: state.queue_wait_us.len(),
            latency_sample_cap: MAX_DIAGNOSTIC_WORKER_LATENCY_SAMPLES,
            queue_wait_p50_us: percentile(&state.queue_wait_us, 50),
            queue_wait_p95_us: percentile(&state.queue_wait_us, 95),
            queue_wait_max_us: state.queue_wait_us.iter().copied().max().unwrap_or(0),
            verification_service_p50_us: percentile(&state.verification_service_us, 50),
            verification_service_p95_us: percentile(&state.verification_service_us, 95),
            verification_service_max_us: state
                .verification_service_us
                .iter()
                .copied()
                .max()
                .unwrap_or(0),
        }
    }

    pub fn close(&mut self) -> Result<(), DiagnosticVerificationWorkerError> {
        self.sender.take();
        let mut workers = std::mem::take(&mut self.workers);
        for worker in workers.drain(..) {
            while !worker.is_finished() {
                match self.result_receiver.try_recv() {
                    Ok(_) => {}
                    Err(mpsc::TryRecvError::Empty) => thread::yield_now(),
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
            worker
                .join()
                .map_err(|_| DiagnosticVerificationWorkerError::WorkerPanicked)?;
        }
        while self.result_receiver.try_recv().is_ok() {}
        Ok(())
    }

    fn release_admission(&self, node_id: u64) {
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        admission.in_flight = admission.in_flight.saturating_sub(1);
        if let Some(count) = admission.per_node.get_mut(&node_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                admission.per_node.remove(&node_id);
            }
        }
    }
}

impl Drop for DiagnosticVerificationWorkerPool {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run_worker(
    receiver: Arc<Mutex<Receiver<WorkerJob>>>,
    result_sender: SyncSender<WorkerResult>,
    metrics: Arc<Mutex<WorkerMetricsState>>,
    start_gate: Option<Arc<Barrier>>,
) {
    if let Some(gate) = start_gate {
        gate.wait();
    }
    loop {
        let job = receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv();
        let Ok(job) = job else {
            break;
        };
        let verification_started = Instant::now();
        let queue_wait_us = verification_started
            .saturating_duration_since(job.submitted_at)
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let input = job.input;
        let node_id = input.node_id;
        let connection_id = input.connection_id;
        let sequence = input.sequence;
        let cancelled_before = job.cancellation.is_cancelled();
        let evidence = if cancelled_before {
            Err(EmissionDiagnosticWorkerError::Cancelled)
        } else {
            let evidence = input
                .verifier
                .verify_stream_evidence_with_cache(
                    &input.attestation,
                    &input.stream,
                    &input.envelope,
                    &input.profile,
                    &input.units,
                    &input.cache,
                    &input.instrumentation,
                )
                .map_err(EmissionDiagnosticWorkerError::Verification);
            if job.cancellation.is_cancelled() {
                Err(EmissionDiagnosticWorkerError::Cancelled)
            } else {
                evidence
            }
        };
        let cancelled = matches!(evidence, Err(EmissionDiagnosticWorkerError::Cancelled));
        let service_us = verification_started
            .elapsed()
            .as_micros()
            .min(u64::MAX as u128) as u64;
        metrics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_completion(queue_wait_us, service_us, evidence.is_ok(), cancelled);
        let result = WorkerResult {
            job_id: job.job_id,
            node_id,
            connection_id,
            sequence,
            cancellation: job.cancellation,
            evidence,
        };
        if result_sender.send(result).is_err() {
            break;
        }
    }
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let index = ((ordered.len() - 1) * percentile / 100).min(ordered.len() - 1);
    ordered[index]
}
