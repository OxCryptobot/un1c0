use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SERVICE_IDENTITY_SCHEMA_VERSION: u8 = 1;
pub const MAX_SERVICE_IDENTITY_ENVELOPE_BYTES: usize = 64 * 1024;
pub const MAX_SERVICE_IDENTITY_OUTBOX_ENTRIES: usize = 100_000;
pub const MAX_SERVICE_IDENTITY_ID_BYTES: usize = 256;
const SERVICE_IDENTITY_DOMAIN: &[u8] = b"un1c0/phase79/service-identity-envelope/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceIdentityError {
    InvalidIdentifier(&'static str),
    InvalidGeneration,
    InvalidCapacity,
    InvalidDigest(&'static str),
    InvalidSignature,
    UntrustedSigner(String),
    RevokedSigner(String),
    SignerAlreadyExists(String),
    SignerRebinding(String),
    RotationConflict(String),
    ServiceMismatch,
    BindingMismatch(&'static str),
    Serialization(String),
    Persistence(String),
    OutboxFull { entries: usize, maximum: usize },
    EnvelopeTooLarge { bytes: usize, maximum: usize },
    EnvelopeCollision,
    MissingEnvelope,
}

impl std::fmt::Display for ServiceIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier(label) => write!(formatter, "invalid service identity {label}"),
            Self::InvalidGeneration => {
                formatter.write_str("service identity generation must be non-zero")
            }
            Self::InvalidCapacity => {
                formatter.write_str("service identity outbox capacity is invalid")
            }
            Self::InvalidDigest(label) => {
                write!(formatter, "service identity {label} digest is empty")
            }
            Self::InvalidSignature => formatter.write_str("service identity signature is invalid"),
            Self::UntrustedSigner(id) => {
                write!(formatter, "service identity signer is untrusted: {id}")
            }
            Self::RevokedSigner(id) => {
                write!(formatter, "service identity signer is revoked: {id}")
            }
            Self::SignerAlreadyExists(id) => {
                write!(formatter, "service identity signer already exists: {id}")
            }
            Self::SignerRebinding(id) => {
                write!(formatter, "service identity signer cannot be rebound: {id}")
            }
            Self::RotationConflict(message) => {
                write!(formatter, "service identity rotation conflict: {message}")
            }
            Self::ServiceMismatch => {
                formatter.write_str("service identity does not match the registry")
            }
            Self::BindingMismatch(label) => {
                write!(formatter, "service identity binding mismatch: {label}")
            }
            Self::Serialization(message) => write!(
                formatter,
                "service identity serialization failed: {message}"
            ),
            Self::Persistence(message) => {
                write!(formatter, "service identity persistence failed: {message}")
            }
            Self::OutboxFull { entries, maximum } => write!(
                formatter,
                "service identity outbox has {entries} entries; maximum is {maximum}"
            ),
            Self::EnvelopeTooLarge { bytes, maximum } => write!(
                formatter,
                "service identity envelope is {bytes} bytes; maximum is {maximum}"
            ),
            Self::EnvelopeCollision => {
                formatter.write_str("service identity outbox envelope collision")
            }
            Self::MissingEnvelope => {
                formatter.write_str("service identity outbox envelope is not pending")
            }
        }
    }
}

impl std::error::Error for ServiceIdentityError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceIdentityDescriptor {
    pub trust_domain: String,
    pub namespace: String,
    pub service_account: String,
}

impl ServiceIdentityDescriptor {
    pub fn new(
        trust_domain: &str,
        namespace: &str,
        service_account: &str,
    ) -> Result<Self, ServiceIdentityError> {
        validate_identifier(trust_domain, "trust domain")?;
        validate_identifier(namespace, "namespace")?;
        validate_identifier(service_account, "service account")?;
        Ok(Self {
            trust_domain: trust_domain.to_string(),
            namespace: namespace.to_string(),
            service_account: service_account.to_string(),
        })
    }

