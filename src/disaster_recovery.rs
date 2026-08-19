use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 4096;
const MAX_TICKS: u64 = 1_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DisasterRecoveryError {
    #[error("invalid disaster-recovery input: {0}")]
    InvalidInput(String),
    #[error("failure observation signature rejected: {0}")]
    SignatureRejected(String),
    #[error("failure observation binding rejected: {0}")]
    BindingRejected(String),
    #[error("disaster-recovery quorum unavailable: observed {observed}, required {required}")]
    QuorumUnavailable { observed: usize, required: usize },
    #[error("stale or invalid failover proposal: {0}")]
    StaleProposal(String),
    #[error("snapshot hash mismatch")]
    SnapshotHashMismatch,
    #[error("disaster-recovery invariant violated: {0}")]
    InvariantViolation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisasterRecoveryConfig {
    pub cluster_id: String,
    pub quorum_size: usize,
    pub max_failover_ticks: u64,
}

impl DisasterRecoveryConfig {
    pub fn new(
        cluster_id: &str,
        quorum_size: usize,
        max_failover_ticks: u64,
    ) -> Result<Self, DisasterRecoveryError> {
        validate_identifier(cluster_id, "cluster")?;
        if quorum_size < 2 || max_failover_ticks == 0 || max_failover_ticks > MAX_TICKS {
            return Err(DisasterRecoveryError::InvalidInput(
                "quorum or failover tick bound is outside the safe range".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            quorum_size,
            max_failover_ticks,
        })
    }

    fn required_observers(&self) -> usize {
        self.quorum_size.saturating_sub(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionStatus {
    pub region_id: String,
    pub snapshot_hash: String,
    pub healthy: bool,
    pub active: bool,
    pub fenced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionFailureObservation {
    pub cluster_id: String,
    pub region_id: String,
    pub observer_id: String,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub observed_tick: u64,
    pub snapshot_hash: String,
    pub reason: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct RegionFailurePayload<'a> {
    cluster_id: &'a str,
    region_id: &'a str,
    observer_id: &'a str,
    owner_term: u64,
    ownership_epoch: u64,
    observed_tick: u64,
    snapshot_hash: &'a str,
    reason: &'a str,
    public_key: &'a [u8],
}

impl RegionFailureObservation {
    pub fn sign(
        cluster_id: &str,
        region_id: &str,
        observer_id: &str,
        owner_term: u64,
        ownership_epoch: u64,
        observed_tick: u64,
        snapshot_hash: &str,
        reason: &str,
        signing_key: &SigningKey,
    ) -> Result<Self, DisasterRecoveryError> {
        let mut observation = Self {
            cluster_id: cluster_id.to_string(),
            region_id: region_id.to_string(),
            observer_id: observer_id.to_string(),
            owner_term,
            ownership_epoch,
            observed_tick,
            snapshot_hash: snapshot_hash.to_string(),
            reason: reason.to_string(),
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
        };
        observation.validate_shape()?;
        observation.signature = signing_key
            .sign(&observation.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(observation)
    }

    pub fn verify(
        &self,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
        expected_observer_id: &str,
    ) -> Result<(), DisasterRecoveryError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.observer_id != expected_observer_id {
            return Err(DisasterRecoveryError::BindingRejected(
                "cluster or observer identity mismatch".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(DisasterRecoveryError::BindingRejected(
                "observation public key is not trusted".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            DisasterRecoveryError::SignatureRejected("signature encoding or length".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                DisasterRecoveryError::SignatureRejected("Ed25519 verification failed".into())
            })
    }

    fn validate_shape(&self) -> Result<(), DisasterRecoveryError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.region_id, "region")?;
        validate_identifier(&self.observer_id, "observer")?;
        validate_digest(&self.snapshot_hash)?;
        if self.owner_term == 0 || self.ownership_epoch == 0 || self.observed_tick > MAX_TICKS {
            return Err(DisasterRecoveryError::InvalidInput(
                "observation term, epoch, or tick is outside bounds".into(),
            ));
        }
        if self.reason.is_empty()
            || self.reason.len() > MAX_REASON_BYTES
            || self.reason.chars().any(char::is_control)
        {
            return Err(DisasterRecoveryError::InvalidInput(
                "observation reason is empty, oversized, or contains control characters".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(DisasterRecoveryError::SignatureRejected(
                "public key or signature length is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, DisasterRecoveryError> {
        serde_json::to_vec(&RegionFailurePayload {
            cluster_id: &self.cluster_id,
            region_id: &self.region_id,
            observer_id: &self.observer_id,
            owner_term: self.owner_term,
            ownership_epoch: self.ownership_epoch,
            observed_tick: self.observed_tick,
            snapshot_hash: &self.snapshot_hash,
            reason: &self.reason,
            public_key: &self.public_key,
        })
        .map_err(|error| DisasterRecoveryError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailoverProposal {
    pub previous_region_id: String,
    pub candidate_region_id: String,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub snapshot_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecoveryPhase {
    Stable,
    DetectingFailure,
    AwaitingObserverQuorum,
    PromotionPrepared,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailoverAction {
    AwaitingQuorum { observed: usize, required: usize },
    Promote(FailoverProposal),
    AlreadyCommitted(FailoverProposal),
    Committed(FailoverProposal),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecoveryEventKind {
    RegionRegistered,
    FailureDetected,
    ObservationAccepted,
    PromotionPrepared,
    PromotionCommitted,
    IdempotentReplay,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryEvent {
    pub sequence: u64,
    pub kind: RecoveryEventKind,
    pub region_id: String,
    pub observer_id: Option<String>,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisasterRecoveryReport {
    pub cluster_id: String,
    pub active_region_id: String,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub phase: RecoveryPhase,
    pub observer_count: usize,
    pub required_observers: usize,
    pub safety_passed: bool,
    pub trace_digest: String,
    pub events: usize,
}

pub struct DisasterRecoveryController {
    config: DisasterRecoveryConfig,
    regions: BTreeMap<String, RegionStatus>,
    trusted_observers: BTreeMap<String, Vec<u8>>,
    active_region_id: String,
    active_owner_term: u64,
    active_ownership_epoch: u64,
    active_snapshot_hash: String,
    phase: RecoveryPhase,
    failure_tick: Option<u64>,
    observations: BTreeMap<String, RegionFailureObservation>,
    pending_proposal: Option<FailoverProposal>,
    events: Vec<RecoveryEvent>,
    next_event_sequence: u64,
    invariant_failures: Vec<String>,
}

impl DisasterRecoveryController {
    pub fn new(
        config: DisasterRecoveryConfig,
        active_region_id: &str,
        active_snapshot_hash: &str,
        owner_term: u64,
        ownership_epoch: u64,
    ) -> Result<Self, DisasterRecoveryError> {
        validate_identifier(active_region_id, "active region")?;
        validate_digest(active_snapshot_hash)?;
        if owner_term == 0 || ownership_epoch == 0 {
            return Err(DisasterRecoveryError::InvalidInput(
                "active term and epoch must be positive".into(),
            ));
        }
        Ok(Self {
            config,
            regions: BTreeMap::new(),
            trusted_observers: BTreeMap::new(),
            active_region_id: active_region_id.to_string(),
            active_owner_term: owner_term,
            active_ownership_epoch: ownership_epoch,
            active_snapshot_hash: active_snapshot_hash.to_string(),
            phase: RecoveryPhase::Stable,
            failure_tick: None,
            observations: BTreeMap::new(),
            pending_proposal: None,
            events: Vec::new(),
            next_event_sequence: 0,
            invariant_failures: Vec::new(),
        })
    }

    pub fn register_region(
        &mut self,
        region_id: &str,
        snapshot_hash: &str,
        healthy: bool,
    ) -> Result<(), DisasterRecoveryError> {
        validate_identifier(region_id, "region")?;
        validate_digest(snapshot_hash)?;
        if self.regions.contains_key(region_id) {
            return Err(DisasterRecoveryError::InvalidInput(
                "region is already registered".into(),
            ));
        }
        self.regions.insert(
            region_id.to_string(),
            RegionStatus {
                region_id: region_id.to_string(),
                snapshot_hash: snapshot_hash.to_string(),
                healthy,
                active: region_id == self.active_region_id,
                fenced: false,
            },
        );
        self.record_event(
            RecoveryEventKind::RegionRegistered,
            region_id,
            None,
            "region registered",
        );
        self.assert_invariants();
        Ok(())
    }

    pub fn register_trusted_observer(
        &mut self,
        observer_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), DisasterRecoveryError> {
        validate_identifier(observer_id, "observer")?;
        if self.trusted_observers.contains_key(observer_id) {
            return Err(DisasterRecoveryError::InvalidInput(
                "observer key is already registered".into(),
            ));
        }
        self.trusted_observers
            .insert(observer_id.to_string(), verifying_key.to_bytes().to_vec());
        Ok(())
    }

    pub fn record_region_failure(
        &mut self,
        region_id: &str,
        observed_tick: u64,
        reason: &str,
    ) -> Result<(), DisasterRecoveryError> {
        if region_id != self.active_region_id {
            return Err(DisasterRecoveryError::BindingRejected(
                "only the active region can enter failure detection".into(),
            ));
        }
        if observed_tick > self.config.max_failover_ticks {
            return Err(DisasterRecoveryError::InvalidInput(
                "failure tick exceeds failover bound".into(),
            ));
        }
        if reason.is_empty()
            || reason.len() > MAX_REASON_BYTES
            || reason.chars().any(char::is_control)
        {
            return Err(DisasterRecoveryError::InvalidInput(
                "failure reason is empty, oversized, or contains control characters".into(),
            ));
        }
        self.failure_tick = Some(observed_tick);
        self.phase = RecoveryPhase::DetectingFailure;
        self.record_event(RecoveryEventKind::FailureDetected, region_id, None, reason);
        self.assert_invariants();
        Ok(())
    }

    pub fn ingest_failure_observation(
        &mut self,
        observation: RegionFailureObservation,
    ) -> Result<bool, DisasterRecoveryError> {
        let key_bytes = self
            .trusted_observers
            .get(&observation.observer_id)
            .ok_or_else(|| DisasterRecoveryError::BindingRejected("unknown observer".into()))?;
        let trusted_key =
            VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                DisasterRecoveryError::BindingRejected("trusted key length".into())
            })?)
            .map_err(|_| DisasterRecoveryError::BindingRejected("trusted key encoding".into()))?;
        observation.verify(
            &trusted_key,
            &self.config.cluster_id,
            &observation.observer_id,
        )?;
        if observation.region_id != self.active_region_id
            || observation.owner_term != self.active_owner_term
            || observation.ownership_epoch != self.active_ownership_epoch
            || observation.snapshot_hash != self.active_snapshot_hash
        {
            return Err(DisasterRecoveryError::BindingRejected(
                "failure observation does not bind to active state".into(),
            ));
        }
        if observation.observer_id == observation.region_id {
            return Err(DisasterRecoveryError::BindingRejected(
                "failed region cannot act as its own observer".into(),
            ));
        }
        if let Some(existing) = self.observations.get(&observation.observer_id) {
            if existing == &observation {
                self.record_event(
                    RecoveryEventKind::IdempotentReplay,
                    &observation.region_id,
                    Some(observation.observer_id.clone()),
                    "identical failure observation replayed",
                );
                return Ok(self.observations.len() >= self.config.required_observers());
            }
            return Err(DisasterRecoveryError::InvariantViolation(
                "observer submitted conflicting failure evidence".into(),
            ));
        }
        self.observations
            .insert(observation.observer_id.clone(), observation.clone());
        self.phase = if self.observations.len() >= self.config.required_observers() {
            RecoveryPhase::AwaitingObserverQuorum
        } else {
            RecoveryPhase::DetectingFailure
        };
        self.record_event(
            RecoveryEventKind::ObservationAccepted,
            &observation.region_id,
            Some(observation.observer_id),
            "signed failure observation accepted",
        );
        self.assert_invariants();
        Ok(self.observations.len() >= self.config.required_observers())
    }

    pub fn prepare_promotion(
        &mut self,
        candidate_region_id: &str,
        owner_term: u64,
        ownership_epoch: u64,
        snapshot_hash: &str,
    ) -> Result<FailoverAction, DisasterRecoveryError> {
        validate_identifier(candidate_region_id, "candidate region")?;
        validate_digest(snapshot_hash)?;
        if self.phase == RecoveryPhase::Committed && candidate_region_id == self.active_region_id {
            return Ok(FailoverAction::AlreadyCommitted(FailoverProposal {
                previous_region_id: candidate_region_id.to_string(),
                candidate_region_id: candidate_region_id.to_string(),
                owner_term: self.active_owner_term,
                ownership_epoch: self.active_ownership_epoch,
                snapshot_hash: self.active_snapshot_hash.clone(),
            }));
        }
        let required = self.config.required_observers();
        if self.observations.len() < required {
            self.phase = RecoveryPhase::AwaitingObserverQuorum;
            return Ok(FailoverAction::AwaitingQuorum {
                observed: self.observations.len(),
                required,
            });
        }
        let candidate = self.regions.get(candidate_region_id).ok_or_else(|| {
            DisasterRecoveryError::InvalidInput("candidate region is unknown".into())
        })?;
        if !candidate.healthy || candidate.fenced || candidate.active {
            return Err(DisasterRecoveryError::StaleProposal(
                "candidate region is unhealthy, fenced, or already active".into(),
            ));
        }
        if owner_term <= self.active_owner_term || ownership_epoch <= self.active_ownership_epoch {
            return Err(DisasterRecoveryError::StaleProposal(
                "promotion term and epoch must both increase".into(),
            ));
        }
        if snapshot_hash != self.active_snapshot_hash || snapshot_hash != candidate.snapshot_hash {
            return Err(DisasterRecoveryError::SnapshotHashMismatch);
        }
        let proposal = FailoverProposal {
            previous_region_id: self.active_region_id.clone(),
            candidate_region_id: candidate_region_id.to_string(),
            owner_term,
            ownership_epoch,
            snapshot_hash: snapshot_hash.to_string(),
        };
        self.pending_proposal = Some(proposal.clone());
        self.phase = RecoveryPhase::PromotionPrepared;
        self.record_event(
            RecoveryEventKind::PromotionPrepared,
            candidate_region_id,
            None,
            "higher-term promotion prepared after observer quorum",
        );
        self.assert_invariants();
        Ok(FailoverAction::Promote(proposal))
    }

    pub fn commit_promotion(
        &mut self,
        proposal: FailoverProposal,
    ) -> Result<FailoverAction, DisasterRecoveryError> {
        if self.phase == RecoveryPhase::Committed
            && proposal.candidate_region_id == self.active_region_id
            && proposal.owner_term == self.active_owner_term
            && proposal.ownership_epoch == self.active_ownership_epoch
        {
            return Ok(FailoverAction::AlreadyCommitted(proposal));
        }
        if self.pending_proposal.as_ref() != Some(&proposal) {
            return Err(DisasterRecoveryError::StaleProposal(
                "promotion was not prepared from the current evidence".into(),
            ));
        }
        if proposal.owner_term <= self.active_owner_term
            || proposal.ownership_epoch <= self.active_ownership_epoch
        {
            return Err(DisasterRecoveryError::StaleProposal(
                "promotion would decrease term or epoch".into(),
            ));
        }
        if proposal.snapshot_hash != self.active_snapshot_hash {
            return Err(DisasterRecoveryError::SnapshotHashMismatch);
        }
        let previous = self
            .regions
            .get_mut(&proposal.previous_region_id)
            .ok_or_else(|| {
                DisasterRecoveryError::InvalidInput("previous region is unknown".into())
            })?;
        previous.active = false;
        previous.healthy = false;
        previous.fenced = true;
        let candidate = self
            .regions
            .get_mut(&proposal.candidate_region_id)
            .ok_or_else(|| {
                DisasterRecoveryError::InvalidInput("candidate region is unknown".into())
            })?;
        candidate.active = true;
        candidate.fenced = false;
        self.active_region_id = proposal.candidate_region_id.clone();
        self.active_owner_term = proposal.owner_term;
        self.active_ownership_epoch = proposal.ownership_epoch;
        self.active_snapshot_hash = proposal.snapshot_hash.clone();
        self.pending_proposal = None;
        self.phase = RecoveryPhase::Committed;
        self.record_event(
            RecoveryEventKind::PromotionCommitted,
            &proposal.candidate_region_id,
            None,
            "automated consensus failover committed",
        );
        self.assert_invariants();
        Ok(FailoverAction::Committed(proposal))
    }

    pub fn report(&self) -> DisasterRecoveryReport {
        let active_count = self.regions.values().filter(|region| region.active).count();
        let safety_passed = self.invariant_failures.is_empty()
            && active_count <= 1
            && self.active_owner_term > 0
            && self.active_ownership_epoch > 0
            && self
                .regions
                .get(&self.active_region_id)
                .map(|region| region.active && !region.fenced)
                .unwrap_or(false);
        DisasterRecoveryReport {
            cluster_id: self.config.cluster_id.clone(),
            active_region_id: self.active_region_id.clone(),
            owner_term: self.active_owner_term,
            ownership_epoch: self.active_ownership_epoch,
            phase: self.phase.clone(),
            observer_count: self.observations.len(),
            required_observers: self.config.required_observers(),
            safety_passed,
            trace_digest: self.trace_digest(),
            events: self.events.len(),
        }
    }

    pub fn region(&self, region_id: &str) -> Option<&RegionStatus> {
        self.regions.get(region_id)
    }

    pub fn events(&self) -> &[RecoveryEvent] {
        &self.events
    }

    pub fn trace_digest(&self) -> String {
        let bytes = serde_json::to_vec(&self.events).unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }

    fn record_event(
        &mut self,
        kind: RecoveryEventKind,
        region_id: &str,
        observer_id: Option<String>,
        detail: &str,
    ) {
        self.next_event_sequence = self.next_event_sequence.saturating_add(1);
        self.events.push(RecoveryEvent {
            sequence: self.next_event_sequence,
            kind,
            region_id: region_id.to_string(),
            observer_id,
            owner_term: self.active_owner_term,
            ownership_epoch: self.active_ownership_epoch,
            detail: detail.to_string(),
        });
    }

    fn assert_invariants(&mut self) {
        let active_count = self.regions.values().filter(|region| region.active).count();
        if active_count > 1 {
            self.invariant_failures
                .push("more than one active region".into());
        }
        if self
            .regions
            .values()
            .any(|region| region.fenced && region.active)
        {
            self.invariant_failures
                .push("fenced region remains active".into());
        }
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<(), DisasterRecoveryError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(DisasterRecoveryError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), DisasterRecoveryError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DisasterRecoveryError::InvalidInput(
            "snapshot hash must be a 64-character hexadecimal digest".into(),
        ));
    }
    Ok(())
}
