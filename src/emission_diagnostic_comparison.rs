use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::emission_diagnostic::{EmissionDiagnosticError, EmissionDiagnosticReport};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticComparisonError {
    Before(EmissionDiagnosticError),
    After(EmissionDiagnosticError),
    ContextMismatch,
}

impl Display for EmissionDiagnosticComparisonError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Before(error) => write!(formatter, "before diagnostic report failed verification: {error}"),
            Self::After(error) => write!(formatter, "after diagnostic report failed verification: {error}"),
            Self::ContextMismatch => formatter.write_str(
                "diagnostic reports do not share the same target, batch, profile, or unit-root context",
            ),
        }
    }
}

impl std::error::Error for EmissionDiagnosticComparisonError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmissionDiagnosticDelta {
    observation_delta: i128,
    chunk_delta: i128,
    byte_delta: i128,
    digest_equal: bool,
}

impl EmissionDiagnosticDelta {
    pub fn observation_delta(&self) -> i128 {
        self.observation_delta
    }

    pub fn chunk_delta(&self) -> i128 {
        self.chunk_delta
    }

    pub fn byte_delta(&self) -> i128 {
        self.byte_delta
    }

    pub fn digest_equal(&self) -> bool {
        self.digest_equal
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticComparison {
    delta: EmissionDiagnosticDelta,
}

impl EmissionDiagnosticComparison {
    pub fn compare(
        before: &EmissionDiagnosticReport,
        after: &EmissionDiagnosticReport,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticComparisonError> {
        before
            .verify_for(envelope, profile, units)
            .map_err(EmissionDiagnosticComparisonError::Before)?;
        after
            .verify_for(envelope, profile, units)
            .map_err(EmissionDiagnosticComparisonError::After)?;

        let before_aggregate = before.aggregate();
        let after_aggregate = after.aggregate();
        if before_aggregate.target() != after_aggregate.target()
            || before_aggregate.batch_id() != after_aggregate.batch_id()
            || before_aggregate.profile_key() != after_aggregate.profile_key()
            || before_aggregate.unit_roots() != after_aggregate.unit_roots()
        {
            return Err(EmissionDiagnosticComparisonError::ContextMismatch);
        }

        Ok(Self {
            delta: EmissionDiagnosticDelta {
                observation_delta: after_aggregate.observations() as i128
                    - before_aggregate.observations() as i128,
                chunk_delta: after_aggregate.chunks_emitted() as i128
                    - before_aggregate.chunks_emitted() as i128,
                byte_delta: after_aggregate.bytes_emitted() as i128
                    - before_aggregate.bytes_emitted() as i128,
                digest_equal: before_aggregate.output_digest() == after_aggregate.output_digest(),
            },
        })
    }

    pub fn delta(&self) -> EmissionDiagnosticDelta {
        self.delta
    }
}
