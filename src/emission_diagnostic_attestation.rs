use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::codegen::TargetBinding;
use crate::emission_diagnostic_cache::DiagnosticEvidenceCache;
use crate::emission_diagnostic_instrumentation::{
    DiagnosticCounter, DiagnosticInstrumentation, DiagnosticStage, DiagnosticVerificationRecorder,
    VerificationOutcome,
};
use crate::emission_diagnostic_stream::{EmissionDiagnosticStream, EmissionDiagnosticStreamError};
use crate::emission_diagnostic_transport::{
    DistributedEmissionAggregateSummary, DistributedEmissionAggregator,
    EmissionDiagnosticTransportError,
};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_TRUSTED_ATTESTATION_KEYS: usize = 32;
pub const MAX_ATTESTATION_METADATA_ENTRIES: usize = 8;
pub const MAX_ATTESTATION_METADATA_KEY_BYTES: usize = 64;
pub const MAX_ATTESTATION_METADATA_VALUE_BYTES: usize = 256;
pub const MAX_SERIALIZED_ATTESTATION_BYTES: usize = 32 * 1024;
const ATTESTATION_VERSION: u8 = 1;
const ATTESTATION_DOMAIN: &[u8] = b"un1c0/phase73/emission-diagnostic-attestation/v1";
const CONTENT_DOMAIN: &[u8] = b"un1c0/phase73/emission-diagnostic-content/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmissionDiagnosticAttestationContent {
    Stream,
    Aggregate,
}

impl EmissionDiagnosticAttestationContent {
    fn tag(self) -> u8 {
        match self {
            Self::Stream => 0,
            Self::Aggregate => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticAttestationError {
    InvalidVersion(u8),
    InvalidAttestationId,
    EmptyAggregate,
    WrongContentType {
        expected: EmissionDiagnosticAttestationContent,
        actual: EmissionDiagnosticAttestationContent,
    },
    UnknownPublicKey,
    TrustEpochMismatch {
        expected: u64,
        actual: u64,
    },
    TooManyTrustedKeys {
        count: usize,
        maximum: usize,
    },
    InvalidPublicKey,
    InvalidSignature,
    ContentMismatch,
    MetadataTooLarge {
        count: usize,
        maximum: usize,
    },
    MetadataKeyTooLarge {
        bytes: usize,
        maximum: usize,
    },
    MetadataValueTooLarge {
        bytes: usize,
        maximum: usize,
    },
    SerializedTooLarge {
        bytes: usize,
        maximum: usize,
    },
    NonCanonical,
    Json(String),
    Stream(EmissionDiagnosticStreamError),
    Aggregate(EmissionDiagnosticTransportError),
}

impl Display for EmissionDiagnosticAttestationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidVersion(version) => {
                write!(formatter, "unsupported attestation version {version}")
            }
            Self::InvalidAttestationId => formatter.write_str("attestation ID must be non-zero"),
            Self::EmptyAggregate => {
                formatter.write_str("aggregate attestation requires observations")
            }
            Self::WrongContentType { expected, actual } => write!(
                formatter,
                "attestation content type is {actual:?}; expected {expected:?}"
            ),
            Self::UnknownPublicKey => formatter.write_str("attestation public key is not trusted"),
            Self::TrustEpochMismatch { expected, actual } => write!(
                formatter,
                "attestation trust epoch mismatch: expected {expected}, received {actual}"
            ),
            Self::TooManyTrustedKeys { count, maximum } => write!(
                formatter,
                "trust store contains {count} keys; maximum is {maximum}"
            ),
            Self::InvalidPublicKey => formatter.write_str("attestation public key is invalid"),
            Self::InvalidSignature => formatter.write_str("attestation signature is invalid"),
            Self::ContentMismatch => formatter.write_str("attestation content hash does not match"),
            Self::MetadataTooLarge { count, maximum } => write!(
                formatter,
                "attestation metadata contains {count} entries; maximum is {maximum}"
            ),
            Self::MetadataKeyTooLarge { bytes, maximum } => write!(
                formatter,
                "attestation metadata key is {bytes} bytes; maximum is {maximum}"
            ),
            Self::MetadataValueTooLarge { bytes, maximum } => write!(
                formatter,
                "attestation metadata value is {bytes} bytes; maximum is {maximum}"
            ),
            Self::SerializedTooLarge { bytes, maximum } => write!(
                formatter,
                "serialized attestation is {bytes} bytes; maximum is {maximum}"
            ),
            Self::NonCanonical => formatter.write_str("attestation bytes are not canonical"),
            Self::Json(error) => write!(formatter, "attestation JSON failed: {error}"),
            Self::Stream(error) => write!(formatter, "attested stream failed: {error}"),
            Self::Aggregate(error) => write!(formatter, "attested aggregate failed: {error}"),
        }
    }
}

