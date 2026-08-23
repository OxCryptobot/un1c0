use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::codegen::{GeneratedChunk, GenerationError, IncrementalCodeGenerator, TargetBinding};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::{SemanticSnapshotEnvelope, SemanticSnapshotEnvelopeError};
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotEmissionError {
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
}

impl Display for SnapshotEmissionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetMismatch { expected, actual } => write!(
                formatter,
                "snapshot emission target mismatch: expected {}, received {}",
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
        }
    }
}

impl std::error::Error for SnapshotEmissionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchGenerationStats {
    pub target: TargetBinding,
    pub units_emitted: usize,
    pub chunks_emitted: usize,
    pub bytes_emitted: usize,
}

pub struct SnapshotBoundBatchEmitter {
    target: TargetBinding,
}

impl SnapshotBoundBatchEmitter {
    pub fn new(target: TargetBinding) -> Self {
        Self { target }
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn emit<F, E>(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        mut sink: F,
    ) -> Result<BatchGenerationStats, SnapshotEmissionError>
    where
        F: FnMut(&SemanticUnitId, GeneratedChunk) -> Result<(), E>,
        E: Display,
    {
        if profile.target != self.target {
            return Err(SnapshotEmissionError::TargetMismatch {
                expected: self.target,
                actual: profile.target,
            });
        }
        envelope
            .verify_for(batch_id, profile, units)
            .map_err(SnapshotEmissionError::Envelope)?;

        let mut stats = BatchGenerationStats {
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
            let mut unit_chunks = 0;
            let result = generator.emit_remaining_with_snapshot(ueg, snapshot, |chunk| {
                sink(unit, chunk).map_err(|error| SnapshotEmissionError::Sink {
                    unit: unit.clone(),
                    message: error.to_string(),
                })?;
                unit_chunks += 1;
                Ok::<(), SnapshotEmissionError>(())
            });
            let unit_stats = result.map_err(|source| SnapshotEmissionError::Unit {
                unit: unit.clone(),
                source,
            })?;
            stats.units_emitted += 1;
            stats.chunks_emitted += unit_chunks.max(unit_stats.chunks_emitted);
            stats.bytes_emitted += unit_stats.bytes_emitted;
        }
        Ok(stats)
    }
}
