use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::codegen::TargetBinding;
use crate::emission_diagnostic_instrumentation::DiagnosticVerificationRecorder;
use crate::emission_receipt::{EmissionReceipt, EmissionReceiptError};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::SemanticCacheKey;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptAggregateError {
    Empty,
    TargetMismatch {
        expected: TargetBinding,
        actual: TargetBinding,
    },
    BatchMismatch {
        expected: u64,
        actual: u64,
    },
    ProfileMismatch,
    UnitRootsMismatch,
    StatisticsMismatch,
    DigestMismatch,
    Verification(EmissionReceiptError),
}

impl Display for ReceiptAggregateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("cannot aggregate an empty receipt set"),
            Self::TargetMismatch { expected, actual } => write!(
                formatter,
                "receipt aggregate target mismatch: expected {}, received {}",
                expected.label(),
                actual.label()
            ),
            Self::BatchMismatch { expected, actual } => write!(
                formatter,
                "receipt aggregate batch mismatch: expected {expected}, received {actual}"
            ),
            Self::ProfileMismatch => formatter.write_str("receipt aggregate profile mismatch"),
            Self::UnitRootsMismatch => formatter.write_str("receipt aggregate unit-root mismatch"),
            Self::StatisticsMismatch => {
                formatter.write_str("receipt aggregate statistics mismatch")
            }
            Self::DigestMismatch => formatter.write_str("receipt aggregate output digest mismatch"),
            Self::Verification(error) => {
                write!(formatter, "receipt aggregate verification failed: {error}")
            }
        }
    }
}

impl std::error::Error for ReceiptAggregateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionReceiptAggregate {
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
    chunks_emitted: usize,
    bytes_emitted: usize,
    output_digest: [u8; 32],
    observations: usize,
}

impl EmissionReceiptAggregate {
    pub fn from_receipts(receipts: &[EmissionReceipt]) -> Result<Self, ReceiptAggregateError> {
        let first = receipts.first().ok_or(ReceiptAggregateError::Empty)?;
        for receipt in &receipts[1..] {
            if receipt.target() != first.target() {
                return Err(ReceiptAggregateError::TargetMismatch {
                    expected: first.target(),
                    actual: receipt.target(),
                });
            }
            if receipt.batch_id() != first.batch_id() {
                return Err(ReceiptAggregateError::BatchMismatch {
                    expected: first.batch_id(),
                    actual: receipt.batch_id(),
                });
            }
            if receipt.profile_key() != first.profile_key() {
                return Err(ReceiptAggregateError::ProfileMismatch);
            }
            if receipt.unit_roots() != first.unit_roots() {
                return Err(ReceiptAggregateError::UnitRootsMismatch);
            }
            if receipt.chunks_emitted() != first.chunks_emitted()
                || receipt.bytes_emitted() != first.bytes_emitted()
            {
                return Err(ReceiptAggregateError::StatisticsMismatch);
            }
            if receipt.output_digest() != first.output_digest() {
                return Err(ReceiptAggregateError::DigestMismatch);
            }
        }
        Ok(Self {
            target: first.target(),
            batch_id: first.batch_id(),
            profile_key: first.profile_key(),
            unit_roots: first.unit_roots().clone(),
            chunks_emitted: first.chunks_emitted(),
            bytes_emitted: first.bytes_emitted(),
            output_digest: first.output_digest(),
            observations: receipts.len(),
        })
    }

    pub(crate) fn from_parts_for_serialization(
        target: TargetBinding,
        batch_id: u64,
        profile_key: SemanticCacheKey,
        unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
        chunks_emitted: usize,
        bytes_emitted: usize,
        output_digest: [u8; 32],
        observations: usize,
    ) -> Self {
        Self {
            target,
            batch_id,
            profile_key,
            unit_roots,
            chunks_emitted,
            bytes_emitted,
            output_digest,
            observations,
        }
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

    pub fn chunks_emitted(&self) -> usize {
        self.chunks_emitted
    }

    pub fn bytes_emitted(&self) -> usize {
        self.bytes_emitted
    }

    pub fn output_digest(&self) -> [u8; 32] {
        self.output_digest
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), ReceiptAggregateError> {
        self.verify_for_inner(envelope, profile, units, None)
    }

    pub(crate) fn verify_for_instrumented(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<(), ReceiptAggregateError> {
        self.verify_for_inner(envelope, profile, units, Some(recorder))
    }

    fn verify_for_inner(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        mut recorder: Option<&mut DiagnosticVerificationRecorder>,
    ) -> Result<(), ReceiptAggregateError> {
        if self.target != profile.target {
            return Err(ReceiptAggregateError::TargetMismatch {
                expected: profile.target,
                actual: self.target,
            });
        }
        let receipt = EmissionReceipt::from_parts_for_verification(
            self.target,
            self.batch_id,
            self.profile_key,
            self.unit_roots.clone(),
            self.chunks_emitted,
            self.bytes_emitted,
            self.output_digest,
        );
        if let Some(recorder) = recorder.as_deref_mut() {
            receipt
                .verify_for_instrumented(envelope, self.batch_id, profile, units, recorder)
                .map_err(ReceiptAggregateError::Verification)
        } else {
            receipt
                .verify_for(envelope, self.batch_id, profile, units)
                .map_err(ReceiptAggregateError::Verification)
        }
    }
}