impl std::error::Error for EmissionDiagnosticAttestationError {}

impl From<EmissionDiagnosticStreamError> for EmissionDiagnosticAttestationError {
    fn from(error: EmissionDiagnosticStreamError) -> Self {
        Self::Stream(error)
    }
}

impl From<EmissionDiagnosticTransportError> for EmissionDiagnosticAttestationError {
    fn from(error: EmissionDiagnosticTransportError) -> Self {
        Self::Aggregate(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmissionDiagnosticAttestation {
    version: u8,
    attestation_id: u64,
    content_type: EmissionDiagnosticAttestationContent,
    content_hash: [u8; 32],
    public_key: Vec<u8>,
    signature: Vec<u8>,
    metadata: BTreeMap<String, String>,
}

impl EmissionDiagnosticAttestation {
    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn attestation_id(&self) -> u64 {
        self.attestation_id
    }

    pub fn content_type(&self) -> EmissionDiagnosticAttestationContent {
        self.content_type
    }

    pub fn content_hash(&self) -> [u8; 32] {
        self.content_hash
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.try_public_key()
            .expect("validated attestation public key length")
    }

    pub(crate) fn try_public_key(&self) -> Result<[u8; 32], EmissionDiagnosticAttestationError> {
        self.public_key
            .as_slice()
            .try_into()
            .map_err(|_| EmissionDiagnosticAttestationError::InvalidPublicKey)
    }

    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    pub fn to_json(&self) -> Result<Vec<u8>, EmissionDiagnosticAttestationError> {
        self.validate_shape()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| EmissionDiagnosticAttestationError::Json(error.to_string()))?;
        check_serialized_size(bytes.len())?;
        Ok(bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, EmissionDiagnosticAttestationError> {
        check_serialized_size(bytes.len())?;
        let attestation: Self = serde_json::from_slice(bytes)
            .map_err(|error| EmissionDiagnosticAttestationError::Json(error.to_string()))?;
        attestation.validate_shape()?;
        let canonical = serde_json::to_vec(&attestation)
            .map_err(|error| EmissionDiagnosticAttestationError::Json(error.to_string()))?;
        if canonical != bytes {
            return Err(EmissionDiagnosticAttestationError::NonCanonical);
        }
        Ok(attestation)
    }

    fn validate_shape(&self) -> Result<(), EmissionDiagnosticAttestationError> {
        if self.version != ATTESTATION_VERSION {
            return Err(EmissionDiagnosticAttestationError::InvalidVersion(
                self.version,
            ));
        }
        if self.attestation_id == 0 {
            return Err(EmissionDiagnosticAttestationError::InvalidAttestationId);
        }
        if self.public_key.len() != 32 {
            return Err(EmissionDiagnosticAttestationError::InvalidPublicKey);
        }
        if self.signature.len() != 64 {
            return Err(EmissionDiagnosticAttestationError::InvalidSignature);
        }
        validate_metadata(&self.metadata)
    }

    fn signing_payload(&self) -> Result<Vec<u8>, EmissionDiagnosticAttestationError> {
        self.validate_shape()?;
        #[derive(Serialize)]
        struct Payload<'a> {
            version: u8,
            attestation_id: u64,
            content_type: EmissionDiagnosticAttestationContent,
            content_hash: [u8; 32],
            public_key: [u8; 32],
            metadata: &'a BTreeMap<String, String>,
        }
        let canonical = serde_json::to_vec(&Payload {
            version: self.version,
            attestation_id: self.attestation_id,
            content_type: self.content_type,
            content_hash: self.content_hash,
            public_key: self.try_public_key()?,
            metadata: &self.metadata,
        })
        .map_err(|error| EmissionDiagnosticAttestationError::Json(error.to_string()))?;
        let mut payload = Vec::with_capacity(ATTESTATION_DOMAIN.len() + canonical.len());
        payload.extend_from_slice(ATTESTATION_DOMAIN);
        payload.extend_from_slice(&canonical);
        Ok(payload)
    }
}

#[derive(Debug, Clone)]
pub struct CanonicalDiagnosticEvidence {
    stream: Arc<EmissionDiagnosticStream>,
    canonical_stream_bytes: Arc<[u8]>,
    stream_digest: [u8; 32],
    content_hash: [u8; 32],
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
    frame_count: u16,
    total_frame_bytes: u32,
}

impl CanonicalDiagnosticEvidence {
    pub fn from_stream(
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        instrumentation: &DiagnosticInstrumentation,
    ) -> Result<Self, EmissionDiagnosticAttestationError> {
        let mut recorder =
            instrumentation.recorder(stream.frame_count(), stream.total_frame_bytes());
        let result =
            Self::from_stream_with_recorder(stream, envelope, profile, units, &mut recorder);
        let outcome = if result.is_ok() {
            VerificationOutcome::Accepted
        } else {
            VerificationOutcome::Rejected
        };
        recorder.finish(outcome);
        result
    }

    fn from_stream_with_recorder(
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<Self, EmissionDiagnosticAttestationError> {
        stream.verify_for_instrumented(envelope, profile, units, recorder)?;
        let canonical_stream_bytes = recorder
            .time(DiagnosticStage::CanonicalStreamSerialize, || {
                Ok::<_, EmissionDiagnosticStreamError>(stream.canonical_json_arc())
            })?;
        let content_hash = recorder.time(DiagnosticStage::ContentHash, || {
            content_hash(
                EmissionDiagnosticAttestationContent::Stream,
                &canonical_stream_bytes,
            )
        });
        recorder.increment(DiagnosticCounter::ContentHash);
        Self::from_verified_stream(stream.clone(), canonical_stream_bytes, content_hash)
    }

    pub fn stream(&self) -> &EmissionDiagnosticStream {
        &self.stream
    }

    pub fn canonical_stream_bytes(&self) -> &[u8] {
        &self.canonical_stream_bytes
    }

    pub fn stream_digest(&self) -> [u8; 32] {
        self.stream_digest
    }

    pub fn content_hash(&self) -> [u8; 32] {
        self.content_hash
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn batch_id(&self) -> u64 {
        self.batch_id
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn unit_roots(&self) -> &BTreeMap<SemanticUnitId, SemanticCacheKey> {
        &self.unit_roots
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count as usize
    }

    pub fn total_frame_bytes(&self) -> usize {
        self.total_frame_bytes as usize
    }

    pub fn matches_context(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
    ) -> bool {
        self.batch_id == envelope.batch_id()
            && self.target == profile.target
            && self.profile_key == envelope.profile_key()
            && self.unit_roots
                == envelope
                    .units()
                    .iter()
                    .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
                    .collect::<BTreeMap<_, _>>()
    }

    pub fn matches_current_candidates(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> bool {
        if !self.matches_context(envelope, profile) || units.len() != self.unit_roots.len() {
            return false;
        }
        units.iter().all(|(unit, ueg)| {
            self.unit_roots.get(unit).is_some_and(|expected| {
                SemanticFingerprint::from_ueg(ueg, profile).root_key() == *expected
            })
        })
    }

    fn from_verified_stream(
        stream: EmissionDiagnosticStream,
        canonical_stream_bytes: Arc<[u8]>,
        content_hash: [u8; 32],
    ) -> Result<Self, EmissionDiagnosticAttestationError> {
        let frame_count = u16::try_from(stream.frame_count()).map_err(|_| {
            EmissionDiagnosticAttestationError::SerializedTooLarge {
                bytes: stream.frame_count(),
                maximum: u16::MAX as usize,
            }
        })?;
        let total_frame_bytes = u32::try_from(stream.total_frame_bytes()).map_err(|_| {
            EmissionDiagnosticAttestationError::SerializedTooLarge {
                bytes: stream.total_frame_bytes(),
                maximum: u32::MAX as usize,
            }
        })?;
        Ok(Self {
            target: stream.target(),
            batch_id: stream.batch_id(),
            profile_key: stream.profile_key(),
            unit_roots: stream.unit_roots().clone(),
            stream_digest: stream.stream_digest(),
            stream: Arc::new(stream),
            canonical_stream_bytes,
            content_hash,
            frame_count,
            total_frame_bytes,
        })
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedDiagnosticEvidence {
    canonical: Arc<CanonicalDiagnosticEvidence>,
    attestation: Arc<EmissionDiagnosticAttestation>,
    trusted_key: [u8; 32],
    trust_epoch: u64,
}

impl VerifiedDiagnosticEvidence {
    pub fn canonical(&self) -> &CanonicalDiagnosticEvidence {
        &self.canonical
    }

    pub fn attestation_id(&self) -> u64 {
        self.attestation.attestation_id()
    }

    pub fn content_type(&self) -> EmissionDiagnosticAttestationContent {
        self.attestation.content_type()
    }

    pub fn trusted_key(&self) -> [u8; 32] {
        self.trusted_key
    }

    pub fn trust_epoch(&self) -> u64 {
        self.trust_epoch
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticAttestationKey {
    signing_key: SigningKey,
}

impl DiagnosticAttestationKey {
    pub fn from_signing_key(signing_key: SigningKey) -> Self {
        Self { signing_key }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn attest_stream(
        &self,
        attestation_id: u64,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        metadata: BTreeMap<String, String>,
    ) -> Result<EmissionDiagnosticAttestation, EmissionDiagnosticAttestationError> {
        stream.verify_for(envelope, profile, units)?;
        let content_hash = stream_content_hash(stream)?;
        self.create(
            attestation_id,
            EmissionDiagnosticAttestationContent::Stream,
            content_hash,
            metadata,
        )
    }

    pub fn attest_aggregate(
        &self,
        attestation_id: u64,
        aggregate: &DistributedEmissionAggregator,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        metadata: BTreeMap<String, String>,
    ) -> Result<EmissionDiagnosticAttestation, EmissionDiagnosticAttestationError> {
        aggregate.verify_for(envelope, profile, units)?;
        let summary = aggregate.summary();
        let content_hash = aggregate_content_hash(&summary)?;
        self.create(
            attestation_id,
            EmissionDiagnosticAttestationContent::Aggregate,
            content_hash,
            metadata,
        )
    }

    fn create(
        &self,
        attestation_id: u64,
        content_type: EmissionDiagnosticAttestationContent,
        content_hash: [u8; 32],
        metadata: BTreeMap<String, String>,
    ) -> Result<EmissionDiagnosticAttestation, EmissionDiagnosticAttestationError> {
        let mut attestation = EmissionDiagnosticAttestation {
            version: ATTESTATION_VERSION,
            attestation_id,
            content_type,
            content_hash,
            public_key: self.public_key().to_vec(),
            signature: vec![0; 64],
            metadata,
        };
        let payload = attestation.signing_payload()?;
        attestation.signature = self.signing_key.sign(&payload).to_bytes().to_vec();
        attestation.validate_shape()?;
        Ok(attestation)
    }
}

#[derive(Debug, Clone)]
struct RegisteredAttestationKey {
    verifying_key: VerifyingKey,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticAttestationVerifier {
    trusted_keys: BTreeMap<[u8; 32], RegisteredAttestationKey>,
    trust_epoch: u64,
}

impl DiagnosticAttestationVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_public_key(
        &mut self,
        public_key: [u8; 32],
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        if self.trusted_keys.contains_key(&public_key) {
            return Ok(());
        }
        if self.trusted_keys.len() >= MAX_TRUSTED_ATTESTATION_KEYS {
            return Err(EmissionDiagnosticAttestationError::TooManyTrustedKeys {
                count: self.trusted_keys.len() + 1,
                maximum: MAX_TRUSTED_ATTESTATION_KEYS,
            });
        }
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| EmissionDiagnosticAttestationError::InvalidPublicKey)?;
        self.trusted_keys
            .insert(public_key, RegisteredAttestationKey { verifying_key });
        self.trust_epoch = self.trust_epoch.saturating_add(1);
        Ok(())
    }

    pub fn revoke_public_key(&mut self, public_key: &[u8; 32]) -> bool {
        let removed = self.trusted_keys.remove(public_key).is_some();
        if removed {
            self.trust_epoch = self.trust_epoch.saturating_add(1);
        }
        removed
    }

    pub fn trusted_key_count(&self) -> usize {
        self.trusted_keys.len()
    }

    pub fn trust_epoch(&self) -> u64 {
        self.trust_epoch
    }

    pub(crate) fn contains_public_key(&self, public_key: &[u8; 32]) -> bool {
        self.trusted_keys.contains_key(public_key)
    }

    pub(crate) fn verify_evidence_current(
        &self,
        evidence: &VerifiedDiagnosticEvidence,
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        if evidence.trust_epoch != self.trust_epoch {
            return Err(EmissionDiagnosticAttestationError::TrustEpochMismatch {
                expected: self.trust_epoch,
                actual: evidence.trust_epoch,
            });
        }
        if !self.trusted_keys.contains_key(&evidence.trusted_key) {
            return Err(EmissionDiagnosticAttestationError::UnknownPublicKey);
        }
        if evidence.attestation.try_public_key()? != evidence.trusted_key {
            return Err(EmissionDiagnosticAttestationError::InvalidPublicKey);
        }
        Ok(())
    }

    pub fn verify_stream(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        stream.verify_for(envelope, profile, units)?;
        self.verify_common(
            attestation,
            EmissionDiagnosticAttestationContent::Stream,
            stream_content_hash(stream)?,
        )
    }

    pub fn verify_stream_evidence(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        instrumentation: &DiagnosticInstrumentation,
    ) -> Result<VerifiedDiagnosticEvidence, EmissionDiagnosticAttestationError> {
        let mut recorder =
            instrumentation.recorder(stream.frame_count(), stream.total_frame_bytes());
        let result = self.verify_stream_evidence_with_recorder(
            attestation,
            stream,
            envelope,
            profile,
            units,
            &mut recorder,
        );
        recorder.finish(if result.is_ok() {
            VerificationOutcome::Accepted
        } else {
            VerificationOutcome::Rejected
        });
        result
    }

    fn verify_stream_evidence_with_recorder(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<VerifiedDiagnosticEvidence, EmissionDiagnosticAttestationError> {
        let canonical = Arc::new(CanonicalDiagnosticEvidence::from_stream_with_recorder(
            stream, envelope, profile, units, recorder,
        )?);
        let trusted_key = self.verify_common_instrumented(
            attestation,
            EmissionDiagnosticAttestationContent::Stream,
            canonical.content_hash(),
            recorder,
        )?;
        Ok(VerifiedDiagnosticEvidence {
            canonical,
            attestation: Arc::new(attestation.clone()),
            trusted_key,
            trust_epoch: self.trust_epoch,
        })
    }

    pub fn verify_stream_evidence_with_cache(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        cache: &DiagnosticEvidenceCache,
        instrumentation: &DiagnosticInstrumentation,
    ) -> Result<VerifiedDiagnosticEvidence, EmissionDiagnosticAttestationError> {
        let mut recorder =
            instrumentation.recorder(stream.frame_count(), stream.total_frame_bytes());
        let result = (|| {
            let invalidated = cache.invalidate_trust_epoch(self.trust_epoch);
            for _ in 0..invalidated {
                recorder.increment(DiagnosticCounter::EvidenceCacheInvalidation);
            }
            let key = recorder.time(DiagnosticStage::EvidenceCacheLookup, || {
                cache.key_for(attestation, stream, envelope, profile, self.trust_epoch)
            })?;
            let cached = recorder.time(DiagnosticStage::EvidenceCacheLookup, || cache.lookup(key));
            if let Some(cached) = cached {
                let current = self.verify_evidence_current(cached.as_ref()).is_ok();
                if current
                    && cached
                        .canonical()
                        .matches_current_candidates(envelope, profile, units)
                {
                    recorder.increment(DiagnosticCounter::EvidenceCacheHit);
                    return Ok(cached.as_ref().clone());
                }
                if cache.invalidate_key(key) {
                    recorder.increment(DiagnosticCounter::EvidenceCacheInvalidation);
                }
            }
            recorder.increment(DiagnosticCounter::EvidenceCacheMiss);
            let evidence = self.verify_stream_evidence_with_recorder(
                attestation,
                stream,
                envelope,
                profile,
                units,
                &mut recorder,
            )?;
            let evidence = Arc::new(evidence);
            let inserted = recorder.time(DiagnosticStage::EvidenceCacheInsert, || {
                cache.insert(key, Arc::clone(&evidence))
            });
            if !inserted {
                return Ok(evidence.as_ref().clone());
            }
            Ok(evidence.as_ref().clone())
        })();
        recorder.finish(if result.is_ok() {
            VerificationOutcome::Accepted
        } else {
            VerificationOutcome::Rejected
        });
        result
    }

    pub fn verify_aggregate(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        summary: &DistributedEmissionAggregateSummary,
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        self.verify_common(
            attestation,
            EmissionDiagnosticAttestationContent::Aggregate,
            aggregate_content_hash(summary)?,
        )
    }

    pub fn verify_aggregate_for(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        aggregate: &DistributedEmissionAggregator,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        aggregate.verify_for(envelope, profile, units)?;
        self.verify_aggregate(attestation, &aggregate.summary())
    }

    fn verify_common(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        expected_type: EmissionDiagnosticAttestationContent,
        expected_hash: [u8; 32],
    ) -> Result<(), EmissionDiagnosticAttestationError> {
        let instrumentation = DiagnosticInstrumentation::disabled();
        let mut recorder = instrumentation.recorder(0, 0);
        self.verify_common_instrumented(attestation, expected_type, expected_hash, &mut recorder)
            .map(|_| ())
    }

    fn verify_common_instrumented(
        &self,
        attestation: &EmissionDiagnosticAttestation,
        expected_type: EmissionDiagnosticAttestationContent,
        expected_hash: [u8; 32],
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<[u8; 32], EmissionDiagnosticAttestationError> {
        recorder.time(DiagnosticStage::AttestationShape, || {
            attestation.validate_shape()
        })?;
        if attestation.content_type != expected_type {
            return Err(EmissionDiagnosticAttestationError::WrongContentType {
                expected: expected_type,
                actual: attestation.content_type,
            });
        }
        let public_key = attestation.try_public_key()?;
        recorder.increment(DiagnosticCounter::TrustLookup);
        let registered = recorder.time(DiagnosticStage::TrustLookup, || {
            self.trusted_keys.get(&public_key)
        });
        let registered = registered.ok_or(EmissionDiagnosticAttestationError::UnknownPublicKey)?;
        if attestation.content_hash != expected_hash {
            return Err(EmissionDiagnosticAttestationError::ContentMismatch);
        }
        let signature_bytes: [u8; 64] = attestation
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| EmissionDiagnosticAttestationError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let payload = recorder.time(DiagnosticStage::SigningPayloadSerialize, || {
            attestation.signing_payload()
        })?;
        recorder.increment(DiagnosticCounter::SignatureVerification);
        recorder
            .time(DiagnosticStage::Ed25519Verify, || {
                registered.verifying_key.verify(&payload, &signature)
            })
            .map_err(|_| EmissionDiagnosticAttestationError::InvalidSignature)?;
        Ok(public_key)
    }
}

fn validate_metadata(
    metadata: &BTreeMap<String, String>,
) -> Result<(), EmissionDiagnosticAttestationError> {
    if metadata.len() > MAX_ATTESTATION_METADATA_ENTRIES {
        return Err(EmissionDiagnosticAttestationError::MetadataTooLarge {
            count: metadata.len(),
            maximum: MAX_ATTESTATION_METADATA_ENTRIES,
        });
    }
    for (key, value) in metadata {
        if key.len() > MAX_ATTESTATION_METADATA_KEY_BYTES {
            return Err(EmissionDiagnosticAttestationError::MetadataKeyTooLarge {
                bytes: key.len(),
                maximum: MAX_ATTESTATION_METADATA_KEY_BYTES,
            });
        }
        if value.len() > MAX_ATTESTATION_METADATA_VALUE_BYTES {
            return Err(EmissionDiagnosticAttestationError::MetadataValueTooLarge {
                bytes: value.len(),
                maximum: MAX_ATTESTATION_METADATA_VALUE_BYTES,
            });
        }
    }
    Ok(())
}

fn check_serialized_size(bytes: usize) -> Result<(), EmissionDiagnosticAttestationError> {
    if bytes > MAX_SERIALIZED_ATTESTATION_BYTES {
        return Err(EmissionDiagnosticAttestationError::SerializedTooLarge {
            bytes,
            maximum: MAX_SERIALIZED_ATTESTATION_BYTES,
        });
    }
    Ok(())
}

fn stream_content_hash(
    stream: &EmissionDiagnosticStream,
) -> Result<[u8; 32], EmissionDiagnosticAttestationError> {
    let bytes = stream.canonical_json_bytes();
    if bytes.is_empty() {
        return Err(EmissionDiagnosticAttestationError::Json(
            "canonical stream JSON cache is unavailable".to_owned(),
        ));
    }
    Ok(content_hash(
        EmissionDiagnosticAttestationContent::Stream,
        bytes,
    ))
}

fn aggregate_content_hash(
    summary: &DistributedEmissionAggregateSummary,
) -> Result<[u8; 32], EmissionDiagnosticAttestationError> {
    if summary.source_count == 0 || summary.total_frames == 0 {
        return Err(EmissionDiagnosticAttestationError::EmptyAggregate);
    }
    #[derive(Serialize)]
    struct AggregateContent<'a> {
        source_count: usize,
        total_frames: usize,
        total_frame_bytes: usize,
        source_sequences: &'a BTreeMap<u64, u64>,
        aggregate_digest: [u8; 32],
    }
    let canonical = serde_json::to_vec(&AggregateContent {
        source_count: summary.source_count,
        total_frames: summary.total_frames,
        total_frame_bytes: summary.total_frame_bytes,
        source_sequences: &summary.source_sequences,
        aggregate_digest: summary.aggregate_digest,
    })
    .map_err(|error| EmissionDiagnosticAttestationError::Json(error.to_string()))?;
    Ok(content_hash(
        EmissionDiagnosticAttestationContent::Aggregate,
        &canonical,
    ))
}

fn content_hash(content_type: EmissionDiagnosticAttestationContent, canonical: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CONTENT_DOMAIN);
    hasher.update([content_type.tag()]);
    hasher.update(canonical);
    hasher.finalize().into()
}