    pub fn canonical_id(&self) -> String {
        format!(
            "spiffe://{}/ns/{}/sa/{}",
            self.trust_domain, self.namespace, self.service_account
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceSignerState {
    pub public_key: [u8; 32],
    pub generation: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceIdentityRegistry {
    service_id: String,
    identity: ServiceIdentityDescriptor,
    active_signer_id: Option<String>,
    signers: BTreeMap<String, ServiceSignerState>,
}

impl ServiceIdentityRegistry {
    pub fn new(
        service_id: &str,
        identity: ServiceIdentityDescriptor,
    ) -> Result<Self, ServiceIdentityError> {
        validate_identifier(service_id, "service id")?;
        Ok(Self {
            service_id: service_id.to_string(),
            identity,
            active_signer_id: None,
            signers: BTreeMap::new(),
        })
    }

    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    pub fn identity(&self) -> &ServiceIdentityDescriptor {
        &self.identity
    }

    pub fn active_signer_id(&self) -> Option<&str> {
        self.active_signer_id.as_deref()
    }

    pub fn signer(&self, signer_id: &str) -> Option<&ServiceSignerState> {
        self.signers.get(signer_id)
    }

    pub fn register_initial_signer(
        &mut self,
        signer_id: &str,
        public_key: [u8; 32],
        generation: u64,
    ) -> Result<(), ServiceIdentityError> {
        validate_signer_inputs(signer_id, generation)?;
        if self.active_signer_id.is_some() {
            return Err(ServiceIdentityError::RotationConflict(
                "initial signer is already configured".into(),
            ));
        }
        if self.signers.contains_key(signer_id) {
            return Err(ServiceIdentityError::SignerRebinding(signer_id.to_string()));
        }
        self.signers.insert(
            signer_id.to_string(),
            ServiceSignerState {
                public_key,
                generation,
                revoked: false,
            },
        );
        self.active_signer_id = Some(signer_id.to_string());
        Ok(())
    }

    pub fn rotate_signer(
        &mut self,
        old_signer_id: &str,
        new_signer_id: &str,
        new_public_key: [u8; 32],
        new_generation: u64,
    ) -> Result<(), ServiceIdentityError> {
        validate_signer_inputs(old_signer_id, new_generation)?;
        validate_signer_inputs(new_signer_id, new_generation)?;
        if old_signer_id == new_signer_id {
            return Err(ServiceIdentityError::RotationConflict(
                "rotation requires distinct signer IDs".into(),
            ));
        }
        if self.active_signer_id.as_deref() != Some(old_signer_id) {
            return Err(ServiceIdentityError::RotationConflict(
                "rotation source is not the active signer".into(),
            ));
        }
        let old = self
            .signers
            .get(old_signer_id)
            .ok_or_else(|| ServiceIdentityError::UntrustedSigner(old_signer_id.to_string()))?;
        if old.revoked {
            return Err(ServiceIdentityError::RevokedSigner(
                old_signer_id.to_string(),
            ));
        }
        if new_generation <= old.generation {
            return Err(ServiceIdentityError::RotationConflict(
                "new signer generation must increase".into(),
            ));
        }
        if self.signers.contains_key(new_signer_id) {
            return Err(ServiceIdentityError::SignerAlreadyExists(
                new_signer_id.to_string(),
            ));
        }
        self.signers
            .get_mut(old_signer_id)
            .expect("validated above")
            .revoked = true;
        self.signers.insert(
            new_signer_id.to_string(),
            ServiceSignerState {
                public_key: new_public_key,
                generation: new_generation,
                revoked: false,
            },
        );
        self.active_signer_id = Some(new_signer_id.to_string());
        Ok(())
    }

    pub fn revoke_signer(&mut self, signer_id: &str) -> Result<(), ServiceIdentityError> {
        validate_identifier(signer_id, "signer id")?;
        let signer = self
            .signers
            .get_mut(signer_id)
            .ok_or_else(|| ServiceIdentityError::UntrustedSigner(signer_id.to_string()))?;
        signer.revoked = true;
        if self.active_signer_id.as_deref() == Some(signer_id) {
            self.active_signer_id = None;
        }
        Ok(())
    }

    pub fn authorize_active(
        &self,
        signer_id: &str,
        generation: u64,
        public_key: &[u8; 32],
    ) -> Result<(), ServiceIdentityError> {
        let signer = self.authorize_historical(signer_id, generation, public_key)?;
        if signer.revoked || self.active_signer_id.as_deref() != Some(signer_id) {
            return Err(ServiceIdentityError::RevokedSigner(signer_id.to_string()));
        }
        Ok(())
    }

    pub fn authorize_historical(
        &self,
        signer_id: &str,
        generation: u64,
        public_key: &[u8; 32],
    ) -> Result<&ServiceSignerState, ServiceIdentityError> {
        let signer = self
            .signers
            .get(signer_id)
            .ok_or_else(|| ServiceIdentityError::UntrustedSigner(signer_id.to_string()))?;
        if signer.generation != generation || &signer.public_key != public_key {
            return Err(ServiceIdentityError::SignerRebinding(signer_id.to_string()));
        }
        Ok(signer)
    }

    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), ServiceIdentityError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))?;
        let temporary = parent.join(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("service-identity")
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ServiceIdentityError> {
        let bytes =
            fs::read(path).map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        let registry: Self = serde_json::from_slice(&bytes)
            .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))?;
        validate_identifier(&registry.service_id, "service id")?;
        if let Some(active) = &registry.active_signer_id {
            let signer = registry
                .signers
                .get(active)
                .ok_or_else(|| ServiceIdentityError::UntrustedSigner(active.clone()))?;
            if signer.revoked {
                return Err(ServiceIdentityError::RevokedSigner(active.clone()));
            }
        }
        for (signer_id, signer) in &registry.signers {
            validate_signer_inputs(signer_id, signer.generation)?;
        }
        Ok(registry)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceIdentityEnvelope {
    pub schema_version: u8,
    pub service_id: String,
    pub identity_id: String,
    pub signer_id: String,
    pub signer_generation: u64,
    pub evidence_digest: [u8; 32],
    pub stream_id: String,
    pub source_sequence: u64,
    pub trust_generation: u64,
    pub predecessor: Option<[u8; 32]>,
    pub signature: Vec<u8>,
}

impl ServiceIdentityEnvelope {
    pub fn signing_payload(&self) -> Result<Vec<u8>, ServiceIdentityError> {
        serde_json::to_vec(&(
            SERVICE_IDENTITY_DOMAIN,
            self.schema_version,
            &self.service_id,
            &self.identity_id,
            &self.signer_id,
            self.signer_generation,
            self.evidence_digest,
            &self.stream_id,
            self.source_sequence,
            self.trust_generation,
            self.predecessor,
        ))
        .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))
    }

