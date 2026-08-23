use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::emission_diagnostic_instrumentation::{DiagnosticStage, DiagnosticVerificationRecorder};
use crate::emission_receipt::EmissionReceipt;
use crate::emission_receipt_aggregate::{EmissionReceiptAggregate, ReceiptAggregateError};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_DIAGNOSTIC_ENTRIES: usize = 4;
pub const MAX_DIAGNOSTIC_ENTRY_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EmissionDiagnosticEntry {
    ObservationCount { count: usize },
    ChunkCount { chunks: usize },
    BytesEmitted { bytes: usize },
    DigestConfirmed { digest: [u8; 32] },
}

impl EmissionDiagnosticEntry {
    fn encoded_size(&self) -> usize {
        match self {
            Self::ObservationCount { .. } => 1 + std::mem::size_of::<usize>(),
            Self::ChunkCount { .. } => 1 + std::mem::size_of::<usize>(),
            Self::BytesEmitted { .. } => 1 + std::mem::size_of::<usize>(),
            Self::DigestConfirmed { digest } => 1 + digest.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticError {
    Aggregate(ReceiptAggregateError),
    TooManyEntries {
        count: usize,
        maximum: usize,
    },
    EntryTooLarge {
        index: usize,
        size: usize,
        maximum: usize,
    },
}

impl Display for EmissionDiagnosticError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aggregate(error) => write!(
                formatter,
                "emission diagnostic verification failed: {error}"
            ),
            Self::TooManyEntries { count, maximum } => write!(
                formatter,
                "emission diagnostic contains {count} entries; maximum is {maximum}"
            ),
            Self::EntryTooLarge {
                index,
                size,
                maximum,
            } => write!(
                formatter,
                "emission diagnostic entry {index} is {size} bytes; maximum is {maximum}"
            ),
        }
    }
}

impl std::error::Error for EmissionDiagnosticError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticReport {
    aggregate: EmissionReceiptAggregate,
    entries: Vec<EmissionDiagnosticEntry>,
}

impl EmissionDiagnosticReport {
    pub fn from_receipts(
        receipts: &[EmissionReceipt],
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticError> {
        let aggregate = EmissionReceiptAggregate::from_receipts(receipts)
            .map_err(EmissionDiagnosticError::Aggregate)?;
        Self::from_verified_aggregate(aggregate, envelope, profile, units)
    }

    pub fn from_verified_aggregate(
        aggregate: EmissionReceiptAggregate,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticError> {
        aggregate
            .verify_for(envelope, profile, units)
            .map_err(EmissionDiagnosticError::Aggregate)?;
        let entries = vec![
            EmissionDiagnosticEntry::ObservationCount {
                count: aggregate.observations(),
            },
            EmissionDiagnosticEntry::ChunkCount {
                chunks: aggregate.chunks_emitted(),
            },
            EmissionDiagnosticEntry::BytesEmitted {
                bytes: aggregate.bytes_emitted(),
            },
            EmissionDiagnosticEntry::DigestConfirmed {
                digest: aggregate.output_digest(),
            },
        ];
        validate_entries(&entries)?;
        Ok(Self { aggregate, entries })
    }

    pub fn aggregate(&self) -> &EmissionReceiptAggregate {
        &self.aggregate
    }

    pub fn entries(&self) -> &[EmissionDiagnosticEntry] {
        &self.entries
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticError> {
        validate_entries(&self.entries)?;
        self.aggregate
            .verify_for(envelope, profile, units)
            .map_err(EmissionDiagnosticError::Aggregate)
    }

    pub(crate) fn verify_for_instrumented(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<(), EmissionDiagnosticError> {
        let started_at = std::time::Instant::now();
        validate_entries(&self.entries)?;
        self.aggregate
            .verify_for_instrumented(envelope, profile, units, recorder)
            .map_err(EmissionDiagnosticError::Aggregate)?;
        recorder.record_elapsed(
            DiagnosticStage::NestedReportVerify,
            started_at.elapsed().as_nanos().min(u64::MAX as u128) as u64,
        );
        Ok(())
    }
}

fn validate_entries(entries: &[EmissionDiagnosticEntry]) -> Result<(), EmissionDiagnosticError> {
    if entries.len() > MAX_DIAGNOSTIC_ENTRIES {
        return Err(EmissionDiagnosticError::TooManyEntries {
            count: entries.len(),
            maximum: MAX_DIAGNOSTIC_ENTRIES,
        });
    }
    for (index, entry) in entries.iter().enumerate() {
        let size = entry.encoded_size();
        if size > MAX_DIAGNOSTIC_ENTRY_BYTES {
            return Err(EmissionDiagnosticError::EntryTooLarge {
                index,
                size,
                maximum: MAX_DIAGNOSTIC_ENTRY_BYTES,
            });
        }
    }
    Ok(())
}
