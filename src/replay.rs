use crate::multiregion::{LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationError};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_REPLAY_EVENTS: usize = 4096;
const MAX_REPLAY_TICKS: u64 = 1_000_000;
const MAX_REPLAY_IDENTIFIER_BYTES: usize = 128;
const MAX_REPLAY_NONCE_BYTES: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReplaySecurityError {
    #[error("invalid replay manifest: {0}")]
    InvalidManifest(String),
    #[error("replay signature rejected: {0}")]
    SignatureRejected(String),
    #[error("replay binding rejected: {0}")]
    BindingRejected(String),
    #[error("replay schedule hash mismatch")]
    ScheduleHashMismatch,
    #[error("replay sequence or tick violation: {0}")]
    SequenceTickViolation(String),
    #[error("replay trace seal rejected: {0}")]
    TraceSealRejected(String),
    #[error("replay simulation failed: {0}")]
    Simulation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayFaultStep {
    pub sequence: u64,
    pub tick: u64,
    pub from: String,
    pub to: String,
    pub fault: LinkFault,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayManifest {
    pub scenario_id: String,
    pub cluster_id: String,
    pub signer_id: String,
    pub replay_epoch: u64,
    pub owner_term: u64,
    pub seed: u64,
    pub nonce: String,
    pub schedule_digest: String,
    pub max_events: usize,
    pub max_tick: u64,
    pub schedule: Vec<ReplayFaultStep>,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct ReplayManifestPayload<'a> {
    scenario_id: &'a str,
    cluster_id: &'a str,
    signer_id: &'a str,
    replay_epoch: u64,
    owner_term: u64,
    seed: u64,
    nonce: &'a str,
    schedule_digest: &'a str,
    max_events: usize,
    max_tick: u64,
    schedule: &'a [ReplayFaultStep],
    public_key: &'a [u8],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayTraceSeal {
    pub scenario_id: String,
    pub cluster_id: String,
    pub signer_id: String,
    pub replay_epoch: u64,
    pub event_digest: String,
    pub event_count: usize,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct ReplayTraceSealPayload<'a> {
    scenario_id: &'a str,
    cluster_id: &'a str,
    signer_id: &'a str,
    replay_epoch: u64,
    event_digest: &'a str,
    event_count: usize,
    public_key: &'a [u8],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayVerificationResult {
    pub scenario_id: String,
    pub trace_digest: String,
    pub event_count: usize,
    pub applied_steps: usize,
    pub safety_passed: bool,
    pub liveness_passed: bool,
}

impl ReplayManifest {
    pub fn new(
        scenario_id: &str,
        cluster_id: &str,
        signer_id: &str,
        replay_epoch: u64,
        owner_term: u64,
        seed: u64,
        nonce: &str,
        schedule: Vec<ReplayFaultStep>,
        signing_key: &SigningKey,
    ) -> Result<Self, ReplaySecurityError> {
        let mut manifest = Self {
            scenario_id: scenario_id.to_string(),
            cluster_id: cluster_id.to_string(),
            signer_id: signer_id.to_string(),
            replay_epoch,
            owner_term,
            seed,
            nonce: nonce.to_string(),
            schedule_digest: digest_json(&schedule)?,
            max_events: MAX_REPLAY_EVENTS,
            max_tick: MAX_REPLAY_TICKS,
            schedule,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
        };
        manifest.validate_shape()?;
        manifest.signature = signing_key
            .sign(&manifest.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(manifest)
    }

    pub fn sign_with(&mut self, signing_key: &SigningKey) -> Result<(), ReplaySecurityError> {
        if self.public_key != signing_key.verifying_key().to_bytes() {
            return Err(ReplaySecurityError::BindingRejected(
                "manifest public key does not match signing key".into(),
            ));
        }
        self.validate_shape()?;
        self.signature = signing_key
            .sign(&self.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(())
    }

    pub fn verify(
        &self,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
        expected_signer_id: &str,
        minimum_replay_epoch: u64,
        minimum_owner_term: u64,
    ) -> Result<(), ReplaySecurityError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.signer_id != expected_signer_id {
            return Err(ReplaySecurityError::BindingRejected(
                "cluster or signer identity mismatch".into(),
            ));
        }
        if self.replay_epoch < minimum_replay_epoch || self.owner_term < minimum_owner_term {
            return Err(ReplaySecurityError::BindingRejected(
                "replay epoch or owner term is stale".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(ReplaySecurityError::BindingRejected(
                "manifest key is not the trusted key".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            ReplaySecurityError::SignatureRejected("signature length or encoding".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                ReplaySecurityError::SignatureRejected("Ed25519 verification failed".into())
            })
    }

    fn validate_shape(&self) -> Result<(), ReplaySecurityError> {
        validate_identifier(&self.scenario_id, "scenario")?;
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.signer_id, "signer")?;
        validate_nonce(&self.nonce)?;
        if self.replay_epoch == 0 || self.owner_term == 0 {
            return Err(ReplaySecurityError::InvalidManifest(
                "replay epoch and owner term must be positive".into(),
            ));
        }
        if self.max_events == 0 || self.max_events > MAX_REPLAY_EVENTS {
            return Err(ReplaySecurityError::InvalidManifest(
                "event bound is outside the safe range".into(),
            ));
        }
        if self.max_tick == 0 || self.max_tick > MAX_REPLAY_TICKS {
            return Err(ReplaySecurityError::InvalidManifest(
                "tick bound is outside the safe range".into(),
            ));
        }
        if self.schedule.len() > self.max_events {
            return Err(ReplaySecurityError::InvalidManifest(
                "schedule exceeds event bound".into(),
            ));
        }
        if digest_json(&self.schedule)? != self.schedule_digest {
            return Err(ReplaySecurityError::ScheduleHashMismatch);
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(ReplaySecurityError::SignatureRejected(
                "public key or signature length is invalid".into(),
            ));
        }
        let mut previous_sequence = 0;
        let mut previous_tick = 0;
        for step in &self.schedule {
            if step.sequence == 0 || step.sequence <= previous_sequence {
                return Err(ReplaySecurityError::SequenceTickViolation(
                    "schedule sequence must increase strictly".into(),
                ));
            }
            if step.tick < previous_tick || step.tick > self.max_tick {
                return Err(ReplaySecurityError::SequenceTickViolation(
                    "schedule ticks must be monotonic and bounded".into(),
                ));
            }
            validate_identifier(&step.from, "fault source")?;
            validate_identifier(&step.to, "fault destination")?;
            if step.from == step.to {
                return Err(ReplaySecurityError::InvalidManifest(
                    "fault endpoints must be distinct".into(),
                ));
            }
            previous_sequence = step.sequence;
            previous_tick = step.tick;
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReplaySecurityError> {
        serde_json::to_vec(&ReplayManifestPayload {
            scenario_id: &self.scenario_id,
            cluster_id: &self.cluster_id,
            signer_id: &self.signer_id,
            replay_epoch: self.replay_epoch,
            owner_term: self.owner_term,
            seed: self.seed,
            nonce: &self.nonce,
            schedule_digest: &self.schedule_digest,
            max_events: self.max_events,
            max_tick: self.max_tick,
            schedule: &self.schedule,
            public_key: &self.public_key,
        })
        .map_err(|error| ReplaySecurityError::InvalidManifest(error.to_string()))
    }
}

impl ReplayTraceSeal {
    pub fn sign_for(
        manifest: &ReplayManifest,
        simulator: &MultiRegionFailoverSimulator,
        signing_key: &SigningKey,
    ) -> Result<Self, ReplaySecurityError> {
        if manifest.public_key != signing_key.verifying_key().to_bytes() {
            return Err(ReplaySecurityError::BindingRejected(
                "trace seal key does not match manifest key".into(),
            ));
        }
        let mut seal = Self {
            scenario_id: manifest.scenario_id.clone(),
            cluster_id: manifest.cluster_id.clone(),
            signer_id: manifest.signer_id.clone(),
            replay_epoch: manifest.replay_epoch,
            event_digest: simulator.trace_digest(),
            event_count: simulator.events().len(),
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: Vec::new(),
        };
        seal.signature = signing_key
            .sign(&seal.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(seal)
    }

    pub fn verify(
        &self,
        manifest: &ReplayManifest,
        trusted_key: &VerifyingKey,
    ) -> Result<(), ReplaySecurityError> {
        self.verify_binding(manifest, trusted_key)
    }

    fn verify_binding(
        &self,
        manifest: &ReplayManifest,
        trusted_key: &VerifyingKey,
    ) -> Result<(), ReplaySecurityError> {
        if self.scenario_id != manifest.scenario_id
            || self.cluster_id != manifest.cluster_id
            || self.signer_id != manifest.signer_id
            || self.replay_epoch != manifest.replay_epoch
        {
            return Err(ReplaySecurityError::BindingRejected(
                "trace seal identity or epoch mismatch".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(ReplaySecurityError::BindingRejected(
                "trace seal key is not trusted".into(),
            ));
        }
        if self.signature.len() != 64 || self.public_key.len() != 32 {
            return Err(ReplaySecurityError::TraceSealRejected(
                "trace seal key or signature length is invalid".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| ReplaySecurityError::TraceSealRejected("trace seal encoding".into()))?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                ReplaySecurityError::TraceSealRejected("trace seal signature failed".into())
            })
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReplaySecurityError> {
        serde_json::to_vec(&ReplayTraceSealPayload {
            scenario_id: &self.scenario_id,
            cluster_id: &self.cluster_id,
            signer_id: &self.signer_id,
            replay_epoch: self.replay_epoch,
            event_digest: &self.event_digest,
            event_count: self.event_count,
            public_key: &self.public_key,
        })
        .map_err(|error| ReplaySecurityError::TraceSealRejected(error.to_string()))
    }
}

pub struct SecureReplayEngine;

impl SecureReplayEngine {
    pub fn prepare_trace_seal(
        simulator: &MultiRegionFailoverSimulator,
        manifest: &ReplayManifest,
        signing_key: &SigningKey,
    ) -> Result<ReplayTraceSeal, ReplaySecurityError> {
        let trusted_key = signing_key.verifying_key();
        manifest.verify(
            &trusted_key,
            &manifest.cluster_id,
            &manifest.signer_id,
            manifest.replay_epoch,
            manifest.owner_term,
        )?;
        let mut candidate = simulator.clone();
        apply_schedule(&mut candidate, manifest)?;
        ReplayTraceSeal::sign_for(manifest, &candidate, signing_key)
    }

    pub fn replay(
        simulator: &mut MultiRegionFailoverSimulator,
        manifest: &ReplayManifest,
        seal: &ReplayTraceSeal,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
        expected_signer_id: &str,
        minimum_replay_epoch: u64,
        minimum_owner_term: u64,
    ) -> Result<ReplayVerificationResult, ReplaySecurityError> {
        manifest.verify(
            trusted_key,
            expected_cluster_id,
            expected_signer_id,
            minimum_replay_epoch,
            minimum_owner_term,
        )?;
        seal.verify_binding(manifest, trusted_key)?;
        if manifest.scenario_id != simulator.report().scenario_id
            || manifest.seed != simulator.report().seed
        {
            return Err(ReplaySecurityError::BindingRejected(
                "manifest scenario or seed mismatch".into(),
            ));
        }
        let mut candidate = simulator.clone();
        apply_schedule(&mut candidate, manifest)?;
        let report = candidate.report();
        if seal.event_count != candidate.events().len()
            || seal.event_digest != candidate.trace_digest()
        {
            return Err(ReplaySecurityError::TraceSealRejected(
                "trace digest or event count mismatch".into(),
            ));
        }
        if !report.safety_passed {
            return Err(ReplaySecurityError::Simulation(
                "replayed simulator failed safety invariants".into(),
            ));
        }
        let result = ReplayVerificationResult {
            scenario_id: report.scenario_id,
            trace_digest: report.trace_digest,
            event_count: report.events,
            applied_steps: manifest.schedule.len(),
            safety_passed: report.safety_passed,
            liveness_passed: report.liveness_passed,
        };
        *simulator = candidate;
        Ok(result)
    }
}

fn apply_schedule(
    simulator: &mut MultiRegionFailoverSimulator,
    manifest: &ReplayManifest,
) -> Result<(), ReplaySecurityError> {
    for step in &manifest.schedule {
        if step.tick < simulator.current_tick() {
            return Err(ReplaySecurityError::SequenceTickViolation(
                "replay step is earlier than simulator tick".into(),
            ));
        }
        simulator
            .advance_ticks(step.tick - simulator.current_tick())
            .map_err(simulation_error)?;
        simulator
            .inject_link_fault(&step.from, &step.to, step.fault.clone())
            .map_err(simulation_error)?;
    }
    Ok(())
}

fn simulation_error(error: MultiRegionSimulationError) -> ReplaySecurityError {
    ReplaySecurityError::Simulation(error.to_string())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, ReplaySecurityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ReplaySecurityError::InvalidManifest(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ReplaySecurityError> {
    if value.trim().is_empty()
        || value.len() > MAX_REPLAY_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ReplaySecurityError::InvalidManifest(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_nonce(nonce: &str) -> Result<(), ReplaySecurityError> {
    if nonce.trim().is_empty()
        || nonce.len() > MAX_REPLAY_NONCE_BYTES
        || nonce.chars().any(char::is_control)
    {
        return Err(ReplaySecurityError::InvalidManifest(
            "nonce is empty, oversized, or contains control characters".into(),
        ));
    }
    Ok(())
}
