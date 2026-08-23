use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::emission_diagnostic::{
    EmissionDiagnosticEntry, EmissionDiagnosticError, EmissionDiagnosticReport,
    MAX_DIAGNOSTIC_ENTRIES,
};
use crate::emission_receipt_aggregate::EmissionReceiptAggregate;
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::SemanticCacheKey;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_SERIALIZED_DIAGNOSTIC_BYTES: usize = 64 * 1024;
pub const MAX_SERIALIZED_DIAGNOSTIC_UNITS: usize = 256;
const SERIALIZATION_DOMAIN: &[u8] = b"un1c0/phase69/emission-diagnostic/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticSerializationError {
    Report(EmissionDiagnosticError),
    Json(String),
    EnvelopeTooLarge { bytes: usize, maximum: usize },
    InvalidEnvelope,
    InvalidTarget,
    InvalidBatchId,
    InvalidObservationCount,
    TooManyUnits { count: usize, maximum: usize },
    InvalidUnitId,
    InvalidEntryCount { count: usize, maximum: usize },
    NonCanonicalEntries,
    IntegrityMismatch,
}

impl Display for EmissionDiagnosticSerializationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Report(error) => {
                write!(formatter, "diagnostic report validation failed: {error}")
            }
            Self::Json(error) => write!(formatter, "diagnostic envelope JSON failed: {error}"),
            Self::EnvelopeTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "diagnostic envelope is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::InvalidEnvelope => {
                formatter.write_str("diagnostic envelope is structurally invalid")
            }
            Self::InvalidTarget => formatter.write_str("diagnostic envelope target is invalid"),
            Self::InvalidBatchId => {
                formatter.write_str("diagnostic envelope batch ID must be non-zero")
            }
            Self::InvalidObservationCount => {
                formatter.write_str("diagnostic envelope observation count must be non-zero")
            }
            Self::TooManyUnits { count, maximum } => {
                write!(
                    formatter,
                    "diagnostic envelope contains {count} units; maximum is {maximum}"
                )
            }
            Self::InvalidUnitId => {
                formatter.write_str("diagnostic envelope contains an invalid unit ID")
            }
            Self::InvalidEntryCount { count, maximum } => write!(
                formatter,
                "diagnostic envelope contains {count} entries; maximum is {maximum}"
            ),
            Self::NonCanonicalEntries => formatter
                .write_str("diagnostic envelope entries are not canonical for its aggregate"),
            Self::IntegrityMismatch => {
                formatter.write_str("diagnostic envelope integrity digest mismatch")
            }
        }
    }
}

impl std::error::Error for EmissionDiagnosticSerializationError {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticEnvelope {
    version: u8,
    target: String,
    batch_id: u64,
    profile_key: [u8; 32],
    unit_roots: BTreeMap<String, [u8; 32]>,
    chunks_emitted: usize,
    bytes_emitted: usize,
    output_digest: [u8; 32],
    observations: usize,
    entries: Vec<EmissionDiagnosticEntry>,
    integrity_digest: [u8; 32],
}

fn digest_payload(payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SERIALIZATION_DOMAIN);
    hasher.update(payload);
    hasher.finalize().into()
}

