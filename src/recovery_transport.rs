use crate::replicated_recovery::{
    ExternalFenceState, ReplicatedRecoveryError, TrustedFencingAuthorityRegistry,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const AUTHENTICATED_TRANSPORT_DOMAIN: &str = "un1c0/recovery-transport/v1";
pub const WITNESS_RESERVATION_DOMAIN: &str = "un1c0/witness-reservation/v1";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_REPLAY_ENTRIES: usize = 4096;
const MAX_RESERVATIONS: usize = 4096;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecoveryTransportError {
    #[error("invalid recovery transport input: {0}")]
    InvalidInput(String),
    #[error("unknown transport sender: {0}")]
    UnknownSender(String),
    #[error("transport envelope rejected: {0}")]
    EnvelopeRejected(String),
    #[error("transport replay rejected: {0}")]
    ReplayRejected(String),
    #[error("reservation rejected: {0}")]
    ReservationRejected(String),
    #[error("reservation persistence failed: {0}")]
    PersistenceFailed(String),
    #[error("protected write rejected: {0}")]
    ProtectedWriteRejected(String),
    #[error("replicated recovery error: {0}")]
    Replicated(ReplicatedRecoveryError),
}

impl From<ReplicatedRecoveryError> for RecoveryTransportError {
    fn from(value: ReplicatedRecoveryError) -> Self {
        Self::Replicated(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportKeyRegistry {
    keys: BTreeMap<String, Vec<u8>>,
}

impl TransportKeyRegistry {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        sender_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), RecoveryTransportError> {
        validate_identifier(sender_id, "sender")?;
        let key = verifying_key.to_bytes().to_vec();
        if let Some(existing) = self.keys.get(sender_id) {
            if existing != &key {
                return Err(RecoveryTransportError::EnvelopeRejected(
                    "transport sender key rebinding is not allowed".into(),
                ));
            }
            return Ok(());
        }
        self.keys.insert(sender_id.to_string(), key);
        Ok(())
    }

    pub fn key_for(&self, sender_id: &str) -> Result<VerifyingKey, RecoveryTransportError> {
        let bytes = self
            .keys
            .get(sender_id)
            .ok_or_else(|| RecoveryTransportError::UnknownSender(sender_id.to_string()))?;
        let key_bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            RecoveryTransportError::EnvelopeRejected("transport sender key length".into())
        })?;
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| {
            RecoveryTransportError::EnvelopeRejected("transport sender key encoding".into())
        })
    }

    pub fn contains(&self, sender_id: &str) -> bool {
        self.keys.contains_key(sender_id)
    }
}

impl Default for TransportKeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportMessageKind {
    LeaderProposal,
    WitnessVote,
    WitnessReservation,
    ExternalFenceAdmission,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedTransportEnvelope {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub sender_id: String,
    pub receiver_id: String,
    pub connection_epoch: u64,
    pub sequence: u64,
    pub nonce: String,
    pub kind: TransportMessageKind,
    pub payload_hash: String,
    pub payload: Vec<u8>,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct TransportEnvelopePayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    sender_id: &'a str,
    receiver_id: &'a str,
    connection_epoch: u64,
    sequence: u64,
    nonce: &'a str,
    kind: &'a TransportMessageKind,
    payload_hash: &'a str,
    public_key: &'a [u8],
}

impl AuthenticatedTransportEnvelope {
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        sender_id: &str,
        receiver_id: &str,
        connection_epoch: u64,
        sequence: u64,
        nonce: &str,
        kind: TransportMessageKind,
        payload: Vec<u8>,
        signing_key: &SigningKey,
    ) -> Result<Self, RecoveryTransportError> {
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(RecoveryTransportError::InvalidInput(
                "transport payload exceeds bound".into(),
            ));
        }
        let mut envelope = Self {
            domain: AUTHENTICATED_TRANSPORT_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            sender_id: sender_id.to_string(),
            receiver_id: receiver_id.to_string(),
            connection_epoch,
            sequence,
            nonce: nonce.to_string(),
            kind,
            payload_hash: digest_bytes(&payload),
            payload,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
        };
        envelope.validate_shape()?;
        envelope.signature = signing_key
            .sign(&envelope.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(envelope)
    }

