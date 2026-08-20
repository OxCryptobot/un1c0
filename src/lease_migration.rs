use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;

pub const LEASE_MIGRATION_DOMAIN: &str = "un1c0/lease-migration/v1";
pub const LEASE_MIGRATION_WITNESS_DOMAIN: &str = "un1c0/lease-migration-witness/v1";
pub const LEASE_MIGRATION_RELEASE_DOMAIN: &str = "un1c0/lease-migration-release/v1";
pub const LEASE_MIGRATION_ACTIVATION_DOMAIN: &str = "un1c0/lease-migration-activation/v1";
pub const LEASE_MIGRATION_SNAPSHOT_DOMAIN: &str = "un1c0/lease-migration-snapshot/v1";
const LEASE_MIGRATION_ZERO_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_NONCE_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 512;
const MAX_WITNESSES: usize = 64;
const MAX_HISTORY: usize = 64;
const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
const MAX_LEASE_TICKS: u64 = 100_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LeaseMigrationError {
    #[error("lease migration input is invalid: {0}")]
    InvalidInput(String),
    #[error("lease migration evidence was rejected: {0}")]
    Rejected(String),
    #[error("lease migration signer is unknown: {0}")]
    UnknownSigner(String),
    #[error("lease migration state conflict: {0}")]
    Conflict(String),
    #[error("lease migration state is not valid for this operation: {0}")]
    InvalidState(String),
    #[error("lease migration witness quorum is unavailable")]
    QuorumUnavailable,
    #[error("lease migration source has not been drained")]
    SourceNotDrained,
    #[error("lease migration source release is missing")]
    ReleaseMissing,
    #[error("lease migration epoch is stale")]
    EpochRegression,
    #[error("lease migration evidence is stale or expired")]
    StaleEvidence,
    #[error("lease migration evidence replay does not match prior evidence")]
    ReplayMismatch,
    #[error("lease migration persistence failed: {0}")]
    PersistenceFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LeaseMigrationState {
    Stable,
    Draining,
    Prepared,
    Released,
    Activated,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseRecord {
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub region_id: String,
    pub owner_id: String,
    pub process_instance: String,
    pub ownership_epoch: u64,
    pub lease_expiry_tick: u64,
    pub generation: u64,
    pub content_hash: String,
    pub record_hash: String,
}

impl LeaseRecord {
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        region_id: &str,
        owner_id: &str,
        process_instance: &str,
        ownership_epoch: u64,
        lease_expiry_tick: u64,
        generation: u64,
        content_hash: &str,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identity(
            cluster_id,
            resource_id,
            snapshot_id,
            region_id,
            owner_id,
            process_instance,
        )?;
        validate_hash(content_hash, "content hash")?;
        if ownership_epoch == 0 || lease_expiry_tick == 0 {
            return Err(LeaseMigrationError::InvalidInput(
                "initial lease epoch and expiry must be positive".into(),
            ));
        }
        let mut record = Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            region_id: region_id.to_string(),
            owner_id: owner_id.to_string(),
            process_instance: process_instance.to_string(),
            ownership_epoch,
            lease_expiry_tick,
            generation,
            content_hash: content_hash.to_string(),
            record_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        record.record_hash = record.compute_hash()?;
        Ok(record)
    }

    fn validate(
        &self,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), LeaseMigrationError> {
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.region_id,
            &self.owner_id,
            &self.process_instance,
        )?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(LeaseMigrationError::Rejected(
                "lease record identity does not match authority".into(),
            ));
        }
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.record_hash, "record hash")?;
        if self.ownership_epoch == 0
            || self.lease_expiry_tick == 0
            || self.record_hash != self.compute_hash()?
        {
            return Err(LeaseMigrationError::Rejected(
                "lease record epoch, expiry, or hash is invalid".into(),
            ));
        }
        Ok(())
    }

    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.region_id,
            &self.owner_id,
            &self.process_instance,
            self.ownership_epoch,
            self.lease_expiry_tick,
            self.generation,
            &self.content_hash,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationIntent {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub source_region: String,
    pub source_owner_id: String,
    pub source_process_instance: String,
    pub destination_region: String,
    pub destination_owner_id: String,
    pub destination_process_instance: String,
    pub current_ownership_epoch: u64,
    pub current_record_hash: String,
    pub generation: u64,
    pub content_hash: String,
    pub migration_nonce: String,
    pub requested_destination_epoch: u64,
    pub intent_expiry_tick: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub intent_hash: String,
}

#[derive(Debug, Serialize)]
struct IntentPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    source_region: &'a str,
    source_owner_id: &'a str,
    source_process_instance: &'a str,
    destination_region: &'a str,
    destination_owner_id: &'a str,
    destination_process_instance: &'a str,
    current_ownership_epoch: u64,
    current_record_hash: &'a str,
    generation: u64,
    content_hash: &'a str,
    migration_nonce: &'a str,
    requested_destination_epoch: u64,
    intent_expiry_tick: u64,
    public_key: &'a [u8],
}

impl LeaseMigrationIntent {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        source_region: &str,
        source_owner_id: &str,
        source_process_instance: &str,
        destination_region: &str,
        destination_owner_id: &str,
        destination_process_instance: &str,
        current_ownership_epoch: u64,
        current_record_hash: &str,
        generation: u64,
        content_hash: &str,
        migration_nonce: &str,
        requested_destination_epoch: u64,
        intent_expiry_tick: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identity(
            cluster_id,
            resource_id,
            snapshot_id,
            source_region,
            source_owner_id,
            source_process_instance,
        )?;
        validate_identity(
            cluster_id,
            resource_id,
            snapshot_id,
            destination_region,
            destination_owner_id,
            destination_process_instance,
        )?;
        validate_identifier(destination_region, "destination region")?;
        validate_identifier(migration_nonce, "migration nonce")?;
        validate_hash(current_record_hash, "current record hash")?;
        validate_hash(content_hash, "content hash")?;
        if migration_nonce.len() > MAX_NONCE_BYTES
            || current_ownership_epoch == 0
            || requested_destination_epoch <= current_ownership_epoch
            || intent_expiry_tick == 0
        {
            return Err(LeaseMigrationError::InvalidInput(
                "intent epoch, nonce, hash, or expiry is invalid".into(),
            ));
        }
        let mut intent = Self {
            domain: LEASE_MIGRATION_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            source_region: source_region.to_string(),
            source_owner_id: source_owner_id.to_string(),
            source_process_instance: source_process_instance.to_string(),
            destination_region: destination_region.to_string(),
            destination_owner_id: destination_owner_id.to_string(),
            destination_process_instance: destination_process_instance.to_string(),
            current_ownership_epoch,
            current_record_hash: current_record_hash.to_string(),
            generation,
            content_hash: content_hash.to_string(),
            migration_nonce: migration_nonce.to_string(),
            requested_destination_epoch,
            intent_expiry_tick,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            intent_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        intent.signature = signing_key.sign(&intent.payload()?).to_bytes().to_vec();
        intent.intent_hash = intent.compute_hash()?;
        intent.validate_shape()?;
        Ok(intent)
    }

