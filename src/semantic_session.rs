use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

use crate::codegen::TargetBinding;
use crate::incremental_semantic::{
    DependencyAwareSemanticValidator, DependencyGraph, IncrementalValidationError,
    IncrementalValidationReport,
};
use crate::semantic::{SemanticValidationReport, TargetCapabilityProfile};
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::semantic_snapshot::{SemanticSnapshotError, SemanticValidationSnapshot};
use crate::walker::{DiagnosticSeverity, SourceSpan, Ueg, UegDiagnostic};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticEditManifestError {
    InvalidRange {
        start_byte: usize,
        end_byte: usize,
    },
    OverlappingRanges {
        previous_end: usize,
        next_start: usize,
    },
}

impl Display for SemanticEditManifestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRange {
                start_byte,
                end_byte,
            } => {
                write!(
                    formatter,
                    "semantic edit range {start_byte}..{end_byte} is invalid"
                )
            }
            Self::OverlappingRanges {
                previous_end,
                next_start,
            } => write!(
                formatter,
                "semantic edit ranges overlap at {previous_end} and {next_start}"
            ),
        }
    }
}

impl std::error::Error for SemanticEditManifestError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEditRange {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl SemanticEditRange {
    pub fn new(start_byte: usize, end_byte: usize) -> Result<Self, SemanticEditManifestError> {
        if start_byte > end_byte {
            return Err(SemanticEditManifestError::InvalidRange {
                start_byte,
                end_byte,
            });
        }
        Ok(Self {
            start_byte,
            end_byte,
        })
    }

