use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use thiserror::Error;

pub const OWNERSHIP_CLAIM_DOMAIN: &str = "un1c0/cross-process-ownership/v1";
pub const RECOVERY_EVIDENCE_DOMAIN: &str = "un1c0/managed-volume-recovery/v1";
pub const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_NONCE_BYTES: usize = 128;
const MAX_REPLICAS: usize = 32;
const MAX_LEASE_TICKS: u64 = 100_000;
const MAX_EVIDENCE: usize = 32;
const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
const LOCK_RETRIES: usize = 32;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OwnershipError {
    #[error("ownership input is invalid: {0}")]
    InvalidInput(String),
    #[error("ownership evidence was rejected: {0}")]
    Rejected(String),
    #[error("ownership key is unknown: {0}")]
    UnknownOwner(String),
    #[error("recovery replica is unknown: {0}")]
    UnknownReplica(String),
    #[error("ownership record is busy")]
    Busy,
    #[error("ownership lease is expired")]
    LeaseExpired,
    #[error("ownership epoch is stale")]
    StaleEpoch,
    #[error("ownership record hash does not match")]
    RecordMismatch,
    #[error("ownership conflict: {0}")]
    Conflict(String),
    #[error("managed-volume recovery quorum is unavailable")]
    QuorumUnavailable,
    #[error("ownership persistence failed: {0}")]
    PersistenceFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnershipClaim {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub owner_id: String,
    pub process_instance: String,
    pub expected_record_hash: String,
    pub requested_epoch: u64,
    pub lease_expiry_tick: u64,
    pub generation: u64,
    pub content_hash: String,
    pub fencing_nonce: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub claim_hash: String,
}

#[derive(Debug, Serialize)]
struct OwnershipClaimPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    owner_id: &'a str,
    process_instance: &'a str,
    expected_record_hash: &'a str,
    requested_epoch: u64,
    lease_expiry_tick: u64,
    generation: u64,
    content_hash: &'a str,
    fencing_nonce: &'a str,
    public_key: &'a [u8],
}

