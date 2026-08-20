use crate::cross_process_ownership::OwnershipWritePermit;
use crate::ownership_bound_cas::{
    OwnershipBoundCasCoordinator, OwnershipBoundCasError, OwnershipBoundCasReceipt,
};
use crate::replicated_durability::{CasWriteRequest, ReplicaDurabilityAcknowledgement};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use thiserror::Error;

const MAX_INTENT_ACKNOWLEDGEMENTS: usize = 64;
const MAX_LATENCY_SAMPLES: usize = 4_096;

#[derive(Debug, Clone)]
pub struct OwnershipBoundCasIntent {
    pub permit: OwnershipWritePermit,
    pub request: CasWriteRequest,
    pub acknowledgements: Vec<ReplicaDurabilityAcknowledgement>,
    pub current_tick: u64,
}

impl OwnershipBoundCasIntent {
    pub fn validate(&self) -> Result<(), OwnershipBoundCasExecutorError> {
        if self.acknowledgements.len() > MAX_INTENT_ACKNOWLEDGEMENTS {
            return Err(OwnershipBoundCasExecutorError::IntentTooLarge);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OwnershipBoundCasExecutorError {
    #[error("ownership-bound CAS executor queue is full")]
    QueueFull,
    #[error("ownership-bound CAS executor is shut down")]
    Shutdown,
    #[error("ownership-bound CAS intent exceeds bounded acknowledgement capacity")]
    IntentTooLarge,
    #[error("ownership-bound CAS executor worker stopped before returning a result")]
    WorkerStopped,
    #[error("ownership-bound CAS executor worker panicked during shutdown")]
    WorkerPanicked,
}

#[derive(Debug)]
struct ExecutorJob {
    intent_id: u64,
    submitted_at: Instant,
    intent: OwnershipBoundCasIntent,
    response: mpsc::Sender<Result<OwnershipBoundCasReceipt, OwnershipBoundCasError>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipBoundCasExecutorMetrics {
    pub accepted_intents: u64,
    pub completed_intents: u64,
    pub failed_intents: u64,
    pub queue_full_rejections: u64,
    pub shutdown_rejections: u64,
    pub latency_sample_count: usize,
    pub latency_sample_cap: usize,
    pub queue_wait_p50_us: u64,
    pub queue_wait_p95_us: u64,
    pub queue_wait_max_us: u64,
    pub service_p50_us: u64,
    pub service_p95_us: u64,
    pub service_max_us: u64,
    pub end_to_end_p50_us: u64,
    pub end_to_end_p95_us: u64,
    pub end_to_end_max_us: u64,
}

#[derive(Debug, Default)]
struct ExecutorMetricsState {
    accepted_intents: u64,
    completed_intents: u64,
    failed_intents: u64,
    queue_full_rejections: u64,
    shutdown_rejections: u64,
    queue_wait_us: Vec<u64>,
    service_us: Vec<u64>,
    end_to_end_us: Vec<u64>,
}

impl ExecutorMetricsState {
    fn record_submission(&mut self) {
        self.accepted_intents = self.accepted_intents.saturating_add(1);
    }

    fn record_queue_full(&mut self) {
        self.queue_full_rejections = self.queue_full_rejections.saturating_add(1);
    }

    fn record_shutdown(&mut self) {
        self.shutdown_rejections = self.shutdown_rejections.saturating_add(1);
    }

    fn record_completion(
        &mut self,
        queue_wait_us: u64,
        service_us: u64,
        end_to_end_us: u64,
        success: bool,
    ) {
        self.completed_intents = self.completed_intents.saturating_add(1);
        if !success {
            self.failed_intents = self.failed_intents.saturating_add(1);
        }
        if self.queue_wait_us.len() < MAX_LATENCY_SAMPLES {
            self.queue_wait_us.push(queue_wait_us);
            self.service_us.push(service_us);
            self.end_to_end_us.push(end_to_end_us);
        }
    }

    fn snapshot(&self) -> OwnershipBoundCasExecutorMetrics {
        OwnershipBoundCasExecutorMetrics {
            accepted_intents: self.accepted_intents,
            completed_intents: self.completed_intents,
            failed_intents: self.failed_intents,
            queue_full_rejections: self.queue_full_rejections,
            shutdown_rejections: self.shutdown_rejections,
            latency_sample_count: self.end_to_end_us.len(),
            latency_sample_cap: MAX_LATENCY_SAMPLES,
            queue_wait_p50_us: percentile(&self.queue_wait_us, 50),
            queue_wait_p95_us: percentile(&self.queue_wait_us, 95),
            queue_wait_max_us: self.queue_wait_us.iter().copied().max().unwrap_or(0),
            service_p50_us: percentile(&self.service_us, 50),
            service_p95_us: percentile(&self.service_us, 95),
            service_max_us: self.service_us.iter().copied().max().unwrap_or(0),
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
pub struct OwnershipBoundCasTicket {
    pub intent_id: u64,
    receiver: Receiver<Result<OwnershipBoundCasReceipt, OwnershipBoundCasError>>,
}

impl OwnershipBoundCasTicket {
    pub fn wait(
        self,
    ) -> Result<
        Result<OwnershipBoundCasReceipt, OwnershipBoundCasError>,
        OwnershipBoundCasExecutorError,
    > {
        self.receiver
            .recv()
            .map_err(|_| OwnershipBoundCasExecutorError::WorkerStopped)
    }
}

#[derive(Debug)]
pub struct OwnershipBoundCasExecutor {
    sender: Option<SyncSender<ExecutorJob>>,
    next_intent_id: AtomicU64,
    metrics: Arc<Mutex<ExecutorMetricsState>>,
    worker: Option<JoinHandle<()>>,
}

impl OwnershipBoundCasExecutor {
    pub fn new(
        coordinator: OwnershipBoundCasCoordinator,
        queue_capacity: usize,
    ) -> Result<Self, OwnershipBoundCasExecutorError> {
        Self::new_internal(coordinator, queue_capacity, None)
    }

    pub fn new_with_worker_start_gate(
        coordinator: OwnershipBoundCasCoordinator,
        queue_capacity: usize,
        gate: Arc<Barrier>,
    ) -> Result<Self, OwnershipBoundCasExecutorError> {
        Self::new_internal(coordinator, queue_capacity, Some(gate))
    }

    fn new_internal(
        coordinator: OwnershipBoundCasCoordinator,
        queue_capacity: usize,
        start_gate: Option<Arc<Barrier>>,
    ) -> Result<Self, OwnershipBoundCasExecutorError> {
        if queue_capacity == 0 {
            return Err(OwnershipBoundCasExecutorError::QueueFull);
        }
        let (sender, receiver) = mpsc::sync_channel(queue_capacity);
        let metrics = Arc::new(Mutex::new(ExecutorMetricsState::default()));
        let worker_metrics = Arc::clone(&metrics);
        let worker = thread::Builder::new()
            .name("ownership-bound-cas-executor".into())
            .spawn(move || run_worker(coordinator, receiver, worker_metrics, start_gate))
            .map_err(|_| OwnershipBoundCasExecutorError::Shutdown)?;
        Ok(Self {
            sender: Some(sender),
            next_intent_id: AtomicU64::new(1),
            metrics,
            worker: Some(worker),
        })
    }

    pub fn submit(
        &self,
        intent: OwnershipBoundCasIntent,
    ) -> Result<OwnershipBoundCasTicket, OwnershipBoundCasExecutorError> {
        intent.validate()?;
        let intent_id = self.next_intent_id.fetch_add(1, Ordering::Relaxed);
        let (response, receiver) = mpsc::channel();
        let job = ExecutorJob {
            intent_id,
            submitted_at: Instant::now(),
            intent,
            response,
        };
        let sender = match self.sender.as_ref() {
            Some(sender) => sender,
            None => {
                self.metrics.lock().expect("metrics lock").record_shutdown();
                return Err(OwnershipBoundCasExecutorError::Shutdown);
            }
        };
        match sender.try_send(job) {
            Ok(()) => {
                self.metrics
                    .lock()
                    .expect("metrics lock")
                    .record_submission();
                Ok(OwnershipBoundCasTicket {
                    intent_id,
                    receiver,
                })
            }
            Err(TrySendError::Full(_)) => {
                self.metrics
                    .lock()
                    .expect("metrics lock")
                    .record_queue_full();
                Err(OwnershipBoundCasExecutorError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.metrics.lock().expect("metrics lock").record_shutdown();
                Err(OwnershipBoundCasExecutorError::Shutdown)
            }
        }
    }

    pub fn metrics(&self) -> OwnershipBoundCasExecutorMetrics {
        self.metrics.lock().expect("metrics lock").snapshot()
    }

    pub fn close(&mut self) -> Result<(), OwnershipBoundCasExecutorError> {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| OwnershipBoundCasExecutorError::WorkerPanicked)?;
        }
        Ok(())
    }
}

impl Drop for OwnershipBoundCasExecutor {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn run_worker(
    mut coordinator: OwnershipBoundCasCoordinator,
    receiver: Receiver<ExecutorJob>,
    metrics: Arc<Mutex<ExecutorMetricsState>>,
    start_gate: Option<Arc<Barrier>>,
) {
    if let Some(gate) = start_gate {
        gate.wait();
    }
    while let Ok(job) = receiver.recv() {
        let started = Instant::now();
        let queue_wait_us = started
            .saturating_duration_since(job.submitted_at)
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let result = coordinator.commit_owned(
            job.intent.permit,
            job.intent.request,
            &job.intent.acknowledgements,
            job.intent.current_tick,
        );
        let service_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let end_to_end_us = job.submitted_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let success = result.is_ok();
        metrics.lock().expect("metrics lock").record_completion(
            queue_wait_us,
            service_us,
            end_to_end_us,
            success,
        );
        let _ = job.response.send(result);
        let _ = job.intent_id;
    }
}