    pub fn verify(
        &self,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
        expected_resource_id: &str,
        expected_receiver_id: &str,
    ) -> Result<(), RecoveryTransportError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id
            || self.resource_id != expected_resource_id
            || self.receiver_id != expected_receiver_id
        {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "transport cluster, resource, or receiver binding mismatch".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "transport signer key is not trusted".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            RecoveryTransportError::EnvelopeRejected("transport signature encoding".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| RecoveryTransportError::EnvelopeRejected("transport signature".into()))?;
        if digest_bytes(&self.payload) != self.payload_hash {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "transport payload hash mismatch".into(),
            ));
        }
        Ok(())
    }

    pub fn envelope_hash(&self) -> String {
        digest_json(self).unwrap_or_default()
    }

    fn validate_shape(&self) -> Result<(), RecoveryTransportError> {
        if self.domain != AUTHENTICATED_TRANSPORT_DOMAIN || self.protocol_version != 1 {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "transport domain or protocol version is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.sender_id, "sender")?;
        validate_identifier(&self.receiver_id, "receiver")?;
        validate_identifier(&self.nonce, "nonce")?;
        if self.connection_epoch == 0 || self.sequence == 0 {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "connection epoch and sequence must be positive".into(),
            ));
        }
        if self.payload.len() > MAX_PAYLOAD_BYTES
            || self.public_key.len() != 32
            || self.signature.len() != 64
            || self.payload_hash.len() != 64
        {
            return Err(RecoveryTransportError::EnvelopeRejected(
                "transport envelope field length is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, RecoveryTransportError> {
        serde_json::to_vec(&TransportEnvelopePayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            sender_id: &self.sender_id,
            receiver_id: &self.receiver_id,
            connection_epoch: self.connection_epoch,
            sequence: self.sequence,
            nonce: &self.nonce,
            kind: &self.kind,
            payload_hash: &self.payload_hash,
            public_key: &self.public_key,
        })
        .map_err(|error| RecoveryTransportError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplayDecision {
    Accepted,
    AlreadySeen,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportReplayWindow {
    pub connection_epoch: u64,
    pub highest_sequence: u64,
    pub seen_envelopes: BTreeSet<String>,
    pub max_entries: usize,
}

impl TransportReplayWindow {
    pub fn new(connection_epoch: u64) -> Result<Self, RecoveryTransportError> {
        if connection_epoch == 0 {
            return Err(RecoveryTransportError::InvalidInput(
                "connection epoch must be positive".into(),
            ));
        }
        Ok(Self {
            connection_epoch,
            highest_sequence: 0,
            seen_envelopes: BTreeSet::new(),
            max_entries: MAX_REPLAY_ENTRIES,
        })
    }

    fn admit(
        &mut self,
        envelope: &AuthenticatedTransportEnvelope,
    ) -> Result<ReplayDecision, RecoveryTransportError> {
        let envelope_hash = envelope.envelope_hash();
        if self.seen_envelopes.contains(&envelope_hash) {
            return Ok(ReplayDecision::AlreadySeen);
        }
        if envelope.connection_epoch < self.connection_epoch {
            return Err(RecoveryTransportError::ReplayRejected(
                "transport connection epoch is stale".into(),
            ));
        }
        if envelope.connection_epoch > self.connection_epoch {
            self.connection_epoch = envelope.connection_epoch;
            self.highest_sequence = 0;
            self.seen_envelopes.clear();
        }
        if envelope.sequence <= self.highest_sequence {
            return Err(RecoveryTransportError::ReplayRejected(
                "transport sequence is stale".into(),
            ));
        }
        if self.seen_envelopes.len() >= self.max_entries {
            self.seen_envelopes.clear();
        }
        self.highest_sequence = envelope.sequence;
        self.seen_envelopes.insert(envelope_hash);
        Ok(ReplayDecision::Accepted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportReceiveResult {
    pub decision: ReplayDecision,
    pub sender_id: String,
    pub kind: TransportMessageKind,
    pub payload: Vec<u8>,
    pub envelope_hash: String,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedTransportReceiver {
    receiver_id: String,
    cluster_id: String,
    resource_id: String,
    registry: TransportKeyRegistry,
    replay_window: TransportReplayWindow,
}

impl AuthenticatedTransportReceiver {
    pub fn new(
        receiver_id: &str,
        cluster_id: &str,
        resource_id: &str,
        connection_epoch: u64,
        registry: TransportKeyRegistry,
    ) -> Result<Self, RecoveryTransportError> {
        validate_identifier(receiver_id, "receiver")?;
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        Ok(Self {
            receiver_id: receiver_id.to_string(),
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            registry,
            replay_window: TransportReplayWindow::new(connection_epoch)?,
        })
    }

    pub fn receive(
        &mut self,
        envelope: AuthenticatedTransportEnvelope,
    ) -> Result<TransportReceiveResult, RecoveryTransportError> {
        let trusted_key = self.registry.key_for(&envelope.sender_id)?;
        envelope.verify(
            &trusted_key,
            &self.cluster_id,
            &self.resource_id,
            &self.receiver_id,
        )?;
        let envelope_hash = envelope.envelope_hash();
        let decision = self.replay_window.admit(&envelope)?;
        Ok(TransportReceiveResult {
            decision,
            sender_id: envelope.sender_id,
            kind: envelope.kind,
            payload: envelope.payload,
            envelope_hash,
        })
    }

    pub fn replay_window(&self) -> &TransportReplayWindow {
        &self.replay_window
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WitnessVoteReservation {
    pub domain: String,
    pub round_id: u64,
    pub witness_id: String,
    pub proposal_digest: String,
    pub membership_epoch: u64,
    pub connection_epoch: u64,
    pub reservation_hash: String,
}

impl WitnessVoteReservation {
    pub fn new(
        round_id: u64,
        witness_id: &str,
        proposal_digest: &str,
        membership_epoch: u64,
        connection_epoch: u64,
    ) -> Result<Self, RecoveryTransportError> {
        validate_identifier(witness_id, "witness")?;
        validate_hash(proposal_digest, "proposal digest")?;
        if round_id == 0 || membership_epoch == 0 || connection_epoch == 0 {
            return Err(RecoveryTransportError::ReservationRejected(
                "reservation generations must be positive".into(),
            ));
        }
        let mut reservation = Self {
            domain: WITNESS_RESERVATION_DOMAIN.to_string(),
            round_id,
            witness_id: witness_id.to_string(),
            proposal_digest: proposal_digest.to_string(),
            membership_epoch,
            connection_epoch,
            reservation_hash: String::new(),
        };
        reservation.reservation_hash = reservation.content_hash()?;
        Ok(reservation)
    }

    fn content_hash(&self) -> Result<String, RecoveryTransportError> {
        digest_json(&(
            &self.domain,
            self.round_id,
            &self.witness_id,
            &self.proposal_digest,
            self.membership_epoch,
            self.connection_epoch,
        ))
    }

    fn validate(&self) -> Result<(), RecoveryTransportError> {
        if self.domain != WITNESS_RESERVATION_DOMAIN {
            return Err(RecoveryTransportError::ReservationRejected(
                "reservation domain is invalid".into(),
            ));
        }
        validate_identifier(&self.witness_id, "witness")?;
        validate_hash(&self.proposal_digest, "proposal digest")?;
        if self.round_id == 0 || self.membership_epoch == 0 || self.connection_epoch == 0 {
            return Err(RecoveryTransportError::ReservationRejected(
                "reservation generations must be positive".into(),
            ));
        }
        if self.content_hash()? != self.reservation_hash {
            return Err(RecoveryTransportError::ReservationRejected(
                "reservation hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WitnessReservationSnapshot {
    reservations: BTreeMap<String, WitnessVoteReservation>,
    state_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReservationAction {
    Reserved,
    AlreadyReserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReservationPersistenceFault {
    BeforeStage,
    AfterStage,
    AfterSyncBeforeRename,
}

#[derive(Debug, Clone)]
pub struct WitnessReservationStore {
    path: PathBuf,
    max_entries: usize,
    fault: Option<ReservationPersistenceFault>,
}

impl WitnessReservationStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_entries: MAX_RESERVATIONS,
            fault: None,
        }
    }

    pub fn inject_fault(&mut self, fault: ReservationPersistenceFault) {
        self.fault = Some(fault);
    }

    pub fn clear_fault(&mut self) {
        self.fault = None;
    }

    pub fn reserve(
        &mut self,
        reservation: WitnessVoteReservation,
    ) -> Result<ReservationAction, RecoveryTransportError> {
        reservation.validate()?;
        let mut snapshot = self.load_snapshot()?;
        let key = reservation_key(&reservation);
        if let Some(existing) = snapshot.reservations.get(&key) {
            if existing == &reservation {
                return Ok(ReservationAction::AlreadyReserved);
            }
            return Err(RecoveryTransportError::ReservationRejected(
                "witness already reserved a conflicting proposal in this round".into(),
            ));
        }
        if snapshot.reservations.len() >= self.max_entries {
            return Err(RecoveryTransportError::ReservationRejected(
                "witness reservation store is full".into(),
            ));
        }
        snapshot.reservations.insert(key, reservation);
        snapshot.state_hash = digest_json(&snapshot.reservations)?;
        self.write_snapshot(&snapshot)?;
        Ok(ReservationAction::Reserved)
    }

    pub fn reservations(
        &self,
    ) -> Result<BTreeMap<String, WitnessVoteReservation>, RecoveryTransportError> {
        Ok(self.load_snapshot()?.reservations)
    }

    pub fn load_snapshot(&self) -> Result<WitnessReservationSnapshot, RecoveryTransportError> {
        let staging = staging_path(&self.path);
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        }
        if !self.path.exists() {
            return Ok(WitnessReservationSnapshot {
                reservations: BTreeMap::new(),
                state_hash: digest_json(&BTreeMap::<String, WitnessVoteReservation>::new())?,
            });
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(RecoveryTransportError::PersistenceFailed(
                "witness reservation snapshot exceeds size bound".into(),
            ));
        }
        let snapshot: WitnessReservationSnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        if snapshot.state_hash != digest_json(&snapshot.reservations)? {
            return Err(RecoveryTransportError::ReservationRejected(
                "witness reservation snapshot hash mismatch".into(),
            ));
        }
        for reservation in snapshot.reservations.values() {
            reservation.validate()?;
        }
        Ok(snapshot)
    }

    fn write_snapshot(
        &self,
        snapshot: &WitnessReservationSnapshot,
    ) -> Result<(), RecoveryTransportError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        }
        if self.fault == Some(ReservationPersistenceFault::BeforeStage) {
            return Err(RecoveryTransportError::PersistenceFailed(
                "injected failure before staging".into(),
            ));
        }
        let staging = staging_path(&self.path);
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        }
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        if self.fault == Some(ReservationPersistenceFault::AfterStage) {
            return Err(RecoveryTransportError::PersistenceFailed(
                "injected failure after staging".into(),
            ));
        }
        file.sync_all()
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        if self.fault == Some(ReservationPersistenceFault::AfterSyncBeforeRename) {
            return Err(RecoveryTransportError::PersistenceFailed(
                "injected failure after sync before rename".into(),
            ));
        }
        fs::rename(&staging, &self.path)
            .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| RecoveryTransportError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtectedWriteRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub owner_region_id: String,
    pub payload_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProtectedWriteAction {
    Accepted,
    AlreadyAccepted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtectedWriteAdmission {
    pub action: ProtectedWriteAction,
    pub operation_id: String,
    pub owner_region_id: String,
    pub fence_epoch: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtectedWriteGatewayReport {
    pub resource_id: String,
    pub accepted_operations: usize,
    pub active_owner_region_id: Option<String>,
    pub accepted_fence_epoch: u64,
}

#[derive(Debug, Clone)]
pub struct ProtectedWriteGateway {
    resource_id: String,
    fence_state: ExternalFenceState,
    accepted_operations: BTreeMap<String, String>,
}

impl ProtectedWriteGateway {
    pub fn new(resource_id: &str) -> Result<Self, RecoveryTransportError> {
        validate_identifier(resource_id, "resource")?;
        Ok(Self {
            resource_id: resource_id.to_string(),
            fence_state: ExternalFenceState::new(resource_id)?,
            accepted_operations: BTreeMap::new(),
        })
    }

    pub fn admit_write(
        &mut self,
        request: ProtectedWriteRequest,
        token: crate::replicated_recovery::ExternalFencingToken,
        registry: &TrustedFencingAuthorityRegistry,
        expected_authority_id: &str,
        expected_cluster_id: &str,
    ) -> Result<ProtectedWriteAdmission, RecoveryTransportError> {
        validate_identifier(&request.operation_id, "operation")?;
        validate_hash(&request.payload_hash, "payload")?;
        if request.resource_id != self.resource_id
            || request.owner_region_id != token.owner_region_id
        {
            return Err(RecoveryTransportError::ProtectedWriteRejected(
                "protected write resource or owner binding mismatch".into(),
            ));
        }
        let request_hash = digest_json(&request)?;
        if let Some(existing_hash) = self.accepted_operations.get(&request.operation_id) {
            if existing_hash == &request_hash {
                return Ok(ProtectedWriteAdmission {
                    action: ProtectedWriteAction::AlreadyAccepted,
                    operation_id: request.operation_id,
                    owner_region_id: token.owner_region_id,
                    fence_epoch: token.fence_epoch,
                    request_hash,
                });
            }
            return Err(RecoveryTransportError::ProtectedWriteRejected(
                "operation ID was previously bound to a different request".into(),
            ));
        }
        let trusted_key = registry.key_for(expected_authority_id)?;
        self.fence_state.apply_with_authority(
            token.clone(),
            expected_authority_id,
            &trusted_key,
            expected_cluster_id,
        )?;
        self.accepted_operations
            .insert(request.operation_id.clone(), request_hash.clone());
        let action = ProtectedWriteAction::Accepted;
        Ok(ProtectedWriteAdmission {
            action,
            operation_id: request.operation_id,
            owner_region_id: token.owner_region_id,
            fence_epoch: token.fence_epoch,
            request_hash,
        })
    }

    pub fn report(&self) -> ProtectedWriteGatewayReport {
        ProtectedWriteGatewayReport {
            resource_id: self.resource_id.clone(),
            accepted_operations: self.accepted_operations.len(),
            active_owner_region_id: self.fence_state.active_owner_region_id.clone(),
            accepted_fence_epoch: self.fence_state.accepted_fence_epoch,
        }
    }

    pub fn fence_state(&self) -> &ExternalFenceState {
        &self.fence_state
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportChaosFault {
    Drop,
    Delay { until_tick: u64 },
    Duplicate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportChaosDelivery {
    Delivered(ReplayDecision),
    Delayed,
    Dropped,
    DuplicateDelivered,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportChaosEvent {
    pub sequence: u64,
    pub tick: u64,
    pub sender_id: String,
    pub receiver_id: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportChaosReport {
    pub delivered: usize,
    pub delayed: usize,
    pub dropped: usize,
    pub duplicated: usize,
    pub replay_rejections: usize,
    pub safety_passed: bool,
    pub trace_digest: String,
}

#[derive(Debug)]
pub struct TransportChaosHarness {
    receiver: AuthenticatedTransportReceiver,
    faults: BTreeMap<(String, String), TransportChaosFault>,
    tick: u64,
    sequence: u64,
    delivered: usize,
    delayed: usize,
    dropped: usize,
    duplicated: usize,
    replay_rejections: usize,
    events: Vec<TransportChaosEvent>,
}

impl TransportChaosHarness {
    pub fn new(receiver: AuthenticatedTransportReceiver) -> Self {
        Self {
            receiver,
            faults: BTreeMap::new(),
            tick: 0,
            sequence: 0,
            delivered: 0,
            delayed: 0,
            dropped: 0,
            duplicated: 0,
            replay_rejections: 0,
            events: Vec::new(),
        }
    }

    pub fn set_fault(
        &mut self,
        sender_id: &str,
        receiver_id: &str,
        fault: TransportChaosFault,
    ) -> Result<(), RecoveryTransportError> {
        validate_identifier(sender_id, "sender")?;
        validate_identifier(receiver_id, "receiver")?;
        self.faults
            .insert((sender_id.to_string(), receiver_id.to_string()), fault);
        self.record(sender_id, receiver_id, "fault-injected");
        Ok(())
    }

    pub fn heal(&mut self, sender_id: &str, receiver_id: &str) {
        self.faults
            .remove(&(sender_id.to_string(), receiver_id.to_string()));
        self.record(sender_id, receiver_id, "healed");
    }

    pub fn advance_tick(&mut self, ticks: u64) {
        self.tick = self.tick.saturating_add(ticks);
        self.record("clock", "clock", &format!("advanced:{ticks}"));
    }

    pub fn deliver(
        &mut self,
        envelope: AuthenticatedTransportEnvelope,
    ) -> Result<TransportChaosDelivery, RecoveryTransportError> {
        let key = (envelope.sender_id.clone(), envelope.receiver_id.clone());
        match self.faults.get(&key).cloned() {
            Some(TransportChaosFault::Drop) => {
                self.dropped += 1;
                self.record(&key.0, &key.1, "dropped");
                Ok(TransportChaosDelivery::Dropped)
            }
            Some(TransportChaosFault::Delay { until_tick }) if self.tick < until_tick => {
                self.delayed += 1;
                self.record(&key.0, &key.1, "delayed");
                Ok(TransportChaosDelivery::Delayed)
            }
            Some(TransportChaosFault::Duplicate) => {
                let first = self.receiver.receive(envelope.clone())?;
                let second = self.receiver.receive(envelope)?;
                self.duplicated += 1;
                self.delivered += 1;
                self.record(&key.0, &key.1, "duplicated");
                if first.decision == ReplayDecision::Accepted
                    && second.decision == ReplayDecision::AlreadySeen
                {
                    Ok(TransportChaosDelivery::DuplicateDelivered)
                } else {
                    Err(RecoveryTransportError::ReplayRejected(
                        "duplicate delivery did not resolve idempotently".into(),
                    ))
                }
            }
            _ => match self.receiver.receive(envelope) {
                Ok(result) => {
                    self.delivered += 1;
                    self.record(&key.0, &key.1, "delivered");
                    Ok(TransportChaosDelivery::Delivered(result.decision))
                }
                Err(RecoveryTransportError::ReplayRejected(reason)) => {
                    self.replay_rejections += 1;
                    Err(RecoveryTransportError::ReplayRejected(reason))
                }
                Err(error) => Err(error),
            },
        }
    }

    pub fn report(&self) -> TransportChaosReport {
        TransportChaosReport {
            delivered: self.delivered,
            delayed: self.delayed,
            dropped: self.dropped,
            duplicated: self.duplicated,
            replay_rejections: self.replay_rejections,
            safety_passed: self.events.len() <= 16_384 && self.replay_rejections == 0,
            trace_digest: digest_json(&self.events).unwrap_or_default(),
        }
    }

    pub fn events(&self) -> &[TransportChaosEvent] {
        &self.events
    }

    fn record(&mut self, sender_id: &str, receiver_id: &str, detail: &str) {
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() < 16_384 {
            self.events.push(TransportChaosEvent {
                sequence: self.sequence,
                tick: self.tick,
                sender_id: sender_id.to_string(),
                receiver_id: receiver_id.to_string(),
                detail: detail.to_string(),
            });
        }
    }
}

fn reservation_key(reservation: &WitnessVoteReservation) -> String {
    format!(
        "{}:{}:{}",
        reservation.membership_epoch, reservation.round_id, reservation.witness_id
    )
}

fn staging_path(path: &Path) -> PathBuf {
    let mut staging = path.as_os_str().to_os_string();
    staging.push(".staging");
    PathBuf::from(staging)
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, RecoveryTransportError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| RecoveryTransportError::InvalidInput(error.to_string()))?;
    Ok(digest_bytes(&bytes))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), RecoveryTransportError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(RecoveryTransportError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), RecoveryTransportError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(RecoveryTransportError::InvalidInput(format!(
            "{label} must be a 64-character hexadecimal digest"
        )));
    }
    Ok(())
}