impl OwnershipClaim {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        owner_id: &str,
        process_instance: &str,
        expected_record_hash: &str,
        requested_epoch: u64,
        lease_expiry_tick: u64,
        generation: u64,
        content_hash: &str,
        fencing_nonce: &str,
        signing_key: &SigningKey,
    ) -> Result<Self, OwnershipError> {
        let mut claim = Self {
            domain: OWNERSHIP_CLAIM_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            owner_id: owner_id.to_string(),
            process_instance: process_instance.to_string(),
            expected_record_hash: expected_record_hash.to_string(),
            requested_epoch,
            lease_expiry_tick,
            generation,
            content_hash: content_hash.to_string(),
            fencing_nonce: fencing_nonce.to_string(),
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            claim_hash: ZERO_HASH.to_string(),
        };
        claim.validate_shape()?;
        claim.signature = signing_key
            .sign(&claim.canonical_payload()?)
            .to_bytes()
            .to_vec();
        claim.claim_hash = claim.content_hash()?;
        claim.validate_shape()?;
        Ok(claim)
    }

    pub fn verify(
        &self,
        registry: &BTreeMap<String, Vec<u8>>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), OwnershipError> {
        self.validate_shape()?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(OwnershipError::Rejected(
                "ownership claim is bound to a different resource".into(),
            ));
        }
        let expected = registry
            .get(&self.owner_id)
            .ok_or_else(|| OwnershipError::UnknownOwner(self.owner_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(OwnershipError::Rejected(
                "owner key does not match its pinned key".into(),
            ));
        }
        let key_bytes: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| OwnershipError::Rejected("owner key shape is invalid".into()))?;
        let verifying_key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| OwnershipError::Rejected("owner key is invalid".into()))?;
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| OwnershipError::Rejected("ownership signature is invalid".into()))?;
        verifying_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| OwnershipError::Rejected("ownership signature is invalid".into()))?;
        if self.claim_hash != self.content_hash()? {
            return Err(OwnershipError::Rejected(
                "ownership claim hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), OwnershipError> {
        if self.domain != OWNERSHIP_CLAIM_DOMAIN || self.protocol_version != 1 {
            return Err(OwnershipError::Rejected(
                "ownership claim domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.snapshot_id, "snapshot")?;
        validate_identifier(&self.owner_id, "owner")?;
        validate_identifier(&self.process_instance, "process instance")?;
        validate_identifier(&self.fencing_nonce, "fencing nonce")?;
        if self.fencing_nonce.len() > MAX_NONCE_BYTES {
            return Err(OwnershipError::Rejected(
                "fencing nonce exceeds its bound".into(),
            ));
        }
        validate_hash(&self.expected_record_hash, "expected record hash")?;
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.claim_hash, "claim hash")?;
        if self.requested_epoch == 0
            || self.lease_expiry_tick == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(OwnershipError::Rejected(
                "ownership epoch, lease, key, or signature is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, OwnershipError> {
        serde_json::to_vec(&OwnershipClaimPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            owner_id: &self.owner_id,
            process_instance: &self.process_instance,
            expected_record_hash: &self.expected_record_hash,
            requested_epoch: self.requested_epoch,
            lease_expiry_tick: self.lease_expiry_tick,
            generation: self.generation,
            content_hash: &self.content_hash,
            fencing_nonce: &self.fencing_nonce,
            public_key: &self.public_key,
        })
        .map_err(|error| OwnershipError::InvalidInput(error.to_string()))
    }

    fn content_hash(&self) -> Result<String, OwnershipError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.owner_id,
            &self.process_instance,
            &self.expected_record_hash,
            self.requested_epoch,
            self.lease_expiry_tick,
            self.generation,
            &self.content_hash,
            &self.fencing_nonce,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnershipRecord {
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub owner_id: String,
    pub process_instance: String,
    pub ownership_epoch: u64,
    pub lease_expiry_tick: u64,
    pub generation: u64,
    pub content_hash: String,
    pub fencing_nonce: String,
    pub fenced: bool,
    pub record_hash: String,
}

impl OwnershipRecord {
    pub(crate) fn recompute_hash(&mut self) -> Result<(), OwnershipError> {
        self.record_hash = self.content_hash()?;
        Ok(())
    }

    fn content_hash(&self) -> Result<String, OwnershipError> {
        digest_json(&(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.owner_id,
            &self.process_instance,
            self.ownership_epoch,
            self.lease_expiry_tick,
            self.generation,
            &self.content_hash,
            &self.fencing_nonce,
            self.fenced,
        ))
    }

    fn validate(&self) -> Result<(), OwnershipError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.snapshot_id, "snapshot")?;
        validate_identifier(&self.owner_id, "owner")?;
        validate_identifier(&self.process_instance, "process instance")?;
        validate_identifier(&self.fencing_nonce, "fencing nonce")?;
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.record_hash, "record hash")?;
        if self.ownership_epoch == 0 || self.lease_expiry_tick == 0 {
            return Err(OwnershipError::Rejected(
                "ownership record epoch or lease is invalid".into(),
            ));
        }
        if self.record_hash != self.content_hash()? {
            return Err(OwnershipError::Rejected(
                "ownership record hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnershipWritePermit {
    pub owner_id: String,
    pub process_instance: String,
    pub ownership_epoch: u64,
    pub record_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnershipSnapshot {
    pub cluster_id: String,
    pub resource_id: String,
    pub record: Option<OwnershipRecord>,
    pub snapshot_hash: String,
}

#[derive(Debug, Clone)]
pub struct CrossProcessOwnershipStore {
    path: PathBuf,
    cluster_id: String,
    resource_id: String,
    snapshot_id: String,
    owners: BTreeMap<String, Vec<u8>>,
    max_lease_ticks: u64,
}

impl CrossProcessOwnershipStore {
    pub fn new(
        path: impl Into<PathBuf>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<Self, OwnershipError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        validate_identifier(snapshot_id, "snapshot")?;
        Ok(Self {
            path: path.into(),
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            owners: BTreeMap::new(),
            max_lease_ticks: MAX_LEASE_TICKS,
        })
    }

    pub fn register_owner(
        &mut self,
        owner_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), OwnershipError> {
        validate_identifier(owner_id, "owner")?;
        let key = verifying_key.to_bytes().to_vec();
        if let Some(existing) = self.owners.get(owner_id) {
            if existing != &key {
                return Err(OwnershipError::Rejected(
                    "owner key rebinding requires an explicit transition".into(),
                ));
            }
            return Ok(());
        }
        if self.owners.len() >= MAX_REPLICAS {
            return Err(OwnershipError::Rejected(
                "owner registry capacity exceeded".into(),
            ));
        }
        self.owners.insert(owner_id.to_string(), key);
        Ok(())
    }

    pub fn current(&self) -> Result<Option<OwnershipRecord>, OwnershipError> {
        self.load_unlocked()
    }

    pub fn acquire(
        &self,
        claim: OwnershipClaim,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipError> {
        claim.verify(
            &self.owners,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
        )?;
        self.with_lock(|| {
            let current = self.load_unlocked()?;
            if claim.lease_expiry_tick <= current_tick
                || claim.lease_expiry_tick - current_tick > self.max_lease_ticks
            {
                return Err(OwnershipError::Rejected(
                    "ownership claim lease is outside its bounded window".into(),
                ));
            }
            if let Some(existing) = current.as_ref() {
                existing.validate()?;
                if claim.expected_record_hash != existing.record_hash {
                    return Err(OwnershipError::RecordMismatch);
                }
                if !existing.fenced && current_tick < existing.lease_expiry_tick {
                    return Err(OwnershipError::Busy);
                }
                if claim.requested_epoch <= existing.ownership_epoch {
                    return Err(OwnershipError::StaleEpoch);
                }
            } else if claim.expected_record_hash != ZERO_HASH || claim.requested_epoch != 1 {
                return Err(OwnershipError::Rejected(
                    "initial ownership claim must use zero hash and epoch one".into(),
                ));
            }
            let mut record = OwnershipRecord {
                cluster_id: claim.cluster_id,
                resource_id: claim.resource_id,
                snapshot_id: claim.snapshot_id,
                owner_id: claim.owner_id,
                process_instance: claim.process_instance,
                ownership_epoch: claim.requested_epoch,
                lease_expiry_tick: claim.lease_expiry_tick,
                generation: claim.generation,
                content_hash: claim.content_hash,
                fencing_nonce: claim.fencing_nonce,
                fenced: false,
                record_hash: ZERO_HASH.to_string(),
            };
            record.record_hash = record.content_hash()?;
            self.persist_unlocked(&record)?;
            Ok(record)
        })
    }

    pub fn renew(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        new_expiry_tick: u64,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipError> {
        self.with_lock(|| {
            let mut record = self.load_required_unlocked()?;
            if record.fenced || current_tick >= record.lease_expiry_tick {
                return Err(OwnershipError::LeaseExpired);
            }
            if record.owner_id != owner_id
                || record.process_instance != process_instance
                || record.ownership_epoch != ownership_epoch
                || record.record_hash != expected_record_hash
            {
                return Err(OwnershipError::RecordMismatch);
            }
            if new_expiry_tick <= current_tick
                || new_expiry_tick - current_tick > self.max_lease_ticks
                || new_expiry_tick <= record.lease_expiry_tick
            {
                return Err(OwnershipError::Rejected(
                    "renewal lease is outside its bounded monotonic window".into(),
                ));
            }
            record.lease_expiry_tick = new_expiry_tick;
            record.record_hash = record.content_hash()?;
            self.persist_unlocked(&record)?;
            Ok(record)
        })
    }

    pub fn release(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        current_tick: u64,
    ) -> Result<OwnershipRecord, OwnershipError> {
        self.with_lock(|| {
            let mut record = self.load_required_unlocked()?;
            if record.owner_id != owner_id
                || record.process_instance != process_instance
                || record.ownership_epoch != ownership_epoch
                || record.record_hash != expected_record_hash
            {
                return Err(OwnershipError::RecordMismatch);
            }
            record.fenced = true;
            record.lease_expiry_tick = current_tick;
            record.record_hash = record.content_hash()?;
            self.persist_unlocked(&record)?;
            Ok(record)
        })
    }

    pub fn admit_write(
        &self,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        expected_record_hash: &str,
        current_tick: u64,
    ) -> Result<OwnershipWritePermit, OwnershipError> {
        let record = self.load_required_unlocked()?;
        record.validate()?;
        if record.fenced || current_tick >= record.lease_expiry_tick {
            return Err(OwnershipError::LeaseExpired);
        }
        if record.owner_id != owner_id
            || record.process_instance != process_instance
            || record.ownership_epoch != ownership_epoch
            || record.record_hash != expected_record_hash
        {
            return Err(OwnershipError::RecordMismatch);
        }
        Ok(OwnershipWritePermit {
            owner_id: owner_id.to_string(),
            process_instance: process_instance.to_string(),
            ownership_epoch,
            record_hash: expected_record_hash.to_string(),
        })
    }

    pub fn snapshot(&self) -> Result<OwnershipSnapshot, OwnershipError> {
        let record = self.load_unlocked()?;
        let snapshot_hash = digest_json(&(&self.cluster_id, &self.resource_id, &record))?;
        Ok(OwnershipSnapshot {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            record,
            snapshot_hash,
        })
    }

    pub(crate) fn with_owned_lock<T, E, F>(
        &self,
        permit: &OwnershipWritePermit,
        current_tick: u64,
        operation: F,
    ) -> Result<T, E>
    where
        E: From<OwnershipError>,
        F: FnOnce(&OwnershipRecord) -> Result<T, E>,
    {
        self.with_lock(|| {
            let record = self.load_required_unlocked().map_err(E::from)?;
            if record.fenced || current_tick >= record.lease_expiry_tick {
                return Err(E::from(OwnershipError::LeaseExpired));
            }
            if record.owner_id != permit.owner_id
                || record.process_instance != permit.process_instance
                || record.ownership_epoch != permit.ownership_epoch
                || record.record_hash != permit.record_hash
            {
                return Err(E::from(OwnershipError::RecordMismatch));
            }
            operation(&record)
        })
    }

    pub(crate) fn persist_owned_record(
        &self,
        record: &OwnershipRecord,
    ) -> Result<(), OwnershipError> {
        record.validate()?;
        self.persist_unlocked(record)
    }

    fn with_lock<T, E, F>(&self, operation: F) -> Result<T, E>
    where
        E: From<OwnershipError>,
        F: FnOnce() -> Result<T, E>,
    {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| E::from(OwnershipError::PersistenceFailed(error.to_string())))?;
        }
        let lock_path = self.path.with_extension("lock");
        let mut lock = None;
        for _ in 0..LOCK_RETRIES {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    file.write_all(std::process::id().to_string().as_bytes())
                        .map_err(|error| {
                            E::from(OwnershipError::PersistenceFailed(error.to_string()))
                        })?;
                    lock = Some(file);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    return Err(E::from(OwnershipError::PersistenceFailed(
                        error.to_string(),
                    )));
                }
            }
        }
        if lock.is_none() {
            return Err(E::from(OwnershipError::Busy));
        }
        let result = operation();
        drop(lock);
        let cleanup = fs::remove_file(&lock_path);
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(E::from(OwnershipError::PersistenceFailed(
                error.to_string(),
            ))),
            (Err(error), Ok(())) | (Err(error), Err(_)) => Err(error),
        }
    }

    fn load_required_unlocked(&self) -> Result<OwnershipRecord, OwnershipError> {
        self.load_unlocked()?.ok_or(OwnershipError::Rejected(
            "ownership record is not initialized".into(),
        ))
    }

    fn load_unlocked(&self) -> Result<Option<OwnershipRecord>, OwnershipError> {
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        }
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(OwnershipError::PersistenceFailed(
                "ownership record exceeds size bound".into(),
            ));
        }
        let record: OwnershipRecord = serde_json::from_slice(&bytes)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        if record.cluster_id != self.cluster_id
            || record.resource_id != self.resource_id
            || record.snapshot_id != self.snapshot_id
        {
            return Err(OwnershipError::Rejected(
                "ownership record identity mismatch".into(),
            ));
        }
        record.validate()?;
        Ok(Some(record))
    }

    fn persist_unlocked(&self, record: &OwnershipRecord) -> Result<(), OwnershipError> {
        record.validate()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        }
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        }
        let bytes = serde_json::to_vec(record)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        fs::rename(&staging, &self.path)
            .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| OwnershipError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ManagedVolumeRecoveryState {
    Prepared,
    Flushed,
    Replicated,
    Recovered,
    Unknown,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedVolumeRecoveryEvidence {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub generation: u64,
    pub content_hash: String,
    pub ownership_epoch: u64,
    pub replica_id: String,
    pub storage_adapter_id: String,
    pub state: ManagedVolumeRecoveryState,
    pub flush_sequence: u64,
    pub replication_sequence: u64,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub evidence_hash: String,
}

#[derive(Debug, Serialize)]
struct RecoveryEvidencePayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    generation: u64,
    content_hash: &'a str,
    ownership_epoch: u64,
    replica_id: &'a str,
    storage_adapter_id: &'a str,
    state: &'a ManagedVolumeRecoveryState,
    flush_sequence: u64,
    replication_sequence: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    public_key: &'a [u8],
}

impl ManagedVolumeRecoveryEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        generation: u64,
        content_hash: &str,
        ownership_epoch: u64,
        replica_id: &str,
        storage_adapter_id: &str,
        state: ManagedVolumeRecoveryState,
        flush_sequence: u64,
        replication_sequence: u64,
        observed_tick: u64,
        ttl_ticks: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, OwnershipError> {
        let mut evidence = Self {
            domain: RECOVERY_EVIDENCE_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            generation,
            content_hash: content_hash.to_string(),
            ownership_epoch,
            replica_id: replica_id.to_string(),
            storage_adapter_id: storage_adapter_id.to_string(),
            state,
            flush_sequence,
            replication_sequence,
            observed_tick,
            ttl_ticks,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            evidence_hash: ZERO_HASH.to_string(),
        };
        evidence.validate_shape()?;
        evidence.signature = signing_key
            .sign(&evidence.canonical_payload()?)
            .to_bytes()
            .to_vec();
        evidence.evidence_hash = evidence.content_hash_value()?;
        evidence.validate_shape()?;
        Ok(evidence)
    }

    pub fn verify(
        &self,
        registry: &BTreeMap<String, Vec<u8>>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), OwnershipError> {
        self.validate_shape()?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(OwnershipError::Rejected(
                "recovery evidence is bound to a different resource".into(),
            ));
        }
        let expected = registry
            .get(&self.replica_id)
            .ok_or_else(|| OwnershipError::UnknownReplica(self.replica_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(OwnershipError::Rejected(
                "recovery replica key does not match its pinned key".into(),
            ));
        }
        let key_bytes: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| OwnershipError::Rejected("recovery key shape is invalid".into()))?;
        let verifying_key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| OwnershipError::Rejected("recovery key is invalid".into()))?;
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| OwnershipError::Rejected("recovery signature is invalid".into()))?;
        verifying_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| OwnershipError::Rejected("recovery signature is invalid".into()))?;
        if self.evidence_hash != self.content_hash_value()? {
            return Err(OwnershipError::Rejected(
                "recovery evidence hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), OwnershipError> {
        if self.domain != RECOVERY_EVIDENCE_DOMAIN || self.protocol_version != 1 {
            return Err(OwnershipError::Rejected(
                "recovery evidence domain or protocol is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.snapshot_id, "snapshot")?;
        validate_identifier(&self.replica_id, "replica")?;
        validate_identifier(&self.storage_adapter_id, "storage adapter")?;
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.evidence_hash, "evidence hash")?;
        if self.generation == 0
            || self.ownership_epoch == 0
            || self.flush_sequence == 0
            || self.replication_sequence == 0
            || self.ttl_ticks == 0
            || self.ttl_ticks > MAX_LEASE_TICKS
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(OwnershipError::Rejected(
                "recovery evidence sequence, TTL, key, or signature is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, OwnershipError> {
        serde_json::to_vec(&RecoveryEvidencePayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            generation: self.generation,
            content_hash: &self.content_hash,
            ownership_epoch: self.ownership_epoch,
            replica_id: &self.replica_id,
            storage_adapter_id: &self.storage_adapter_id,
            state: &self.state,
            flush_sequence: self.flush_sequence,
            replication_sequence: self.replication_sequence,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            public_key: &self.public_key,
        })
        .map_err(|error| OwnershipError::InvalidInput(error.to_string()))
    }

    fn content_hash_value(&self) -> Result<String, OwnershipError> {
        let payload = self.canonical_payload()?;
        digest_json(&(payload, &self.signature))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryDecision {
    pub state: ManagedVolumeRecoveryState,
    pub snapshot_id: String,
    pub generation: u64,
    pub content_hash: String,
    pub ownership_epoch: u64,
    pub evidence_count: usize,
    pub replica_set_hash: String,
    pub decision_hash: String,
}

#[derive(Debug, Clone)]
pub struct ManagedVolumeRecoveryGate {
    cluster_id: String,
    resource_id: String,
    snapshot_id: String,
    required_quorum: usize,
    replicas: BTreeMap<String, Vec<u8>>,
}

impl ManagedVolumeRecoveryGate {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        required_quorum: usize,
    ) -> Result<Self, OwnershipError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        validate_identifier(snapshot_id, "snapshot")?;
        if required_quorum == 0 || required_quorum > MAX_REPLICAS {
            return Err(OwnershipError::InvalidInput(
                "recovery quorum is outside its bounded range".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            required_quorum,
            replicas: BTreeMap::new(),
        })
    }

    pub fn register_replica(
        &mut self,
        replica_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), OwnershipError> {
        validate_identifier(replica_id, "replica")?;
        let key = verifying_key.to_bytes().to_vec();
        if let Some(existing) = self.replicas.get(replica_id) {
            if existing != &key {
                return Err(OwnershipError::Rejected(
                    "recovery replica key rebinding is rejected".into(),
                ));
            }
            return Ok(());
        }
        if self.replicas.len() >= MAX_REPLICAS {
            return Err(OwnershipError::Rejected(
                "recovery replica registry capacity exceeded".into(),
            ));
        }
        self.replicas.insert(replica_id.to_string(), key);
        Ok(())
    }

    pub fn admit(
        &self,
        record: &OwnershipRecord,
        evidence: &[ManagedVolumeRecoveryEvidence],
        current_tick: u64,
    ) -> Result<RecoveryDecision, OwnershipError> {
        record.validate()?;
        if evidence.len() > MAX_EVIDENCE {
            return Err(OwnershipError::Rejected(
                "recovery evidence cardinality exceeds its bound".into(),
            ));
        }
        let mut accepted: BTreeMap<String, String> = BTreeMap::new();
        for item in evidence {
            item.verify(
                &self.replicas,
                &self.cluster_id,
                &self.resource_id,
                &self.snapshot_id,
            )?;
            if item.generation != record.generation
                || item.content_hash != record.content_hash
                || item.ownership_epoch != record.ownership_epoch
                || matches!(
                    item.state,
                    ManagedVolumeRecoveryState::Prepared
                        | ManagedVolumeRecoveryState::Flushed
                        | ManagedVolumeRecoveryState::Unknown
                        | ManagedVolumeRecoveryState::Failed
                )
            {
                return Err(OwnershipError::Rejected(
                    "recovery evidence is not a fresh replicated state".into(),
                ));
            }
            if item.observed_tick > current_tick
                || current_tick > item.observed_tick.saturating_add(item.ttl_ticks)
            {
                return Err(OwnershipError::Rejected(
                    "recovery evidence is stale or future-dated".into(),
                ));
            }
            if let Some(previous_hash) =
                accepted.insert(item.replica_id.clone(), item.evidence_hash.clone())
            {
                if previous_hash != item.evidence_hash {
                    return Err(OwnershipError::Conflict(
                        "recovery replica supplied conflicting evidence".into(),
                    ));
                }
            }
        }
        if accepted.len() < self.required_quorum {
            return Err(OwnershipError::QuorumUnavailable);
        }
        let replica_set_hash = digest_json(&accepted.keys().collect::<Vec<_>>())?;
        let decision_hash = digest_json(&(
            &record.snapshot_id,
            record.generation,
            &record.content_hash,
            record.ownership_epoch,
            &replica_set_hash,
        ))?;
        Ok(RecoveryDecision {
            state: ManagedVolumeRecoveryState::Recovered,
            snapshot_id: record.snapshot_id.clone(),
            generation: record.generation,
            content_hash: record.content_hash.clone(),
            ownership_epoch: record.ownership_epoch,
            evidence_count: accepted.len(),
            replica_set_hash,
            decision_hash,
        })
    }
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, OwnershipError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| OwnershipError::InvalidInput(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), OwnershipError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(OwnershipError::Rejected(format!(
            "{label} identifier is outside its bound"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), OwnershipError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(OwnershipError::Rejected(format!(
            "{label} is not a bounded hexadecimal digest"
        )));
    }
    Ok(())
}
