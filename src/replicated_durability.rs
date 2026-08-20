use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;

pub const CAS_WRITE_DOMAIN: &str = "un1c0/replicated-cas-write/v1";
pub const REPLICA_ACK_DOMAIN: &str = "un1c0/replica-durability-ack/v1";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_HASH_BYTES: usize = 64;
const MAX_NONCE_BYTES: usize = 128;
const MAX_REPLICAS: usize = 32;
const MAX_COMMITTED_REQUESTS: usize = 512;
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReplicatedDurabilityError {
    #[error("replicated durability input is invalid: {0}")]
    InvalidInput(String),
    #[error("replicated durability evidence was rejected: {0}")]
    Rejected(String),
    #[error("replicated durability replay was rejected: {0}")]
    ReplayRejected(String),
    #[error("replicated durability conflict: {0}")]
    Conflict(String),
    #[error("compare-and-swap generation mismatch")]
    CasMismatch,
    #[error("replica durability quorum is unavailable")]
    QuorumUnavailable,
    #[error("replicated durability persistence failed: {0}")]
    PersistenceFailed(String),
    #[error("unknown writer: {0}")]
    UnknownWriter(String),
    #[error("unknown replica: {0}")]
    UnknownReplica(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasWriteRequest {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub writer_id: String,
    pub writer_epoch: u64,
    pub request_nonce: String,
    pub expected_generation: u64,
    pub expected_hash: String,
    pub proposed_generation: u64,
    pub proposed_hash: String,
    pub payload_hash: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub request_hash: String,
}

#[derive(Debug, Serialize)]
struct CasWritePayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    writer_id: &'a str,
    writer_epoch: u64,
    request_nonce: &'a str,
    expected_generation: u64,
    expected_hash: &'a str,
    proposed_generation: u64,
    proposed_hash: &'a str,
    payload_hash: &'a str,
    public_key: &'a [u8],
}

impl CasWriteRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        writer_id: &str,
        writer_epoch: u64,
        request_nonce: &str,
        expected_generation: u64,
        expected_hash: &str,
        proposed_generation: u64,
        proposed_hash: &str,
        payload_hash: &str,
        signing_key: &SigningKey,
    ) -> Result<Self, ReplicatedDurabilityError> {
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut request = Self {
            domain: CAS_WRITE_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            writer_id: writer_id.to_string(),
            writer_epoch,
            request_nonce: request_nonce.to_string(),
            expected_generation,
            expected_hash: expected_hash.to_string(),
            proposed_generation,
            proposed_hash: proposed_hash.to_string(),
            payload_hash: payload_hash.to_string(),
            public_key,
            signature: vec![0; 64],
            request_hash: "0".repeat(MAX_HASH_BYTES),
        };
        request.validate_shape()?;
        let payload = request.canonical_payload()?;
        request.signature = signing_key.sign(&payload).to_bytes().to_vec();
        request.request_hash = request.content_hash()?;
        request.validate_shape()?;
        Ok(request)
    }

    pub fn verify(
        &self,
        registry: &BTreeMap<String, Vec<u8>>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), ReplicatedDurabilityError> {
        self.validate_shape()?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS request is bound to a different resource".into(),
            ));
        }
        let expected = registry
            .get(&self.writer_id)
            .ok_or_else(|| ReplicatedDurabilityError::UnknownWriter(self.writer_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS writer key does not match its pinned key".into(),
            ));
        }
        let key_bytes: [u8; 32] = self.public_key.as_slice().try_into().map_err(|_| {
            ReplicatedDurabilityError::Rejected("writer key shape is invalid".into())
        })?;
        let key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| ReplicatedDurabilityError::Rejected("writer key is invalid".into()))?;
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            ReplicatedDurabilityError::Rejected("writer signature is invalid".into())
        })?;
        key.verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                ReplicatedDurabilityError::Rejected("CAS writer signature is invalid".into())
            })?;
        if self.request_hash != self.content_hash()? {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS request hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), ReplicatedDurabilityError> {
        if self.domain != CAS_WRITE_DOMAIN || self.protocol_version != 1 {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS request domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.snapshot_id, "snapshot")?;
        validate_identifier(&self.writer_id, "writer")?;
        validate_identifier(&self.request_nonce, "request nonce")?;
        if self.request_nonce.len() > MAX_NONCE_BYTES {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS request nonce exceeds its bound".into(),
            ));
        }
        if self.writer_epoch == 0 || self.proposed_generation <= self.expected_generation {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS writer epoch or generation is invalid".into(),
            ));
        }
        validate_hash(&self.expected_hash, "expected hash")?;
        validate_hash(&self.proposed_hash, "proposed hash")?;
        validate_hash(&self.payload_hash, "payload hash")?;
        validate_hash(&self.request_hash, "request hash")?;
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS writer key or signature shape is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReplicatedDurabilityError> {
        serde_json::to_vec(&CasWritePayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            writer_id: &self.writer_id,
            writer_epoch: self.writer_epoch,
            request_nonce: &self.request_nonce,
            expected_generation: self.expected_generation,
            expected_hash: &self.expected_hash,
            proposed_generation: self.proposed_generation,
            proposed_hash: &self.proposed_hash,
            payload_hash: &self.payload_hash,
            public_key: &self.public_key,
        })
        .map_err(|error| ReplicatedDurabilityError::InvalidInput(error.to_string()))
    }

    fn content_hash(&self) -> Result<String, ReplicatedDurabilityError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.writer_id,
            self.writer_epoch,
            &self.request_nonce,
            self.expected_generation,
            &self.expected_hash,
            self.proposed_generation,
            &self.proposed_hash,
            &self.payload_hash,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplicaDurabilityMode {
    LocalStableMedia,
    ManagedVolume,
    ReplicatedVolume,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicaDurabilityAcknowledgement {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub request_hash: String,
    pub proposed_generation: u64,
    pub proposed_hash: String,
    pub replica_id: String,
    pub durability_mode: ReplicaDurabilityMode,
    pub flush_sequence: u64,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub event_hash: String,
}

#[derive(Debug, Serialize)]
struct ReplicaAckPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    request_hash: &'a str,
    proposed_generation: u64,
    proposed_hash: &'a str,
    replica_id: &'a str,
    durability_mode: &'a ReplicaDurabilityMode,
    flush_sequence: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    public_key: &'a [u8],
}

