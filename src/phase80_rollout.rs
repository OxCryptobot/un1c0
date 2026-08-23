use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PHASE80_ROLLOUT_SCHEMA_VERSION: u8 = 1;
pub const MAX_ROLLOUT_MANIFEST_BYTES: usize = 128 * 1024;
pub const MAX_ROLLOUT_GATE_COUNT: usize = 32;
pub const MAX_ROLLOUT_ID_BYTES: usize = 256;
const ROLLOUT_APPROVAL_DOMAIN: &[u8] = b"un1c0/phase80/rollout-approval/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase80RolloutError {
    InvalidIdentifier(&'static str),
    InvalidDigest(&'static str),
    InvalidSchema,
    TooManyGates,
    DuplicateGate(String),
    FailedGate(String),
    ManifestTooLarge,
    Serialization(String),
    ManifestMismatch,
    ReportMismatch(&'static str),
    MutationDetected,
    ApprovalRequired,
    ApprovalMismatch(&'static str),
    ApprovalSignerMismatch,
    ApprovalGenerationMismatch,
    InvalidSignature,
}

impl std::fmt::Display for Phase80RolloutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier(label) => write!(formatter, "invalid rollout {label}"),
            Self::InvalidDigest(label) => write!(formatter, "rollout {label} digest is empty"),
            Self::InvalidSchema => formatter.write_str("unsupported Phase 80 rollout schema"),
            Self::TooManyGates => formatter.write_str("rollout has too many gates"),
            Self::DuplicateGate(gate) => write!(formatter, "rollout gate is duplicated: {gate}"),
            Self::FailedGate(gate) => write!(formatter, "rollout gate failed: {gate}"),
            Self::ManifestTooLarge => formatter.write_str("rollout manifest exceeds its bound"),
            Self::Serialization(message) => {
                write!(formatter, "rollout serialization failed: {message}")
            }
            Self::ManifestMismatch => {
                formatter.write_str("rollout manifest does not match dry-run evidence")
            }
            Self::ReportMismatch(label) => {
                write!(formatter, "rollout dry-run report mismatch: {label}")
            }
            Self::MutationDetected => {
                formatter.write_str("staging dry-run reported an unauthorized mutation")
            }
            Self::ApprovalRequired => {
                formatter.write_str("rollout requires an independent approval")
            }
            Self::ApprovalMismatch(label) => {
                write!(formatter, "rollout approval mismatch: {label}")
            }
            Self::ApprovalSignerMismatch => {
                formatter.write_str("rollout approval signer is not authorized")
            }
            Self::ApprovalGenerationMismatch => {
                formatter.write_str("rollout approval signer generation is stale")
            }
            Self::InvalidSignature => formatter.write_str("rollout approval signature is invalid"),
        }
    }
}

