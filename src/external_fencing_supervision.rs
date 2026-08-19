use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;

pub const FENCING_HEARTBEAT_DOMAIN: &str = "un1c0/fencing-authority-heartbeat/v1";
pub const FENCE_CONSUMER_ACK_DOMAIN: &str = "un1c0/fence-consumer-ack/v1";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_HASH_BYTES: usize = 64;
const MAX_CONSUMERS: usize = 32;
const MAX_JOURNAL_ENTRIES: usize = 4096;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const MAX_TTL_TICKS: u64 = 1_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FencingSupervisionError {
    #[error("invalid fencing supervision input: {0}")]
    InvalidInput(String),
    #[error("unknown fencing authority: {0}")]
    UnknownAuthority(String),
    #[error("unknown fence consumer: {0}")]
    UnknownConsumer(String),
    #[error("fencing supervision evidence is stale: {0}")]
    StaleEvidence(String),
    #[error("fencing supervision replay rejected: {0}")]
    ReplayRejected(String),
    #[error("fencing supervision conflict: {0}")]
    Conflict(String),
    #[error("fencing supervision rejected: {0}")]
    Rejected(String),
    #[error("fencing supervision persistence failed: {0}")]
    PersistenceFailed(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FenceConsumerKind {
    WriteGateway,
    WorkerScheduler,
    SocketOwnership,
    Routing,
    ProcessFence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FenceApplicationOutcome {
    Applied,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisionKeyRegistry {
    authority_keys: BTreeMap<String, Vec<u8>>,
    consumer_keys: BTreeMap<String, Vec<u8>>,
}

impl SupervisionKeyRegistry {
    pub fn new() -> Self {
        Self {
            authority_keys: BTreeMap::new(),
            consumer_keys: BTreeMap::new(),
        }
    }

    pub fn register_authority(
        &mut self,
        authority_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), FencingSupervisionError> {
        validate_identifier(authority_id, "authority")?;
        register_pinned_key(
            &mut self.authority_keys,
            authority_id,
            verifying_key,
            "authority",
        )
    }

    pub fn register_consumer(
        &mut self,
        consumer_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), FencingSupervisionError> {
        validate_identifier(consumer_id, "consumer")?;
        register_pinned_key(
            &mut self.consumer_keys,
            consumer_id,
            verifying_key,
            "consumer",
        )
    }

    fn authority_key(&self, authority_id: &str) -> Result<VerifyingKey, FencingSupervisionError> {
        lookup_key(&self.authority_keys, authority_id, "authority")
    }

    fn consumer_key(&self, consumer_id: &str) -> Result<VerifyingKey, FencingSupervisionError> {
        lookup_key(&self.consumer_keys, consumer_id, "consumer")
    }
}

impl Default for SupervisionKeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FencingAuthorityHeartbeat {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub authority_id: String,
    pub membership_epoch: u64,
    pub fence_epoch: u64,
    pub log_index: u64,
    pub token_hash: String,
    pub state_hash: String,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct HeartbeatPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    authority_id: &'a str,
    membership_epoch: u64,
    fence_epoch: u64,
    log_index: u64,
    token_hash: &'a str,
    state_hash: &'a str,
    observed_tick: u64,
    ttl_ticks: u64,
    public_key: &'a [u8],
}

impl FencingAuthorityHeartbeat {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        authority_id: &str,
        membership_epoch: u64,
        fence_epoch: u64,
        log_index: u64,
        token_hash: &str,
        state_hash: &str,
        observed_tick: u64,
        ttl_ticks: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, FencingSupervisionError> {
        let mut heartbeat = Self {
            domain: FENCING_HEARTBEAT_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            authority_id: authority_id.to_string(),
            membership_epoch,
            fence_epoch,
            log_index,
            token_hash: token_hash.to_string(),
            state_hash: state_hash.to_string(),
            observed_tick,
            ttl_ticks,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            event_hash: "0".repeat(64),
        };
        heartbeat.validate_shape()?;
        heartbeat.signature = signing_key
            .sign(&heartbeat.canonical_payload()?)
            .to_bytes()
            .to_vec();
        heartbeat.event_hash = heartbeat.content_hash()?;
        Ok(heartbeat)
    }

    pub fn verify(
        &self,
        registry: &SupervisionKeyRegistry,
        expected_cluster_id: &str,
        expected_resource_id: &str,
    ) -> Result<(), FencingSupervisionError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.resource_id != expected_resource_id {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat cluster/resource mismatch".into(),
            ));
        }
        let trusted_key = registry.authority_key(&self.authority_id)?;
        if self.public_key != trusted_key.to_bytes() {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat signer mismatch".into(),
            ));
        }
        verify_signature(
            &trusted_key,
            &self.canonical_payload()?,
            &self.signature,
            "authority heartbeat",
        )?;
        if self.event_hash != self.content_hash()? {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat digest mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), FencingSupervisionError> {
        if self.domain != FENCING_HEARTBEAT_DOMAIN || self.protocol_version != 1 {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.authority_id, "authority")?;
        validate_hash(&self.token_hash, "token hash")?;
        validate_hash(&self.state_hash, "state hash")?;
        validate_hash(&self.event_hash, "event hash")?;
        if self.membership_epoch == 0
            || self.fence_epoch == 0
            || self.log_index == 0
            || self.ttl_ticks == 0
            || self.ttl_ticks > MAX_TTL_TICKS
        {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat generations or TTL are invalid".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(FencingSupervisionError::Rejected(
                "authority heartbeat key or signature shape is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, FencingSupervisionError> {
        serde_json::to_vec(&HeartbeatPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            authority_id: &self.authority_id,
            membership_epoch: self.membership_epoch,
            fence_epoch: self.fence_epoch,
            log_index: self.log_index,
            token_hash: &self.token_hash,
            state_hash: &self.state_hash,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            public_key: &self.public_key,
        })
        .map_err(|error| FencingSupervisionError::InvalidInput(error.to_string()))
    }

    fn content_hash(&self) -> Result<String, FencingSupervisionError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.authority_id,
            self.membership_epoch,
            self.fence_epoch,
            self.log_index,
            &self.token_hash,
            &self.state_hash,
            self.observed_tick,
            self.ttl_ticks,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FenceConsumerAcknowledgement {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub authority_id: String,
    pub consumer_id: String,
    pub consumer_kind: FenceConsumerKind,
    pub token_hash: String,
    pub owner_region_id: String,
    pub membership_epoch: u64,
    pub fence_epoch: u64,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub outcome: FenceApplicationOutcome,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct ConsumerAckPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    authority_id: &'a str,
    consumer_id: &'a str,
    consumer_kind: &'a FenceConsumerKind,
    token_hash: &'a str,
    owner_region_id: &'a str,
    membership_epoch: u64,
    fence_epoch: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    outcome: &'a FenceApplicationOutcome,
    public_key: &'a [u8],
}

impl FenceConsumerAcknowledgement {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        authority_id: &str,
        consumer_id: &str,
        consumer_kind: FenceConsumerKind,
        token_hash: &str,
        owner_region_id: &str,
        membership_epoch: u64,
        fence_epoch: u64,
        observed_tick: u64,
        ttl_ticks: u64,
        outcome: FenceApplicationOutcome,
        signing_key: &SigningKey,
    ) -> Result<Self, FencingSupervisionError> {
        let mut acknowledgement = Self {
            domain: FENCE_CONSUMER_ACK_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            authority_id: authority_id.to_string(),
            consumer_id: consumer_id.to_string(),
            consumer_kind,
            token_hash: token_hash.to_string(),
            owner_region_id: owner_region_id.to_string(),
            membership_epoch,
            fence_epoch,
            observed_tick,
            ttl_ticks,
            outcome,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            event_hash: "0".repeat(64),
        };
        acknowledgement.validate_shape()?;
        acknowledgement.signature = signing_key
            .sign(&acknowledgement.canonical_payload()?)
            .to_bytes()
            .to_vec();
        acknowledgement.event_hash = acknowledgement.content_hash()?;
        Ok(acknowledgement)
    }

    pub fn verify(
        &self,
        registry: &SupervisionKeyRegistry,
        expected_cluster_id: &str,
        expected_resource_id: &str,
    ) -> Result<(), FencingSupervisionError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.resource_id != expected_resource_id {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement cluster/resource mismatch".into(),
            ));
        }
        let trusted_key = registry.consumer_key(&self.consumer_id)?;
        if self.public_key != trusted_key.to_bytes() {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement signer mismatch".into(),
            ));
        }
        verify_signature(
            &trusted_key,
            &self.canonical_payload()?,
            &self.signature,
            "consumer acknowledgement",
        )?;
        if self.event_hash != self.content_hash()? {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement digest mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), FencingSupervisionError> {
        if self.domain != FENCE_CONSUMER_ACK_DOMAIN || self.protocol_version != 1 {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.authority_id, "authority")?;
        validate_identifier(&self.consumer_id, "consumer")?;
        validate_identifier(&self.owner_region_id, "owner region")?;
        validate_hash(&self.token_hash, "token hash")?;
        validate_hash(&self.event_hash, "event hash")?;
        if self.membership_epoch == 0
            || self.fence_epoch == 0
            || self.ttl_ticks == 0
            || self.ttl_ticks > MAX_TTL_TICKS
        {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement generations or TTL are invalid".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement key or signature shape is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, FencingSupervisionError> {
        serde_json::to_vec(&ConsumerAckPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            authority_id: &self.authority_id,
            consumer_id: &self.consumer_id,
            consumer_kind: &self.consumer_kind,
            token_hash: &self.token_hash,
            owner_region_id: &self.owner_region_id,
            membership_epoch: self.membership_epoch,
            fence_epoch: self.fence_epoch,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            outcome: &self.outcome,
            public_key: &self.public_key,
        })
        .map_err(|error| FencingSupervisionError::InvalidInput(error.to_string()))
    }

    fn content_hash(&self) -> Result<String, FencingSupervisionError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.authority_id,
            &self.consumer_id,
            &self.consumer_kind,
            &self.token_hash,
            &self.owner_region_id,
            self.membership_epoch,
            self.fence_epoch,
            self.observed_tick,
            self.ttl_ticks,
            &self.outcome,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FencingSupervisionStatus {
    AuthorityMissing,
    AuthorityStale,
    MissingConsumer,
    ConsumerStale,
    ConsumerQuarantined,
    GenerationMismatch,
    Quarantined,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisionJournalEntry {
    pub sequence: u64,
    pub evidence_hash: String,
    pub previous_hash: String,
    pub evidence_type: String,
    pub observed_tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FencingSupervisionReport {
    pub cluster_id: String,
    pub resource_id: String,
    pub status: FencingSupervisionStatus,
    pub authority_id: Option<String>,
    pub membership_epoch: u64,
    pub fence_epoch: u64,
    pub acknowledged_consumers: usize,
    pub required_consumers: usize,
    pub journal_entries: usize,
    pub trace_digest: String,
    pub safety_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FencingSupervisionSnapshot {
    pub authority: Option<FencingAuthorityHeartbeat>,
    pub consumer_acknowledgements: BTreeMap<String, FenceConsumerAcknowledgement>,
    pub quarantined: bool,
    pub journal: Vec<SupervisionJournalEntry>,
    pub state_hash: String,
}

#[derive(Debug, Clone)]
pub struct SupervisionSnapshotStore {
    path: PathBuf,
}

impl SupervisionSnapshotStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn save(
        &self,
        snapshot: &FencingSupervisionSnapshot,
    ) -> Result<(), FencingSupervisionError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(FencingSupervisionError::PersistenceFailed(
                "supervision snapshot exceeds size bound".into(),
            ));
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        }
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        fs::rename(&staging, &self.path)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<FencingSupervisionSnapshot>, FencingSupervisionError> {
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        }
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(FencingSupervisionError::PersistenceFailed(
                "supervision snapshot exceeds size bound".into(),
            ));
        }
        let snapshot: FencingSupervisionSnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| FencingSupervisionError::PersistenceFailed(error.to_string()))?;
        snapshot.validate()?;
        Ok(Some(snapshot))
    }
}