impl ReplicaDurabilityAcknowledgement {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        request_hash: &str,
        proposed_generation: u64,
        proposed_hash: &str,
        replica_id: &str,
        durability_mode: ReplicaDurabilityMode,
        flush_sequence: u64,
        observed_tick: u64,
        ttl_ticks: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, ReplicatedDurabilityError> {
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut acknowledgement = Self {
            domain: REPLICA_ACK_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            request_hash: request_hash.to_string(),
            proposed_generation,
            proposed_hash: proposed_hash.to_string(),
            replica_id: replica_id.to_string(),
            durability_mode,
            flush_sequence,
            observed_tick,
            ttl_ticks,
            public_key,
            signature: vec![0; 64],
            event_hash: "0".repeat(MAX_HASH_BYTES),
        };
        acknowledgement.validate_shape()?;
        let payload = acknowledgement.canonical_payload()?;
        acknowledgement.signature = signing_key.sign(&payload).to_bytes().to_vec();
        acknowledgement.event_hash = acknowledgement.content_hash()?;
        acknowledgement.validate_shape()?;
        Ok(acknowledgement)
    }

    pub fn verify(
        &self,
        registry: &BTreeMap<String, Vec<u8>>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), ReplicatedDurabilityError> {
        self.validate_shape()?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica acknowledgement is bound to a different resource".into(),
            ));
        }
        let expected = registry
            .get(&self.replica_id)
            .ok_or_else(|| ReplicatedDurabilityError::UnknownReplica(self.replica_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica key does not match its pinned key".into(),
            ));
        }
        let key_bytes: [u8; 32] = self.public_key.as_slice().try_into().map_err(|_| {
            ReplicatedDurabilityError::Rejected("replica key shape is invalid".into())
        })?;
        let key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| ReplicatedDurabilityError::Rejected("replica key is invalid".into()))?;
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            ReplicatedDurabilityError::Rejected("replica signature is invalid".into())
        })?;
        key.verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                ReplicatedDurabilityError::Rejected("replica signature is invalid".into())
            })?;
        if self.event_hash != self.content_hash()? {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica acknowledgement hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), ReplicatedDurabilityError> {
        if self.domain != REPLICA_ACK_DOMAIN || self.protocol_version != 1 {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica acknowledgement domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.snapshot_id, "snapshot")?;
        validate_identifier(&self.replica_id, "replica")?;
        validate_hash(&self.request_hash, "request hash")?;
        validate_hash(&self.proposed_hash, "proposed hash")?;
        validate_hash(&self.event_hash, "event hash")?;
        if self.flush_sequence == 0 || self.ttl_ticks == 0 || self.ttl_ticks > 100_000 {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica flush sequence or TTL is invalid".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica key or signature shape is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReplicatedDurabilityError> {
        serde_json::to_vec(&ReplicaAckPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            request_hash: &self.request_hash,
            proposed_generation: self.proposed_generation,
            proposed_hash: &self.proposed_hash,
            replica_id: &self.replica_id,
            durability_mode: &self.durability_mode,
            flush_sequence: self.flush_sequence,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            public_key: &self.public_key,
        })
        .map_err(|error| ReplicatedDurabilityError::InvalidInput(error.to_string()))
    }

    fn content_hash(&self) -> Result<String, ReplicatedDurabilityError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.request_hash,
            self.proposed_generation,
            &self.proposed_hash,
            &self.replica_id,
            &self.durability_mode,
            self.flush_sequence,
            self.observed_tick,
            self.ttl_ticks,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasState {
    pub snapshot_id: String,
    pub generation: u64,
    pub content_hash: String,
    pub writer_id: String,
    pub writer_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasCommitReceipt {
    pub request_nonce: String,
    pub request_hash: String,
    pub snapshot_id: String,
    pub generation: u64,
    pub content_hash: String,
    pub quorum_count: usize,
    pub replica_set_hash: String,
    pub receipt_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CasDurabilitySnapshot {
    pub cluster_id: String,
    pub resource_id: String,
    pub state: CasState,
    pub committed_requests: BTreeMap<String, CasCommitReceipt>,
    pub state_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CasCommitOutcome {
    Committed(CasCommitReceipt),
    Idempotent(CasCommitReceipt),
}

#[derive(Debug, Clone)]
pub struct CasPreAdmissionContext {
    cluster_id: String,
    resource_id: String,
    snapshot_id: String,
    required_quorum: usize,
    writers: BTreeMap<String, Vec<u8>>,
    replicas: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasPreAdmissionEvidence {
    pub request_hash: String,
    pub verified_replica_count: usize,
}

#[derive(Debug, Clone)]
pub struct SingleWriterCasStore {
    cluster_id: String,
    resource_id: String,
    required_quorum: usize,
    max_committed_requests: usize,
    state: CasState,
    writers: BTreeMap<String, Vec<u8>>,
    replicas: BTreeMap<String, Vec<u8>>,
    committed_requests: BTreeMap<String, CasCommitReceipt>,
}

impl CasPreAdmissionContext {
    pub fn verify(
        &self,
        request: &CasWriteRequest,
        acknowledgements: &[ReplicaDurabilityAcknowledgement],
        current_tick: u64,
    ) -> Result<CasPreAdmissionEvidence, ReplicatedDurabilityError> {
        request.verify(
            &self.writers,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
        )?;
        if request.proposed_hash != request.payload_hash {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS proposed hash does not match payload hash".into(),
            ));
        }
        let mut accepted: BTreeMap<String, String> = BTreeMap::new();
        for acknowledgement in acknowledgements {
            acknowledgement.verify(
                &self.replicas,
                &self.cluster_id,
                &self.resource_id,
                &self.snapshot_id,
            )?;
            if acknowledgement.request_hash != request.request_hash
                || acknowledgement.proposed_generation != request.proposed_generation
                || acknowledgement.proposed_hash != request.proposed_hash
            {
                return Err(ReplicatedDurabilityError::Rejected(
                    "replica acknowledgement is not bound to the CAS request".into(),
                ));
            }
            if acknowledgement.observed_tick > current_tick
                || current_tick
                    > acknowledgement
                        .observed_tick
                        .saturating_add(acknowledgement.ttl_ticks)
            {
                return Err(ReplicatedDurabilityError::Rejected(
                    "replica acknowledgement is stale or future-dated".into(),
                ));
            }
            if let Some(previous_hash) = accepted.insert(
                acknowledgement.replica_id.clone(),
                acknowledgement.event_hash.clone(),
            ) {
                if previous_hash != acknowledgement.event_hash {
                    return Err(ReplicatedDurabilityError::Conflict(
                        "replica supplied conflicting acknowledgements".into(),
                    ));
                }
            }
        }
        if accepted.len() < self.required_quorum {
            return Err(ReplicatedDurabilityError::QuorumUnavailable);
        }
        Ok(CasPreAdmissionEvidence {
            request_hash: request.request_hash.clone(),
            verified_replica_count: accepted.len(),
        })
    }
}

impl SingleWriterCasStore {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        required_quorum: usize,
        max_committed_requests: usize,
    ) -> Result<Self, ReplicatedDurabilityError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        validate_identifier(snapshot_id, "snapshot")?;
        if required_quorum == 0
            || required_quorum > MAX_REPLICAS
            || max_committed_requests == 0
            || max_committed_requests > MAX_COMMITTED_REQUESTS
        {
            return Err(ReplicatedDurabilityError::InvalidInput(
                "CAS quorum or nonce bound is outside the safe range".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            required_quorum,
            max_committed_requests,
            state: CasState {
                snapshot_id: snapshot_id.to_string(),
                generation: 0,
                content_hash: "0".repeat(MAX_HASH_BYTES),
                writer_id: "unassigned".to_string(),
                writer_epoch: 0,
            },
            writers: BTreeMap::new(),
            replicas: BTreeMap::new(),
            committed_requests: BTreeMap::new(),
        })
    }

    pub fn register_writer(
        &mut self,
        writer_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), ReplicatedDurabilityError> {
        validate_identifier(writer_id, "writer")?;
        register_pinned_key(&mut self.writers, writer_id, verifying_key, "writer")
    }

    pub fn register_replica(
        &mut self,
        replica_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), ReplicatedDurabilityError> {
        validate_identifier(replica_id, "replica")?;
        if self.replicas.len() >= MAX_REPLICAS && !self.replicas.contains_key(replica_id) {
            return Err(ReplicatedDurabilityError::Rejected(
                "replica registry capacity exceeded".into(),
            ));
        }
        register_pinned_key(&mut self.replicas, replica_id, verifying_key, "replica")
    }

    pub fn state(&self) -> &CasState {
        &self.state
    }

    pub fn pre_admission_context(&self) -> CasPreAdmissionContext {
        CasPreAdmissionContext {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            snapshot_id: self.state.snapshot_id.clone(),
            required_quorum: self.required_quorum,
            writers: self.writers.clone(),
            replicas: self.replicas.clone(),
        }
    }

    pub fn committed_request_count(&self) -> usize {
        self.committed_requests.len()
    }

    pub fn commit(
        &mut self,
        request: CasWriteRequest,
        acknowledgements: &[ReplicaDurabilityAcknowledgement],
        current_tick: u64,
    ) -> Result<CasCommitOutcome, ReplicatedDurabilityError> {
        request.verify(
            &self.writers,
            &self.cluster_id,
            &self.resource_id,
            &self.state.snapshot_id,
        )?;
        if let Some(existing) = self.committed_requests.get(&request.request_nonce) {
            if existing.request_hash == request.request_hash {
                return Ok(CasCommitOutcome::Idempotent(existing.clone()));
            }
            return Err(ReplicatedDurabilityError::Conflict(
                "request nonce was reused for a different CAS request".into(),
            ));
        }
        if self.committed_requests.len() >= self.max_committed_requests {
            return Err(ReplicatedDurabilityError::Rejected(
                "committed request nonce capacity exceeded".into(),
            ));
        }
        if request.writer_epoch < self.state.writer_epoch {
            return Err(ReplicatedDurabilityError::ReplayRejected(
                "writer epoch regressed".into(),
            ));
        }
        if request.writer_epoch == self.state.writer_epoch
            && self.state.writer_id != "unassigned"
            && request.writer_id != self.state.writer_id
        {
            return Err(ReplicatedDurabilityError::Conflict(
                "writer identity changed without an epoch transition".into(),
            ));
        }
        if request.expected_generation != self.state.generation
            || request.expected_hash != self.state.content_hash
        {
            return Err(ReplicatedDurabilityError::CasMismatch);
        }
        if request.proposed_generation != self.state.generation.saturating_add(1) {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS proposed generation is not the next generation".into(),
            ));
        }
        if request.proposed_hash != request.payload_hash {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS proposed hash does not match payload hash".into(),
            ));
        }
        let mut accepted: BTreeMap<String, String> = BTreeMap::new();
        for acknowledgement in acknowledgements {
            acknowledgement.verify(
                &self.replicas,
                &self.cluster_id,
                &self.resource_id,
                &self.state.snapshot_id,
            )?;
            if acknowledgement.request_hash != request.request_hash
                || acknowledgement.proposed_generation != request.proposed_generation
                || acknowledgement.proposed_hash != request.proposed_hash
            {
                return Err(ReplicatedDurabilityError::Rejected(
                    "replica acknowledgement is not bound to the CAS request".into(),
                ));
            }
            if acknowledgement.observed_tick > current_tick
                || current_tick
                    > acknowledgement
                        .observed_tick
                        .saturating_add(acknowledgement.ttl_ticks)
            {
                return Err(ReplicatedDurabilityError::Rejected(
                    "replica acknowledgement is stale or future-dated".into(),
                ));
            }
            if let Some(previous_hash) = accepted.insert(
                acknowledgement.replica_id.clone(),
                acknowledgement.event_hash.clone(),
            ) {
                if previous_hash != acknowledgement.event_hash {
                    return Err(ReplicatedDurabilityError::Conflict(
                        "replica supplied conflicting acknowledgements".into(),
                    ));
                }
            }
        }
        if accepted.len() < self.required_quorum {
            return Err(ReplicatedDurabilityError::QuorumUnavailable);
        }
        let replica_set_hash = digest_json(&accepted.keys().collect::<Vec<_>>())?;
        let receipt_hash = digest_json(&(
            &request.request_nonce,
            &request.request_hash,
            &request.snapshot_id,
            request.proposed_generation,
            &request.proposed_hash,
            accepted.len(),
            &replica_set_hash,
        ))?;
        let receipt = CasCommitReceipt {
            request_nonce: request.request_nonce.clone(),
            request_hash: request.request_hash.clone(),
            snapshot_id: request.snapshot_id.clone(),
            generation: request.proposed_generation,
            content_hash: request.proposed_hash.clone(),
            quorum_count: accepted.len(),
            replica_set_hash,
            receipt_hash,
        };
        let next_state = CasState {
            snapshot_id: request.snapshot_id,
            generation: request.proposed_generation,
            content_hash: request.proposed_hash,
            writer_id: request.writer_id,
            writer_epoch: request.writer_epoch,
        };
        self.state = next_state;
        self.committed_requests
            .insert(request.request_nonce, receipt.clone());
        Ok(CasCommitOutcome::Committed(receipt))
    }

    pub fn snapshot(&self) -> Result<CasDurabilitySnapshot, ReplicatedDurabilityError> {
        let state_hash = snapshot_hash(
            &self.cluster_id,
            &self.resource_id,
            &self.state,
            &self.committed_requests,
        )?;
        Ok(CasDurabilitySnapshot {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            state: self.state.clone(),
            committed_requests: self.committed_requests.clone(),
            state_hash,
        })
    }

    pub fn restore(
        &mut self,
        snapshot: CasDurabilitySnapshot,
    ) -> Result<(), ReplicatedDurabilityError> {
        validate_snapshot(
            &snapshot,
            &self.cluster_id,
            &self.resource_id,
            &self.state.snapshot_id,
            self.max_committed_requests,
        )?;
        self.state = snapshot.state;
        self.committed_requests = snapshot.committed_requests;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CasDurabilitySnapshotStore {
    path: PathBuf,
    cluster_id: String,
    resource_id: String,
    snapshot_id: String,
}

impl CasDurabilitySnapshotStore {
    pub fn new(
        path: impl Into<PathBuf>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<Self, ReplicatedDurabilityError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        validate_identifier(snapshot_id, "snapshot")?;
        Ok(Self {
            path: path.into(),
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
        })
    }

    pub fn save(&self, snapshot: &CasDurabilitySnapshot) -> Result<(), ReplicatedDurabilityError> {
        validate_snapshot(
            snapshot,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            MAX_COMMITTED_REQUESTS,
        )?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(ReplicatedDurabilityError::PersistenceFailed(
                "CAS durability snapshot exceeds size bound".into(),
            ));
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        fs::rename(&staging, &self.path)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<CasDurabilitySnapshot>, ReplicatedDurabilityError> {
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        }
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(ReplicatedDurabilityError::PersistenceFailed(
                "CAS durability snapshot exceeds size bound".into(),
            ));
        }
        let snapshot: CasDurabilitySnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| ReplicatedDurabilityError::PersistenceFailed(error.to_string()))?;
        validate_snapshot(
            &snapshot,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            MAX_COMMITTED_REQUESTS,
        )?;
        Ok(Some(snapshot))
    }
}

