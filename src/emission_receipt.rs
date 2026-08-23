use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

use crate::codegen::{GeneratedChunk, GenerationError, IncrementalCodeGenerator, TargetBinding};
use crate::emission_diagnostic_instrumentation::DiagnosticVerificationRecorder;
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::SemanticCacheKey;
use crate::semantic_snapshot_envelope::{SemanticSnapshotEnvelope, SemanticSnapshotEnvelopeError};
use crate::walker::Ueg;

const RECEIPT_DOMAIN: &[u8] = b"un1c0/phase65/emission-receipt/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionReceiptError {
    TargetMismatch {
        expected: TargetBinding,
        actual: TargetBinding,
    },
    Envelope(SemanticSnapshotEnvelopeError),
    Unit {
        unit: SemanticUnitId,
        source: GenerationError,
    },
    Sink {
        unit: SemanticUnitId,
        message: String,
    },
    ReceiptTargetMismatch {
        expected: TargetBinding,
        actual: TargetBinding,
    },
    ReceiptBatchMismatch {
        expected: u64,
        actual: u64,
    },
    ReceiptProfileMismatch {
        expected: SemanticCacheKey,
        actual: SemanticCacheKey,
    },
    ReceiptUnitsMismatch,
    ReceiptStatsMismatch,
    ReceiptDigestMismatch,
}

impl Display for EmissionReceiptError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetMismatch { expected, actual } => write!(
                formatter,
                "emission target mismatch: expected {}, received {}",
                expected.label(),
                actual.label()
            ),
            Self::Envelope(error) => {
                write!(formatter, "snapshot envelope rejected emission: {error}")
            }
            Self::Unit { unit, source } => {
                write!(formatter, "unit {unit} emission failed: {source}")
            }
            Self::Sink { unit, message } => {
                write!(formatter, "unit {unit} sink rejected a chunk: {message}")
            }
            Self::ReceiptTargetMismatch { expected, actual } => write!(
                formatter,
                "receipt target mismatch: expected {}, received {}",
                expected.label(),
                actual.label()
            ),
            Self::ReceiptBatchMismatch { expected, actual } => write!(
                formatter,
                "receipt batch mismatch: expected {expected}, received {actual}"
            ),
            Self::ReceiptProfileMismatch { .. } => {
                formatter.write_str("receipt profile key mismatch")
            }
            Self::ReceiptUnitsMismatch => formatter.write_str("receipt unit-root map mismatch"),
            Self::ReceiptStatsMismatch => {
                formatter.write_str("receipt generation statistics mismatch")
            }
            Self::ReceiptDigestMismatch => formatter.write_str("receipt output digest mismatch"),
        }
    }
}

impl std::error::Error for EmissionReceiptError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionReceipt {
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
    chunks_emitted: usize,
    bytes_emitted: usize,
    output_digest: [u8; 32],
}

impl EmissionReceipt {
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

    pub fn chunks_emitted(&self) -> usize {
        self.chunks_emitted
    }

    pub fn bytes_emitted(&self) -> usize {
        self.bytes_emitted
    }

    pub fn output_digest(&self) -> [u8; 32] {
        self.output_digest
    }

