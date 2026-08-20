use crate::ownership_bound_cas::{
    OwnershipBoundCasCoordinator, OwnershipBoundCasError, OwnershipBoundCasReceipt,
};
use crate::ownership_bound_cas_executor::OwnershipBoundCasIntent;
use crate::replicated_durability::{
    CasPreAdmissionContext, CasPreAdmissionEvidence, ReplicatedDurabilityError,
};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use thiserror::Error;

const MAX_VERIFICATION_WORKERS: usize = 32;
const MAX_INTENT_ACKNOWLEDGEMENTS: usize = 64;
const MAX_LATENCY_SAMPLES: usize = 4_096;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OwnershipBoundCasVerifierError {
    #[error("phase 45 verification queue is full")]
    VerificationQueueFull,
    #[error("phase 45 pipeline is shut down")]
    Shutdown,
    #[error("ownership-bound CAS intent exceeds bounded acknowledgement capacity")]
    IntentTooLarge,
    #[error("phase 45 pre-admission verification failed: {0}")]
    PreAdmission(ReplicatedDurabilityError),
    #[error("phase 45 mutation failed: {0}")]
    Mutation(OwnershipBoundCasError),
    #[error("phase 45 pre-admission evidence did not match the intent")]
    VerificationEvidenceMismatch,
    #[error("phase 45 worker stopped before returning a result")]
    WorkerStopped,
    #[error("phase 45 worker panicked during shutdown")]
    WorkerPanicked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipBoundCasVerifierMetrics {
    pub submitted_intents: u64,
    pub pre_admitted_intents: u64,
    pub pre_admission_failures: u64,
    pub completed_intents: u64,
    pub failed_intents: u64,
    pub verification_queue_full_rejections: u64,
    pub shutdown_rejections: u64,
    pub latency_sample_count: usize,
    pub latency_sample_cap: usize,
    pub verification_wait_p50_us: u64,
    pub verification_wait_p95_us: u64,
    pub verification_wait_max_us: u64,
    pub verification_service_p50_us: u64,
    pub verification_service_p95_us: u64,
    pub verification_service_max_us: u64,
    pub mutation_service_p50_us: u64,
    pub mutation_service_p95_us: u64,
    pub mutation_service_max_us: u64,
    pub end_to_end_p50_us: u64,
    pub end_to_end_p95_us: u64,
    pub end_to_end_max_us: u64,
}

#[derive(Debug, Default)]
struct VerifierMetricsState {
    submitted_intents: u64,
    pre_admitted_intents: u64,
    pre_admission_failures: u64,
    completed_intents: u64,
    failed_intents: u64,
    verification_queue_full_rejections: u64,
    shutdown_rejections: u64,
    verification_wait_us: Vec<u64>,
    verification_service_us: Vec<u64>,
    mutation_service_us: Vec<u64>,
    end_to_end_us: Vec<u64>,
}

impl VerifierMetricsState {
    fn record_submission(&mut self) {
        self.submitted_intents = self.submitted_intents.saturating_add(1);
    }

    fn record_queue_full(&mut self) {
        self.verification_queue_full_rejections =
            self.verification_queue_full_rejections.saturating_add(1);
    }

    fn record_shutdown(&mut self) {
        self.shutdown_rejections = self.shutdown_rejections.saturating_add(1);
    }

    fn record_pre_admitted(&mut self) {
        self.pre_admitted_intents = self.pre_admitted_intents.saturating_add(1);
    }

    fn record_pre_admission_failure(
        &mut self,
        queue_wait_us: u64,
        service_us: u64,
        end_to_end_us: u64,
    ) {
        self.pre_admission_failures = self.pre_admission_failures.saturating_add(1);
        self.failed_intents = self.failed_intents.saturating_add(1);
        self.record_sample(queue_wait_us, service_us, 0, end_to_end_us);
    }

    fn record_mutation_completion(
        &mut self,
        verification_wait_us: u64,
        verification_service_us: u64,
        mutation_service_us: u64,
        end_to_end_us: u64,
        success: bool,
    ) {
        self.completed_intents = self.completed_intents.saturating_add(1);
        if !success {
            self.failed_intents = self.failed_intents.saturating_add(1);
        }
        self.record_sample(
            verification_wait_us,
            verification_service_us,
            mutation_service_us,
            end_to_end_us,
        );
    }

    fn record_sample(
        &mut self,
        verification_wait_us: u64,
        verification_service_us: u64,
        mutation_service_us: u64,
        end_to_end_us: u64,
    ) {
        if self.end_to_end_us.len() < MAX_LATENCY_SAMPLES {
            self.verification_wait_us.push(verification_wait_us);
            self.verification_service_us.push(verification_service_us);
            self.mutation_service_us.push(mutation_service_us);
            self.end_to_end_us.push(end_to_end_us);
        }
    }

    fn snapshot(&self) -> OwnershipBoundCasVerifierMetrics {
        OwnershipBoundCasVerifierMetrics {
            submitted_intents: self.submitted_intents,
            pre_admitted_intents: self.pre_admitted_intents,
            pre_admission_failures: self.pre_admission_failures,
            completed_intents: self.completed_intents,
            failed_intents: self.failed_intents,
            verification_queue_full_rejections: self.verification_queue_full_rejections,
            shutdown_rejections: self.shutdown_rejections,
            latency_sample_count: self.end_to_end_us.len(),
            latency_sample_cap: MAX_LATENCY_SAMPLES,
            verification_wait_p50_us: percentile(&self.verification_wait_us, 50),
            verification_wait_p95_us: percentile(&self.verification_wait_us, 95),
            verification_wait_max_us: self.verification_wait_us.iter().copied().max().unwrap_or(0),
            verification_service_p50_us: percentile(&self.verification_service_us, 50),
            verification_service_p95_us: percentile(&self.verification_service_us, 95),
            verification_service_max_us: self
                .verification_service_us
                .iter()
                .copied()
                .max()
                .unwrap_or(0),
            mutation_service_p50_us: percentile(&self.mutation_service_us, 50),
            mutation_service_p95_us: percentile(&self.mutation_service_us, 95),
            mutation_service_max_us: self.mutation_service_us.iter().copied().max().unwrap_or(0),
            end_to_end_p50_us: percentile(&self.end_to_end_us, 50),
            end_to_end_p95_us: percentile(&self.end_to_end_us, 95),
            end_to_end_max_us: self.end_to_end_us.iter().copied().max().unwrap_or(0),
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

#[derive(Debug)]
struct VerificationJob {
    intent_id: u64,
    submitted_at: Instant,
    intent: OwnershipBoundCasIntent,
    response: mpsc::Sender<Result<OwnershipBoundCasReceipt, OwnershipBoundCasVerifierError>>,
}

#[derive(Debug)]
struct VerifiedJob {
    intent_id: u64,
    submitted_at: Instant,
    verification_wait_us: u64,
    verification_service_us: u64,
    intent: OwnershipBoundCasIntent,
    verification: Result<CasPreAdmissionEvidence, ReplicatedDurabilityError>,
    response: mpsc::Sender<Result<OwnershipBoundCasReceipt, OwnershipBoundCasVerifierError>>,
}

#[derive(Debug)]
struct MutationJob {
    intent_id: u64,
    submitted_at: Instant,
    verification_wait_us: u64,
    verification_service_us: u64,
    intent: OwnershipBoundCasIntent,
    evidence: CasPreAdmissionEvidence,
    response: mpsc::Sender<Result<OwnershipBoundCasReceipt, OwnershipBoundCasVerifierError>>,
}

#[derive(Debug)]
pub struct OwnershipBoundCasVerifierTicket {
    pub intent_id: u64,
    receiver: Receiver<Result<OwnershipBoundCasReceipt, OwnershipBoundCasVerifierError>>,
}

impl OwnershipBoundCasVerifierTicket {
    pub fn wait(self) -> Result<OwnershipBoundCasReceipt, OwnershipBoundCasVerifierError> {
        self.receiver
            .recv()
            .map_err(|_| OwnershipBoundCasVerifierError::WorkerStopped)?
    }
}

#[derive(Debug)]
pub struct OwnershipBoundCasVerifierPipeline {
    verification_sender: Option<SyncSender<VerificationJob>>,
    next_intent_id: Mutex<u64>,
    metrics: Arc<Mutex<VerifierMetricsState>>,
    verification_workers: Vec<JoinHandle<()>>,
    result_sender: Option<SyncSender<VerifiedJob>>,
    dispatcher_worker: Option<JoinHandle<()>>,
    mutation_sender: Option<SyncSender<MutationJob>>,
    mutation_worker: Option<JoinHandle<()>>,
}

impl OwnershipBoundCasVerifierPipeline {
    pub fn new(
        coordinator: OwnershipBoundCasCoordinator,
        verification_workers: usize,
        queue_capacity: usize,
    ) -> Result<Self, OwnershipBoundCasVerifierError> {
        Self::new_internal(coordinator, verification_workers, queue_capacity, None)
    }

    pub fn new_with_verifier_start_gate(
        coordinator: OwnershipBoundCasCoordinator,
        verification_workers: usize,
        queue_capacity: usize,
        start_gate: Arc<Barrier>,
    ) -> Result<Self, OwnershipBoundCasVerifierError> {
        Self::new_internal(
            coordinator,
            verification_workers,
            queue_capacity,
            Some(start_gate),
        )
    }

    fn new_internal(
        coordinator: OwnershipBoundCasCoordinator,
        verification_workers: usize,
        queue_capacity: usize,
        start_gate: Option<Arc<Barrier>>,
    ) -> Result<Self, OwnershipBoundCasVerifierError> {
        if verification_workers == 0 || verification_workers > MAX_VERIFICATION_WORKERS {
            return Err(OwnershipBoundCasVerifierError::IntentTooLarge);
        }
        if queue_capacity == 0 {
            return Err(OwnershipBoundCasVerifierError::VerificationQueueFull);
        }
        let context = coordinator.pre_admission_context();
        let (verification_sender, verification_receiver) = mpsc::sync_channel(queue_capacity);
        let verification_receiver = Arc::new(Mutex::new(verification_receiver));
        let (result_sender, result_receiver) = mpsc::sync_channel(queue_capacity);
        let (mutation_sender, mutation_receiver) = mpsc::sync_channel(queue_capacity);
        let metrics = Arc::new(Mutex::new(VerifierMetricsState::default()));
        let mutation_metrics = Arc::clone(&metrics);
        let mutation_worker = thread::Builder::new()
            .name("ownership-bound-cas-mutation".into())
            .spawn(move || run_mutation_worker(coordinator, mutation_receiver, mutation_metrics))
            .map_err(|_| OwnershipBoundCasVerifierError::Shutdown)?;
        let dispatcher_metrics = Arc::clone(&metrics);
        let dispatcher_sender = mutation_sender.clone();
        let dispatcher_worker = thread::Builder::new()
            .name("ownership-bound-cas-verifier-dispatch".into())
            .spawn(move || run_dispatcher(result_receiver, dispatcher_sender, dispatcher_metrics))
            .map_err(|_| OwnershipBoundCasVerifierError::Shutdown)?;

        let mut workers = Vec::with_capacity(verification_workers);
        for index in 0..verification_workers {
            let receiver = Arc::clone(&verification_receiver);
            let worker_context = context.clone();
            let worker_sender = result_sender.clone();
            let worker_gate = start_gate.clone();
            let worker = thread::Builder::new()
                .name(format!("ownership-bound-cas-verifier-{index}"))
                .spawn(move || {
                    run_verification_worker(worker_context, receiver, worker_sender, worker_gate)
                })
                .map_err(|_| OwnershipBoundCasVerifierError::Shutdown)?;
            workers.push(worker);
        }

        Ok(Self {
            verification_sender: Some(verification_sender),
            next_intent_id: Mutex::new(1),
            metrics,
            verification_workers: workers,
            result_sender: Some(result_sender),
            dispatcher_worker: Some(dispatcher_worker),
            mutation_sender: Some(mutation_sender),
            mutation_worker: Some(mutation_worker),
        })
    }

    pub fn submit(
        &self,
        intent: OwnershipBoundCasIntent,
    ) -> Result<OwnershipBoundCasVerifierTicket, OwnershipBoundCasVerifierError> {
        if intent.acknowledgements.len() > MAX_INTENT_ACKNOWLEDGEMENTS {
            return Err(OwnershipBoundCasVerifierError::IntentTooLarge);
        }
        let (response, receiver) = mpsc::channel();
        let mut next_id = self.next_intent_id.lock().expect("intent id lock");
        let intent_id = *next_id;
        let job = VerificationJob {
            intent_id,
            submitted_at: Instant::now(),
            intent,
            response,
        };
        let sender = match self.verification_sender.as_ref() {
            Some(sender) => sender,
            None => {
                self.metrics.lock().expect("metrics lock").record_shutdown();
                return Err(OwnershipBoundCasVerifierError::Shutdown);
            }
        };
        match sender.try_send(job) {
            Ok(()) => {
                *next_id = next_id.saturating_add(1);
                self.metrics
                    .lock()
                    .expect("metrics lock")
                    .record_submission();
                Ok(OwnershipBoundCasVerifierTicket {
                    intent_id,
                    receiver,
                })
            }
            Err(TrySendError::Full(_)) => {
                self.metrics
                    .lock()
                    .expect("metrics lock")
                    .record_queue_full();
                Err(OwnershipBoundCasVerifierError::VerificationQueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.metrics.lock().expect("metrics lock").record_shutdown();
                Err(OwnershipBoundCasVerifierError::Shutdown)
            }
        }
    }

    pub fn metrics(&self) -> OwnershipBoundCasVerifierMetrics {
        self.metrics.lock().expect("metrics lock").snapshot()
    }

    pub fn close(&mut self) -> Result<(), OwnershipBoundCasVerifierError> {
        self.verification_sender.take();
        for worker in self.verification_workers.drain(..) {
            worker
                .join()
                .map_err(|_| OwnershipBoundCasVerifierError::WorkerPanicked)?;
        }
        self.result_sender.take();
        if let Some(worker) = self.dispatcher_worker.take() {
            worker
                .join()
                .map_err(|_| OwnershipBoundCasVerifierError::WorkerPanicked)?;
        }
        self.mutation_sender.take();
        if let Some(worker) = self.mutation_worker.take() {
            worker
                .join()
                .map_err(|_| OwnershipBoundCasVerifierError::WorkerPanicked)?;
        }
        Ok(())
    }
}

impl Drop for OwnershipBoundCasVerifierPipeline {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run_verification_worker(
    context: CasPreAdmissionContext,
    receiver: Arc<Mutex<Receiver<VerificationJob>>>,
    result_sender: SyncSender<VerifiedJob>,
    start_gate: Option<Arc<Barrier>>,
) {
    if let Some(gate) = start_gate {
        gate.wait();
    }
    loop {
        let job = receiver.lock().expect("verification queue lock").recv();
        let Ok(job) = job else {
            break;
        };
        let verification_started = Instant::now();
        let verification_wait_us = verification_started
            .saturating_duration_since(job.submitted_at)
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let verification = context.verify(
            &job.intent.request,
            &job.intent.acknowledgements,
            job.intent.current_tick,
        );
        let verification_service_us = verification_started
            .elapsed()
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let verified = VerifiedJob {
            intent_id: job.intent_id,
            submitted_at: job.submitted_at,
            verification_wait_us,
            verification_service_us,
            intent: job.intent,
            verification,
            response: job.response,
        };
        if result_sender.send(verified).is_err() {
            break;
        }
    }
}

fn run_dispatcher(
    receiver: Receiver<VerifiedJob>,
    mutation_sender: SyncSender<MutationJob>,
    metrics: Arc<Mutex<VerifierMetricsState>>,
) {
    let mut pending = BTreeMap::new();
    let mut next_intent_id = 1_u64;
    while let Ok(job) = receiver.recv() {
        pending.insert(job.intent_id, job);
        while let Some(job) = pending.remove(&next_intent_id) {
            match job.verification {
                Ok(evidence) => {
                    if evidence.request_hash != job.intent.request.request_hash {
                        let end_to_end_us =
                            job.submitted_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
                        metrics
                            .lock()
                            .expect("metrics lock")
                            .record_pre_admission_failure(
                                job.verification_wait_us,
                                job.verification_service_us,
                                end_to_end_us,
                            );
                        let _ = job.response.send(Err(
                            OwnershipBoundCasVerifierError::VerificationEvidenceMismatch,
                        ));
                    } else {
                        metrics.lock().expect("metrics lock").record_pre_admitted();
                        let mutation_job = MutationJob {
                            intent_id: job.intent_id,
                            submitted_at: job.submitted_at,
                            verification_wait_us: job.verification_wait_us,
                            verification_service_us: job.verification_service_us,
                            intent: job.intent,
                            evidence,
                            response: job.response,
                        };
                        if let Err(error) = mutation_sender.send(mutation_job) {
                            metrics.lock().expect("metrics lock").record_shutdown();
                            let failed_job = error.0;
                            let _ = failed_job
                                .response
                                .send(Err(OwnershipBoundCasVerifierError::Shutdown));
                            return;
                        }
                    }
                }
                Err(error) => {
                    let end_to_end_us =
                        job.submitted_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
                    metrics
                        .lock()
                        .expect("metrics lock")
                        .record_pre_admission_failure(
                            job.verification_wait_us,
                            job.verification_service_us,
                            end_to_end_us,
                        );
                    let _ = job
                        .response
                        .send(Err(OwnershipBoundCasVerifierError::PreAdmission(error)));
                }
            }
            next_intent_id = next_intent_id.saturating_add(1);
        }
    }
}

fn run_mutation_worker(
    mut coordinator: OwnershipBoundCasCoordinator,
    receiver: Receiver<MutationJob>,
    metrics: Arc<Mutex<VerifierMetricsState>>,
) {
    while let Ok(job) = receiver.recv() {
        let mutation_started = Instant::now();
        let result = if job.evidence.request_hash != job.intent.request.request_hash {
            Err(OwnershipBoundCasVerifierError::VerificationEvidenceMismatch)
        } else {
            coordinator
                .commit_owned(
                    job.intent.permit,
                    job.intent.request,
                    &job.intent.acknowledgements,
                    job.intent.current_tick,
                )
                .map_err(OwnershipBoundCasVerifierError::Mutation)
        };
        let mutation_service_us =
            mutation_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let end_to_end_us = job.submitted_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let success = result.is_ok();
        metrics
            .lock()
            .expect("metrics lock")
            .record_mutation_completion(
                job.verification_wait_us,
                job.verification_service_us,
                mutation_service_us,
                end_to_end_us,
                success,
            );
        let _ = job.response.send(result);
        let _ = job.intent_id;
    }
}