impl EmissionDiagnosticReport {
    pub fn to_json(&self) -> Result<Vec<u8>, EmissionDiagnosticSerializationError> {
        self.verify_entry_bounds()?;
        if self.aggregate().unit_roots().len() > MAX_SERIALIZED_DIAGNOSTIC_UNITS {
            return Err(EmissionDiagnosticSerializationError::TooManyUnits {
                count: self.aggregate().unit_roots().len(),
                maximum: MAX_SERIALIZED_DIAGNOSTIC_UNITS,
            });
        }
        let mut envelope = DiagnosticEnvelope {
            version: 1,
            target: self.aggregate().target().label().to_owned(),
            batch_id: self.aggregate().batch_id(),
            profile_key: *self.aggregate().profile_key().as_bytes(),
            unit_roots: self
                .aggregate()
                .unit_roots()
                .iter()
                .map(|(unit, root)| (unit.as_str().to_owned(), *root.as_bytes()))
                .collect(),
            chunks_emitted: self.aggregate().chunks_emitted(),
            bytes_emitted: self.aggregate().bytes_emitted(),
            output_digest: self.aggregate().output_digest(),
            observations: self.aggregate().observations(),
            entries: self.entries().to_vec(),
            integrity_digest: [0; 32],
        };
        let payload = serde_json::to_vec(&envelope)
            .map_err(|error| EmissionDiagnosticSerializationError::Json(error.to_string()))?;
        envelope.integrity_digest = digest_payload(&payload);
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| EmissionDiagnosticSerializationError::Json(error.to_string()))?;
        if bytes.len() > MAX_SERIALIZED_DIAGNOSTIC_BYTES {
            return Err(EmissionDiagnosticSerializationError::EnvelopeTooLarge {
                bytes: bytes.len(),
                maximum: MAX_SERIALIZED_DIAGNOSTIC_BYTES,
            });
        }
        Ok(bytes)
    }

    pub fn from_json_for(
        bytes: &[u8],
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticSerializationError> {
        if bytes.len() > MAX_SERIALIZED_DIAGNOSTIC_BYTES {
            return Err(EmissionDiagnosticSerializationError::EnvelopeTooLarge {
                bytes: bytes.len(),
                maximum: MAX_SERIALIZED_DIAGNOSTIC_BYTES,
            });
        }
        let mut wire: DiagnosticEnvelope = serde_json::from_slice(bytes)
            .map_err(|error| EmissionDiagnosticSerializationError::Json(error.to_string()))?;
        let provided_digest = wire.integrity_digest;
        wire.integrity_digest = [0; 32];
        let payload = serde_json::to_vec(&wire)
            .map_err(|error| EmissionDiagnosticSerializationError::Json(error.to_string()))?;
        if digest_payload(&payload) != provided_digest {
            return Err(EmissionDiagnosticSerializationError::IntegrityMismatch);
        }
        wire.integrity_digest = provided_digest;
        let canonical = serde_json::to_vec(&wire)
            .map_err(|error| EmissionDiagnosticSerializationError::Json(error.to_string()))?;
        if canonical != bytes {
            return Err(EmissionDiagnosticSerializationError::Json(
                "diagnostic envelope is not canonical".to_owned(),
            ));
        }
        if wire.version != 1 || wire.target != profile.target.label() {
            return Err(EmissionDiagnosticSerializationError::InvalidTarget);
        }
        if wire.batch_id == 0 {
            return Err(EmissionDiagnosticSerializationError::InvalidBatchId);
        }
        if wire.observations == 0 {
            return Err(EmissionDiagnosticSerializationError::InvalidObservationCount);
        }
        if wire.unit_roots.len() > MAX_SERIALIZED_DIAGNOSTIC_UNITS {
            return Err(EmissionDiagnosticSerializationError::TooManyUnits {
                count: wire.unit_roots.len(),
                maximum: MAX_SERIALIZED_DIAGNOSTIC_UNITS,
            });
        }
        if wire.entries.len() > MAX_DIAGNOSTIC_ENTRIES {
            return Err(EmissionDiagnosticSerializationError::InvalidEntryCount {
                count: wire.entries.len(),
                maximum: MAX_DIAGNOSTIC_ENTRIES,
            });
        }

        let mut unit_roots = BTreeMap::new();
        for (unit, root) in wire.unit_roots {
            let id = SemanticUnitId::new(unit)
                .map_err(|_| EmissionDiagnosticSerializationError::InvalidUnitId)?;
            unit_roots.insert(id, SemanticCacheKey::from_bytes(root));
        }
        if unit_roots.len() != units.len() {
            return Err(EmissionDiagnosticSerializationError::InvalidEnvelope);
        }
        let aggregate = EmissionReceiptAggregate::from_parts_for_serialization(
            profile.target,
            wire.batch_id,
            SemanticCacheKey::from_bytes(wire.profile_key),
            unit_roots,
            wire.chunks_emitted,
            wire.bytes_emitted,
            wire.output_digest,
            wire.observations,
        );
        let report =
            EmissionDiagnosticReport::from_verified_aggregate(aggregate, envelope, profile, units)
                .map_err(EmissionDiagnosticSerializationError::Report)?;
        if report.entries() != wire.entries {
            return Err(EmissionDiagnosticSerializationError::NonCanonicalEntries);
        }
        Ok(report)
    }

    fn verify_entry_bounds(&self) -> Result<(), EmissionDiagnosticSerializationError> {
        if self.entries().len() > MAX_DIAGNOSTIC_ENTRIES {
            return Err(EmissionDiagnosticSerializationError::InvalidEntryCount {
                count: self.entries().len(),
                maximum: MAX_DIAGNOSTIC_ENTRIES,
            });
        }
        self.entries()
            .iter()
            .enumerate()
            .try_for_each(|(index, entry)| {
                if entry.encoded_size_for_serialization()
                    > crate::emission_diagnostic::MAX_DIAGNOSTIC_ENTRY_BYTES
                {
                    return Err(EmissionDiagnosticSerializationError::Report(
                        EmissionDiagnosticError::EntryTooLarge {
                            index,
                            size: entry.encoded_size_for_serialization(),
                            maximum: crate::emission_diagnostic::MAX_DIAGNOSTIC_ENTRY_BYTES,
                        },
                    ));
                }
                Ok(())
            })
    }
}

impl EmissionDiagnosticEntry {
    fn encoded_size_for_serialization(&self) -> usize {
        match self {
            Self::ObservationCount { .. } => 1 + std::mem::size_of::<usize>(),
            Self::ChunkCount { .. } => 1 + std::mem::size_of::<usize>(),
            Self::BytesEmitted { .. } => 1 + std::mem::size_of::<usize>(),
            Self::DigestConfirmed { digest } => 1 + digest.len(),
        }
    }
}