impl std::error::Error for Phase80RolloutError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolloutGate {
    pub id: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolloutManifest {
    pub schema_version: u8,
    pub release_id: String,
    pub artifact_digest: [u8; 32],
    pub configuration_digest: [u8; 32],
    pub expected_commit: String,
    pub gates: Vec<RolloutGate>,
}

impl RolloutManifest {
    pub fn new(
        release_id: &str,
        artifact_digest: [u8; 32],
        configuration_digest: [u8; 32],
        expected_commit: &str,
        gates: Vec<RolloutGate>,
    ) -> Result<Self, Phase80RolloutError> {
        let manifest = Self {
            schema_version: PHASE80_ROLLOUT_SCHEMA_VERSION,
            release_id: release_id.to_string(),
            artifact_digest,
            configuration_digest,
            expected_commit: expected_commit.to_string(),
            gates,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), Phase80RolloutError> {
        if self.schema_version != PHASE80_ROLLOUT_SCHEMA_VERSION {
            return Err(Phase80RolloutError::InvalidSchema);
        }
        validate_identifier(&self.release_id, "release id")?;
        validate_identifier(&self.expected_commit, "expected commit")?;
        if self.artifact_digest == [0; 32] {
            return Err(Phase80RolloutError::InvalidDigest("artifact"));
        }
        if self.configuration_digest == [0; 32] {
            return Err(Phase80RolloutError::InvalidDigest("configuration"));
        }
        if self.gates.is_empty() || self.gates.len() > MAX_ROLLOUT_GATE_COUNT {
            return Err(Phase80RolloutError::TooManyGates);
        }
        let mut seen = std::collections::BTreeSet::new();
        for gate in &self.gates {
            validate_identifier(&gate.id, "gate id")?;
            if !seen.insert(&gate.id) {
                return Err(Phase80RolloutError::DuplicateGate(gate.id.clone()));
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| Phase80RolloutError::Serialization(error.to_string()))?;
        if bytes.len() > MAX_ROLLOUT_MANIFEST_BYTES {
            return Err(Phase80RolloutError::ManifestTooLarge);
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<[u8; 32], Phase80RolloutError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| Phase80RolloutError::Serialization(error.to_string()))?;
        Ok(hash_bytes(&bytes))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StagingDryRunReport {
    pub schema_version: u8,
    pub release_id: String,
    pub manifest_digest: [u8; 32],
    pub evaluated_gate_ids: Vec<String>,
    pub passed: bool,
    pub mutation_count: u64,
    pub external_mutation: bool,
}

impl StagingDryRunReport {
    pub fn execute(manifest: &RolloutManifest) -> Result<Self, Phase80RolloutError> {
        manifest.validate()?;
        let mut evaluated_gate_ids = Vec::with_capacity(manifest.gates.len());
        let mut passed = true;
        for gate in &manifest.gates {
            evaluated_gate_ids.push(gate.id.clone());
            if !gate.passed {
                passed = false;
            }
        }
        Ok(Self {
            schema_version: PHASE80_ROLLOUT_SCHEMA_VERSION,
            release_id: manifest.release_id.clone(),
            manifest_digest: manifest.digest()?,
            evaluated_gate_ids,
            passed,
            mutation_count: 0,
            external_mutation: false,
        })
    }

    pub fn validate_against(&self, manifest: &RolloutManifest) -> Result<(), Phase80RolloutError> {
        manifest.validate()?;
        if self.schema_version != PHASE80_ROLLOUT_SCHEMA_VERSION {
            return Err(Phase80RolloutError::InvalidSchema);
        }
        if self.release_id != manifest.release_id {
            return Err(Phase80RolloutError::ReportMismatch("release id"));
        }
        if self.manifest_digest != manifest.digest()? {
            return Err(Phase80RolloutError::ManifestMismatch);
        }
        if self.evaluated_gate_ids
            != manifest
                .gates
                .iter()
                .map(|gate| gate.id.clone())
                .collect::<Vec<_>>()
        {
            return Err(Phase80RolloutError::ReportMismatch("gate order"));
        }
        if self.mutation_count != 0 || self.external_mutation {
            return Err(Phase80RolloutError::MutationDetected);
        }
        let expected_passed = manifest.gates.iter().all(|gate| gate.passed);
        if self.passed != expected_passed {
            return Err(Phase80RolloutError::ReportMismatch("pass result"));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<[u8; 32], Phase80RolloutError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| Phase80RolloutError::Serialization(error.to_string()))?;
        Ok(hash_bytes(&bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutApprovalPolicy {
    pub approver_id: String,
    pub generation: u64,
    pub public_key: [u8; 32],
}

impl RolloutApprovalPolicy {
    pub fn new(
        approver_id: &str,
        generation: u64,
        public_key: [u8; 32],
    ) -> Result<Self, Phase80RolloutError> {
        validate_identifier(approver_id, "approver id")?;
        if generation == 0 {
            return Err(Phase80RolloutError::ApprovalGenerationMismatch);
        }
        Ok(Self {
            approver_id: approver_id.to_string(),
            generation,
            public_key,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolloutApproval {
    pub schema_version: u8,
    pub release_id: String,
    pub manifest_digest: [u8; 32],
    pub dry_run_digest: [u8; 32],
    pub approver_id: String,
    pub approver_generation: u64,
    pub signature: Vec<u8>,
}

impl RolloutApproval {
    pub fn signing_payload(&self) -> Result<Vec<u8>, Phase80RolloutError> {
        serde_json::to_vec(&(
            ROLLOUT_APPROVAL_DOMAIN,
            self.schema_version,
            &self.release_id,
            self.manifest_digest,
            self.dry_run_digest,
            &self.approver_id,
            self.approver_generation,
        ))
        .map_err(|error| Phase80RolloutError::Serialization(error.to_string()))
    }

    pub fn verify(
        &self,
        manifest: &RolloutManifest,
        report: &StagingDryRunReport,
        policy: &RolloutApprovalPolicy,
    ) -> Result<(), Phase80RolloutError> {
        report.validate_against(manifest)?;
        if !report.passed {
            return Err(Phase80RolloutError::FailedGate("staging dry-run".into()));
        }
        if self.schema_version != PHASE80_ROLLOUT_SCHEMA_VERSION {
            return Err(Phase80RolloutError::InvalidSchema);
        }
        if self.release_id != manifest.release_id {
            return Err(Phase80RolloutError::ApprovalMismatch("release id"));
        }
        if self.manifest_digest != manifest.digest()? {
            return Err(Phase80RolloutError::ApprovalMismatch("manifest digest"));
        }
        if self.dry_run_digest != report.digest()? {
            return Err(Phase80RolloutError::ApprovalMismatch("dry-run digest"));
        }
        if self.approver_id != policy.approver_id {
            return Err(Phase80RolloutError::ApprovalSignerMismatch);
        }
        if self.approver_generation != policy.generation {
            return Err(Phase80RolloutError::ApprovalGenerationMismatch);
        }
        let public_key = VerifyingKey::from_bytes(&policy.public_key)
            .map_err(|_| Phase80RolloutError::InvalidSignature)?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| Phase80RolloutError::InvalidSignature)?;
        public_key
            .verify(&self.signing_payload()?, &Signature::from_bytes(&signature))
            .map_err(|_| Phase80RolloutError::InvalidSignature)
    }
}

#[derive(Debug, Clone)]
pub struct RolloutApprovalAuthority {
    policy: RolloutApprovalPolicy,
    signing_key: SigningKey,
}

impl RolloutApprovalAuthority {
    pub fn new(
        policy: RolloutApprovalPolicy,
        signing_key: SigningKey,
    ) -> Result<Self, Phase80RolloutError> {
        if signing_key.verifying_key().to_bytes() != policy.public_key {
            return Err(Phase80RolloutError::ApprovalSignerMismatch);
        }
        Ok(Self {
            policy,
            signing_key,
        })
    }

    pub fn policy(&self) -> &RolloutApprovalPolicy {
        &self.policy
    }

    pub fn issue(
        &self,
        manifest: &RolloutManifest,
        report: &StagingDryRunReport,
    ) -> Result<RolloutApproval, Phase80RolloutError> {
        report.validate_against(manifest)?;
        if !report.passed {
            return Err(Phase80RolloutError::FailedGate("staging dry-run".into()));
        }
        let mut approval = RolloutApproval {
            schema_version: PHASE80_ROLLOUT_SCHEMA_VERSION,
            release_id: manifest.release_id.clone(),
            manifest_digest: manifest.digest()?,
            dry_run_digest: report.digest()?,
            approver_id: self.policy.approver_id.clone(),
            approver_generation: self.policy.generation,
            signature: vec![0; 64],
        };
        approval.signature = self
            .signing_key
            .sign(&approval.signing_payload()?)
            .to_bytes()
            .to_vec();
        Ok(approval)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedRollout {
    pub release_id: String,
    pub manifest_digest: [u8; 32],
    pub dry_run_digest: [u8; 32],
    pub approver_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct Phase80RolloutGate;

impl Phase80RolloutGate {
    pub fn dry_run(
        &self,
        manifest: &RolloutManifest,
    ) -> Result<StagingDryRunReport, Phase80RolloutError> {
        StagingDryRunReport::execute(manifest)
    }

    pub fn authorize(
        &self,
        manifest: &RolloutManifest,
        report: &StagingDryRunReport,
        approval: Option<&RolloutApproval>,
        policy: &RolloutApprovalPolicy,
    ) -> Result<AuthorizedRollout, Phase80RolloutError> {
        let approval = approval.ok_or(Phase80RolloutError::ApprovalRequired)?;
        approval.verify(manifest, report, policy)?;
        Ok(AuthorizedRollout {
            release_id: manifest.release_id.clone(),
            manifest_digest: manifest.digest()?,
            dry_run_digest: report.digest()?,
            approver_id: approval.approver_id.clone(),
        })
    }
}

fn validate_identifier(value: &str, label: &'static str) -> Result<(), Phase80RolloutError> {
    if value.is_empty()
        || value.len() > MAX_ROLLOUT_ID_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || character == '/')
    {
        return Err(Phase80RolloutError::InvalidIdentifier(label));
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