    pub fn envelope_digest(&self) -> Result<[u8; 32], ServiceIdentityError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))?;
        Ok(hash_bytes(&bytes))
    }

    pub fn verify(&self, registry: &ServiceIdentityRegistry) -> Result<(), ServiceIdentityError> {
        validate_envelope_shape(self)?;
        if self.service_id != registry.service_id()
            || self.identity_id != registry.identity().canonical_id()
        {
            return Err(ServiceIdentityError::ServiceMismatch);
        }
        let signer = registry.authorize_historical(
            &self.signer_id,
            self.signer_generation,
            &self.signer_public_key(registry)?,
        )?;
        let _ = signer;
        let public_key = self.signer_public_key(registry)?;
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| ServiceIdentityError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ServiceIdentityError::InvalidSignature)?;
        let payload = self.signing_payload()?;
        verifying_key
            .verify(&payload, &Signature::from_bytes(&signature))
            .map_err(|_| ServiceIdentityError::InvalidSignature)
    }

    fn signer_public_key(
        &self,
        registry: &ServiceIdentityRegistry,
    ) -> Result<[u8; 32], ServiceIdentityError> {
        registry
            .signer(&self.signer_id)
            .map(|signer| signer.public_key)
            .ok_or_else(|| ServiceIdentityError::UntrustedSigner(self.signer_id.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct ServiceIdentityAuthority {
    registry: ServiceIdentityRegistry,
    signer_id: String,
    signing_key: SigningKey,
    trust_generation: u64,
}

impl ServiceIdentityAuthority {
    pub fn new(
        registry: ServiceIdentityRegistry,
        signer_id: &str,
        signing_key: SigningKey,
        trust_generation: u64,
    ) -> Result<Self, ServiceIdentityError> {
        if trust_generation == 0 {
            return Err(ServiceIdentityError::InvalidGeneration);
        }
        let public_key = signing_key.verifying_key().to_bytes();
        registry
            .authorize_active(signer_id, 1, &public_key)
            .or_else(|error| {
                let generation = registry
                    .signer(signer_id)
                    .map(|signer| signer.generation)
                    .unwrap_or(0);
                if generation == 0 {
                    Err(error)
                } else {
                    registry.authorize_active(signer_id, generation, &public_key)
                }
            })?;
        Ok(Self {
            registry,
            signer_id: signer_id.to_string(),
            signing_key,
            trust_generation,
        })
    }

    pub fn registry(&self) -> &ServiceIdentityRegistry {
        &self.registry
    }

    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    pub fn issue(
        &self,
        evidence_digest: [u8; 32],
        stream_id: &str,
        source_sequence: u64,
        predecessor: Option<[u8; 32]>,
    ) -> Result<ServiceIdentityEnvelope, ServiceIdentityError> {
        if evidence_digest == [0; 32] {
            return Err(ServiceIdentityError::InvalidDigest("evidence"));
        }
        validate_identifier(stream_id, "stream id")?;
        if source_sequence == 0 {
            return Err(ServiceIdentityError::InvalidGeneration);
        }
        let signer = self
            .registry
            .signer(&self.signer_id)
            .ok_or_else(|| ServiceIdentityError::UntrustedSigner(self.signer_id.clone()))?;
        self.registry.authorize_active(
            &self.signer_id,
            signer.generation,
            &self.signing_key.verifying_key().to_bytes(),
        )?;
        let mut envelope = ServiceIdentityEnvelope {
            schema_version: SERVICE_IDENTITY_SCHEMA_VERSION,
            service_id: self.registry.service_id().to_string(),
            identity_id: self.registry.identity().canonical_id(),
            signer_id: self.signer_id.clone(),
            signer_generation: signer.generation,
            evidence_digest,
            stream_id: stream_id.to_string(),
            source_sequence,
            trust_generation: self.trust_generation,
            predecessor,
            signature: vec![0; 64],
        };
        let payload = envelope.signing_payload()?;
        envelope.signature = self.signing_key.sign(&payload).to_bytes().to_vec();
        Ok(envelope)
    }

    pub fn rotate_signer(
        &mut self,
        new_signer_id: &str,
        new_signing_key: SigningKey,
        new_generation: u64,
        registry_path: impl AsRef<Path>,
    ) -> Result<(), ServiceIdentityError> {
        let mut next = self.registry.clone();
        next.rotate_signer(
            &self.signer_id,
            new_signer_id,
            new_signing_key.verifying_key().to_bytes(),
            new_generation,
        )?;
        next.save_atomic(registry_path)?;
        self.registry = next;
        self.signer_id = new_signer_id.to_string();
        self.signing_key = new_signing_key;
        Ok(())
    }

    pub fn revoke_signer(
        &mut self,
        signer_id: &str,
        registry_path: impl AsRef<Path>,
    ) -> Result<(), ServiceIdentityError> {
        let mut next = self.registry.clone();
        next.revoke_signer(signer_id)?;
        next.save_atomic(registry_path)?;
        self.registry = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboxSyncMode {
    Durable,
    #[cfg(feature = "benchmark")]
    NoSyncBenchmarkOnly,
}

#[derive(Debug, Clone)]
pub struct DurableServiceIdentityOutbox {
    directory: PathBuf,
    maximum_entries: usize,
}

impl DurableServiceIdentityOutbox {
    pub fn open(
        directory: impl AsRef<Path>,
        maximum_entries: usize,
    ) -> Result<Self, ServiceIdentityError> {
        if maximum_entries == 0 || maximum_entries > MAX_SERVICE_IDENTITY_OUTBOX_ENTRIES {
            return Err(ServiceIdentityError::InvalidCapacity);
        }
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory)
            .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        Ok(Self {
            directory,
            maximum_entries,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn pending(
        &self,
        registry: &ServiceIdentityRegistry,
    ) -> Result<Vec<ServiceIdentityEnvelope>, ServiceIdentityError> {
        let mut envelopes = Vec::new();
        for entry in fs::read_dir(&self.directory)
            .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?
        {
            let entry =
                entry.map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
            if !entry
                .file_type()
                .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?
                .is_file()
            {
                continue;
            }
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path())
                .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
            if bytes.len() > MAX_SERVICE_IDENTITY_ENVELOPE_BYTES {
                return Err(ServiceIdentityError::EnvelopeTooLarge {
                    bytes: bytes.len(),
                    maximum: MAX_SERVICE_IDENTITY_ENVELOPE_BYTES,
                });
            }
            let envelope: ServiceIdentityEnvelope = serde_json::from_slice(&bytes)
                .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))?;
            envelope.verify(registry)?;
            envelopes.push(envelope);
        }
        envelopes.sort_by(|left, right| {
            left.stream_id
                .cmp(&right.stream_id)
                .then(left.source_sequence.cmp(&right.source_sequence))
                .then(
                    left.envelope_digest()
                        .unwrap_or([0; 32])
                        .cmp(&right.envelope_digest().unwrap_or([0; 32])),
                )
        });
        Ok(envelopes)
    }

    pub fn enqueue(
        &self,
        envelope: &ServiceIdentityEnvelope,
        registry: &ServiceIdentityRegistry,
    ) -> Result<bool, ServiceIdentityError> {
        self.enqueue_with_sync_mode(envelope, registry, OutboxSyncMode::Durable)
    }

    #[cfg(feature = "benchmark")]
    pub fn enqueue_without_sync_for_benchmark(
        &self,
        envelope: &ServiceIdentityEnvelope,
        registry: &ServiceIdentityRegistry,
    ) -> Result<bool, ServiceIdentityError> {
        self.enqueue_with_sync_mode(envelope, registry, OutboxSyncMode::NoSyncBenchmarkOnly)
    }

    fn enqueue_with_sync_mode(
        &self,
        envelope: &ServiceIdentityEnvelope,
        registry: &ServiceIdentityRegistry,
        sync_mode: OutboxSyncMode,
    ) -> Result<bool, ServiceIdentityError> {
        envelope.verify(registry)?;
        let bytes = serde_json::to_vec(envelope)
            .map_err(|error| ServiceIdentityError::Serialization(error.to_string()))?;
        if bytes.len() > MAX_SERVICE_IDENTITY_ENVELOPE_BYTES {
            return Err(ServiceIdentityError::EnvelopeTooLarge {
                bytes: bytes.len(),
                maximum: MAX_SERVICE_IDENTITY_ENVELOPE_BYTES,
            });
        }
        let pending = self.pending(registry)?;
        validate_outbox_binding(&pending, envelope)?;
        if pending.len() >= self.maximum_entries {
            if pending.iter().any(|item| item == envelope) {
                return Ok(false);
            }
            return Err(ServiceIdentityError::OutboxFull {
                entries: pending.len(),
                maximum: self.maximum_entries,
            });
        }
        let digest = envelope.envelope_digest()?;
        let path = self.directory.join(format!("{}.json", hex_digest(&digest)));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)
                    .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
                if sync_mode == OutboxSyncMode::Durable {
                    file.sync_all()
                        .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
                    sync_directory(&self.directory)?;
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read(&path).map_err(|read_error| {
                    ServiceIdentityError::Persistence(read_error.to_string())
                })?;
                if existing == bytes {
                    Ok(false)
                } else {
                    Err(ServiceIdentityError::EnvelopeCollision)
                }
            }
            Err(error) => Err(ServiceIdentityError::Persistence(error.to_string())),
        }
    }

    pub fn acknowledge(
        &self,
        envelope: &ServiceIdentityEnvelope,
        registry: &ServiceIdentityRegistry,
    ) -> Result<(), ServiceIdentityError> {
        envelope.verify(registry)?;
        let path = self
            .directory
            .join(format!("{}.json", hex_digest(&envelope.envelope_digest()?)));
        if !path.exists() {
            return Err(ServiceIdentityError::MissingEnvelope);
        }
        fs::remove_file(&path)
            .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
        sync_directory(&self.directory)?;
        Ok(())
    }
}

fn validate_outbox_binding(
    pending: &[ServiceIdentityEnvelope],
    envelope: &ServiceIdentityEnvelope,
) -> Result<(), ServiceIdentityError> {
    if envelope.source_sequence == 1 && envelope.predecessor.is_some() {
        return Err(ServiceIdentityError::BindingMismatch("first predecessor"));
    }
    if envelope.source_sequence > 1 && envelope.predecessor.is_none() {
        return Err(ServiceIdentityError::BindingMismatch("predecessor"));
    }
    let digest = envelope.envelope_digest()?;
    for existing in pending
        .iter()
        .filter(|item| item.stream_id == envelope.stream_id)
    {
        if existing.source_sequence == envelope.source_sequence {
            if existing.envelope_digest()? != digest {
                return Err(ServiceIdentityError::EnvelopeCollision);
            }
            continue;
        }
        if existing.source_sequence + 1 == envelope.source_sequence
            && Some(existing.envelope_digest()?) != envelope.predecessor
        {
            return Err(ServiceIdentityError::BindingMismatch("predecessor digest"));
        }
        if envelope.source_sequence + 1 == existing.source_sequence
            && existing.predecessor != Some(digest)
        {
            return Err(ServiceIdentityError::BindingMismatch(
                "successor predecessor digest",
            ));
        }
    }
    Ok(())
}

fn validate_signer_inputs(signer_id: &str, generation: u64) -> Result<(), ServiceIdentityError> {
    validate_identifier(signer_id, "signer id")?;
    if generation == 0 {
        return Err(ServiceIdentityError::InvalidGeneration);
    }
    Ok(())
}

fn validate_envelope_shape(envelope: &ServiceIdentityEnvelope) -> Result<(), ServiceIdentityError> {
    if envelope.schema_version != SERVICE_IDENTITY_SCHEMA_VERSION {
        return Err(ServiceIdentityError::BindingMismatch("schema version"));
    }
    validate_identifier(&envelope.service_id, "service id")?;
    validate_bound_identity_id(&envelope.identity_id)?;
    validate_signer_inputs(&envelope.signer_id, envelope.signer_generation)?;
    validate_identifier(&envelope.stream_id, "stream id")?;
    if envelope.evidence_digest == [0; 32] {
        return Err(ServiceIdentityError::InvalidDigest("evidence"));
    }
    if envelope.source_sequence == 0 || envelope.trust_generation == 0 {
        return Err(ServiceIdentityError::InvalidGeneration);
    }
    Ok(())
}

fn validate_bound_identity_id(value: &str) -> Result<(), ServiceIdentityError> {
    if value.is_empty()
        || value.len() > MAX_SERVICE_IDENTITY_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ServiceIdentityError::InvalidIdentifier("identity id"));
    }
    Ok(())
}

fn validate_identifier(value: &str, _label: &'static str) -> Result<(), ServiceIdentityError> {
    if value.is_empty()
        || value.len() > MAX_SERVICE_IDENTITY_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character == '/')
    {
        return Err(ServiceIdentityError::InvalidIdentifier(_label));
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sync_directory(directory: &Path) -> Result<(), ServiceIdentityError> {
    let file = OpenOptions::new()
        .read(true)
        .open(directory)
        .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))?;
    file.sync_all()
        .map_err(|error| ServiceIdentityError::Persistence(error.to_string()))
}