fn validate_snapshot(
    snapshot: &CasDurabilitySnapshot,
    cluster_id: &str,
    resource_id: &str,
    snapshot_id: &str,
    max_committed_requests: usize,
) -> Result<(), ReplicatedDurabilityError> {
    if snapshot.cluster_id != cluster_id
        || snapshot.resource_id != resource_id
        || snapshot.state.snapshot_id != snapshot_id
    {
        return Err(ReplicatedDurabilityError::Rejected(
            "CAS durability snapshot identity mismatch".into(),
        ));
    }
    if snapshot.committed_requests.len() > max_committed_requests {
        return Err(ReplicatedDurabilityError::Rejected(
            "CAS durability snapshot nonce cardinality exceeds bound".into(),
        ));
    }
    validate_identifier(&snapshot.cluster_id, "cluster")?;
    validate_identifier(&snapshot.resource_id, "resource")?;
    validate_identifier(&snapshot.state.snapshot_id, "snapshot")?;
    validate_hash(&snapshot.state.content_hash, "state content hash")?;
    if snapshot.state.writer_epoch == 0 && snapshot.state.writer_id != "unassigned" {
        return Err(ReplicatedDurabilityError::Rejected(
            "unassigned CAS state has an invalid writer epoch".into(),
        ));
    }
    for (nonce, receipt) in &snapshot.committed_requests {
        validate_identifier(nonce, "request nonce")?;
        validate_hash(&receipt.request_hash, "receipt request hash")?;
        validate_hash(&receipt.content_hash, "receipt content hash")?;
        validate_hash(&receipt.replica_set_hash, "receipt replica set hash")?;
        validate_hash(&receipt.receipt_hash, "receipt hash")?;
        if receipt.request_nonce != *nonce
            || receipt.snapshot_id != snapshot.state.snapshot_id
            || receipt.generation == 0
            || receipt.generation > snapshot.state.generation
            || receipt.quorum_count == 0
            || receipt.quorum_count > MAX_REPLICAS
        {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS durability receipt is inconsistent".into(),
            ));
        }
        let expected_receipt_hash = digest_json(&(
            &receipt.request_nonce,
            &receipt.request_hash,
            &receipt.snapshot_id,
            receipt.generation,
            &receipt.content_hash,
            receipt.quorum_count,
            &receipt.replica_set_hash,
        ))?;
        if receipt.receipt_hash != expected_receipt_hash {
            return Err(ReplicatedDurabilityError::Rejected(
                "CAS durability receipt hash mismatch".into(),
            ));
        }
    }
    let generations = snapshot
        .committed_requests
        .values()
        .map(|receipt| receipt.generation)
        .collect::<BTreeSet<_>>();
    if snapshot.state.generation != generations.len() as u64
        || generations
            .iter()
            .copied()
            .enumerate()
            .any(|(index, generation)| generation != index as u64 + 1)
    {
        return Err(ReplicatedDurabilityError::Rejected(
            "CAS durability generations are not contiguous".into(),
        ));
    }
    let expected_hash = snapshot_hash(
        &snapshot.cluster_id,
        &snapshot.resource_id,
        &snapshot.state,
        &snapshot.committed_requests,
    )?;
    if snapshot.state_hash != expected_hash {
        return Err(ReplicatedDurabilityError::Rejected(
            "CAS durability snapshot hash mismatch".into(),
        ));
    }
    Ok(())
}

