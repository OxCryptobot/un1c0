use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

use crate::emission_receipt_aggregate::{EmissionReceiptAggregate, ReceiptAggregateError};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;
use crate::EmissionReceipt;

const EVIDENCE_DOMAIN: &[u8] = b"un1c0/phase67/emission-evidence/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionEvidenceError {
    Aggregate(ReceiptAggregateError),
    DigestMismatch,
}

impl Display for EmissionEvidenceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Aggregate(error) => {
                write!(formatter, "emission evidence aggregate failed: {error}")
            }
            Self::DigestMismatch => formatter.write_str("emission evidence digest mismatch"),
        }
    }
}

impl std::error::Error for EmissionEvidenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionEvidenceBundle {
    aggregate: EmissionReceiptAggregate,
    evidence_digest: [u8; 32],
}

impl EmissionEvidenceBundle {
    pub fn from_receipts(receipts: &[EmissionReceipt]) -> Result<Self, EmissionEvidenceError> {
        let aggregate = EmissionReceiptAggregate::from_receipts(receipts)
            .map_err(EmissionEvidenceError::Aggregate)?;
        let evidence_digest = digest_aggregate(&aggregate);
        Ok(Self {
            aggregate,
            evidence_digest,
        })
    }

    pub fn aggregate(&self) -> &EmissionReceiptAggregate {
        &self.aggregate
    }

    pub fn evidence_digest(&self) -> [u8; 32] {
        self.evidence_digest
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionEvidenceError> {
        if digest_aggregate(&self.aggregate) != self.evidence_digest {
            return Err(EmissionEvidenceError::DigestMismatch);
        }
        self.aggregate
            .verify_for(envelope, profile, units)
            .map_err(EmissionEvidenceError::Aggregate)
    }
}

fn digest_aggregate(aggregate: &EmissionReceiptAggregate) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(EVIDENCE_DOMAIN);
    feed_str(&mut hasher, aggregate.target().label());
    hasher.update(aggregate.batch_id().to_be_bytes());
    hasher.update(aggregate.profile_key().as_bytes());
    hasher.update((aggregate.unit_roots().len() as u64).to_be_bytes());
    for (unit, root) in aggregate.unit_roots() {
        feed_str(&mut hasher, unit.as_str());
        hasher.update(root.as_bytes());
    }
    hasher.update((aggregate.chunks_emitted() as u64).to_be_bytes());
    hasher.update((aggregate.bytes_emitted() as u64).to_be_bytes());
    hasher.update(aggregate.output_digest());
    hasher.update((aggregate.observations() as u64).to_be_bytes());
    hasher.finalize().into()
}

fn feed_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}