impl FencingSupervisionSnapshot {
    fn validate(&self) -> Result<(), FencingSupervisionError> {
        if self.consumer_acknowledgements.len() > MAX_CONSUMERS
            || self.journal.len() > MAX_JOURNAL_ENTRIES
        {
            return Err(FencingSupervisionError::Rejected(
                "supervision snapshot cardinality exceeds bound".into(),
            ));
        }
        for acknowledgement in self.consumer_acknowledgements.values() {
            acknowledgement.validate_shape()?;
        }
        if let Some(authority) = &self.authority {
            authority.validate_shape()?;
        }
        if self.state_hash.len() != MAX_HASH_BYTES
            || self.state_hash
                != snapshot_hash(
                    &self.authority,
                    &self.consumer_acknowledgements,
                    self.quarantined,
                    &self.journal,
                )?
        {
            return Err(FencingSupervisionError::Rejected(
                "supervision snapshot hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FencingSupervisor {
    cluster_id: String,
    resource_id: String,
    required_consumers: BTreeMap<String, FenceConsumerKind>,
    key_registry: SupervisionKeyRegistry,
    authority: Option<FencingAuthorityHeartbeat>,
    consumer_acknowledgements: BTreeMap<String, FenceConsumerAcknowledgement>,
    quarantined: bool,
    journal: Vec<SupervisionJournalEntry>,
    max_journal_entries: usize,
}

impl FencingSupervisor {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        required_consumers: BTreeMap<String, FenceConsumerKind>,
        max_journal_entries: usize,
    ) -> Result<Self, FencingSupervisionError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        if required_consumers.is_empty()
            || required_consumers.len() > MAX_CONSUMERS
            || max_journal_entries == 0
            || max_journal_entries > MAX_JOURNAL_ENTRIES
        {
            return Err(FencingSupervisionError::InvalidInput(
                "supervision consumer or journal bound is outside the safe range".into(),
            ));
        }
        for consumer_id in required_consumers.keys() {
            validate_identifier(consumer_id, "consumer")?;
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            required_consumers,
            key_registry: SupervisionKeyRegistry::new(),
            authority: None,
            consumer_acknowledgements: BTreeMap::new(),
            quarantined: false,
            journal: Vec::new(),
            max_journal_entries,
        })
    }

    pub fn register_authority(
        &mut self,
        authority_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), FencingSupervisionError> {
        self.key_registry
            .register_authority(authority_id, verifying_key)
    }

    pub fn register_consumer(
        &mut self,
        consumer_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), FencingSupervisionError> {
        if !self.required_consumers.contains_key(consumer_id) {
            return Err(FencingSupervisionError::UnknownConsumer(
                consumer_id.to_string(),
            ));
        }
        self.key_registry
            .register_consumer(consumer_id, verifying_key)
    }

    pub fn ingest_heartbeat(
        &mut self,
        heartbeat: FencingAuthorityHeartbeat,
        current_tick: u64,
    ) -> Result<(), FencingSupervisionError> {
        heartbeat.verify(&self.key_registry, &self.cluster_id, &self.resource_id)?;
        if heartbeat.observed_tick > current_tick {
            return Err(FencingSupervisionError::StaleEvidence(
                "authority heartbeat is dated in the future".into(),
            ));
        }
        if current_tick > heartbeat.observed_tick.saturating_add(heartbeat.ttl_ticks) {
            return Err(FencingSupervisionError::StaleEvidence(
                "authority heartbeat is already expired".into(),
            ));
        }
        if let Some(previous) = &self.authority {
            if heartbeat.authority_id != previous.authority_id {
                self.quarantined = true;
                return Err(FencingSupervisionError::Conflict(
                    "authority identity changed without an explicit transition".into(),
                ));
            }
            if heartbeat.membership_epoch < previous.membership_epoch
                || heartbeat.fence_epoch < previous.fence_epoch
                || heartbeat.log_index < previous.log_index
            {
                return Err(FencingSupervisionError::ReplayRejected(
                    "authority heartbeat generation regressed".into(),
                ));
            }
            if heartbeat.membership_epoch == previous.membership_epoch
                && heartbeat.fence_epoch == previous.fence_epoch
                && heartbeat.log_index == previous.log_index
            {
                if heartbeat.event_hash == previous.event_hash {
                    return Ok(());
                }
                self.quarantined = true;
                return Err(FencingSupervisionError::Conflict(
                    "same authority generation has conflicting heartbeat evidence".into(),
                ));
            }
        }
        let previous = self.authority.clone();
        self.authority = Some(heartbeat.clone());
        if let Err(error) = self.append_journal(
            heartbeat.event_hash.clone(),
            "authority_heartbeat",
            heartbeat.observed_tick,
        ) {
            self.authority = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn ingest_consumer_acknowledgement(
        &mut self,
        acknowledgement: FenceConsumerAcknowledgement,
        current_tick: u64,
    ) -> Result<(), FencingSupervisionError> {
        acknowledgement.verify(&self.key_registry, &self.cluster_id, &self.resource_id)?;
        let authority = self.authority.as_ref().ok_or_else(|| {
            FencingSupervisionError::Rejected(
                "consumer acknowledgement lacks authority context".into(),
            )
        })?;
        let expected_kind = self
            .required_consumers
            .get(&acknowledgement.consumer_id)
            .ok_or_else(|| {
                FencingSupervisionError::UnknownConsumer(acknowledgement.consumer_id.clone())
            })?;
        if expected_kind != &acknowledgement.consumer_kind {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement kind is not bound to its registry entry".into(),
            ));
        }
        if acknowledgement.observed_tick > current_tick {
            return Err(FencingSupervisionError::StaleEvidence(
                "consumer acknowledgement is dated in the future".into(),
            ));
        }
        if current_tick
            > acknowledgement
                .observed_tick
                .saturating_add(acknowledgement.ttl_ticks)
        {
            return Err(FencingSupervisionError::StaleEvidence(
                "consumer acknowledgement is already expired".into(),
            ));
        }
        if acknowledgement.authority_id != authority.authority_id
            || acknowledgement.membership_epoch != authority.membership_epoch
            || acknowledgement.fence_epoch != authority.fence_epoch
            || acknowledgement.token_hash != authority.token_hash
        {
            return Err(FencingSupervisionError::Rejected(
                "consumer acknowledgement is not bound to the current authority fence".into(),
            ));
        }
        if let Some(previous) = self
            .consumer_acknowledgements
            .get(&acknowledgement.consumer_id)
        {
            if acknowledgement.fence_epoch < previous.fence_epoch
                || acknowledgement.observed_tick < previous.observed_tick
            {
                return Err(FencingSupervisionError::ReplayRejected(
                    "consumer acknowledgement regressed".into(),
                ));
            }
            if acknowledgement.fence_epoch == previous.fence_epoch
                && acknowledgement.event_hash == previous.event_hash
            {
                return Ok(());
            }
            if acknowledgement.fence_epoch == previous.fence_epoch {
                self.quarantined = true;
                return Err(FencingSupervisionError::Conflict(
                    "consumer produced conflicting evidence for one fence epoch".into(),
                ));
            }
        }
        let previous = self
            .consumer_acknowledgements
            .insert(acknowledgement.consumer_id.clone(), acknowledgement.clone());
        if let Err(error) = self.append_journal(
            acknowledgement.event_hash.clone(),
            "consumer_acknowledgement",
            acknowledgement.observed_tick,
        ) {
            match previous {
                Some(previous) => {
                    self.consumer_acknowledgements
                        .insert(acknowledgement.consumer_id.clone(), previous);
                }
                None => {
                    self.consumer_acknowledgements
                        .remove(&acknowledgement.consumer_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn evaluate(&self, current_tick: u64) -> FencingSupervisionReport {
        let status = if self.quarantined {
            FencingSupervisionStatus::Quarantined
        } else if self.authority.is_none() {
            FencingSupervisionStatus::AuthorityMissing
        } else {
            let authority = self.authority.as_ref().expect("authority checked above");
            if current_tick > authority.observed_tick.saturating_add(authority.ttl_ticks) {
                FencingSupervisionStatus::AuthorityStale
            } else if let Some(consumer_id) = self
                .required_consumers
                .keys()
                .find(|consumer_id| !self.consumer_acknowledgements.contains_key(*consumer_id))
            {
                let _ = consumer_id;
                FencingSupervisionStatus::MissingConsumer
            } else if self
                .required_consumers
                .keys()
                .filter_map(|consumer_id| self.consumer_acknowledgements.get(consumer_id))
                .any(|ack| ack.outcome == FenceApplicationOutcome::Quarantined)
            {
                FencingSupervisionStatus::ConsumerQuarantined
            } else if self
                .required_consumers
                .keys()
                .filter_map(|consumer_id| self.consumer_acknowledgements.get(consumer_id))
                .any(|ack| current_tick > ack.observed_tick.saturating_add(ack.ttl_ticks))
            {
                FencingSupervisionStatus::ConsumerStale
            } else if self
                .required_consumers
                .keys()
                .filter_map(|consumer_id| self.consumer_acknowledgements.get(consumer_id))
                .any(|ack| {
                    ack.authority_id != authority.authority_id
                        || ack.membership_epoch != authority.membership_epoch
                        || ack.fence_epoch != authority.fence_epoch
                        || ack.token_hash != authority.token_hash
                })
            {
                FencingSupervisionStatus::GenerationMismatch
            } else {
                FencingSupervisionStatus::Ready
            }
        };
        let (authority_id, membership_epoch, fence_epoch) = self
            .authority
            .as_ref()
            .map(|authority| {
                (
                    Some(authority.authority_id.clone()),
                    authority.membership_epoch,
                    authority.fence_epoch,
                )
            })
            .unwrap_or((None, 0, 0));
        FencingSupervisionReport {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            status,
            authority_id,
            membership_epoch,
            fence_epoch,
            acknowledged_consumers: self.consumer_acknowledgements.len(),
            required_consumers: self.required_consumers.len(),
            journal_entries: self.journal.len(),
            trace_digest: digest_json(&self.journal).unwrap_or_default(),
            safety_passed: !matches!(status, FencingSupervisionStatus::Ready)
                || self.consumer_acknowledgements.len() == self.required_consumers.len(),
        }
    }

    pub fn snapshot(&self) -> Result<FencingSupervisionSnapshot, FencingSupervisionError> {
        let state_hash = snapshot_hash(
            &self.authority,
            &self.consumer_acknowledgements,
            self.quarantined,
            &self.journal,
        )?;
        Ok(FencingSupervisionSnapshot {
            authority: self.authority.clone(),
            consumer_acknowledgements: self.consumer_acknowledgements.clone(),
            quarantined: self.quarantined,
            journal: self.journal.clone(),
            state_hash,
        })
    }

    pub fn restore(
        &mut self,
        snapshot: FencingSupervisionSnapshot,
    ) -> Result<(), FencingSupervisionError> {
        snapshot.validate()?;
        if let Some(authority) = &snapshot.authority {
            authority.verify(&self.key_registry, &self.cluster_id, &self.resource_id)?;
        }
        for acknowledgement in snapshot.consumer_acknowledgements.values() {
            acknowledgement.verify(&self.key_registry, &self.cluster_id, &self.resource_id)?;
        }
        self.authority = snapshot.authority;
        self.consumer_acknowledgements = snapshot.consumer_acknowledgements;
        self.quarantined = snapshot.quarantined;
        self.journal = snapshot.journal;
        Ok(())
    }

    pub fn journal_integrity(&self) -> bool {
        let mut previous = "0".repeat(64);
        for (index, entry) in self.journal.iter().enumerate() {
            if entry.sequence != index as u64 + 1
                || entry.previous_hash != previous
                || entry.evidence_hash.len() != 64
                || !entry
                    .evidence_hash
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return false;
            }
            previous = entry.evidence_hash.clone();
        }
        true
    }

    fn append_journal(
        &mut self,
        evidence_hash: String,
        evidence_type: &str,
        observed_tick: u64,
    ) -> Result<(), FencingSupervisionError> {
        if self.journal.len() >= self.max_journal_entries {
            return Err(FencingSupervisionError::Rejected(
                "supervision journal capacity exceeded".into(),
            ));
        }
        let previous_hash = self
            .journal
            .last()
            .map(|entry| entry.evidence_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));
        self.journal.push(SupervisionJournalEntry {
            sequence: self.journal.len() as u64 + 1,
            evidence_hash,
            previous_hash,
            evidence_type: evidence_type.to_string(),
            observed_tick,
        });
        Ok(())
    }
}

fn register_pinned_key(
    registry: &mut BTreeMap<String, Vec<u8>>,
    identity: &str,
    verifying_key: &VerifyingKey,
    label: &str,
) -> Result<(), FencingSupervisionError> {
    let key = verifying_key.to_bytes().to_vec();
    if let Some(existing) = registry.get(identity) {
        if existing != &key {
            return Err(FencingSupervisionError::Rejected(format!(
                "{label} key rebinding requires an explicit transition"
            )));
        }
        return Ok(());
    }
    registry.insert(identity.to_string(), key);
    Ok(())
}

fn lookup_key(
    registry: &BTreeMap<String, Vec<u8>>,
    identity: &str,
    label: &str,
) -> Result<VerifyingKey, FencingSupervisionError> {
    let bytes = registry.get(identity).ok_or_else(|| {
        if label == "authority" {
            FencingSupervisionError::UnknownAuthority(identity.to_string())
        } else {
            FencingSupervisionError::UnknownConsumer(identity.to_string())
        }
    })?;
    let key_bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| FencingSupervisionError::Rejected(format!("{label} key length is invalid")))?;
    VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| FencingSupervisionError::Rejected(format!("{label} key encoding is invalid")))
}

fn verify_signature(
    trusted_key: &VerifyingKey,
    payload: &[u8],
    signature_bytes: &[u8],
    label: &str,
) -> Result<(), FencingSupervisionError> {
    let signature = Signature::from_slice(signature_bytes)
        .map_err(|_| FencingSupervisionError::Rejected(format!("{label} signature encoding")))?;
    trusted_key
        .verify(payload, &signature)
        .map_err(|_| FencingSupervisionError::Rejected(format!("{label} signature verification")))
}

fn snapshot_hash(
    authority: &Option<FencingAuthorityHeartbeat>,
    acknowledgements: &BTreeMap<String, FenceConsumerAcknowledgement>,
    quarantined: bool,
    journal: &[SupervisionJournalEntry],
) -> Result<String, FencingSupervisionError> {
    digest_json(&(authority, acknowledgements, quarantined, journal))
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, FencingSupervisionError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| FencingSupervisionError::InvalidInput(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), FencingSupervisionError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(FencingSupervisionError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), FencingSupervisionError> {
    if value.len() != MAX_HASH_BYTES
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(FencingSupervisionError::InvalidInput(format!(
            "{label} must be a 64-character hexadecimal digest"
        )));
    }
    Ok(())
}
