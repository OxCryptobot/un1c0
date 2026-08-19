use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 4096;
const MAX_TICKS: u64 = 1_000_000;
const MAX_DURABLE_RECOVERY_BYTES: u64 = 2 * 1024 * 1024;

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
    #[error("durable disaster-recovery snapshot failure: {0}")]
    DurableSnapshot(String),
    #[error("stale observer membership epoch: expected {expected}, observed {observed}")]
    StaleMembershipEpoch { expected: u64, observed: u64 },
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
        let config = Self {
            cluster_id: cluster_id.to_string(),
            quorum_size,
            max_failover_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), DisasterRecoveryError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        if self.quorum_size < 2
            || self.max_failover_ticks == 0
            || self.max_failover_ticks > MAX_TICKS
        {
            return Err(DisasterRecoveryError::InvalidInput(
                "quorum or failover tick bound is outside the safe range".into(),
            ));
        }
        Ok(())
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
    pub membership_epoch: u64,
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
    membership_epoch: u64,
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
        Self::sign_at_membership_epoch(
            cluster_id,
            1,
            region_id,
            observer_id,
            owner_term,
            ownership_epoch,
            observed_tick,
            snapshot_hash,
            reason,
            signing_key,
        )
    }

    pub fn sign_at_membership_epoch(
        cluster_id: &str,
        membership_epoch: u64,
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
            membership_epoch,
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
        if self.membership_epoch == 0
            || self.owner_term == 0
            || self.ownership_epoch == 0
            || self.observed_tick > MAX_TICKS
        {
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
            membership_epoch: self.membership_epoch,
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
    MembershipRotated,
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
    pub membership_epoch: u64,
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
    membership_epoch: u64,
    active_region_id: String,
    active_owner_term: u64,
    active_ownership_epoch: u64,
    active_snapshot_hash: String,
    phase: RecoveryPhase,
    failure_tick: Option<u64>,
    observations: BTreeMap<String, RegionFailureObservation>,
    pending_proposal: Option<FailoverProposal>,
    committed_proposal: Option<FailoverProposal>,
    events: Vec<RecoveryEvent>,
    next_event_sequence: u64,
    invariant_failures: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisasterRecoverySnapshot {
    pub config: DisasterRecoveryConfig,
    pub regions: BTreeMap<String, RegionStatus>,
    pub trusted_observers: BTreeMap<String, Vec<u8>>,
    pub membership_epoch: u64,
    pub active_region_id: String,
    pub active_owner_term: u64,
    pub active_ownership_epoch: u64,
    pub active_snapshot_hash: String,
    pub phase: RecoveryPhase,
    pub failure_tick: Option<u64>,
    pub observations: BTreeMap<String, RegionFailureObservation>,
    pub pending_proposal: Option<FailoverProposal>,
    pub committed_proposal: Option<FailoverProposal>,
    pub events: Vec<RecoveryEvent>,
    pub next_event_sequence: u64,
    pub invariant_failures: Vec<String>,
    pub state_hash: String,
}

impl DisasterRecoverySnapshot {
    fn content_bytes(&self) -> Result<Vec<u8>, DisasterRecoveryError> {
        serde_json::to_vec(&(
            &self.config,
            &self.regions,
            &self.trusted_observers,
            self.membership_epoch,
            &self.active_region_id,
            self.active_owner_term,
            self.active_ownership_epoch,
            &self.active_snapshot_hash,
            &self.phase,
            self.failure_tick,
            &self.observations,
            &self.pending_proposal,
            &self.committed_proposal,
            &self.events,
            self.next_event_sequence,
            &self.invariant_failures,
        ))
        .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))
    }

    fn computed_hash(&self) -> Result<String, DisasterRecoveryError> {
        let mut digest = Sha256::new();
        digest.update(self.content_bytes()?);
        Ok(format!("{:x}", digest.finalize()))
    }

    pub fn validate(&self) -> Result<(), DisasterRecoveryError> {
        self.config.clone().validate()?;
        validate_identifier(&self.active_region_id, "active region")?;
        validate_digest(&self.active_snapshot_hash)?;
        if self.membership_epoch == 0
            || self.active_owner_term == 0
            || self.active_ownership_epoch == 0
            || self.next_event_sequence
                < self.events.last().map(|event| event.sequence).unwrap_or(0)
        {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "snapshot counters are outside the safe range".into(),
            ));
        }
        let active_regions: Vec<_> = self
            .regions
            .values()
            .filter(|region| region.active)
            .collect();
        if active_regions.len() != 1
            || active_regions[0].region_id != self.active_region_id
            || active_regions[0].fenced
            || active_regions[0].snapshot_hash != self.active_snapshot_hash
        {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "snapshot active-region invariant is invalid".into(),
            ));
        }
        for (region_id, region) in &self.regions {
            validate_identifier(region_id, "region")?;
            if region_id != &region.region_id {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "region map key does not match region identity".into(),
                ));
            }
            validate_digest(&region.snapshot_hash)?;
        }
        for (observer_id, key_bytes) in &self.trusted_observers {
            validate_identifier(observer_id, "observer")?;
            if key_bytes.len() != 32 {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "trusted observer key length is invalid".into(),
                ));
            }
            VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                DisasterRecoveryError::DurableSnapshot("trusted observer key length".into())
            })?)
            .map_err(|_| {
                DisasterRecoveryError::DurableSnapshot("trusted observer key encoding".into())
            })?;
        }
        for (observer_id, observation) in &self.observations {
            let current_cycle_binding = observation.region_id == self.active_region_id
                && observation.owner_term == self.active_owner_term
                && observation.ownership_epoch == self.active_ownership_epoch;
            let committed_history_binding = self
                .committed_proposal
                .as_ref()
                .map(|proposal| {
                    observation.region_id == proposal.previous_region_id
                        && observation.owner_term < self.active_owner_term
                        && observation.ownership_epoch < self.active_ownership_epoch
                })
                .unwrap_or(false);
            if observer_id != &observation.observer_id
                || observation.membership_epoch != self.membership_epoch
                || observation.cluster_id != self.config.cluster_id
                || (!current_cycle_binding && !committed_history_binding)
                || observation.snapshot_hash != self.active_snapshot_hash
            {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "observation is not bound to the restored membership".into(),
                ));
            }
            let key_bytes = self.trusted_observers.get(observer_id).ok_or_else(|| {
                DisasterRecoveryError::DurableSnapshot(
                    "observation references unknown observer".into(),
                )
            })?;
            let key = VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                DisasterRecoveryError::DurableSnapshot("trusted observer key length".into())
            })?)
            .map_err(|_| {
                DisasterRecoveryError::DurableSnapshot("trusted observer key encoding".into())
            })?;
            observation.verify(&key, &self.config.cluster_id, observer_id)?;
        }
        if let Some(proposal) = &self.pending_proposal {
            validate_proposal(proposal)?;
            if proposal.previous_region_id != self.active_region_id
                || proposal.owner_term <= self.active_owner_term
                || proposal.ownership_epoch <= self.active_ownership_epoch
                || proposal.snapshot_hash != self.active_snapshot_hash
            {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "pending proposal is not bound to active state".into(),
                ));
            }
        }
        if let Some(proposal) = &self.committed_proposal {
            validate_proposal(proposal)?;
            if proposal.candidate_region_id != self.active_region_id
                || proposal.owner_term != self.active_owner_term
                || proposal.ownership_epoch != self.active_ownership_epoch
                || proposal.snapshot_hash != self.active_snapshot_hash
            {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "committed proposal is not bound to active state".into(),
                ));
            }
        }
        if matches!(self.phase, RecoveryPhase::PromotionPrepared) != self.pending_proposal.is_some()
            || matches!(self.phase, RecoveryPhase::Committed) != self.committed_proposal.is_some()
        {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "snapshot phase does not match proposal authority".into(),
            ));
        }
        let mut previous_sequence = 0;
        for event in &self.events {
            if event.sequence == 0 || event.sequence <= previous_sequence {
                return Err(DisasterRecoveryError::DurableSnapshot(
                    "event sequence is not strictly monotonic".into(),
                ));
            }
            previous_sequence = event.sequence;
        }
        if self.computed_hash()? != self.state_hash {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "snapshot state hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DisasterRecoverySnapshotStore {
    path: PathBuf,
}

impl DisasterRecoverySnapshotStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn staging_path(&self) -> PathBuf {
        self.path.with_extension("recovery.tmp")
    }

    pub fn save(&self, snapshot: &DisasterRecoverySnapshot) -> Result<(), DisasterRecoveryError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        if bytes.len() as u64 > MAX_DURABLE_RECOVERY_BYTES {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "recovery snapshot exceeds the configured byte bound".into(),
            ));
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        let temporary = self.staging_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn recover_staging(&self) -> Result<bool, DisasterRecoveryError> {
        let temporary = self.staging_path();
        if !temporary.exists() {
            return Ok(false);
        }
        fs::remove_file(&temporary)
            .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        Ok(true)
    }

    pub fn load(&self) -> Result<DisasterRecoverySnapshot, DisasterRecoveryError> {
        let metadata = fs::metadata(&self.path)
            .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        if metadata.len() > MAX_DURABLE_RECOVERY_BYTES {
            return Err(DisasterRecoveryError::DurableSnapshot(
                "recovery snapshot exceeds the configured byte bound".into(),
            ));
        }
        let snapshot: DisasterRecoverySnapshot = serde_json::from_slice(
            &fs::read(&self.path)
                .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?,
        )
        .map_err(|error| DisasterRecoveryError::DurableSnapshot(error.to_string()))?;
        snapshot.validate()?;
        Ok(snapshot)
    }
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
            membership_epoch: 1,
            active_region_id: active_region_id.to_string(),
            active_owner_term: owner_term,
            active_ownership_epoch: ownership_epoch,
            active_snapshot_hash: active_snapshot_hash.to_string(),
            phase: RecoveryPhase::Stable,
            failure_tick: None,
            observations: BTreeMap::new(),
            pending_proposal: None,
            committed_proposal: None,
            events: Vec::new(),
            next_event_sequence: 0,
            invariant_failures: Vec::new(),
        })
    }

    pub fn snapshot(&self) -> Result<DisasterRecoverySnapshot, DisasterRecoveryError> {
        let mut snapshot = DisasterRecoverySnapshot {
            config: self.config.clone(),
            regions: self.regions.clone(),
            trusted_observers: self.trusted_observers.clone(),
            membership_epoch: self.membership_epoch,
            active_region_id: self.active_region_id.clone(),
            active_owner_term: self.active_owner_term,
            active_ownership_epoch: self.active_ownership_epoch,
            active_snapshot_hash: self.active_snapshot_hash.clone(),
            phase: self.phase.clone(),
            failure_tick: self.failure_tick,
            observations: self.observations.clone(),
            pending_proposal: self.pending_proposal.clone(),
            committed_proposal: self.committed_proposal.clone(),
            events: self.events.clone(),
            next_event_sequence: self.next_event_sequence,
            invariant_failures: self.invariant_failures.clone(),
            state_hash: String::new(),
        };
        snapshot.state_hash = snapshot.computed_hash()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn from_snapshot(
        snapshot: DisasterRecoverySnapshot,
    ) -> Result<Self, DisasterRecoveryError> {
        snapshot.validate()?;
        let controller = Self {
            config: snapshot.config,
            regions: snapshot.regions,
            trusted_observers: snapshot.trusted_observers,
            membership_epoch: snapshot.membership_epoch,
            active_region_id: snapshot.active_region_id,
            active_owner_term: snapshot.active_owner_term,
            active_ownership_epoch: snapshot.active_ownership_epoch,
            active_snapshot_hash: snapshot.active_snapshot_hash,
            phase: snapshot.phase,
            failure_tick: snapshot.failure_tick,
            observations: snapshot.observations,
            pending_proposal: snapshot.pending_proposal,
            committed_proposal: snapshot.committed_proposal,
            events: snapshot.events,
            next_event_sequence: snapshot.next_event_sequence,
            invariant_failures: snapshot.invariant_failures,
        };
        Ok(controller)
    }

    pub fn restore_snapshot(
        &mut self,
        snapshot: DisasterRecoverySnapshot,
    ) -> Result<(), DisasterRecoveryError> {
        let restored = Self::from_snapshot(snapshot)?;
        *self = restored;
        Ok(())
    }

    pub fn save_snapshot(
        &self,
        store: &DisasterRecoverySnapshotStore,
    ) -> Result<(), DisasterRecoveryError> {
        store.save(&self.snapshot()?)
    }

    pub fn load_snapshot(
        store: &DisasterRecoverySnapshotStore,
    ) -> Result<Self, DisasterRecoveryError> {
        Self::from_snapshot(store.load()?)
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

    pub fn membership_epoch(&self) -> u64 {
        self.membership_epoch
    }

    pub fn rotate_observer_membership(
        &mut self,
        membership_epoch: u64,
        observers: BTreeMap<String, Vec<u8>>,
    ) -> Result<(), DisasterRecoveryError> {
        if membership_epoch <= self.membership_epoch {
            return Err(DisasterRecoveryError::StaleMembershipEpoch {
                expected: self.membership_epoch + 1,
                observed: membership_epoch,
            });
        }
        if matches!(
            self.phase,
            RecoveryPhase::PromotionPrepared | RecoveryPhase::Committed
        ) {
            return Err(DisasterRecoveryError::StaleProposal(
                "observer membership cannot rotate during prepared or committed recovery".into(),
            ));
        }
        if observers.len() < self.config.required_observers() {
            return Err(DisasterRecoveryError::InvalidInput(
                "observer membership is smaller than the configured quorum requirement".into(),
            ));
        }
        for (observer_id, key_bytes) in &observers {
            validate_identifier(observer_id, "observer")?;
            if key_bytes.len() != 32 {
                return Err(DisasterRecoveryError::BindingRejected(
                    "observer membership key length is invalid".into(),
                ));
            }
            VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                DisasterRecoveryError::BindingRejected("observer membership key length".into())
            })?)
            .map_err(|_| {
                DisasterRecoveryError::BindingRejected("observer membership key encoding".into())
            })?;
        }
        self.membership_epoch = membership_epoch;
        self.trusted_observers = observers;
        self.observations.clear();
        self.pending_proposal = None;
        self.phase = if self.failure_tick.is_some() {
            RecoveryPhase::DetectingFailure
        } else {
            RecoveryPhase::Stable
        };
        self.record_event(
            RecoveryEventKind::MembershipRotated,
            &self.active_region_id.clone(),
            None,
            "observer membership epoch rotated",
        );
        self.assert_invariants();
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
        if matches!(
            self.phase,
            RecoveryPhase::PromotionPrepared | RecoveryPhase::Committed
        ) {
            return Err(DisasterRecoveryError::StaleProposal(
                "recovery cycle already has a prepared or committed promotion".into(),
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
        if observation.membership_epoch != self.membership_epoch {
            return Err(DisasterRecoveryError::StaleMembershipEpoch {
                expected: self.membership_epoch,
                observed: observation.membership_epoch,
            });
        }
        if observation.region_id != self.active_region_id
            || observation.owner_term != self.active_owner_term
            || observation.ownership_epoch != self.active_ownership_epoch
            || observation.snapshot_hash != self.active_snapshot_hash
        {
            return Err(DisasterRecoveryError::BindingRejected(
                "failure observation does not bind to active state".into(),
            ));
        }
        if self.failure_tick.is_none() {
            return Err(DisasterRecoveryError::BindingRejected(
                "local failure detection has not been recorded".into(),
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
        self.phase = if self.pending_proposal.is_some() {
            RecoveryPhase::PromotionPrepared
        } else if self.observations.len() >= self.config.required_observers() {
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
            let committed = self.committed_proposal.as_ref().ok_or_else(|| {
                DisasterRecoveryError::InvariantViolation(
                    "committed phase is missing its proposal identity".into(),
                )
            })?;
            if committed.owner_term == owner_term
                && committed.ownership_epoch == ownership_epoch
                && committed.snapshot_hash == snapshot_hash
            {
                return Ok(FailoverAction::AlreadyCommitted(committed.clone()));
            }
            return Err(DisasterRecoveryError::StaleProposal(
                "committed promotion replay does not match its proposal identity".into(),
            ));
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
        if let Some(existing) = &self.pending_proposal {
            if existing != &proposal {
                return Err(DisasterRecoveryError::StaleProposal(
                    "a conflicting promotion is already prepared".into(),
                ));
            }
            return Ok(FailoverAction::Promote(proposal));
        }
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
        if self.phase == RecoveryPhase::Committed {
            if self.committed_proposal.as_ref() == Some(&proposal) {
                return Ok(FailoverAction::AlreadyCommitted(proposal));
            }
            return Err(DisasterRecoveryError::StaleProposal(
                "committed promotion replay does not match its proposal identity".into(),
            ));
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
        self.committed_proposal = Some(proposal.clone());
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
            membership_epoch: self.membership_epoch,
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

fn validate_proposal(proposal: &FailoverProposal) -> Result<(), DisasterRecoveryError> {
    validate_identifier(&proposal.previous_region_id, "previous region")?;
    validate_identifier(&proposal.candidate_region_id, "candidate region")?;
    validate_digest(&proposal.snapshot_hash)?;
    if proposal.owner_term == 0 || proposal.ownership_epoch == 0 {
        return Err(DisasterRecoveryError::DurableSnapshot(
            "proposal term and epoch must be positive".into(),
        ));
    }
    if proposal.previous_region_id == proposal.candidate_region_id {
        return Err(DisasterRecoveryError::DurableSnapshot(
            "proposal must change region".into(),
        ));
    }
    Ok(())
}