    pub(crate) fn from_parts_for_verification(
        target: TargetBinding,
        batch_id: u64,
        profile_key: SemanticCacheKey,
        unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
        chunks_emitted: usize,
        bytes_emitted: usize,
        output_digest: [u8; 32],
    ) -> Self {
        Self {
            target,
            batch_id,
            profile_key,
            unit_roots,
            chunks_emitted,
            bytes_emitted,
            output_digest,
        }
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionReceiptError> {
        self.verify_for_inner(envelope, batch_id, profile, units, None)
    }

    pub(crate) fn verify_for_instrumented(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<(), EmissionReceiptError> {
        self.verify_for_inner(envelope, batch_id, profile, units, Some(recorder))
    }

    fn verify_for_inner(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        mut recorder: Option<&mut DiagnosticVerificationRecorder>,
    ) -> Result<(), EmissionReceiptError> {
        if self.target != profile.target {
            return Err(EmissionReceiptError::ReceiptTargetMismatch {
                expected: profile.target,
                actual: self.target,
            });
        }
        if self.batch_id != batch_id || envelope.batch_id() != batch_id {
            return Err(EmissionReceiptError::ReceiptBatchMismatch {
                expected: envelope.batch_id(),
                actual: batch_id,
            });
        }
        if let Some(recorder) = recorder.as_deref_mut() {
            envelope
                .verify_for_instrumented(batch_id, profile, units, recorder)
                .map_err(EmissionReceiptError::Envelope)?;
        } else {
            envelope
                .verify_for(batch_id, profile, units)
                .map_err(EmissionReceiptError::Envelope)?;
        }
        if self.profile_key != envelope.profile_key() {
            return Err(EmissionReceiptError::ReceiptProfileMismatch {
                expected: envelope.profile_key(),
                actual: self.profile_key,
            });
        }
        let expected_roots = envelope
            .units()
            .iter()
            .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
            .collect::<BTreeMap<_, _>>();
        if self.unit_roots != expected_roots {
            return Err(EmissionReceiptError::ReceiptUnitsMismatch);
        }
        let expected_chunks = units.values().map(|ueg| ueg.nodes.len()).sum::<usize>();
        if self.chunks_emitted != expected_chunks {
            return Err(EmissionReceiptError::ReceiptStatsMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptGenerationStats {
    pub target: TargetBinding,
    pub units_emitted: usize,
    pub chunks_emitted: usize,
    pub bytes_emitted: usize,
}

pub struct ReceiptBoundBatchEmitter {
    target: TargetBinding,
}

impl ReceiptBoundBatchEmitter {
    pub fn new(target: TargetBinding) -> Self {
        Self { target }
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn emit_with_receipt<F, E>(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        mut sink: F,
    ) -> Result<(EmissionReceipt, ReceiptGenerationStats), EmissionReceiptError>
    where
        F: FnMut(&SemanticUnitId, GeneratedChunk) -> Result<(), E>,
        E: Display,
    {
        if profile.target != self.target {
            return Err(EmissionReceiptError::TargetMismatch {
                expected: self.target,
                actual: profile.target,
            });
        }
        envelope
            .verify_for(batch_id, profile, units)
            .map_err(EmissionReceiptError::Envelope)?;

        let mut hasher = Sha256::new();
        hasher.update(RECEIPT_DOMAIN);
        hasher.update(self.target.label().as_bytes());
        hasher.update(batch_id.to_le_bytes());
        hasher.update(envelope.profile_key().as_bytes());
        let mut stats = ReceiptGenerationStats {
            target: self.target,
            units_emitted: 0,
            chunks_emitted: 0,
            bytes_emitted: 0,
        };
        for (unit, ueg) in units {
            let mut generator = IncrementalCodeGenerator::new(self.target);
            let snapshot = envelope
                .units()
                .get(unit)
                .expect("envelope unit set was verified")
                .semantic_snapshot();
            let unit_stats = generator
                .emit_remaining_with_snapshot(ueg, snapshot, |chunk| {
                    sink(unit, chunk.clone()).map_err(|error| EmissionReceiptError::Sink {
                        unit: unit.clone(),
                        message: error.to_string(),
                    })?;
                    hash_chunk(&mut hasher, unit, &chunk);
                    Ok::<(), EmissionReceiptError>(())
                })
                .map_err(|source| EmissionReceiptError::Unit {
                    unit: unit.clone(),
                    source,
                })?;
            stats.units_emitted += 1;
            stats.chunks_emitted += unit_stats.chunks_emitted;
            stats.bytes_emitted += unit_stats.bytes_emitted;
        }
        let output_digest = hasher.finalize().into();
        let unit_roots = envelope
            .units()
            .iter()
            .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
            .collect();
        let receipt = EmissionReceipt {
            target: self.target,
            batch_id,
            profile_key: envelope.profile_key(),
            unit_roots,
            chunks_emitted: stats.chunks_emitted,
            bytes_emitted: stats.bytes_emitted,
            output_digest,
        };
        Ok((receipt, stats))
    }
}

fn hash_chunk(hasher: &mut Sha256, unit: &SemanticUnitId, chunk: &GeneratedChunk) {
    let unit_bytes = unit.as_str().as_bytes();
    hasher.update((unit_bytes.len() as u64).to_le_bytes());
    hasher.update(unit_bytes);
    hasher.update((chunk.node_index as u64).to_le_bytes());
    hasher.update((chunk.code.len() as u64).to_le_bytes());
    hasher.update(chunk.code.as_bytes());
}
