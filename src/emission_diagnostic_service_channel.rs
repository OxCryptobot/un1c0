use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::emission_diagnostic_service_identity::{ServiceIdentityError, ServiceIdentityRegistry};

pub const SERVICE_CHANNEL_SCHEMA_VERSION: u8 = 1;
pub const MAX_SERVICE_CHANNEL_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_SERVICE_CHANNEL_ID_BYTES: usize = 256;
pub const MAX_REPLAY_EPOCH_SEEN_HASHES: usize = 4096;
pub const MAX_SERVICE_CHANNEL_REPLAY_STATE_BYTES: usize = 512 * 1024;
pub const REPLAY_EPOCH_STATE_SCHEMA_VERSION: u8 = 2;
const SERVICE_CHANNEL_DOMAIN: &[u8] = b"un1c0/phase81/authenticated-service-channel/v1";
const REPLAY_EPOCH_DOMAIN: &[u8] = b"un1c0/phase81/durable-replay-epoch/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceChannelError {
    InvalidIdentifier(&'static str),
    InvalidGeneration(&'static str),
    InvalidSequence,
    InvalidEpoch,
    InvalidPayload,
    InvalidSignature,
    UnsupportedSchema(u8),
    ServiceMismatch(&'static str),
    Signer(ServiceIdentityError),
    Replay(&'static str),
    Gap { expected: u64, actual: u64 },
    EpochMismatch { expected: u64, actual: u64 },
    AlreadySeen,
    ReplayWindowFull,
    Serialization(String),
    Persistence(String),
    Collision,
    InvalidResourceBudget(&'static str),
    ResourceLimit(&'static str),
    NotReady(&'static str),
}

impl std::fmt::Display for ServiceChannelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier(label) => write!(formatter, "invalid service-channel {label}"),
            Self::InvalidGeneration(label) => {
                write!(formatter, "service-channel {label} must be positive")
            }
            Self::InvalidSequence => {
                formatter.write_str("service-channel sequence must be positive")
            }
            Self::InvalidEpoch => formatter.write_str("service-channel epoch must be positive"),
            Self::InvalidPayload => formatter.write_str("service-channel payload is invalid"),
            Self::InvalidSignature => formatter.write_str("service-channel signature is invalid"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported service-channel schema {version}")
            }
            Self::ServiceMismatch(label) => write!(formatter, "service-channel {label} mismatch"),
            Self::Signer(error) => write!(formatter, "service-channel signer rejected: {error}"),
            Self::Replay(label) => write!(formatter, "service-channel replay rejected: {label}"),
            Self::Gap { expected, actual } => write!(
                formatter,
                "service-channel gap: expected {expected}, received {actual}"
            ),
            Self::EpochMismatch { expected, actual } => write!(
                formatter,
                "service-channel epoch mismatch: expected {expected}, received {actual}"
            ),
            Self::AlreadySeen => formatter.write_str("service-channel envelope was already seen"),
            Self::ReplayWindowFull => formatter.write_str("service-channel replay window is full"),
            Self::Serialization(message) => {
                write!(formatter, "service-channel serialization failed: {message}")
            }
            Self::Persistence(message) => write!(
                formatter,
                "service-channel replay persistence failed: {message}"
            ),
            Self::Collision => formatter.write_str("service-channel replay artifact collision"),
            Self::InvalidResourceBudget(label) => {
                write!(
                    formatter,
                    "service-channel resource budget is invalid: {label}"
                )
            }
            Self::ResourceLimit(label) => {
                write!(
                    formatter,
                    "service-channel resource limit exceeded: {label}"
                )
            }
            Self::NotReady(label) => write!(formatter, "service-channel is not ready: {label}"),
        }
    }
}

impl std::error::Error for ServiceChannelError {}

impl From<ServiceIdentityError> for ServiceChannelError {
    fn from(value: ServiceIdentityError) -> Self {
        Self::Signer(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceChannelResourceBudget {
    pub max_payload_bytes: usize,
    pub max_replay_state_bytes: usize,
    pub max_seen_envelope_hashes: usize,
}

impl Default for ServiceChannelResourceBudget {
    fn default() -> Self {
        Self {
            max_payload_bytes: MAX_SERVICE_CHANNEL_PAYLOAD_BYTES,
            max_replay_state_bytes: MAX_SERVICE_CHANNEL_REPLAY_STATE_BYTES,
            max_seen_envelope_hashes: MAX_REPLAY_EPOCH_SEEN_HASHES,
        }
    }
}

impl ServiceChannelResourceBudget {
    fn validate(&self) -> Result<(), ServiceChannelError> {
        if self.max_payload_bytes == 0 || self.max_payload_bytes > MAX_SERVICE_CHANNEL_PAYLOAD_BYTES
        {
            return Err(ServiceChannelError::InvalidResourceBudget("payload"));
        }
        if self.max_replay_state_bytes == 0
            || self.max_replay_state_bytes > MAX_SERVICE_CHANNEL_REPLAY_STATE_BYTES
        {
            return Err(ServiceChannelError::InvalidResourceBudget("replay state"));
        }
        if self.max_seen_envelope_hashes == 0
            || self.max_seen_envelope_hashes > MAX_REPLAY_EPOCH_SEEN_HASHES
        {
            return Err(ServiceChannelError::InvalidResourceBudget("replay window"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceChannelReadiness {
    Ready,
    NoActiveSigner,
    ReplayStateInvalid,
    ReplayStateTooLarge,
}

impl ServiceChannelReadiness {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedServiceChannelEnvelope {
    pub schema_version: u8,
    pub channel_id: String,
    pub sender_service_id: String,
    pub sender_identity_id: String,
    pub receiver_service_id: String,
    pub receiver_identity_id: String,
    pub signer_id: String,
    pub signer_generation: u64,
    pub connection_epoch: u64,
    pub sequence: u64,
    pub nonce: [u8; 16],
    pub payload_hash: [u8; 32],
    pub payload: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct ServiceChannelSigningPayload<'a> {
    domain: &'static [u8],
    schema_version: u8,
    channel_id: &'a str,
    sender_service_id: &'a str,
    sender_identity_id: &'a str,
    receiver_service_id: &'a str,
    receiver_identity_id: &'a str,
    signer_id: &'a str,
    signer_generation: u64,
    connection_epoch: u64,
    sequence: u64,
    nonce: [u8; 16],
    payload_hash: [u8; 32],
}

impl AuthenticatedServiceChannelEnvelope {
    fn signing_payload(&self) -> Result<Vec<u8>, ServiceChannelError> {
        serde_json::to_vec(&ServiceChannelSigningPayload {
            domain: SERVICE_CHANNEL_DOMAIN,
            schema_version: self.schema_version,
            channel_id: &self.channel_id,
            sender_service_id: &self.sender_service_id,
            sender_identity_id: &self.sender_identity_id,
            receiver_service_id: &self.receiver_service_id,
            receiver_identity_id: &self.receiver_identity_id,
            signer_id: &self.signer_id,
            signer_generation: self.signer_generation,
            connection_epoch: self.connection_epoch,
            sequence: self.sequence,
            nonce: self.nonce,
            payload_hash: self.payload_hash,
        })
        .map_err(|error| ServiceChannelError::Serialization(error.to_string()))
    }

    pub fn envelope_hash(&self) -> Result<[u8; 32], ServiceChannelError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ServiceChannelError::Serialization(error.to_string()))?;
        Ok(hash_bytes(&bytes))
    }

    fn validate_shape(&self) -> Result<(), ServiceChannelError> {
        if self.schema_version != SERVICE_CHANNEL_SCHEMA_VERSION {
            return Err(ServiceChannelError::UnsupportedSchema(self.schema_version));
        }
        validate_identifier(&self.channel_id, "channel id")?;
        validate_identifier(&self.sender_service_id, "sender service id")?;
        validate_identity_id(&self.sender_identity_id, "sender identity id")?;
        validate_identifier(&self.receiver_service_id, "receiver service id")?;
        validate_identity_id(&self.receiver_identity_id, "receiver identity id")?;
        validate_identifier(&self.signer_id, "signer id")?;
        if self.signer_generation == 0 {
            return Err(ServiceChannelError::InvalidGeneration("signer generation"));
        }
        if self.connection_epoch == 0 {
            return Err(ServiceChannelError::InvalidEpoch);
        }
        if self.sequence == 0 {
            return Err(ServiceChannelError::InvalidSequence);
        }
        if self.payload.len() > MAX_SERVICE_CHANNEL_PAYLOAD_BYTES || self.signature.len() != 64 {
            return Err(ServiceChannelError::InvalidPayload);
        }
        if hash_bytes(&self.payload) != self.payload_hash {
            return Err(ServiceChannelError::InvalidPayload);
        }
        Ok(())
    }

    pub fn verify(
        &self,
        sender_registry: &ServiceIdentityRegistry,
        expected_channel_id: &str,
        expected_receiver_service_id: &str,
        expected_receiver_identity_id: &str,
    ) -> Result<(), ServiceChannelError> {
        self.validate_shape()?;
        if self.channel_id != expected_channel_id {
            return Err(ServiceChannelError::ServiceMismatch("channel"));
        }
        if self.sender_service_id != sender_registry.service_id()
            || self.sender_identity_id != sender_registry.identity().canonical_id()
        {
            return Err(ServiceChannelError::ServiceMismatch("sender identity"));
        }
        if self.receiver_service_id != expected_receiver_service_id {
            return Err(ServiceChannelError::ServiceMismatch("receiver service"));
        }
        if self.receiver_identity_id != expected_receiver_identity_id {
            return Err(ServiceChannelError::ServiceMismatch("receiver identity"));
        }
        let signer = sender_registry.signer(&self.signer_id).ok_or_else(|| {
            ServiceChannelError::Signer(ServiceIdentityError::UntrustedSigner(
                self.signer_id.clone(),
            ))
        })?;
        sender_registry.authorize_active(
            &self.signer_id,
            self.signer_generation,
            &signer.public_key,
        )?;
        let verifying_key = VerifyingKey::from_bytes(&signer.public_key)
            .map_err(|_| ServiceChannelError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ServiceChannelError::InvalidSignature)?;
        verifying_key
            .verify(&self.signing_payload()?, &Signature::from_bytes(&signature))
            .map_err(|_| ServiceChannelError::InvalidSignature)
    }
}

#[derive(Debug, Clone)]
pub struct ServiceChannelSender {
    registry: ServiceIdentityRegistry,
    signer_id: String,
    signing_key: SigningKey,
    channel_id: String,
    receiver_service_id: String,
    receiver_identity_id: String,
    connection_epoch: u64,
    next_sequence: u64,
}

impl ServiceChannelSender {
    pub fn new(
        registry: ServiceIdentityRegistry,
        signer_id: &str,
        signing_key: SigningKey,
        channel_id: &str,
        receiver_service_id: &str,
        receiver_identity_id: &str,
        connection_epoch: u64,
    ) -> Result<Self, ServiceChannelError> {
        validate_identifier(channel_id, "channel id")?;
        validate_identifier(receiver_service_id, "receiver service id")?;
        validate_identity_id(receiver_identity_id, "receiver identity id")?;
        if connection_epoch == 0 {
            return Err(ServiceChannelError::InvalidEpoch);
        }
        let signer = registry.signer(signer_id).ok_or_else(|| {
            ServiceChannelError::Signer(ServiceIdentityError::UntrustedSigner(
                signer_id.to_string(),
            ))
        })?;
        registry.authorize_active(
            signer_id,
            signer.generation,
            &signing_key.verifying_key().to_bytes(),
        )?;
        Ok(Self {
            registry,
            signer_id: signer_id.to_string(),
            signing_key,
            channel_id: channel_id.to_string(),
            receiver_service_id: receiver_service_id.to_string(),
            receiver_identity_id: receiver_identity_id.to_string(),
            connection_epoch,
            next_sequence: 1,
        })
    }

    pub fn send(
        &mut self,
        payload: Vec<u8>,
        nonce: [u8; 16],
    ) -> Result<AuthenticatedServiceChannelEnvelope, ServiceChannelError> {
        if payload.len() > MAX_SERVICE_CHANNEL_PAYLOAD_BYTES {
            return Err(ServiceChannelError::InvalidPayload);
        }
        let signer = self.registry.signer(&self.signer_id).ok_or_else(|| {
            ServiceChannelError::Signer(ServiceIdentityError::UntrustedSigner(
                self.signer_id.clone(),
            ))
        })?;
        self.registry.authorize_active(
            &self.signer_id,
            signer.generation,
            &self.signing_key.verifying_key().to_bytes(),
        )?;
        let mut envelope = AuthenticatedServiceChannelEnvelope {
            schema_version: SERVICE_CHANNEL_SCHEMA_VERSION,
            channel_id: self.channel_id.clone(),
            sender_service_id: self.registry.service_id().to_string(),
            sender_identity_id: self.registry.identity().canonical_id(),
            receiver_service_id: self.receiver_service_id.clone(),
            receiver_identity_id: self.receiver_identity_id.clone(),
            signer_id: self.signer_id.clone(),
            signer_generation: signer.generation,
            connection_epoch: self.connection_epoch,
            sequence: self.next_sequence,
            nonce,
            payload_hash: hash_bytes(&payload),
            payload,
            signature: vec![0; 64],
        };
        envelope.signature = self
            .signing_key
            .sign(&envelope.signing_payload()?)
            .to_bytes()
            .to_vec();
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(ServiceChannelError::InvalidSequence)?;
        Ok(envelope)
    }

    pub fn connection_epoch(&self) -> u64 {
        self.connection_epoch
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DurableReplayEpochState {
    pub schema_version: u8,
    pub channel_id: String,
    pub sender_service_id: String,
    pub sender_identity_id: String,
    pub receiver_service_id: String,
    pub receiver_identity_id: String,
    pub max_payload_bytes: usize,
    pub max_replay_state_bytes: usize,
    pub max_seen_envelope_hashes: usize,
    pub connection_epoch: u64,
    pub highest_sequence: u64,
    pub seen_envelope_hashes: BTreeSet<[u8; 32]>,
    pub state_digest: [u8; 32],
}

impl DurableReplayEpochState {
    fn new(
        channel_id: &str,
        sender_service_id: &str,
        sender_identity_id: &str,
        receiver_service_id: &str,
        receiver_identity_id: &str,
        connection_epoch: u64,
        budget: ServiceChannelResourceBudget,
    ) -> Result<Self, ServiceChannelError> {
        validate_identifier(channel_id, "channel id")?;
        validate_identifier(sender_service_id, "sender service id")?;
        validate_identity_id(sender_identity_id, "sender identity id")?;
        validate_identifier(receiver_service_id, "receiver service id")?;
        validate_identity_id(receiver_identity_id, "receiver identity id")?;
        budget.validate()?;
        if connection_epoch == 0 {
            return Err(ServiceChannelError::InvalidEpoch);
        }
        Ok(Self {
            schema_version: REPLAY_EPOCH_STATE_SCHEMA_VERSION,
            channel_id: channel_id.to_string(),
            sender_service_id: sender_service_id.to_string(),
            sender_identity_id: sender_identity_id.to_string(),
            receiver_service_id: receiver_service_id.to_string(),
            receiver_identity_id: receiver_identity_id.to_string(),
            max_payload_bytes: budget.max_payload_bytes,
            max_replay_state_bytes: budget.max_replay_state_bytes,
            max_seen_envelope_hashes: budget.max_seen_envelope_hashes,
            connection_epoch,
            highest_sequence: 0,
            seen_envelope_hashes: BTreeSet::new(),
            state_digest: [0; 32],
        })
    }

    fn validate(&self) -> Result<(), ServiceChannelError> {
        if self.schema_version != REPLAY_EPOCH_STATE_SCHEMA_VERSION {
            return Err(ServiceChannelError::UnsupportedSchema(self.schema_version));
        }
        validate_identifier(&self.channel_id, "channel id")?;
        validate_identifier(&self.sender_service_id, "sender service id")?;
        validate_identity_id(&self.sender_identity_id, "sender identity id")?;
        validate_identifier(&self.receiver_service_id, "receiver service id")?;
        validate_identity_id(&self.receiver_identity_id, "receiver identity id")?;
        let budget = self.resource_budget()?;
        budget.validate()?;
        if self.connection_epoch == 0 {
            return Err(ServiceChannelError::InvalidEpoch);
        }
        if self.seen_envelope_hashes.len() > budget.max_seen_envelope_hashes {
            return Err(ServiceChannelError::ReplayWindowFull);
        }
        if self.state_digest != self.digest()? {
            return Err(ServiceChannelError::Replay("replay state digest mismatch"));
        }
        Ok(())
    }

    fn resource_budget(&self) -> Result<ServiceChannelResourceBudget, ServiceChannelError> {
        Ok(ServiceChannelResourceBudget {
            max_payload_bytes: self.max_payload_bytes,
            max_replay_state_bytes: self.max_replay_state_bytes,
            max_seen_envelope_hashes: self.max_seen_envelope_hashes,
        })
    }

    fn digest(&self) -> Result<[u8; 32], ServiceChannelError> {
        let bytes = serde_json::to_vec(&(
            REPLAY_EPOCH_DOMAIN,
            self.schema_version,
            &self.channel_id,
            &self.sender_service_id,
            &self.sender_identity_id,
            &self.receiver_service_id,
            &self.receiver_identity_id,
            self.max_payload_bytes,
            self.max_replay_state_bytes,
            self.max_seen_envelope_hashes,
            self.connection_epoch,
            self.highest_sequence,
            &self.seen_envelope_hashes,
        ))
        .map_err(|error| ServiceChannelError::Serialization(error.to_string()))?;
        Ok(hash_bytes(&bytes))
    }

    fn refresh_digest(&mut self) -> Result<(), ServiceChannelError> {
        self.state_digest = self.digest()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceChannelReplayDecision {
    Accepted,
    AlreadySeen,
}

#[derive(Debug, Clone)]
pub struct DurableReplayEpochStore {
    path: PathBuf,
    state: DurableReplayEpochState,
    budget: ServiceChannelResourceBudget,
}

impl DurableReplayEpochStore {
    pub fn open(
        path: impl AsRef<Path>,
        channel_id: &str,
        sender_service_id: &str,
        sender_identity_id: &str,
        receiver_service_id: &str,
        receiver_identity_id: &str,
        initial_epoch: u64,
    ) -> Result<Self, ServiceChannelError> {
        Self::open_with_budget(
            path,
            channel_id,
            sender_service_id,
            sender_identity_id,
            receiver_service_id,
            receiver_identity_id,
            initial_epoch,
            ServiceChannelResourceBudget::default(),
        )
    }

    pub fn open_with_budget(
        path: impl AsRef<Path>,
        channel_id: &str,
        sender_service_id: &str,
        sender_identity_id: &str,
        receiver_service_id: &str,
        receiver_identity_id: &str,
        initial_epoch: u64,
        budget: ServiceChannelResourceBudget,
    ) -> Result<Self, ServiceChannelError> {
        budget.validate()?;
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            let metadata = fs::metadata(&path)
                .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
            if metadata.len() > budget.max_replay_state_bytes as u64 {
                return Err(ServiceChannelError::ResourceLimit("replay state bytes"));
            }
            let bytes = fs::read(&path)
                .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
            let state: DurableReplayEpochState = serde_json::from_slice(&bytes)
                .map_err(|error| ServiceChannelError::Serialization(error.to_string()))?;
            state.validate()?;
            if state.channel_id != channel_id
                || state.sender_service_id != sender_service_id
                || state.sender_identity_id != sender_identity_id
                || state.receiver_service_id != receiver_service_id
                || state.receiver_identity_id != receiver_identity_id
            {
                return Err(ServiceChannelError::ServiceMismatch("replay state"));
            }
            if state.resource_budget()? != budget {
                return Err(ServiceChannelError::ResourceLimit("replay budget mismatch"));
            }
            remove_stale_temporary(&path)?;
            return Ok(Self {
                path,
                state,
                budget,
            });
        }
        let mut state = DurableReplayEpochState::new(
            channel_id,
            sender_service_id,
            sender_identity_id,
            receiver_service_id,
            receiver_identity_id,
            initial_epoch,
            budget,
        )?;
        state.refresh_digest()?;
        let store = Self {
            path,
            state,
            budget,
        };
        store.persist(&store.state)?;
        Ok(store)
    }

    pub fn state(&self) -> &DurableReplayEpochState {
        &self.state
    }

    pub fn advance_epoch(&mut self, next_epoch: u64) -> Result<(), ServiceChannelError> {
        if next_epoch <= self.state.connection_epoch {
            return Err(ServiceChannelError::Replay("epoch must increase"));
        }
        let mut next = self.state.clone();
        next.connection_epoch = next_epoch;
        next.highest_sequence = 0;
        next.seen_envelope_hashes.clear();
        next.refresh_digest()?;
        self.persist(&next)?;
        self.state = next;
        Ok(())
    }

    fn admit(
        &mut self,
        envelope: &AuthenticatedServiceChannelEnvelope,
    ) -> Result<ServiceChannelReplayDecision, ServiceChannelError> {
        let envelope_hash = envelope.envelope_hash()?;
        if self.state.seen_envelope_hashes.contains(&envelope_hash) {
            return Ok(ServiceChannelReplayDecision::AlreadySeen);
        }
        if envelope.connection_epoch != self.state.connection_epoch {
            return Err(ServiceChannelError::EpochMismatch {
                expected: self.state.connection_epoch,
                actual: envelope.connection_epoch,
            });
        }
        let expected = self.state.highest_sequence.saturating_add(1);
        if envelope.sequence < expected {
            return Err(ServiceChannelError::Replay("sequence is stale"));
        }
        if envelope.sequence > expected {
            return Err(ServiceChannelError::Gap {
                expected,
                actual: envelope.sequence,
            });
        }
        let mut next = self.state.clone();
        if next.seen_envelope_hashes.len() >= self.budget.max_seen_envelope_hashes {
            return Err(ServiceChannelError::ReplayWindowFull);
        }
        next.highest_sequence = envelope.sequence;
        next.seen_envelope_hashes.insert(envelope_hash);
        next.refresh_digest()?;
        self.persist(&next)?;
        self.state = next;
        Ok(ServiceChannelReplayDecision::Accepted)
    }

    fn persist(&self, state: &DurableReplayEpochState) -> Result<(), ServiceChannelError> {
        state.validate()?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
        let bytes = serde_json::to_vec(state)
            .map_err(|error| ServiceChannelError::Serialization(error.to_string()))?;
        if bytes.len() > self.budget.max_replay_state_bytes {
            return Err(ServiceChannelError::ResourceLimit("replay state bytes"));
        }
        let temporary = parent.join(format!(
            ".{}.tmp",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("replay-epoch")
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
        let directory = OpenOptions::new()
            .read(true)
            .open(parent)
            .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
        directory
            .sync_all()
            .map_err(|error| ServiceChannelError::Persistence(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceChannelReceiveResult {
    pub decision: ServiceChannelReplayDecision,
    pub payload: Vec<u8>,
    pub envelope_hash: [u8; 32],
    pub sequence: u64,
    pub connection_epoch: u64,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedServiceChannelReceiver {
    channel_id: String,
    receiver_service_id: String,
    receiver_identity_id: String,
    sender_registry: ServiceIdentityRegistry,
    replay_store: DurableReplayEpochStore,
}

impl AuthenticatedServiceChannelReceiver {
    pub fn new(
        channel_id: &str,
        receiver_service_id: &str,
        receiver_identity_id: &str,
        sender_registry: ServiceIdentityRegistry,
        replay_store: DurableReplayEpochStore,
    ) -> Result<Self, ServiceChannelError> {
        validate_identifier(channel_id, "channel id")?;
        validate_identifier(receiver_service_id, "receiver service id")?;
        validate_identity_id(receiver_identity_id, "receiver identity id")?;
        if replay_store.state.channel_id != channel_id
            || replay_store.state.sender_service_id != sender_registry.service_id()
            || replay_store.state.sender_identity_id != sender_registry.identity().canonical_id()
            || replay_store.state.receiver_service_id != receiver_service_id
            || replay_store.state.receiver_identity_id != receiver_identity_id
        {
            return Err(ServiceChannelError::ServiceMismatch("replay store binding"));
        }
        Ok(Self {
            channel_id: channel_id.to_string(),
            receiver_service_id: receiver_service_id.to_string(),
            receiver_identity_id: receiver_identity_id.to_string(),
            sender_registry,
            replay_store,
        })
    }

    pub fn readiness(&self) -> ServiceChannelReadiness {
        let sender_identity_id = self.sender_registry.identity().canonical_id();
        if validate_identity_id(&sender_identity_id, "sender identity id").is_err() {
            return ServiceChannelReadiness::NoActiveSigner;
        }
        let Some(active_signer_id) = self.sender_registry.active_signer_id() else {
            return ServiceChannelReadiness::NoActiveSigner;
        };
        let Some(active_signer) = self.sender_registry.signer(active_signer_id) else {
            return ServiceChannelReadiness::NoActiveSigner;
        };
        if active_signer.revoked
            || active_signer.generation == 0
            || VerifyingKey::from_bytes(&active_signer.public_key).is_err()
        {
            return ServiceChannelReadiness::NoActiveSigner;
        }
        if self.replay_store.state.validate().is_err() {
            return ServiceChannelReadiness::ReplayStateInvalid;
        }
        match serde_json::to_vec(&self.replay_store.state) {
            Ok(bytes) if bytes.len() <= self.replay_store.budget.max_replay_state_bytes => {
                ServiceChannelReadiness::Ready
            }
            Ok(_) => ServiceChannelReadiness::ReplayStateTooLarge,
            Err(_) => ServiceChannelReadiness::ReplayStateInvalid,
        }
    }

    pub fn require_ready(&self) -> Result<(), ServiceChannelError> {
        match self.readiness() {
            ServiceChannelReadiness::Ready => Ok(()),
            ServiceChannelReadiness::NoActiveSigner => {
                Err(ServiceChannelError::NotReady("active signer"))
            }
            ServiceChannelReadiness::ReplayStateInvalid => {
                Err(ServiceChannelError::NotReady("replay state"))
            }
            ServiceChannelReadiness::ReplayStateTooLarge => {
                Err(ServiceChannelError::NotReady("replay state bytes"))
            }
        }
    }

    pub fn receive(
        &mut self,
        envelope: AuthenticatedServiceChannelEnvelope,
    ) -> Result<ServiceChannelReceiveResult, ServiceChannelError> {
        self.require_ready()?;
        if envelope.payload.len() > self.replay_store.budget.max_payload_bytes {
            return Err(ServiceChannelError::ResourceLimit("payload bytes"));
        }
        envelope.verify(
            &self.sender_registry,
            &self.channel_id,
            &self.receiver_service_id,
            &self.receiver_identity_id,
        )?;
        let envelope_hash = envelope.envelope_hash()?;
        let decision = self.replay_store.admit(&envelope)?;
        Ok(ServiceChannelReceiveResult {
            decision,
            payload: envelope.payload,
            envelope_hash,
            sequence: envelope.sequence,
            connection_epoch: envelope.connection_epoch,
        })
    }

    pub fn replay_state(&self) -> &DurableReplayEpochState {
        self.replay_store.state()
    }

    pub fn advance_epoch(&mut self, next_epoch: u64) -> Result<(), ServiceChannelError> {
        self.replay_store.advance_epoch(next_epoch)
    }
}

fn remove_stale_temporary(path: &Path) -> Result<(), ServiceChannelError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("replay-epoch")
    ));
    match fs::remove_file(temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ServiceChannelError::Persistence(error.to_string())),
    }
}

fn validate_identity_id(value: &str, label: &'static str) -> Result<(), ServiceChannelError> {
    if value.is_empty()
        || value.len() > MAX_SERVICE_CHANNEL_ID_BYTES
        || value.chars().any(|character| character.is_control())
    {
        return Err(ServiceChannelError::InvalidIdentifier(label));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &'static str) -> Result<(), ServiceChannelError> {
    if value.is_empty()
        || value.len() > MAX_SERVICE_CHANNEL_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character == '/')
    {
        return Err(ServiceChannelError::InvalidIdentifier(label));
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
