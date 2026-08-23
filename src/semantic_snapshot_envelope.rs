use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

use crate::emission_diagnostic_instrumentation::{DiagnosticStage, DiagnosticVerificationRecorder};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::{SemanticBatchError, SemanticBatchSession, SemanticUnitId};
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSnapshotEnvelopeError {
    InvalidBatchId,
    BatchNotApplied {
        batch_id: u64,
        next_batch_id: u64,
    },
    BatchIdMismatch {
        expected: u64,
        actual: u64,
    },
    SessionInvalidated,
    Session {
        unit: SemanticUnitId,
        source: SemanticBatchError,
    },
    EmptyUnitSet,
    MissingUnit(SemanticUnitId),
    UnexpectedUnit(SemanticUnitId),
    ProfileChanged {
        unit: SemanticUnitId,
        expected: SemanticCacheKey,
        actual: SemanticCacheKey,
    },
    UegChanged {
        unit: SemanticUnitId,
        expected: SemanticCacheKey,
        actual: SemanticCacheKey,
    },
}

impl Display for SemanticSnapshotEnvelopeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBatchId => formatter.write_str("semantic snapshot envelope id must be non-zero"),
            Self::BatchNotApplied {
                batch_id,
                next_batch_id,
            } => write!(
                formatter,
                "semantic snapshot envelope batch {batch_id} is not the current applied batch; next is {next_batch_id}"
            ),
            Self::BatchIdMismatch { expected, actual } => write!(
                formatter,
                "semantic snapshot envelope batch mismatch: expected {expected}, received {actual}"
            ),
            Self::SessionInvalidated => formatter.write_str("cannot capture a snapshot envelope from an invalidated session"),
            Self::Session { unit, source } => write!(formatter, "semantic unit {unit} snapshot failed: {source}"),
            Self::EmptyUnitSet => formatter.write_str("semantic snapshot envelope cannot contain zero units"),
            Self::MissingUnit(unit) => write!(formatter, "semantic snapshot envelope is missing unit {unit}"),
            Self::UnexpectedUnit(unit) => write!(formatter, "semantic snapshot verification received unexpected unit {unit}"),
            Self::ProfileChanged { unit, .. } => write!(formatter, "semantic snapshot profile changed for unit {unit}"),
            Self::UegChanged { unit, .. } => write!(formatter, "semantic snapshot UEG changed for unit {unit}"),
        }
    }
}

impl std::error::Error for SemanticSnapshotEnvelopeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticUnitSnapshot {
    snapshot: crate::semantic_snapshot::SemanticValidationSnapshot,
}

impl SemanticUnitSnapshot {
    pub fn profile_key(&self) -> SemanticCacheKey {
        self.snapshot.fingerprint().profile_key()
    }

    pub fn root_key(&self) -> SemanticCacheKey {
        self.snapshot.fingerprint().root_key()
    }

    pub fn semantic_snapshot(&self) -> &crate::semantic_snapshot::SemanticValidationSnapshot {
        &self.snapshot
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSnapshotEnvelope {
    batch_id: u64,
    profile_key: SemanticCacheKey,
    units: BTreeMap<SemanticUnitId, SemanticUnitSnapshot>,
}

impl SemanticSnapshotEnvelope {
    pub fn capture(
        session: &SemanticBatchSession,
        batch_id: u64,
    ) -> Result<Self, SemanticSnapshotEnvelopeError> {
        if batch_id == 0 {
            return Err(SemanticSnapshotEnvelopeError::InvalidBatchId);
        }
        if !session.is_valid() {
            return Err(SemanticSnapshotEnvelopeError::SessionInvalidated);
        }
        if batch_id.saturating_add(1) != session.next_batch_id() {
            return Err(SemanticSnapshotEnvelopeError::BatchNotApplied {
                batch_id,
                next_batch_id: session.next_batch_id(),
            });
        }
        let mut units = BTreeMap::new();
        for unit in session.unit_ids() {
            let snapshot = session.snapshot_for(unit).map_err(|source| {
                SemanticSnapshotEnvelopeError::Session {
                    unit: unit.clone(),
                    source,
                }
            })?;
            units.insert(
                unit.clone(),
                SemanticUnitSnapshot {
                    snapshot: snapshot.clone(),
                },
            );
        }
        if units.is_empty() {
            return Err(SemanticSnapshotEnvelopeError::EmptyUnitSet);
        }
        Ok(Self {
            batch_id,
            profile_key: session.profile_key(),
            units,
        })
    }

    pub fn batch_id(&self) -> u64 {
        self.batch_id
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn units(&self) -> &BTreeMap<SemanticUnitId, SemanticUnitSnapshot> {
        &self.units
    }

    pub fn verify_for(
        &self,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        candidates: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), SemanticSnapshotEnvelopeError> {
        self.verify_for_with_fingerprint(batch_id, profile, candidates, |candidate, profile| {
            SemanticFingerprint::from_ueg(candidate, profile)
        })
    }

    pub(crate) fn verify_for_instrumented(
        &self,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        candidates: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<(), SemanticSnapshotEnvelopeError> {
        self.verify_for_with_fingerprint(batch_id, profile, candidates, |candidate, profile| {
            recorder.time(DiagnosticStage::SnapshotFingerprint, || {
                SemanticFingerprint::from_ueg(candidate, profile)
            })
        })
    }

    fn verify_for_with_fingerprint<F>(
        &self,
        batch_id: u64,
        profile: &TargetCapabilityProfile,
        candidates: &BTreeMap<SemanticUnitId, Ueg>,
        mut fingerprint: F,
    ) -> Result<(), SemanticSnapshotEnvelopeError>
    where
        F: FnMut(&Ueg, &TargetCapabilityProfile) -> SemanticFingerprint,
    {
        if batch_id != self.batch_id {
            return Err(SemanticSnapshotEnvelopeError::BatchIdMismatch {
                expected: self.batch_id,
                actual: batch_id,
            });
        }
        if candidates.is_empty() {
            return Err(SemanticSnapshotEnvelopeError::EmptyUnitSet);
        }
        let expected_units = self.units.keys().cloned().collect::<BTreeSet<_>>();
        let actual_units = candidates.keys().cloned().collect::<BTreeSet<_>>();
        if let Some(unit) = expected_units.difference(&actual_units).next() {
            return Err(SemanticSnapshotEnvelopeError::MissingUnit(unit.clone()));
        }
        if let Some(unit) = actual_units.difference(&expected_units).next() {
            return Err(SemanticSnapshotEnvelopeError::UnexpectedUnit(unit.clone()));
        }
        for (unit, candidate) in candidates {
            let current = fingerprint(candidate, profile);
            if current.profile_key() != self.profile_key {
                return Err(SemanticSnapshotEnvelopeError::ProfileChanged {
                    unit: unit.clone(),
                    expected: self.profile_key,
                    actual: current.profile_key(),
                });
            }
            let expected = self
                .units
                .get(unit)
                .ok_or_else(|| SemanticSnapshotEnvelopeError::UnexpectedUnit(unit.clone()))?;
            if current.profile_key() != expected.profile_key() {
                return Err(SemanticSnapshotEnvelopeError::ProfileChanged {
                    unit: unit.clone(),
                    expected: expected.profile_key(),
                    actual: current.profile_key(),
                });
            }
            if current.root_key() != expected.root_key() {
                return Err(SemanticSnapshotEnvelopeError::UegChanged {
                    unit: unit.clone(),
                    expected: expected.root_key(),
                    actual: current.root_key(),
                });
            }
        }
        Ok(())
    }
}
