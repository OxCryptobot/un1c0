use crate::ownership_bound_cas::OwnershipBoundCasReceipt;
use crate::ownership_bound_cas_executor::OwnershipBoundCasIntent;
use crate::ownership_bound_cas_verifier::{
    OwnershipBoundCasVerifierError, OwnershipBoundCasVerifierMetrics,
    OwnershipBoundCasVerifierPipeline, OwnershipBoundCasVerifierTicket,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use thiserror::Error;

const MAX_ADMISSION_PERMITS: usize = 64;
const MAX_ADMISSION_SAMPLES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveAdmissionConfig {
    pub initial_permits: usize,
    pub minimum_permits: usize,
    pub maximum_permits: usize,
    pub target_service_p95_us: u64,
    pub failure_threshold: u64,
    pub adjustment_window: u64,
}

impl Default for AdaptiveAdmissionConfig {
    fn default() -> Self {
        Self {
            initial_permits: 4,
            minimum_permits: 1,
            maximum_permits: 32,
            target_service_p95_us: 30_000,
            failure_threshold: 8,
            adjustment_window: 16,
        }
    }
}

impl AdaptiveAdmissionConfig {
    fn validate(&self) -> Result<(), AdaptiveAdmissionError> {
        if self.minimum_permits == 0
            || self.initial_permits < self.minimum_permits
            || self.maximum_permits < self.initial_permits
            || self.maximum_permits > MAX_ADMISSION_PERMITS
            || self.target_service_p95_us == 0
            || self.failure_threshold == 0
            || self.adjustment_window == 0
        {
            return Err(AdaptiveAdmissionError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdaptiveAdmissionError {
    #[error("phase 46 adaptive admission configuration is invalid")]
    InvalidConfiguration,
    #[error("phase 46 adaptive admission is limiting in-flight work ({in_flight}/{permits})")]
    Limited { in_flight: usize, permits: usize },
    #[error("phase 46 adaptive admission is shut down")]
    Shutdown,
    #[error("phase 46 underlying verifier rejected the intent: {0}")]
    Verifier(OwnershipBoundCasVerifierError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AdaptiveAdmissionSnapshot {
    pub in_flight: usize,
    pub permits: usize,
    pub minimum_permits: usize,
    pub maximum_permits: usize,
    pub adjustment_window: u64,
    pub window_completions: u64,
    pub window_failures: u64,
    pub total_completions: u64,
    pub total_failures: u64,
    pub limiter_rejections: u64,
    pub service_p95_us: u64,
    pub service_sample_count: usize,
    pub service_sample_cap: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AdaptiveAdmissionMetrics {
    pub verifier: OwnershipBoundCasVerifierMetrics,
    pub admission: AdaptiveAdmissionSnapshot,
}

#[derive(Debug)]
struct AdmissionState {
    in_flight: usize,
    permits: usize,
    window_completions: u64,
    window_failures: u64,
    total_completions: u64,
    total_failures: u64,
    limiter_rejections: u64,
    service_samples_us: Vec<u64>,
}

impl AdmissionState {
    fn new(config: &AdaptiveAdmissionConfig) -> Self {
        Self {
            in_flight: 0,
            permits: config.initial_permits,
            window_completions: 0,
            window_failures: 0,
            total_completions: 0,
            total_failures: 0,
            limiter_rejections: 0,
            service_samples_us: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct AdaptiveAdmissionController {
    config: AdaptiveAdmissionConfig,
    state: Mutex<AdmissionState>,
}

impl AdaptiveAdmissionController {
    fn new(config: AdaptiveAdmissionConfig) -> Result<Arc<Self>, AdaptiveAdmissionError> {
        config.validate()?;
        Ok(Arc::new(Self {
            state: Mutex::new(AdmissionState::new(&config)),
            config,
        }))
    }

    fn try_acquire(self: &Arc<Self>) -> Result<AdmissionPermit, AdaptiveAdmissionError> {
        let mut state = self.state.lock().expect("adaptive admission lock");
        if state.in_flight >= state.permits {
            state.limiter_rejections = state.limiter_rejections.saturating_add(1);
            return Err(AdaptiveAdmissionError::Limited {
                in_flight: state.in_flight,
                permits: state.permits,
            });
        }
        state.in_flight = state.in_flight.saturating_add(1);
        Ok(AdmissionPermit {
            controller: Arc::clone(self),
            admitted_at: Instant::now(),
            completed: false,
        })
    }

    fn finish(&self, validation_failure: bool, admitted_at: Instant) {
        let service_us = admitted_at.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let mut state = self.state.lock().expect("adaptive admission lock");
        state.in_flight = state.in_flight.saturating_sub(1);
        state.window_completions = state.window_completions.saturating_add(1);
        state.total_completions = state.total_completions.saturating_add(1);
        if validation_failure {
            state.window_failures = state.window_failures.saturating_add(1);
            state.total_failures = state.total_failures.saturating_add(1);
        }
        if state.service_samples_us.len() < MAX_ADMISSION_SAMPLES {
            state.service_samples_us.push(service_us);
        }
        if state.window_completions >= self.config.adjustment_window {
            let p95 = percentile(&state.service_samples_us, 95);
            if p95 > self.config.target_service_p95_us
                || state.window_failures >= self.config.failure_threshold
            {
                state.permits = (state.permits / 2).max(self.config.minimum_permits);
            } else {
                state.permits = state
                    .permits
                    .saturating_add(1)
                    .min(self.config.maximum_permits);
            }
            state.window_completions = 0;
            state.window_failures = 0;
        }
    }

    fn cancel(&self) {
        let mut state = self.state.lock().expect("adaptive admission lock");
        state.in_flight = state.in_flight.saturating_sub(1);
    }

    fn snapshot(&self) -> AdaptiveAdmissionSnapshot {
        let state = self.state.lock().expect("adaptive admission lock");
        AdaptiveAdmissionSnapshot {
            in_flight: state.in_flight,
            permits: state.permits,
            minimum_permits: self.config.minimum_permits,
            maximum_permits: self.config.maximum_permits,
            adjustment_window: self.config.adjustment_window,
            window_completions: state.window_completions,
            window_failures: state.window_failures,
            total_completions: state.total_completions,
            total_failures: state.total_failures,
            limiter_rejections: state.limiter_rejections,
            service_p95_us: percentile(&state.service_samples_us, 95),
            service_sample_count: state.service_samples_us.len(),
            service_sample_cap: MAX_ADMISSION_SAMPLES,
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
struct AdmissionPermit {
    controller: Arc<AdaptiveAdmissionController>,
    admitted_at: Instant,
    completed: bool,
}

impl AdmissionPermit {
    fn finish(mut self, validation_failure: bool) {
        self.completed = true;
        self.controller.finish(validation_failure, self.admitted_at);
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        if !self.completed {
            self.controller.cancel();
        }
    }
}

#[derive(Debug)]
pub struct AdaptiveOwnershipBoundCasTicket {
    pub intent_id: u64,
    verifier_ticket: OwnershipBoundCasVerifierTicket,
    permit: Option<AdmissionPermit>,
}

impl AdaptiveOwnershipBoundCasTicket {
    pub fn wait(mut self) -> Result<OwnershipBoundCasReceipt, AdaptiveAdmissionError> {
        let result = self.verifier_ticket.wait();
        if let Some(permit) = self.permit.take() {
            let validation_failure = matches!(
                &result,
                Err(OwnershipBoundCasVerifierError::PreAdmission(_))
                    | Err(OwnershipBoundCasVerifierError::VerificationEvidenceMismatch)
            );
            permit.finish(validation_failure);
        }
        result.map_err(AdaptiveAdmissionError::Verifier)
    }
}

#[derive(Debug)]
pub struct AdaptiveOwnershipBoundCasAdmission {
    pipeline: OwnershipBoundCasVerifierPipeline,
    controller: Arc<AdaptiveAdmissionController>,
}

impl AdaptiveOwnershipBoundCasAdmission {
    pub fn new(
        coordinator: crate::ownership_bound_cas::OwnershipBoundCasCoordinator,
        verification_workers: usize,
        queue_capacity: usize,
        config: AdaptiveAdmissionConfig,
    ) -> Result<Self, AdaptiveAdmissionError> {
        let controller = AdaptiveAdmissionController::new(config)?;
        let pipeline = OwnershipBoundCasVerifierPipeline::new(
            coordinator,
            verification_workers,
            queue_capacity,
        )
        .map_err(AdaptiveAdmissionError::Verifier)?;
        Ok(Self {
            pipeline,
            controller,
        })
    }

    pub fn submit(
        &self,
        intent: OwnershipBoundCasIntent,
    ) -> Result<AdaptiveOwnershipBoundCasTicket, AdaptiveAdmissionError> {
        let permit = self.controller.try_acquire()?;
        match self.pipeline.submit(intent) {
            Ok(verifier_ticket) => Ok(AdaptiveOwnershipBoundCasTicket {
                intent_id: verifier_ticket.intent_id,
                verifier_ticket,
                permit: Some(permit),
            }),
            Err(error) => {
                drop(permit);
                Err(AdaptiveAdmissionError::Verifier(error))
            }
        }
    }

    pub fn metrics(&self) -> AdaptiveAdmissionMetrics {
        AdaptiveAdmissionMetrics {
            verifier: self.pipeline.metrics(),
            admission: self.controller.snapshot(),
        }
    }

    pub fn close(&mut self) -> Result<(), AdaptiveAdmissionError> {
        self.pipeline
            .close()
            .map_err(AdaptiveAdmissionError::Verifier)
    }
}

impl Drop for AdaptiveOwnershipBoundCasAdmission {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