fn snapshot_hash(
    cluster_id: &str,
    resource_id: &str,
    state: &CasState,
    committed_requests: &BTreeMap<String, CasCommitReceipt>,
) -> Result<String, ReplicatedDurabilityError> {
    digest_json(&(cluster_id, resource_id, state, committed_requests))
}

fn register_pinned_key(
    registry: &mut BTreeMap<String, Vec<u8>>,
    identity: &str,
    verifying_key: &VerifyingKey,
    label: &str,
) -> Result<(), ReplicatedDurabilityError> {
    let key = verifying_key.to_bytes().to_vec();
    if let Some(existing) = registry.get(identity) {
        if existing != &key {
            return Err(ReplicatedDurabilityError::Rejected(format!(
                "{label} key rebinding requires an explicit transition"
            )));
        }
        return Ok(());
    }
    registry.insert(identity.to_string(), key);
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, ReplicatedDurabilityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ReplicatedDurabilityError::InvalidInput(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ReplicatedDurabilityError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(ReplicatedDurabilityError::Rejected(format!(
            "{label} identifier is outside its bound"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), ReplicatedDurabilityError> {
    if value.len() != MAX_HASH_BYTES
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ReplicatedDurabilityError::Rejected(format!(
            "{label} is not a bounded hexadecimal digest"
        )));
    }
    Ok(())
}