    fn overlaps(&self, span: &SourceSpan) -> bool {
        if self.start_byte == self.end_byte {
            self.start_byte >= span.start_byte && self.start_byte <= span.end_byte
        } else {
            self.start_byte < span.end_byte && self.end_byte > span.start_byte
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEditManifest {
    base_root: SemanticCacheKey,
    profile_key: SemanticCacheKey,
    ranges: Vec<SemanticEditRange>,
}

impl SemanticEditManifest {
    pub fn new(
        base_root: SemanticCacheKey,
        profile_key: SemanticCacheKey,
        mut ranges: Vec<SemanticEditRange>,
    ) -> Result<Self, SemanticEditManifestError> {
        ranges.sort_by_key(|range| (range.start_byte, range.end_byte));
        for window in ranges.windows(2) {
            if window[0].end_byte > window[1].start_byte {
                return Err(SemanticEditManifestError::OverlappingRanges {
                    previous_end: window[0].end_byte,
                    next_start: window[1].start_byte,
                });
            }
        }
        Ok(Self {
            base_root,
            profile_key,
            ranges,
        })
    }

    pub fn base_root(&self) -> SemanticCacheKey {
        self.base_root
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn ranges(&self) -> &[SemanticEditRange] {
        &self.ranges
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSessionError {
    Incremental(IncrementalValidationError),
    Snapshot(SemanticSnapshotError),
    EditManifest(SemanticEditManifestError),
    EditManifestProfileMismatch,
    EditManifestBaseMismatch,
    EditRangeUnmapped {
        start_byte: usize,
        end_byte: usize,
    },
    EditRangeAmbiguous {
        start_byte: usize,
        end_byte: usize,
        function_indexes: BTreeSet<usize>,
    },
    SemanticChangeOutsideManifest {
        mapped: BTreeSet<usize>,
        derived: BTreeSet<usize>,
    },
    ProfileChanged,
    TargetChanged {
        expected: TargetBinding,
        actual: TargetBinding,
    },
    StructuralChange,
    ChangedSetMismatch {
        declared: BTreeSet<usize>,
        derived: BTreeSet<usize>,
    },
    Invalidated,
}

impl Display for SemanticSessionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incremental(error) => Display::fmt(error, formatter),
            Self::Snapshot(error) => Display::fmt(error, formatter),
            Self::EditManifest(error) => Display::fmt(error, formatter),
            Self::EditManifestProfileMismatch => {
                formatter.write_str("semantic edit manifest profile does not match session")
            }
            Self::EditManifestBaseMismatch => {
                formatter.write_str("semantic edit manifest base root does not match session")
            }
            Self::EditRangeUnmapped {
                start_byte,
                end_byte,
            } => write!(
                formatter,
                "semantic edit range {start_byte}..{end_byte} does not map to one function"
            ),
            Self::EditRangeAmbiguous {
                start_byte,
                end_byte,
                function_indexes,
            } => write!(
                formatter,
                "semantic edit range {start_byte}..{end_byte} maps ambiguously to {function_indexes:?}"
            ),
            Self::SemanticChangeOutsideManifest { mapped, derived } => write!(
                formatter,
                "semantic changes {derived:?} are outside mapped edit functions {mapped:?}"
            ),
            Self::ProfileChanged => formatter.write_str("semantic session profile cannot change"),
            Self::TargetChanged { expected, actual } => write!(
                formatter,
                "semantic session target changed from {} to {}",
                expected.label(),
                actual.label()
            ),
            Self::StructuralChange => {
                formatter.write_str("semantic session invalidated by a UEG structural change")
            }
            Self::ChangedSetMismatch { declared, derived } => write!(
                formatter,
                "semantic change set mismatch: declared {declared:?}, derived {derived:?}"
            ),
            Self::Invalidated => formatter.write_str("semantic session has no valid snapshot"),
        }
    }
}

impl std::error::Error for SemanticSessionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticChangeSet {
    pub changed_functions: BTreeSet<usize>,
    pub unchanged_functions: BTreeSet<usize>,
    pub previous_function_count: usize,
    pub current_function_count: usize,
    pub previous_root: SemanticCacheKey,
    pub current_root: SemanticCacheKey,
}

impl SemanticChangeSet {
    pub fn is_noop(&self) -> bool {
        self.changed_functions.is_empty() && self.previous_root == self.current_root
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEditResolution {
    pub mapped_functions: BTreeSet<usize>,
    pub semantic_changes: SemanticChangeSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyAwareRefresh {
    pub snapshot: SemanticValidationSnapshot,
    pub validation: IncrementalValidationReport,
}

#[derive(Debug, Clone)]
pub struct DependencyAwareSemanticSession {
    profile: TargetCapabilityProfile,
    function_names: Vec<String>,
    validator: DependencyAwareSemanticValidator,
    current_fingerprint: Option<SemanticFingerprint>,
    snapshot: Option<SemanticValidationSnapshot>,
}

impl DependencyAwareSemanticSession {
    pub fn start(
        ueg: &Ueg,
        profile: TargetCapabilityProfile,
        capacity: usize,
    ) -> Result<Self, SemanticSessionError> {
        let graph = DependencyGraph::from_ueg(ueg)
            .map_err(IncrementalValidationError::Dependency)
            .map_err(SemanticSessionError::Incremental)?;
        let fingerprint = SemanticFingerprint::from_ueg(ueg, &profile);
        let mut validator = DependencyAwareSemanticValidator::new(capacity.max(1));
        let all_functions = (0..ueg.nodes.len()).collect::<BTreeSet<_>>();
        let validation = validator
            .validate(ueg, profile.clone(), &fingerprint, &all_functions)
            .map_err(SemanticSessionError::Incremental)?;
        let snapshot = SemanticValidationSnapshot::from_validated_report(
            ueg,
            &profile,
            validation.report.clone(),
        )
        .map_err(SemanticSessionError::Snapshot)?;
        Ok(Self {
            profile,
            function_names: graph.function_names().to_vec(),
            validator,
            current_fingerprint: Some(fingerprint),
            snapshot: Some(snapshot),
        })
    }

    pub fn profile(&self) -> &TargetCapabilityProfile {
        &self.profile
    }

    pub fn snapshot(&self) -> Option<&SemanticValidationSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn current_fingerprint(&self) -> Option<&SemanticFingerprint> {
        self.current_fingerprint.as_ref()
    }

    pub fn snapshot_for(
        &self,
        target: TargetBinding,
    ) -> Result<&SemanticValidationSnapshot, SemanticSessionError> {
        if target != self.profile.target {
            return Err(SemanticSessionError::TargetChanged {
                expected: self.profile.target,
                actual: target,
            });
        }
        self.snapshot
            .as_ref()
            .ok_or(SemanticSessionError::Invalidated)
    }

    pub fn manifest_for_edits(
        &self,
        ranges: Vec<SemanticEditRange>,
    ) -> Result<SemanticEditManifest, SemanticSessionError> {
        let fingerprint = self
            .current_fingerprint
            .as_ref()
            .ok_or(SemanticSessionError::Invalidated)?;
        SemanticEditManifest::new(fingerprint.root_key(), fingerprint.profile_key(), ranges)
            .map_err(SemanticSessionError::EditManifest)
    }

    pub fn derive_change_set(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> Result<SemanticChangeSet, SemanticSessionError> {
        let result = self.derive_change_set_inner(ueg, profile);
        if result.is_err() {
            self.invalidate();
        }
        result
    }

    pub fn derive_edit_resolution(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        manifest: &SemanticEditManifest,
    ) -> Result<SemanticEditResolution, SemanticSessionError> {
        let result = self.derive_edit_resolution_inner(ueg, profile, manifest);
        if result.is_err() {
            self.invalidate();
        }
        result
    }

    pub fn refresh_auto(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> Result<DependencyAwareRefresh, SemanticSessionError> {
        let changes = match self.derive_change_set(ueg, profile) {
            Ok(changes) => changes,
            Err(error) => return Err(error),
        };
        self.refresh(ueg, &changes.changed_functions, profile)
    }

    pub fn refresh_from_edit_manifest(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        manifest: &SemanticEditManifest,
    ) -> Result<DependencyAwareRefresh, SemanticSessionError> {
        let resolution = self.derive_edit_resolution(ueg, profile, manifest)?;
        self.refresh(ueg, &resolution.semantic_changes.changed_functions, profile)
    }

    pub fn refresh(
        &mut self,
        ueg: &Ueg,
        changed_functions: &BTreeSet<usize>,
        profile: &TargetCapabilityProfile,
    ) -> Result<DependencyAwareRefresh, SemanticSessionError> {
        self.check_profile(profile)?;
        let changes = match self.derive_change_set(ueg, profile) {
            Ok(changes) => changes,
            Err(error) => return Err(error),
        };
        if changes.changed_functions != *changed_functions {
            self.invalidate();
            return Err(SemanticSessionError::ChangedSetMismatch {
                declared: changed_functions.clone(),
                derived: changes.changed_functions,
            });
        }
        if changes.is_noop() {
            let snapshot = self
                .snapshot
                .clone()
                .ok_or(SemanticSessionError::Invalidated)?;
            let validation = IncrementalValidationReport {
                report: snapshot.report().clone(),
                changed_functions: BTreeSet::new(),
                affected_functions: BTreeSet::new(),
                revalidated_functions: BTreeSet::new(),
                cache_hits: 0,
                cache_misses: 0,
            };
            return Ok(DependencyAwareRefresh {
                snapshot,
                validation,
            });
        }

        let fingerprint = SemanticFingerprint::from_ueg(ueg, &self.profile);
        let validation = match self.validator.validate(
            ueg,
            self.profile.clone(),
            &fingerprint,
            changed_functions,
        ) {
            Ok(validation) => validation,
            Err(error) => {
                self.invalidate();
                return Err(SemanticSessionError::Incremental(error));
            }
        };
        let snapshot = match SemanticValidationSnapshot::from_validated_report(
            ueg,
            &self.profile,
            validation.report.clone(),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.invalidate();
                return Err(SemanticSessionError::Snapshot(error));
            }
        };
        self.current_fingerprint = Some(fingerprint);
        self.snapshot = Some(snapshot.clone());
        Ok(DependencyAwareRefresh {
            snapshot,
            validation,
        })
    }

    pub fn invalidate(&mut self) {
        self.current_fingerprint = None;
        self.snapshot = None;
    }

    pub fn is_valid(&self) -> bool {
        self.snapshot.is_some()
    }

    pub fn cache_metrics(&self) -> (usize, usize, u64, u64, u64) {
        self.validator.cache_metrics()
    }

    pub fn current_report(&self) -> Option<&SemanticValidationReport> {
        self.snapshot
            .as_ref()
            .map(SemanticValidationSnapshot::report)
    }

    fn derive_change_set_inner(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> Result<SemanticChangeSet, SemanticSessionError> {
        self.check_profile(profile)?;
        let blocking_diagnostics = ueg
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .cloned()
            .collect::<Vec<UegDiagnostic>>();
        if !blocking_diagnostics.is_empty() || ueg.nodes.is_empty() {
            return Err(SemanticSessionError::Incremental(
                IncrementalValidationError::InvalidUeg {
                    diagnostics: blocking_diagnostics,
                },
            ));
        }
        let graph = DependencyGraph::from_ueg(ueg).map_err(|error| {
            SemanticSessionError::Incremental(IncrementalValidationError::Dependency(error))
        })?;
        if graph.function_names() != self.function_names.as_slice() {
            return Err(SemanticSessionError::StructuralChange);
        }
        let previous = self
            .current_fingerprint
            .as_ref()
            .ok_or(SemanticSessionError::Invalidated)?;
        let current = SemanticFingerprint::from_ueg(ueg, profile);
        if previous.function_keys().len() != current.function_keys().len() {
            return Err(SemanticSessionError::StructuralChange);
        }
        let changed_functions = previous
            .function_keys()
            .iter()
            .zip(current.function_keys())
            .enumerate()
            .filter_map(|(index, (before, after))| (before != after).then_some(index))
            .collect::<BTreeSet<_>>();
        let unchanged_functions = (0..current.function_keys().len())
            .filter(|index| !changed_functions.contains(index))
            .collect::<BTreeSet<_>>();
        Ok(SemanticChangeSet {
            changed_functions,
            unchanged_functions,
            previous_function_count: previous.function_keys().len(),
            current_function_count: current.function_keys().len(),
            previous_root: previous.root_key(),
            current_root: current.root_key(),
        })
    }

    fn derive_edit_resolution_inner(
        &mut self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        manifest: &SemanticEditManifest,
    ) -> Result<SemanticEditResolution, SemanticSessionError> {
        let changes = self.derive_change_set_inner(ueg, profile)?;
        let current = self
            .current_fingerprint
            .as_ref()
            .ok_or(SemanticSessionError::Invalidated)?;
        if manifest.profile_key != current.profile_key() {
            return Err(SemanticSessionError::EditManifestProfileMismatch);
        }
        if manifest.base_root != current.root_key() {
            return Err(SemanticSessionError::EditManifestBaseMismatch);
        }

        let mut mapped_functions = BTreeSet::new();
        for edit in &manifest.ranges {
            let matches = ueg
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    let crate::walker::NodeKind::Lambda(lambda) = node;
                    edit.overlaps(&lambda.source_span).then_some(index)
                })
                .collect::<BTreeSet<_>>();
            match matches.len() {
                0 => {
                    return Err(SemanticSessionError::EditRangeUnmapped {
                        start_byte: edit.start_byte,
                        end_byte: edit.end_byte,
                    });
                }
                1 => mapped_functions.extend(matches),
                _ => {
                    return Err(SemanticSessionError::EditRangeAmbiguous {
                        start_byte: edit.start_byte,
                        end_byte: edit.end_byte,
                        function_indexes: matches,
                    });
                }
            }
        }
        if !changes.changed_functions.is_subset(&mapped_functions) {
            return Err(SemanticSessionError::SemanticChangeOutsideManifest {
                mapped: mapped_functions,
                derived: changes.changed_functions,
            });
        }
        Ok(SemanticEditResolution {
            mapped_functions,
            semantic_changes: changes,
        })
    }

    fn check_profile(
        &mut self,
        profile: &TargetCapabilityProfile,
    ) -> Result<(), SemanticSessionError> {
        if profile.target != self.profile.target {
            self.invalidate();
            return Err(SemanticSessionError::TargetChanged {
                expected: self.profile.target,
                actual: profile.target,
            });
        }
        if profile != &self.profile {
            self.invalidate();
            return Err(SemanticSessionError::ProfileChanged);
        }
        Ok(())
    }
}

pub type SemanticSession = DependencyAwareSemanticSession;