    pub fn verify(
        &self,
        owner_keys: &BTreeMap<String, Vec<u8>>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
    ) -> Result<(), LeaseMigrationError> {
        self.validate_shape()?;
        if self.cluster_id != cluster_id
            || self.resource_id != resource_id
            || self.snapshot_id != snapshot_id
        {
            return Err(LeaseMigrationError::Rejected(
                "intent resource binding mismatch".into(),
            ));
        }
        let expected = owner_keys
            .get(&self.source_owner_id)
            .ok_or_else(|| LeaseMigrationError::UnknownSigner(self.source_owner_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(LeaseMigrationError::Rejected(
                "source owner key mismatch".into(),
            ));
        }
        verify_signature(&self.public_key, &self.signature, &self.payload()?)?;
        if self.intent_hash != self.compute_hash()? {
            return Err(LeaseMigrationError::Rejected("intent hash mismatch".into()));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), LeaseMigrationError> {
        if self.domain != LEASE_MIGRATION_DOMAIN || self.protocol_version != 1 {
            return Err(LeaseMigrationError::Rejected(
                "intent domain or version is invalid".into(),
            ));
        }
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.source_region,
            &self.source_owner_id,
            &self.source_process_instance,
        )?;
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.destination_region,
            &self.destination_owner_id,
            &self.destination_process_instance,
        )?;
        if self.source_region == self.destination_region
            || self.source_owner_id == self.destination_owner_id
            || self.source_process_instance == self.destination_process_instance
        {
            return Err(LeaseMigrationError::Rejected(
                "migration must change region, owner, and process".into(),
            ));
        }
        validate_hash(&self.current_record_hash, "current record hash")?;
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.intent_hash, "intent hash")?;
        validate_identifier(&self.migration_nonce, "migration nonce")?;
        if self.migration_nonce.len() > MAX_NONCE_BYTES
            || self.current_ownership_epoch == 0
            || self.requested_destination_epoch <= self.current_ownership_epoch
            || self.intent_expiry_tick == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(LeaseMigrationError::Rejected(
                "intent bounds are invalid".into(),
            ));
        }
        Ok(())
    }

    fn payload(&self) -> Result<Vec<u8>, LeaseMigrationError> {
        serde_json::to_vec(&IntentPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            source_region: &self.source_region,
            source_owner_id: &self.source_owner_id,
            source_process_instance: &self.source_process_instance,
            destination_region: &self.destination_region,
            destination_owner_id: &self.destination_owner_id,
            destination_process_instance: &self.destination_process_instance,
            current_ownership_epoch: self.current_ownership_epoch,
            current_record_hash: &self.current_record_hash,
            generation: self.generation,
            content_hash: &self.content_hash,
            migration_nonce: &self.migration_nonce,
            requested_destination_epoch: self.requested_destination_epoch,
            intent_expiry_tick: self.intent_expiry_tick,
            public_key: &self.public_key,
        })
        .map_err(|error| LeaseMigrationError::InvalidInput(error.to_string()))
    }

    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&serde_json::json!({
            "domain": &self.domain,
            "protocol_version": self.protocol_version,
            "cluster_id": &self.cluster_id,
            "resource_id": &self.resource_id,
            "snapshot_id": &self.snapshot_id,
            "source_region": &self.source_region,
            "source_owner_id": &self.source_owner_id,
            "source_process_instance": &self.source_process_instance,
            "destination_region": &self.destination_region,
            "destination_owner_id": &self.destination_owner_id,
            "destination_process_instance": &self.destination_process_instance,
            "current_ownership_epoch": self.current_ownership_epoch,
            "current_record_hash": &self.current_record_hash,
            "generation": self.generation,
            "content_hash": &self.content_hash,
            "migration_nonce": &self.migration_nonce,
            "requested_destination_epoch": self.requested_destination_epoch,
            "intent_expiry_tick": self.intent_expiry_tick,
            "public_key": &self.public_key,
            "signature": &self.signature,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationWitnessAck {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub intent_hash: String,
    pub witness_id: String,
    pub witness_membership_epoch: u64,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub acknowledgement_hash: String,
}

#[derive(Debug, Serialize)]
struct WitnessPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    intent_hash: &'a str,
    witness_id: &'a str,
    witness_membership_epoch: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    public_key: &'a [u8],
}

impl LeaseMigrationWitnessAck {
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        intent_hash: &str,
        witness_id: &str,
        witness_membership_epoch: u64,
        observed_tick: u64,
        ttl_ticks: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identity(
            cluster_id,
            resource_id,
            snapshot_id,
            "witness-region",
            witness_id,
            "witness-process",
        )?;
        validate_hash(intent_hash, "intent hash")?;
        if witness_membership_epoch == 0 || ttl_ticks == 0 || ttl_ticks > MAX_LEASE_TICKS {
            return Err(LeaseMigrationError::InvalidInput(
                "witness epoch or ttl is invalid".into(),
            ));
        }
        let mut ack = Self {
            domain: LEASE_MIGRATION_WITNESS_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            intent_hash: intent_hash.to_string(),
            witness_id: witness_id.to_string(),
            witness_membership_epoch,
            observed_tick,
            ttl_ticks,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            acknowledgement_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        ack.signature = signing_key.sign(&ack.payload()?).to_bytes().to_vec();
        ack.acknowledgement_hash = ack.compute_hash()?;
        ack.validate_shape()?;
        Ok(ack)
    }

    fn verify(
        &self,
        witness_keys: &BTreeMap<String, Vec<u8>>,
        intent: &LeaseMigrationIntent,
        current_tick: u64,
    ) -> Result<(), LeaseMigrationError> {
        self.validate_shape()?;
        if self.cluster_id != intent.cluster_id
            || self.resource_id != intent.resource_id
            || self.snapshot_id != intent.snapshot_id
            || self.intent_hash != intent.intent_hash
        {
            return Err(LeaseMigrationError::Rejected(
                "witness acknowledgement binding mismatch".into(),
            ));
        }
        if current_tick > intent.intent_expiry_tick
            || current_tick < self.observed_tick
            || current_tick - self.observed_tick > self.ttl_ticks
        {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        let expected = witness_keys
            .get(&self.witness_id)
            .ok_or_else(|| LeaseMigrationError::UnknownSigner(self.witness_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(LeaseMigrationError::Rejected("witness key mismatch".into()));
        }
        verify_signature(&self.public_key, &self.signature, &self.payload()?)?;
        if self.acknowledgement_hash != self.compute_hash()? {
            return Err(LeaseMigrationError::Rejected(
                "witness acknowledgement hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), LeaseMigrationError> {
        if self.domain != LEASE_MIGRATION_WITNESS_DOMAIN || self.protocol_version != 1 {
            return Err(LeaseMigrationError::Rejected(
                "witness domain or version is invalid".into(),
            ));
        }
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            "witness-region",
            &self.witness_id,
            "witness-process",
        )?;
        validate_hash(&self.intent_hash, "intent hash")?;
        validate_hash(&self.acknowledgement_hash, "acknowledgement hash")?;
        if self.witness_membership_epoch == 0
            || self.ttl_ticks == 0
            || self.ttl_ticks > MAX_LEASE_TICKS
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(LeaseMigrationError::Rejected(
                "witness acknowledgement bounds are invalid".into(),
            ));
        }
        Ok(())
    }

    fn payload(&self) -> Result<Vec<u8>, LeaseMigrationError> {
        serde_json::to_vec(&WitnessPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            intent_hash: &self.intent_hash,
            witness_id: &self.witness_id,
            witness_membership_epoch: self.witness_membership_epoch,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            public_key: &self.public_key,
        })
        .map_err(|error| LeaseMigrationError::InvalidInput(error.to_string()))
    }

    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.intent_hash,
            &self.witness_id,
            self.witness_membership_epoch,
            self.observed_tick,
            self.ttl_ticks,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationRelease {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub intent_hash: String,
    pub source_owner_id: String,
    pub source_process_instance: String,
    pub source_ownership_epoch: u64,
    pub source_record_hash: String,
    pub release_tick: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub release_hash: String,
}

#[derive(Debug, Serialize)]
struct ReleasePayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    intent_hash: &'a str,
    source_owner_id: &'a str,
    source_process_instance: &'a str,
    source_ownership_epoch: u64,
    source_record_hash: &'a str,
    release_tick: u64,
    public_key: &'a [u8],
}

impl LeaseMigrationRelease {
    pub fn sign(
        intent: &LeaseMigrationIntent,
        source_owner_id: &str,
        source_process_instance: &str,
        source_ownership_epoch: u64,
        source_record_hash: &str,
        release_tick: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identifier(source_owner_id, "source owner")?;
        validate_identifier(source_process_instance, "source process")?;
        validate_hash(source_record_hash, "source record hash")?;
        if source_ownership_epoch == 0 || release_tick == 0 {
            return Err(LeaseMigrationError::InvalidInput(
                "release epoch or tick is invalid".into(),
            ));
        }
        let mut release = Self {
            domain: LEASE_MIGRATION_RELEASE_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: intent.cluster_id.clone(),
            resource_id: intent.resource_id.clone(),
            snapshot_id: intent.snapshot_id.clone(),
            intent_hash: intent.intent_hash.clone(),
            source_owner_id: source_owner_id.to_string(),
            source_process_instance: source_process_instance.to_string(),
            source_ownership_epoch,
            source_record_hash: source_record_hash.to_string(),
            release_tick,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            release_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        release.signature = signing_key.sign(&release.payload()?).to_bytes().to_vec();
        release.release_hash = release.compute_hash()?;
        release.validate_shape()?;
        Ok(release)
    }

    fn verify(
        &self,
        owner_keys: &BTreeMap<String, Vec<u8>>,
        intent: &LeaseMigrationIntent,
        current_tick: u64,
    ) -> Result<(), LeaseMigrationError> {
        self.validate_shape()?;
        if self.cluster_id != intent.cluster_id
            || self.resource_id != intent.resource_id
            || self.snapshot_id != intent.snapshot_id
            || self.intent_hash != intent.intent_hash
            || self.source_owner_id != intent.source_owner_id
            || self.source_process_instance != intent.source_process_instance
            || self.source_ownership_epoch != intent.current_ownership_epoch
            || self.source_record_hash != intent.current_record_hash
        {
            return Err(LeaseMigrationError::Rejected(
                "source release binding mismatch".into(),
            ));
        }
        if current_tick > intent.intent_expiry_tick || self.release_tick > intent.intent_expiry_tick
        {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        let expected = owner_keys
            .get(&self.source_owner_id)
            .ok_or_else(|| LeaseMigrationError::UnknownSigner(self.source_owner_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(LeaseMigrationError::Rejected(
                "source release key mismatch".into(),
            ));
        }
        verify_signature(&self.public_key, &self.signature, &self.payload()?)?;
        if self.release_hash != self.compute_hash()? {
            return Err(LeaseMigrationError::Rejected(
                "source release hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), LeaseMigrationError> {
        if self.domain != LEASE_MIGRATION_RELEASE_DOMAIN || self.protocol_version != 1 {
            return Err(LeaseMigrationError::Rejected(
                "release domain or version is invalid".into(),
            ));
        }
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            "source-region",
            &self.source_owner_id,
            &self.source_process_instance,
        )?;
        validate_hash(&self.intent_hash, "intent hash")?;
        validate_hash(&self.source_record_hash, "source record hash")?;
        validate_hash(&self.release_hash, "release hash")?;
        if self.source_ownership_epoch == 0
            || self.release_tick == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(LeaseMigrationError::Rejected(
                "release bounds are invalid".into(),
            ));
        }
        Ok(())
    }

    fn payload(&self) -> Result<Vec<u8>, LeaseMigrationError> {
        serde_json::to_vec(&ReleasePayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            intent_hash: &self.intent_hash,
            source_owner_id: &self.source_owner_id,
            source_process_instance: &self.source_process_instance,
            source_ownership_epoch: self.source_ownership_epoch,
            source_record_hash: &self.source_record_hash,
            release_tick: self.release_tick,
            public_key: &self.public_key,
        })
        .map_err(|error| LeaseMigrationError::InvalidInput(error.to_string()))
    }

    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.intent_hash,
            &self.source_owner_id,
            &self.source_process_instance,
            self.source_ownership_epoch,
            &self.source_record_hash,
            self.release_tick,
            &self.public_key,
            &self.signature,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationActivation {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub intent_hash: String,
    pub release_hash: String,
    pub destination_region: String,
    pub destination_owner_id: String,
    pub destination_process_instance: String,
    pub destination_ownership_epoch: u64,
    pub destination_lease_expiry_tick: u64,
    pub generation: u64,
    pub content_hash: String,
    pub destination_record_hash: String,
    pub activation_tick: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub activation_hash: String,
}

#[derive(Debug, Serialize)]
struct ActivationPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    snapshot_id: &'a str,
    intent_hash: &'a str,
    release_hash: &'a str,
    destination_region: &'a str,
    destination_owner_id: &'a str,
    destination_process_instance: &'a str,
    destination_ownership_epoch: u64,
    destination_lease_expiry_tick: u64,
    generation: u64,
    content_hash: &'a str,
    destination_record_hash: &'a str,
    activation_tick: u64,
    public_key: &'a [u8],
}

impl LeaseMigrationActivation {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        intent: &LeaseMigrationIntent,
        release: &LeaseMigrationRelease,
        destination_region: &str,
        destination_owner_id: &str,
        destination_process_instance: &str,
        destination_ownership_epoch: u64,
        destination_lease_expiry_tick: u64,
        generation: u64,
        content_hash: &str,
        destination_record_hash: &str,
        activation_tick: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identity(
            &intent.cluster_id,
            &intent.resource_id,
            &intent.snapshot_id,
            destination_region,
            destination_owner_id,
            destination_process_instance,
        )?;
        validate_hash(content_hash, "content hash")?;
        validate_hash(destination_record_hash, "destination record hash")?;
        if destination_ownership_epoch <= intent.current_ownership_epoch
            || destination_lease_expiry_tick <= activation_tick
            || activation_tick > intent.intent_expiry_tick
            || generation != intent.generation
            || content_hash != intent.content_hash
        {
            return Err(LeaseMigrationError::InvalidInput(
                "activation state is not bound to the migration intent".into(),
            ));
        }
        let mut activation = Self {
            domain: LEASE_MIGRATION_ACTIVATION_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: intent.cluster_id.clone(),
            resource_id: intent.resource_id.clone(),
            snapshot_id: intent.snapshot_id.clone(),
            intent_hash: intent.intent_hash.clone(),
            release_hash: release.release_hash.clone(),
            destination_region: destination_region.to_string(),
            destination_owner_id: destination_owner_id.to_string(),
            destination_process_instance: destination_process_instance.to_string(),
            destination_ownership_epoch,
            destination_lease_expiry_tick,
            generation,
            content_hash: content_hash.to_string(),
            destination_record_hash: destination_record_hash.to_string(),
            activation_tick,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            activation_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        activation.signature = signing_key.sign(&activation.payload()?).to_bytes().to_vec();
        activation.activation_hash = activation.compute_hash()?;
        activation.validate_shape()?;
        Ok(activation)
    }

    fn verify(
        &self,
        owner_keys: &BTreeMap<String, Vec<u8>>,
        intent: &LeaseMigrationIntent,
        release: &LeaseMigrationRelease,
        current_tick: u64,
    ) -> Result<(), LeaseMigrationError> {
        self.validate_shape()?;
        if self.cluster_id != intent.cluster_id
            || self.resource_id != intent.resource_id
            || self.snapshot_id != intent.snapshot_id
            || self.intent_hash != intent.intent_hash
            || self.release_hash != release.release_hash
            || self.destination_region != intent.destination_region
            || self.destination_owner_id != intent.destination_owner_id
            || self.destination_process_instance != intent.destination_process_instance
            || self.generation != intent.generation
            || self.content_hash != intent.content_hash
        {
            return Err(LeaseMigrationError::Rejected(
                "destination activation binding mismatch".into(),
            ));
        }
        if current_tick > intent.intent_expiry_tick
            || self.activation_tick > intent.intent_expiry_tick
            || self.activation_tick > current_tick
            || self.destination_lease_expiry_tick <= current_tick
        {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        if self.destination_ownership_epoch <= intent.current_ownership_epoch {
            return Err(LeaseMigrationError::EpochRegression);
        }
        let expected = owner_keys
            .get(&self.destination_owner_id)
            .ok_or_else(|| LeaseMigrationError::UnknownSigner(self.destination_owner_id.clone()))?;
        if expected.as_slice() != self.public_key.as_slice() {
            return Err(LeaseMigrationError::Rejected(
                "destination activation key mismatch".into(),
            ));
        }
        verify_signature(&self.public_key, &self.signature, &self.payload()?)?;
        if self.activation_hash != self.compute_hash()? {
            return Err(LeaseMigrationError::Rejected(
                "activation hash mismatch".into(),
            ));
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), LeaseMigrationError> {
        if self.domain != LEASE_MIGRATION_ACTIVATION_DOMAIN || self.protocol_version != 1 {
            return Err(LeaseMigrationError::Rejected(
                "activation domain or version is invalid".into(),
            ));
        }
        validate_identity(
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            &self.destination_region,
            &self.destination_owner_id,
            &self.destination_process_instance,
        )?;
        validate_hash(&self.intent_hash, "intent hash")?;
        validate_hash(&self.release_hash, "release hash")?;
        validate_hash(&self.content_hash, "content hash")?;
        validate_hash(&self.destination_record_hash, "destination record hash")?;
        validate_hash(&self.activation_hash, "activation hash")?;
        if self.destination_ownership_epoch == 0
            || self.destination_lease_expiry_tick == 0
            || self.activation_tick == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(LeaseMigrationError::Rejected(
                "activation bounds are invalid".into(),
            ));
        }
        Ok(())
    }

    fn payload(&self) -> Result<Vec<u8>, LeaseMigrationError> {
        serde_json::to_vec(&ActivationPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            snapshot_id: &self.snapshot_id,
            intent_hash: &self.intent_hash,
            release_hash: &self.release_hash,
            destination_region: &self.destination_region,
            destination_owner_id: &self.destination_owner_id,
            destination_process_instance: &self.destination_process_instance,
            destination_ownership_epoch: self.destination_ownership_epoch,
            destination_lease_expiry_tick: self.destination_lease_expiry_tick,
            generation: self.generation,
            content_hash: &self.content_hash,
            destination_record_hash: &self.destination_record_hash,
            activation_tick: self.activation_tick,
            public_key: &self.public_key,
        })
        .map_err(|error| LeaseMigrationError::InvalidInput(error.to_string()))
    }

    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&serde_json::json!({
            "domain": &self.domain,
            "protocol_version": self.protocol_version,
            "cluster_id": &self.cluster_id,
            "resource_id": &self.resource_id,
            "snapshot_id": &self.snapshot_id,
            "intent_hash": &self.intent_hash,
            "release_hash": &self.release_hash,
            "destination_region": &self.destination_region,
            "destination_owner_id": &self.destination_owner_id,
            "destination_process_instance": &self.destination_process_instance,
            "destination_ownership_epoch": self.destination_ownership_epoch,
            "destination_lease_expiry_tick": self.destination_lease_expiry_tick,
            "generation": self.generation,
            "content_hash": &self.content_hash,
            "destination_record_hash": &self.destination_record_hash,
            "activation_tick": self.activation_tick,
            "public_key": &self.public_key,
            "signature": &self.signature,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationRecord {
    pub state: LeaseMigrationState,
    pub intent: LeaseMigrationIntent,
    pub witness_acks: BTreeMap<String, LeaseMigrationWitnessAck>,
    pub release: Option<LeaseMigrationRelease>,
    pub activation: Option<LeaseMigrationActivation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationSnapshot {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub snapshot_id: String,
    pub quorum_size: usize,
    pub current_lease: LeaseRecord,
    pub state: LeaseMigrationState,
    pub migration: Option<LeaseMigrationRecord>,
    pub last_activation_epoch: u64,
    pub completed_nonces: BTreeMap<String, String>,
    pub snapshot_hash: String,
}

#[derive(Debug, Clone)]
pub struct LeaseMigrationAuthority {
    path: PathBuf,
    cluster_id: String,
    resource_id: String,
    snapshot_id: String,
    quorum_size: usize,
    owner_keys: BTreeMap<String, Vec<u8>>,
    witness_keys: BTreeMap<String, Vec<u8>>,
    current_lease: Option<LeaseRecord>,
    state: LeaseMigrationState,
    migration: Option<LeaseMigrationRecord>,
    last_activation_epoch: u64,
    completed_nonces: BTreeMap<String, String>,
}

impl LeaseMigrationAuthority {
    pub fn new(
        path: impl Into<PathBuf>,
        cluster_id: &str,
        resource_id: &str,
        snapshot_id: &str,
        quorum_size: usize,
    ) -> Result<Self, LeaseMigrationError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        validate_identifier(snapshot_id, "snapshot")?;
        if quorum_size == 0 || quorum_size > MAX_WITNESSES {
            return Err(LeaseMigrationError::InvalidInput(
                "witness quorum is outside its bound".into(),
            ));
        }
        Ok(Self {
            path: path.into(),
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            snapshot_id: snapshot_id.to_string(),
            quorum_size,
            owner_keys: BTreeMap::new(),
            witness_keys: BTreeMap::new(),
            current_lease: None,
            state: LeaseMigrationState::Stable,
            migration: None,
            last_activation_epoch: 0,
            completed_nonces: BTreeMap::new(),
        })
    }

    pub fn register_owner(
        &mut self,
        owner_id: &str,
        key: &VerifyingKey,
    ) -> Result<(), LeaseMigrationError> {
        register_key(&mut self.owner_keys, owner_id, key, "owner")
    }

    pub fn register_witness(
        &mut self,
        witness_id: &str,
        key: &VerifyingKey,
    ) -> Result<(), LeaseMigrationError> {
        register_key(&mut self.witness_keys, witness_id, key, "witness")
    }

    pub fn initialize(&mut self, lease: LeaseRecord) -> Result<(), LeaseMigrationError> {
        lease.validate(&self.cluster_id, &self.resource_id, &self.snapshot_id)?;
        if self.current_lease.is_some() {
            return Err(LeaseMigrationError::Conflict(
                "initial lease already exists".into(),
            ));
        }
        if !self.owner_keys.contains_key(&lease.owner_id) {
            return Err(LeaseMigrationError::UnknownSigner(lease.owner_id));
        }
        let previous = self.clone_state();
        self.current_lease = Some(lease);
        self.last_activation_epoch = self
            .current_lease
            .as_ref()
            .map(|value| value.ownership_epoch)
            .unwrap_or(0);
        self.commit_or_rollback(previous).map(|_| ())
    }

    pub fn state(&self) -> LeaseMigrationState {
        self.state.clone()
    }

    pub fn current_lease(&self) -> Option<LeaseRecord> {
        self.current_lease.clone()
    }

    pub fn migration(&self) -> Option<LeaseMigrationRecord> {
        self.migration.clone()
    }

    pub fn begin(
        &mut self,
        intent: LeaseMigrationIntent,
        current_tick: u64,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        intent.verify(
            &self.owner_keys,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
        )?;
        if current_tick > intent.intent_expiry_tick {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        let lease = self.current_lease.as_ref().ok_or_else(|| {
            LeaseMigrationError::InvalidState("current lease is not initialized".into())
        })?;
        lease.validate(&self.cluster_id, &self.resource_id, &self.snapshot_id)?;
        if lease.region_id != intent.source_region
            || lease.owner_id != intent.source_owner_id
            || lease.process_instance != intent.source_process_instance
            || lease.ownership_epoch != intent.current_ownership_epoch
            || lease.record_hash != intent.current_record_hash
            || lease.generation != intent.generation
            || lease.content_hash != intent.content_hash
        {
            return Err(LeaseMigrationError::Rejected(
                "intent does not match the current lease".into(),
            ));
        }
        if lease.lease_expiry_tick <= current_tick
            || intent.requested_destination_epoch <= self.last_activation_epoch
        {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        if let Some(existing) = self.migration.as_ref() {
            if existing.intent.intent_hash == intent.intent_hash {
                return Ok(existing.state.clone());
            }
            if !matches!(
                existing.state,
                LeaseMigrationState::Activated | LeaseMigrationState::Aborted
            ) {
                return Err(LeaseMigrationError::Conflict(
                    "another migration is already in progress".into(),
                ));
            }
            let previous = self.clone_state();
            self.migration = None;
            self.state = LeaseMigrationState::Stable;
            self.commit_or_rollback(previous)?;
        }
        if let Some(previous_hash) = self.completed_nonces.get(&intent.migration_nonce) {
            if previous_hash != &intent.intent_hash {
                return Err(LeaseMigrationError::ReplayMismatch);
            }
            return Err(LeaseMigrationError::Conflict(
                "migration nonce was already completed".into(),
            ));
        }
        let previous = self.clone_state();
        self.state = LeaseMigrationState::Draining;
        self.migration = Some(LeaseMigrationRecord {
            state: LeaseMigrationState::Draining,
            intent,
            witness_acks: BTreeMap::new(),
            release: None,
            activation: None,
        });
        self.commit_or_rollback(previous)
    }

    pub fn accept_witness_ack(
        &mut self,
        ack: LeaseMigrationWitnessAck,
        current_tick: u64,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        let migration = self
            .migration
            .as_ref()
            .ok_or_else(|| LeaseMigrationError::InvalidState("migration is not started".into()))?;
        if matches!(
            migration.state,
            LeaseMigrationState::Released
                | LeaseMigrationState::Activated
                | LeaseMigrationState::Aborted
        ) {
            return Err(LeaseMigrationError::InvalidState(
                "witness acknowledgement arrived after migration transition".into(),
            ));
        }
        ack.verify(&self.witness_keys, &migration.intent, current_tick)?;
        if let Some(existing) = migration.witness_acks.get(&ack.witness_id) {
            if existing.acknowledgement_hash == ack.acknowledgement_hash {
                return Ok(migration.state.clone());
            }
            return Err(LeaseMigrationError::Conflict(
                "witness changed its vote for this migration".into(),
            ));
        }
        let previous = self.clone_state();
        let migration = self.migration.as_mut().expect("migration checked above");
        migration.witness_acks.insert(ack.witness_id.clone(), ack);
        self.commit_or_rollback(previous)?;
        Ok(self.state.clone())
    }

    pub fn prepare(
        &mut self,
        current_tick: u64,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        let migration = self
            .migration
            .as_ref()
            .ok_or_else(|| LeaseMigrationError::InvalidState("migration is not started".into()))?;
        if matches!(
            migration.state,
            LeaseMigrationState::Prepared | LeaseMigrationState::Released
        ) {
            return Ok(migration.state.clone());
        }
        if migration.state != LeaseMigrationState::Draining {
            return Err(LeaseMigrationError::InvalidState(
                "migration is not draining".into(),
            ));
        }
        if current_tick > migration.intent.intent_expiry_tick {
            return Err(LeaseMigrationError::StaleEvidence);
        }
        if migration.witness_acks.len() < self.quorum_size {
            return Err(LeaseMigrationError::QuorumUnavailable);
        }
        let previous = self.clone_state();
        let migration = self.migration.as_mut().expect("migration checked above");
        migration.state = LeaseMigrationState::Prepared;
        self.state = LeaseMigrationState::Prepared;
        self.commit_or_rollback(previous)
    }

    pub fn release_source(
        &mut self,
        release: LeaseMigrationRelease,
        current_tick: u64,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        let migration = self
            .migration
            .as_ref()
            .ok_or_else(|| LeaseMigrationError::InvalidState("migration is not started".into()))?;
        if migration.state == LeaseMigrationState::Activated {
            if migration
                .release
                .as_ref()
                .map(|value| value.release_hash.as_str())
                == Some(release.release_hash.as_str())
            {
                return Ok(LeaseMigrationState::Activated);
            }
            return Err(LeaseMigrationError::Conflict(
                "activated migration release differs".into(),
            ));
        }
        if migration.state == LeaseMigrationState::Released {
            if migration
                .release
                .as_ref()
                .map(|value| value.release_hash.as_str())
                == Some(release.release_hash.as_str())
            {
                return Ok(LeaseMigrationState::Released);
            }
            return Err(LeaseMigrationError::Conflict(
                "source release differs".into(),
            ));
        }
        if migration.state != LeaseMigrationState::Prepared {
            return Err(LeaseMigrationError::SourceNotDrained);
        }
        release.verify(&self.owner_keys, &migration.intent, current_tick)?;
        let previous = self.clone_state();
        let migration = self.migration.as_mut().expect("migration checked above");
        migration.release = Some(release);
        migration.state = LeaseMigrationState::Released;
        self.state = LeaseMigrationState::Released;
        self.commit_or_rollback(previous)
    }

    pub fn activate_destination(
        &mut self,
        activation: LeaseMigrationActivation,
        current_tick: u64,
    ) -> Result<LeaseRecord, LeaseMigrationError> {
        let migration = self
            .migration
            .as_ref()
            .ok_or_else(|| LeaseMigrationError::InvalidState("migration is not started".into()))?;
        if migration.state == LeaseMigrationState::Activated {
            if let Some(existing) = migration.activation.as_ref() {
                if existing.activation_hash == activation.activation_hash {
                    return self.current_lease.clone().ok_or_else(|| {
                        LeaseMigrationError::InvalidState("activated lease is missing".into())
                    });
                }
            }
            return Err(LeaseMigrationError::Conflict(
                "destination activation differs".into(),
            ));
        }
        if migration.state != LeaseMigrationState::Released {
            return Err(LeaseMigrationError::ReleaseMissing);
        }
        let release = migration
            .release
            .as_ref()
            .ok_or(LeaseMigrationError::ReleaseMissing)?;
        activation.verify(&self.owner_keys, &migration.intent, release, current_tick)?;
        if activation.destination_lease_expiry_tick - current_tick > MAX_LEASE_TICKS {
            return Err(LeaseMigrationError::Rejected(
                "destination lease exceeds its bounded window".into(),
            ));
        }
        if activation.destination_ownership_epoch <= self.last_activation_epoch {
            return Err(LeaseMigrationError::EpochRegression);
        }
        let lease = LeaseRecord {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            snapshot_id: self.snapshot_id.clone(),
            region_id: activation.destination_region.clone(),
            owner_id: activation.destination_owner_id.clone(),
            process_instance: activation.destination_process_instance.clone(),
            ownership_epoch: activation.destination_ownership_epoch,
            lease_expiry_tick: activation.destination_lease_expiry_tick,
            generation: activation.generation,
            content_hash: activation.content_hash.clone(),
            record_hash: activation.destination_record_hash.clone(),
        };
        lease.validate(&self.cluster_id, &self.resource_id, &self.snapshot_id)?;
        let previous = self.clone_state();
        let migration = self.migration.as_mut().expect("migration checked above");
        migration.activation = Some(activation);
        migration.state = LeaseMigrationState::Activated;
        self.current_lease = Some(lease.clone());
        self.last_activation_epoch = lease.ownership_epoch;
        self.state = LeaseMigrationState::Activated;
        self.completed_nonces.insert(
            migration.intent.migration_nonce.clone(),
            migration.intent.intent_hash.clone(),
        );
        trim_history(&mut self.completed_nonces);
        self.commit_or_rollback(previous)?;
        Ok(lease)
    }

    pub fn abort(
        &mut self,
        reason: &str,
        current_tick: u64,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        validate_reason(reason)?;
        let migration = self
            .migration
            .as_ref()
            .ok_or_else(|| LeaseMigrationError::InvalidState("migration is not started".into()))?;
        if migration.state == LeaseMigrationState::Aborted {
            return Ok(LeaseMigrationState::Aborted);
        }
        if matches!(
            migration.state,
            LeaseMigrationState::Released | LeaseMigrationState::Activated
        ) {
            return Err(LeaseMigrationError::InvalidState(
                "released migration cannot be aborted".into(),
            ));
        }
        if current_tick < migration.intent.intent_expiry_tick && reason.is_empty() {
            return Err(LeaseMigrationError::InvalidInput(
                "abort reason is required".into(),
            ));
        }
        let previous = self.clone_state();
        let migration = self.migration.as_mut().expect("migration checked above");
        migration.state = LeaseMigrationState::Aborted;
        self.state = LeaseMigrationState::Aborted;
        self.commit_or_rollback(previous)
    }

    pub fn snapshot(&self) -> Result<LeaseMigrationSnapshot, LeaseMigrationError> {
        let current_lease = self.current_lease.clone().ok_or_else(|| {
            LeaseMigrationError::InvalidState("current lease is not initialized".into())
        })?;
        let mut snapshot = LeaseMigrationSnapshot {
            domain: LEASE_MIGRATION_SNAPSHOT_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            snapshot_id: self.snapshot_id.clone(),
            quorum_size: self.quorum_size,
            current_lease,
            state: self.state.clone(),
            migration: self.migration.clone(),
            last_activation_epoch: self.last_activation_epoch,
            completed_nonces: self.completed_nonces.clone(),
            snapshot_hash: LEASE_MIGRATION_ZERO_HASH.to_string(),
        };
        snapshot.snapshot_hash = snapshot.compute_hash()?;
        Ok(snapshot)
    }

    pub fn persist(&self) -> Result<(), LeaseMigrationError> {
        let snapshot = self.snapshot()?;
        self.persist_snapshot(&snapshot)
    }

    pub fn restore_persisted(&mut self) -> Result<(), LeaseMigrationError> {
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(LeaseMigrationError::PersistenceFailed(
                "lease migration snapshot exceeds size bound".into(),
            ));
        }
        let snapshot: LeaseMigrationSnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        self.restore(snapshot)
    }

    pub fn restore(&mut self, snapshot: LeaseMigrationSnapshot) -> Result<(), LeaseMigrationError> {
        validate_snapshot(
            &snapshot,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
        )?;
        snapshot
            .current_lease
            .validate(&self.cluster_id, &self.resource_id, &self.snapshot_id)?;
        if !self
            .owner_keys
            .contains_key(&snapshot.current_lease.owner_id)
        {
            return Err(LeaseMigrationError::UnknownSigner(
                snapshot.current_lease.owner_id,
            ));
        }
        if let Some(migration) = snapshot.migration.as_ref() {
            self.validate_migration_record(migration)?;
            if snapshot.state != migration.state {
                return Err(LeaseMigrationError::Rejected(
                    "snapshot state does not match migration state".into(),
                ));
            }
            if matches!(snapshot.state, LeaseMigrationState::Released)
                && migration.release.is_none()
            {
                return Err(LeaseMigrationError::Rejected(
                    "released snapshot is missing source release".into(),
                ));
            }
            if matches!(snapshot.state, LeaseMigrationState::Activated)
                && (migration.release.is_none() || migration.activation.is_none())
            {
                return Err(LeaseMigrationError::Rejected(
                    "activated snapshot is missing release or activation evidence".into(),
                ));
            }
        } else if !matches!(snapshot.state, LeaseMigrationState::Stable) {
            return Err(LeaseMigrationError::Rejected(
                "non-stable snapshot is missing migration state".into(),
            ));
        }
        let previous = self.clone_state();
        self.current_lease = Some(snapshot.current_lease);
        self.state = snapshot.state;
        self.migration = snapshot.migration;
        self.last_activation_epoch = snapshot.last_activation_epoch;
        self.completed_nonces = snapshot.completed_nonces;
        self.commit_or_rollback(previous).map(|_| ())
    }

    fn validate_migration_record(
        &self,
        migration: &LeaseMigrationRecord,
    ) -> Result<(), LeaseMigrationError> {
        migration.intent.verify(
            &self.owner_keys,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
        )?;
        for acknowledgement in migration.witness_acks.values() {
            acknowledgement.verify(
                &self.witness_keys,
                &migration.intent,
                acknowledgement.observed_tick,
            )?;
        }
        if let Some(release) = migration.release.as_ref() {
            release.verify(&self.owner_keys, &migration.intent, release.release_tick)?;
        }
        if let Some(activation) = migration.activation.as_ref() {
            let release = migration
                .release
                .as_ref()
                .ok_or(LeaseMigrationError::ReleaseMissing)?;
            activation.verify(
                &self.owner_keys,
                &migration.intent,
                release,
                activation.activation_tick,
            )?;
        }
        Ok(())
    }

    pub fn metrics(&self) -> LeaseMigrationMetrics {
        LeaseMigrationMetrics {
            state: self.state.clone(),
            witness_count: self
                .migration
                .as_ref()
                .map(|value| value.witness_acks.len())
                .unwrap_or(0),
            quorum_size: self.quorum_size,
            ownership_epoch: self
                .current_lease
                .as_ref()
                .map(|value| value.ownership_epoch)
                .unwrap_or(0),
            last_activation_epoch: self.last_activation_epoch,
            completed_nonce_count: self.completed_nonces.len(),
            migration_present: self.migration.is_some(),
            secret_material_recorded: false,
        }
    }

    fn clone_state(&self) -> AuthorityState {
        AuthorityState {
            current_lease: self.current_lease.clone(),
            state: self.state.clone(),
            migration: self.migration.clone(),
            last_activation_epoch: self.last_activation_epoch,
            completed_nonces: self.completed_nonces.clone(),
        }
    }

    fn restore_state(&mut self, state: AuthorityState) {
        self.current_lease = state.current_lease;
        self.state = state.state;
        self.migration = state.migration;
        self.last_activation_epoch = state.last_activation_epoch;
        self.completed_nonces = state.completed_nonces;
    }

    fn commit_or_rollback(
        &mut self,
        previous: AuthorityState,
    ) -> Result<LeaseMigrationState, LeaseMigrationError> {
        match self.persist() {
            Ok(()) => Ok(self.state.clone()),
            Err(error) => {
                self.restore_state(previous);
                Err(error)
            }
        }
    }

    fn persist_snapshot(
        &self,
        snapshot: &LeaseMigrationSnapshot,
    ) -> Result<(), LeaseMigrationError> {
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(LeaseMigrationError::PersistenceFailed(
                "lease migration snapshot exceeds size bound".into(),
            ));
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        }
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        fs::rename(&staging, &self.path)
            .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| LeaseMigrationError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseMigrationMetrics {
    pub state: LeaseMigrationState,
    pub witness_count: usize,
    pub quorum_size: usize,
    pub ownership_epoch: u64,
    pub last_activation_epoch: u64,
    pub completed_nonce_count: usize,
    pub migration_present: bool,
    pub secret_material_recorded: bool,
}

#[derive(Debug, Clone)]
struct AuthorityState {
    current_lease: Option<LeaseRecord>,
    state: LeaseMigrationState,
    migration: Option<LeaseMigrationRecord>,
    last_activation_epoch: u64,
    completed_nonces: BTreeMap<String, String>,
}

impl LeaseMigrationSnapshot {
    fn compute_hash(&self) -> Result<String, LeaseMigrationError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.snapshot_id,
            self.quorum_size,
            &self.current_lease,
            &self.state,
            &self.migration,
            self.last_activation_epoch,
            &self.completed_nonces,
        ))
    }
}

fn validate_snapshot(
    snapshot: &LeaseMigrationSnapshot,
    cluster_id: &str,
    resource_id: &str,
    snapshot_id: &str,
) -> Result<(), LeaseMigrationError> {
    if snapshot.domain != LEASE_MIGRATION_SNAPSHOT_DOMAIN || snapshot.protocol_version != 1 {
        return Err(LeaseMigrationError::Rejected(
            "snapshot domain or version is invalid".into(),
        ));
    }
    if snapshot.cluster_id != cluster_id
        || snapshot.resource_id != resource_id
        || snapshot.snapshot_id != snapshot_id
    {
        return Err(LeaseMigrationError::Rejected(
            "snapshot identity mismatch".into(),
        ));
    }
    if snapshot.quorum_size == 0
        || snapshot.quorum_size > MAX_WITNESSES
        || snapshot.completed_nonces.len() > MAX_HISTORY
    {
        return Err(LeaseMigrationError::Rejected(
            "snapshot bounds are invalid".into(),
        ));
    }
    if snapshot.snapshot_hash != snapshot.compute_hash()? {
        return Err(LeaseMigrationError::Rejected(
            "snapshot hash mismatch".into(),
        ));
    }
    if let Some(migration) = &snapshot.migration {
        migration.intent.validate_shape()?;
        if migration.witness_acks.len() > MAX_WITNESSES {
            return Err(LeaseMigrationError::Rejected(
                "witness acknowledgement bound exceeded".into(),
            ));
        }
        for (witness_id, ack) in &migration.witness_acks {
            if witness_id != &ack.witness_id {
                return Err(LeaseMigrationError::Rejected(
                    "witness map key mismatch".into(),
                ));
            }
            ack.validate_shape()?;
            if ack.intent_hash != migration.intent.intent_hash {
                return Err(LeaseMigrationError::Rejected(
                    "witness intent hash mismatch".into(),
                ));
            }
        }
    }
    Ok(())
}

fn register_key(
    registry: &mut BTreeMap<String, Vec<u8>>,
    identity: &str,
    key: &VerifyingKey,
    kind: &str,
) -> Result<(), LeaseMigrationError> {
    validate_identifier(identity, kind)?;
    let key_bytes = key.to_bytes().to_vec();
    if let Some(existing) = registry.get(identity) {
        if existing != &key_bytes {
            return Err(LeaseMigrationError::Rejected(format!(
                "{kind} key rebinding requires an explicit transition"
            )));
        }
        return Ok(());
    }
    if registry.len() >= MAX_WITNESSES {
        return Err(LeaseMigrationError::Rejected(format!(
            "{kind} registry capacity exceeded"
        )));
    }
    registry.insert(identity.to_string(), key_bytes);
    Ok(())
}

fn verify_signature(
    public_key: &[u8],
    signature: &[u8],
    payload: &[u8],
) -> Result<(), LeaseMigrationError> {
    let key_bytes: [u8; 32] = public_key
        .try_into()
        .map_err(|_| LeaseMigrationError::Rejected("public key shape is invalid".into()))?;
    let key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| LeaseMigrationError::Rejected("public key is invalid".into()))?;
    let signature = Signature::from_slice(signature)
        .map_err(|_| LeaseMigrationError::Rejected("signature is invalid".into()))?;
    key.verify(payload, &signature)
        .map_err(|_| LeaseMigrationError::Rejected("signature verification failed".into()))
}

fn validate_identity(
    cluster_id: &str,
    resource_id: &str,
    snapshot_id: &str,
    region_id: &str,
    owner_id: &str,
    process_instance: &str,
) -> Result<(), LeaseMigrationError> {
    validate_identifier(cluster_id, "cluster")?;
    validate_identifier(resource_id, "resource")?;
    validate_identifier(snapshot_id, "snapshot")?;
    validate_identifier(region_id, "region")?;
    validate_identifier(owner_id, "owner")?;
    validate_identifier(process_instance, "process instance")?;
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), LeaseMigrationError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(LeaseMigrationError::InvalidInput(format!(
            "{label} identifier is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), LeaseMigrationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LeaseMigrationError::InvalidInput(format!(
            "{label} must be a 64-character hexadecimal digest"
        )));
    }
    Ok(())
}

fn validate_reason(reason: &str) -> Result<(), LeaseMigrationError> {
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES || reason.chars().any(char::is_control)
    {
        return Err(LeaseMigrationError::InvalidInput(
            "abort reason is empty, oversized, or contains control characters".into(),
        ));
    }
    Ok(())
}

fn trim_history(history: &mut BTreeMap<String, String>) {
    while history.len() > MAX_HISTORY {
        let Some(first) = history.keys().next().cloned() else {
            break;
        };
        history.remove(&first);
    }
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, LeaseMigrationError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| LeaseMigrationError::InvalidInput(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:064x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn hash(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn authority() -> (
        tempfile::TempDir,
        LeaseMigrationAuthority,
        SigningKey,
        SigningKey,
        [SigningKey; 3],
    ) {
        let directory = tempdir().expect("temporary directory");
        let source = key(41);
        let destination = key(42);
        let witnesses = [key(51), key(52), key(53)];
        let mut authority = LeaseMigrationAuthority::new(
            directory.path().join("migration.json"),
            "cluster-a",
            "resource-a",
            "snapshot-a",
            2,
        )
        .expect("authority");
        authority
            .register_owner("owner-a", &source.verifying_key())
            .expect("source key");
        authority
            .register_owner("owner-b", &destination.verifying_key())
            .expect("destination key");
        for (index, witness) in witnesses.iter().enumerate() {
            authority
                .register_witness(&format!("witness-{index}"), &witness.verifying_key())
                .expect("witness key");
        }
        let lease = LeaseRecord::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            "region-a",
            "owner-a",
            "process-a",
            7,
            200,
            3,
            &hash('a'),
        )
        .expect("lease");
        authority.initialize(lease).expect("initialize");
        (directory, authority, source, destination, witnesses)
    }

    fn intent(authority: &LeaseMigrationAuthority, source: &SigningKey) -> LeaseMigrationIntent {
        let lease = authority.current_lease().expect("lease");
        LeaseMigrationIntent::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &lease.region_id,
            &lease.owner_id,
            &lease.process_instance,
            "region-b",
            "owner-b",
            "process-b",
            lease.ownership_epoch,
            &lease.record_hash,
            lease.generation,
            &lease.content_hash,
            "migration-1",
            lease.ownership_epoch + 1,
            150,
            source,
        )
        .expect("intent")
    }

    #[test]
    fn valid_handoff_requires_quorum_release_and_higher_epoch() {
        let (_directory, mut authority, source, destination, witnesses) = authority();
        let intent = intent(&authority, &source);
        authority.begin(intent.clone(), 10).expect("begin");
        let ack0 = LeaseMigrationWitnessAck::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &intent.intent_hash,
            "witness-0",
            1,
            11,
            20,
            &witnesses[0],
        )
        .expect("ack");
        let ack1 = LeaseMigrationWitnessAck::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &intent.intent_hash,
            "witness-1",
            1,
            11,
            20,
            &witnesses[1],
        )
        .expect("ack");
        authority.accept_witness_ack(ack0, 11).expect("ack0");
        authority.accept_witness_ack(ack1, 11).expect("ack1");
        authority.prepare(12).expect("prepare");
        let release = LeaseMigrationRelease::sign(
            &intent,
            "owner-a",
            "process-a",
            7,
            &intent.current_record_hash,
            13,
            &source,
        )
        .expect("release");
        authority
            .release_source(release.clone(), 13)
            .expect("release");
        let destination_lease = LeaseRecord::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            "region-b",
            "owner-b",
            "process-b",
            8,
            240,
            3,
            &intent.content_hash,
        )
        .expect("destination lease");
        let activation = LeaseMigrationActivation::sign(
            &intent,
            &release,
            "region-b",
            "owner-b",
            "process-b",
            8,
            240,
            3,
            &intent.content_hash,
            &destination_lease.record_hash,
            14,
            &destination,
        )
        .expect("activation");
        let lease = authority
            .activate_destination(activation, 14)
            .expect("activation");
        assert_eq!(lease.region_id, "region-b");
        assert_eq!(lease.ownership_epoch, 8);
        assert_eq!(authority.state(), LeaseMigrationState::Activated);
    }

    #[test]
    fn activation_before_release_and_conflicting_votes_fail_closed() {
        let (_directory, mut authority, source, destination, witnesses) = authority();
        let intent = intent(&authority, &source);
        authority.begin(intent.clone(), 10).expect("begin");
        let ack0 = LeaseMigrationWitnessAck::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &intent.intent_hash,
            "witness-0",
            1,
            11,
            20,
            &witnesses[0],
        )
        .expect("ack");
        authority.accept_witness_ack(ack0, 11).expect("ack");
        assert_eq!(
            authority.prepare(12),
            Err(LeaseMigrationError::QuorumUnavailable)
        );
        let conflicting = LeaseMigrationWitnessAck::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &hash('c'),
            "witness-0",
            1,
            11,
            20,
            &witnesses[0],
        )
        .expect("conflicting ack");
        assert!(authority.accept_witness_ack(conflicting, 11).is_err());
        let release = LeaseMigrationRelease::sign(
            &intent,
            "owner-a",
            "process-a",
            7,
            &intent.current_record_hash,
            13,
            &source,
        )
        .expect("release");
        let activation = LeaseMigrationActivation::sign(
            &intent,
            &release,
            "region-b",
            "owner-b",
            "process-b",
            8,
            240,
            3,
            &intent.content_hash,
            &hash('b'),
            14,
            &destination,
        )
        .expect("activation");
        assert_eq!(
            authority.activate_destination(activation, 14),
            Err(LeaseMigrationError::ReleaseMissing)
        );
    }

    #[test]
    fn snapshot_restore_rejects_tampering_and_cleans_staging() {
        let (directory, mut authority, source, _destination, witnesses) = authority();
        let intent = intent(&authority, &source);
        authority.begin(intent.clone(), 10).expect("begin");
        let ack = LeaseMigrationWitnessAck::sign(
            "cluster-a",
            "resource-a",
            "snapshot-a",
            &intent.intent_hash,
            "witness-0",
            1,
            11,
            20,
            &witnesses[0],
        )
        .expect("ack");
        authority.accept_witness_ack(ack, 11).expect("ack");
        authority.persist().expect("persist");
        let mut snapshot = authority.snapshot().expect("snapshot");
        snapshot.state = LeaseMigrationState::Activated;
        assert!(authority.restore(snapshot).is_err());
        std::fs::write(directory.path().join("migration.staging"), b"stale").expect("staging");
        let mut restarted = LeaseMigrationAuthority::new(
            directory.path().join("migration.json"),
            "cluster-a",
            "resource-a",
            "snapshot-a",
            2,
        )
        .expect("restarted");
        restarted
            .register_owner("owner-a", &source.verifying_key())
            .expect("source key");
        restarted
            .register_witness("witness-0", &witnesses[0].verifying_key())
            .expect("witness key");
        restarted.restore_persisted().expect("restore");
        assert!(!directory.path().join("migration.staging").exists());
        assert_eq!(restarted.state(), LeaseMigrationState::Draining);
    }
}
