use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

use crate::semantic::TargetCapabilityProfile;
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::semantic_session::{
    DependencyAwareRefresh, DependencyAwareSemanticSession, SemanticEditManifest,
    SemanticEditManifestError, SemanticEditRange, SemanticSessionError,
};
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticUnitId(String);

impl SemanticUnitId {
    pub fn new(value: impl Into<String>) -> Result<Self, SemanticBatchError> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(SemanticBatchError::InvalidUnitId(value));
        }
        if value.starts_with('/')
            || value.contains('\\')
            || value
                .split('/')
                .any(|segment| segment == ".." || segment.is_empty())
        {
            return Err(SemanticBatchError::InvalidUnitId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SemanticUnitId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticBatchError {
    EmptyBatch,
    DuplicateUnit(SemanticUnitId),
    UnknownUnit(SemanticUnitId),
    InvalidUnitId(String),
    ProfileChanged,
    BatchProfileMismatch,
    InvalidBatchId,
    BatchSequenceMismatch {
        expected: u64,
        actual: u64,
    },
    Unit {
        unit: SemanticUnitId,
        source: SemanticSessionError,
    },
    Manifest(SemanticEditManifestError),
    Invalidated,
}

impl Display for SemanticBatchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBatch => formatter.write_str("semantic edit batch cannot be empty"),
            Self::DuplicateUnit(unit) => {
                write!(formatter, "semantic edit batch repeats unit {unit}")
            }
            Self::UnknownUnit(unit) => write!(
                formatter,
                "semantic edit batch references unknown unit {unit}"
            ),
            Self::InvalidUnitId(value) => {
                write!(formatter, "semantic unit identity is invalid: {value:?}")
            }
            Self::ProfileChanged => formatter.write_str("semantic batch profile cannot change"),
            Self::BatchProfileMismatch => {
                formatter.write_str("semantic batch envelope profile key does not match session")
            }
            Self::InvalidBatchId => {
                formatter.write_str("semantic batch envelope id must be non-zero")
            }
            Self::BatchSequenceMismatch { expected, actual } => write!(
                formatter,
                "semantic batch envelope sequence mismatch: expected {expected}, received {actual}"
            ),
            Self::Unit { unit, source } => {
                write!(formatter, "semantic unit {unit} failed: {source}")
            }
            Self::Manifest(error) => Display::fmt(error, formatter),
            Self::Invalidated => formatter.write_str("semantic batch session is invalidated"),
        }
    }
}

impl std::error::Error for SemanticBatchError {}

#[derive(Clone)]
pub struct SemanticUnitStart {
    pub unit: SemanticUnitId,
    pub ueg: Ueg,
    pub capacity: usize,
}

#[derive(Clone)]
pub struct SemanticEditUpdate {
    pub unit: SemanticUnitId,
    pub ueg: Ueg,
    pub manifest: SemanticEditManifest,
}

#[derive(Clone)]
pub struct SemanticEditBatch {
    updates: Vec<SemanticEditUpdate>,
}

#[derive(Clone)]
pub struct SemanticBatchEnvelope {
    batch_id: u64,
    profile_key: SemanticCacheKey,
    batch: SemanticEditBatch,
}

impl SemanticBatchEnvelope {
    pub fn new(
        batch_id: u64,
        profile_key: SemanticCacheKey,
        batch: SemanticEditBatch,
    ) -> Result<Self, SemanticBatchError> {
        if batch_id == 0 {
            return Err(SemanticBatchError::InvalidBatchId);
        }
        Ok(Self {
            batch_id,
            profile_key,
            batch,
        })
    }

    pub fn batch_id(&self) -> u64 {
        self.batch_id
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn batch(&self) -> &SemanticEditBatch {
        &self.batch
    }
}

impl SemanticEditBatch {
    pub fn new(mut updates: Vec<SemanticEditUpdate>) -> Result<Self, SemanticBatchError> {
        if updates.is_empty() {
            return Err(SemanticBatchError::EmptyBatch);
        }
        updates.sort_by(|left, right| left.unit.cmp(&right.unit));
        for window in updates.windows(2) {
            if window[0].unit == window[1].unit {
                return Err(SemanticBatchError::DuplicateUnit(window[0].unit.clone()));
            }
        }
        Ok(Self { updates })
    }

