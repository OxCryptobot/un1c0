use std::fmt::{Display, Formatter};

use crate::codegen::TargetBinding;
use crate::semantic::{
    validate_ueg_with_profile, SemanticValidationReport, TargetCapabilityProfile,
};
use crate::semantic_cache::{SemanticCacheKey, SemanticFingerprint};
use crate::walker::Ueg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticSnapshotError {
    ValidationFailed {
        report: SemanticValidationReport,
    },
    UegChanged {
        expected: SemanticCacheKey,
        actual: SemanticCacheKey,
    },
    ProfileChanged {
        expected: SemanticCacheKey,
        actual: SemanticCacheKey,
    },
    StoredReportInvalid,
}

impl Display for SemanticSnapshotError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ValidationFailed { report } => write!(
                formatter,
                "cannot snapshot {} semantic errors for {} target",
                report.error_count(),
                report.target.label()
            ),
            Self::UegChanged { .. } => {
                formatter.write_str("semantic snapshot does not match UEG fingerprint")
            }
            Self::ProfileChanged { .. } => {
                formatter.write_str("semantic snapshot does not match target profile")
            }
            Self::StoredReportInvalid => {
                formatter.write_str("semantic snapshot contains an invalid report")
            }
        }
    }
}

impl std::error::Error for SemanticSnapshotError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticValidationSnapshot {
    target: TargetBinding,
    fingerprint: SemanticFingerprint,
    report: SemanticValidationReport,
}

impl SemanticValidationSnapshot {
    pub fn capture(
        ueg: &Ueg,
        profile: TargetCapabilityProfile,
    ) -> Result<Self, SemanticSnapshotError> {
        let report = validate_ueg_with_profile(ueg, profile.clone());
        if !report.is_valid() {
            return Err(SemanticSnapshotError::ValidationFailed { report });
        }
        Ok(Self {
            target: profile.target,
            fingerprint: SemanticFingerprint::from_ueg(ueg, &profile),
            report,
        })
    }

    pub(crate) fn from_validated_report(
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
        report: SemanticValidationReport,
    ) -> Result<Self, SemanticSnapshotError> {
        if !report.is_valid() {
            return Err(SemanticSnapshotError::ValidationFailed { report });
        }
        Ok(Self {
            target: profile.target,
            fingerprint: SemanticFingerprint::from_ueg(ueg, profile),
            report,
        })
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn report(&self) -> &SemanticValidationReport {
        &self.report
    }

    pub fn fingerprint(&self) -> &SemanticFingerprint {
        &self.fingerprint
    }

    pub fn verify_for(
        &self,
        ueg: &Ueg,
        profile: &TargetCapabilityProfile,
    ) -> Result<(), SemanticSnapshotError> {
        if !self.report.is_valid() {
            return Err(SemanticSnapshotError::StoredReportInvalid);
        }
        let current = SemanticFingerprint::from_ueg(ueg, profile);
        if current.profile_key() != self.fingerprint.profile_key() {
            return Err(SemanticSnapshotError::ProfileChanged {
                expected: self.fingerprint.profile_key(),
                actual: current.profile_key(),
            });
        }
        if current.root_key() != self.fingerprint.root_key() {
            return Err(SemanticSnapshotError::UegChanged {
                expected: self.fingerprint.root_key(),
                actual: current.root_key(),
            });
        }
        if profile.target != self.target {
            return Err(SemanticSnapshotError::ProfileChanged {
                expected: self.fingerprint.profile_key(),
                actual: current.profile_key(),
            });
        }
        Ok(())
    }
}