    pub fn updates(&self) -> &[SemanticEditUpdate] {
        &self.updates
    }
}

#[derive(Debug, Clone)]
pub struct SemanticBatchRefresh {
    pub refreshed: BTreeMap<SemanticUnitId, DependencyAwareRefresh>,
}

pub struct SemanticBatchSession {
    profile: TargetCapabilityProfile,
    profile_key: SemanticCacheKey,
    next_batch_id: u64,
    sessions: BTreeMap<SemanticUnitId, DependencyAwareSemanticSession>,
}

impl SemanticBatchSession {
    pub fn start(
        profile: TargetCapabilityProfile,
        units: Vec<SemanticUnitStart>,
    ) -> Result<Self, SemanticBatchError> {
        if units.is_empty() {
            return Err(SemanticBatchError::EmptyBatch);
        }
        let profile_key = SemanticFingerprint::from_ueg(&units[0].ueg, &profile).profile_key();
        let mut sessions = BTreeMap::new();
        for unit in units {
            if sessions.contains_key(&unit.unit) {
                return Err(SemanticBatchError::DuplicateUnit(unit.unit));
            }
            let session =
                DependencyAwareSemanticSession::start(&unit.ueg, profile.clone(), unit.capacity)
                    .map_err(|source| SemanticBatchError::Unit {
                        unit: unit.unit.clone(),
                        source,
                    })?;
            sessions.insert(unit.unit, session);
        }
        Ok(Self {
            profile,
            profile_key,
            next_batch_id: 1,
            sessions,
        })
    }

    pub fn profile(&self) -> &TargetCapabilityProfile {
        &self.profile
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn next_batch_id(&self) -> u64 {
        self.next_batch_id
    }

    pub fn unit_ids(&self) -> impl Iterator<Item = &SemanticUnitId> {
        self.sessions.keys()
    }

    pub fn is_valid(&self) -> bool {
        !self.sessions.is_empty()
            && self
                .sessions
                .values()
                .all(DependencyAwareSemanticSession::is_valid)
    }

    pub fn manifest_for(
        &self,
        unit: &SemanticUnitId,
        ranges: Vec<SemanticEditRange>,
    ) -> Result<SemanticEditManifest, SemanticBatchError> {
        let session = self
            .sessions
            .get(unit)
            .ok_or_else(|| SemanticBatchError::UnknownUnit(unit.clone()))?;
        session
            .manifest_for_edits(ranges)
            .map_err(|error| match error {
                SemanticSessionError::EditManifest(error) => SemanticBatchError::Manifest(error),
                other => SemanticBatchError::Unit {
                    unit: unit.clone(),
                    source: other,
                },
            })
    }

    pub fn refresh_batch(
        &mut self,
        batch: &SemanticEditBatch,
        profile: &TargetCapabilityProfile,
    ) -> Result<SemanticBatchRefresh, SemanticBatchError> {
        if profile != &self.profile {
            self.invalidate();
            return Err(SemanticBatchError::ProfileChanged);
        }
        let mut staged = self
            .sessions
            .iter()
            .map(|(unit, session)| (unit.clone(), session.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut refreshed = BTreeMap::new();
        for update in batch.updates() {
            let session = match staged.get_mut(&update.unit) {
                Some(session) => session,
                None => {
                    self.invalidate();
                    return Err(SemanticBatchError::UnknownUnit(update.unit.clone()));
                }
            };
            match session.refresh_from_edit_manifest(&update.ueg, profile, &update.manifest) {
                Ok(result) => {
                    refreshed.insert(update.unit.clone(), result);
                }
                Err(source) => {
                    self.invalidate();
                    return Err(SemanticBatchError::Unit {
                        unit: update.unit.clone(),
                        source,
                    });
                }
            }
        }
        self.sessions = staged;
        Ok(SemanticBatchRefresh { refreshed })
    }

    pub fn refresh_envelope(
        &mut self,
        envelope: &SemanticBatchEnvelope,
        profile: &TargetCapabilityProfile,
    ) -> Result<SemanticBatchRefresh, SemanticBatchError> {
        if envelope.profile_key != self.profile_key {
            self.invalidate();
            return Err(SemanticBatchError::BatchProfileMismatch);
        }
        if envelope.batch_id != self.next_batch_id {
            self.invalidate();
            return Err(SemanticBatchError::BatchSequenceMismatch {
                expected: self.next_batch_id,
                actual: envelope.batch_id,
            });
        }
        let result = self.refresh_batch(envelope.batch(), profile)?;
        self.next_batch_id = self.next_batch_id.saturating_add(1);
        Ok(result)
    }

    pub fn snapshot_for(
        &self,
        unit: &SemanticUnitId,
    ) -> Result<&crate::semantic_snapshot::SemanticValidationSnapshot, SemanticBatchError> {
        self.sessions
            .get(unit)
            .ok_or_else(|| SemanticBatchError::UnknownUnit(unit.clone()))?
            .snapshot()
            .ok_or(SemanticBatchError::Invalidated)
    }

    pub fn invalidate(&mut self) {
        for session in self.sessions.values_mut() {
            session.invalidate();
        }
    }
}
