//! Controlled evolution proposals.
//!
//! Evolution is treated as a release process, not as self-modifying execution:
//! proposals are content-hashed, Ed25519-signed, explicitly approved, canaried,
//! and either applied or rolled back with evidence. The ledger is fail-closed
//! and persists each transition atomically.

use crate::agentic::EvolutionProposal;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MAX_CANARY_CHECKS: usize = 64;
const MAX_CHANGED_FILES: usize = 2_000;
const MAX_EVIDENCE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHANGED_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum EvolutionError {
    #[error("invalid evolution proposal: {0}")]
    InvalidProposal(String),
    #[error("evolution signature verification failed")]
    InvalidSignature,
    #[error("evolution signer is not trusted: {0}")]
    UntrustedSigner(String),
    #[error("evolution state conflict: {0}")]
    StateConflict(String),
    #[error("evolution persistence error: {0}")]
    Persistence(String),
    #[error("evolution serialization error: {0}")]
    Serialization(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Draft,
    Approved {
        approved_by: String,
        approved_at_ms: u128,
    },
    Canary {
        run_id: String,
        started_at_ms: u128,
    },
    Applied {
        evidence: String,
        completed_at_ms: u128,
    },
    RolledBack {
        reason: String,
        completed_at_ms: u128,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvaluationCheck {
    pub name: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub duration_ms: u64,
}

impl EvaluationCheck {
    pub fn from_output(
        name: &str,
        passed: bool,
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        duration_ms: u64,
    ) -> Result<Self, EvolutionError> {
        validate_identifier(name, "evaluation check name")?;
        if stdout.len() > MAX_EVIDENCE_OUTPUT_BYTES || stderr.len() > MAX_EVIDENCE_OUTPUT_BYTES {
            return Err(EvolutionError::InvalidProposal(
                "evaluation output exceeds the 16 MiB evidence bound".into(),
            ));
        }
        if passed != (exit_code == Some(0)) {
            return Err(EvolutionError::InvalidProposal(
                "evaluation pass state must agree with a zero exit code".into(),
            ));
        }
        Ok(Self {
            name: name.to_string(),
            passed,
            exit_code,
            stdout_digest: hex_digest(stdout.as_bytes()),
            stderr_digest: hex_digest(stderr.as_bytes()),
            duration_ms,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CanaryReport {
    pub run_id: String,
    pub checks: Vec<EvaluationCheck>,
    pub changed_files: BTreeMap<String, String>,
}

impl CanaryReport {
    pub fn new(
        run_id: &str,
        checks: Vec<EvaluationCheck>,
        changed_files: BTreeMap<String, String>,
    ) -> Result<Self, EvolutionError> {
        validate_identifier(run_id, "canary run id")?;
        if checks.is_empty() || checks.len() > MAX_CANARY_CHECKS {
            return Err(EvolutionError::InvalidProposal(
                "canary report requires a run id of 1 to 256 bytes and 1 to 64 checks".into(),
            ));
        }
        if changed_files.len() > MAX_CHANGED_FILES {
            return Err(EvolutionError::InvalidProposal(
                "canary report cannot contain more than 2000 changed files".into(),
            ));
        }
        for check in &checks {
            validate_check(check)?;
        }
        for (path, digest) in &changed_files {
            if !valid_relative_path(path) {
                return Err(EvolutionError::InvalidProposal(format!(
                    "invalid changed-file path '{}'",
                    path
                )));
            }
            if !is_hex_digest(digest) {
                return Err(EvolutionError::InvalidProposal(format!(
                    "changed-file hash for '{}' is not a 64-character digest",
                    path
                )));
            }
        }
        Ok(Self {
            run_id: run_id.to_string(),
            checks,
            changed_files,
        })
    }

    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|check| check.passed)
    }

    pub fn from_workspace(
        root: impl AsRef<Path>,
        run_id: &str,
        checks: Vec<EvaluationCheck>,
        changed_paths: &[String],
    ) -> Result<Self, EvolutionError> {
        let root = fs::canonicalize(root.as_ref())
            .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
        if changed_paths.len() > MAX_CHANGED_FILES {
            return Err(EvolutionError::InvalidProposal(
                "canary report cannot contain more than 2000 changed files".into(),
            ));
        }
        let mut changed_files = BTreeMap::new();
        for path in changed_paths {
            if !valid_relative_path(path) {
                return Err(EvolutionError::InvalidProposal(format!(
                    "invalid changed-file path '{}'",
                    path
                )));
            }
            let candidate = root.join(path);
            let metadata = fs::symlink_metadata(&candidate)
                .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(EvolutionError::InvalidProposal(format!(
                    "changed-file path '{}' is not a regular file",
                    path
                )));
            }
            if metadata.len() > MAX_CHANGED_FILE_BYTES {
                return Err(EvolutionError::InvalidProposal(format!(
                    "changed file '{}' exceeds the 16 MiB evidence bound",
                    path
                )));
            }
            let canonical = fs::canonicalize(&candidate)
                .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
            if !canonical.starts_with(&root) {
                return Err(EvolutionError::InvalidProposal(format!(
                    "changed-file path '{}' escapes the workspace",
                    path
                )));
            }
            let content = fs::read(&canonical)
                .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
            changed_files.insert(path.clone(), hex_digest(&content));
        }
        Self::new(run_id, checks, changed_files)
    }

    pub fn evidence_digest(&self) -> Result<String, EvolutionError> {
        let payload = serde_json::to_vec(self)
            .map_err(|error| EvolutionError::Serialization(error.to_string()))?;
        Ok(hex_digest(&payload))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrustedSignerStore {
    keys: BTreeMap<String, [u8; 32]>,
}

impl TrustedSignerStore {
    pub fn trust_public_key(
        &mut self,
        signer_id: &str,
        public_key: &[u8],
    ) -> Result<(), EvolutionError> {
        validate_identifier(signer_id, "signer id")?;
        let key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| EvolutionError::InvalidSignature)?;
        if let Some(existing) = self.keys.get(signer_id) {
            if existing != &key {
                return Err(EvolutionError::StateConflict(format!(
                    "trusted signer '{}' cannot be rebound without explicit revocation",
                    signer_id
                )));
            }
        } else {
            self.keys.insert(signer_id.to_string(), key);
        }
        Ok(())
    }

    pub fn revoke(&mut self, signer_id: &str) -> bool {
        self.keys.remove(signer_id).is_some()
    }

    fn authorize(&self, signed: &SignedEvolutionProposal) -> Result<(), EvolutionError> {
        let trusted = self
            .keys
            .get(&signed.signer_id)
            .ok_or_else(|| EvolutionError::UntrustedSigner(signed.signer_id.clone()))?;
        if trusted.as_slice() != signed.public_key.as_slice() {
            return Err(EvolutionError::UntrustedSigner(format!(
                "public key mismatch for '{}'",
                signed.signer_id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedEvolutionProposal {
    pub proposal: EvolutionProposal,
    pub signer_id: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SignedEvolutionProposal {
    pub fn sign(
        proposal: EvolutionProposal,
        signer_id: &str,
        signing_key: &SigningKey,
    ) -> Result<Self, EvolutionError> {
        validate_identifier(signer_id, "signer id")?;
        if proposal.approved {
            return Err(EvolutionError::InvalidProposal(
                "proposal must be unapproved before signing".into(),
            ));
        }
        let public_key = signing_key.verifying_key().to_bytes();
        let payload = signing_payload(&proposal, signer_id, &public_key)?;
        let signature = signing_key.sign(&payload).to_bytes();
        Ok(Self {
            proposal,
            signer_id: signer_id.to_string(),
            public_key: public_key.to_vec(),
            signature: signature.to_vec(),
        })
    }

    pub fn verify(&self) -> Result<(), EvolutionError> {
        validate_identifier(&self.signer_id, "signer id")?;
        let expected_hash = proposal_content_hash(&self.proposal)?;
        if expected_hash != self.proposal.content_hash {
            return Err(EvolutionError::InvalidProposal(
                "proposal content hash does not match its immutable content".into(),
            ));
        }
        let expected_id = format!("evo-{}", &expected_hash[..12]);
        if expected_id != self.proposal.id {
            return Err(EvolutionError::InvalidProposal(
                "proposal id is not canonical for its content hash".into(),
            ));
        }
        let public_key: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| EvolutionError::InvalidSignature)?;
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| EvolutionError::InvalidSignature)?;
        let verifying_key =
            VerifyingKey::from_bytes(&public_key).map_err(|_| EvolutionError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let payload = signing_payload(&self.proposal, &self.signer_id, &public_key)?;
        verifying_key
            .verify(&payload, &signature)
            .map_err(|_| EvolutionError::InvalidSignature)
    }

    pub fn verify_with_trust(
        &self,
        trusted_signers: &TrustedSignerStore,
    ) -> Result<(), EvolutionError> {
        self.verify()?;
        trusted_signers.authorize(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub signed: SignedEvolutionProposal,
    pub state: ProposalState,
}

#[derive(Debug, Clone)]
pub struct EvolutionLedger {
    path: PathBuf,
    records: Arc<Mutex<BTreeMap<String, ProposalRecord>>>,
    trusted_signers: TrustedSignerStore,
}

impl EvolutionLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EvolutionError> {
        Self::open_with_trusted_signers(path, TrustedSignerStore::default())
    }

    pub fn open_with_trusted_signers(
        path: impl AsRef<Path>,
        trusted_signers: TrustedSignerStore,
    ) -> Result<Self, EvolutionError> {
        let path = path.as_ref().to_path_buf();
        let records: BTreeMap<String, ProposalRecord> = if path.exists() {
            let content = fs::read_to_string(&path)
                .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
            serde_json::from_str(&content)
                .map_err(|error| EvolutionError::Serialization(error.to_string()))?
        } else {
            BTreeMap::new()
        };
        for (id, record) in &records {
            if id != &record.signed.proposal.id {
                return Err(EvolutionError::InvalidProposal(
                    "ledger key does not match proposal id".into(),
                ));
            }
            record.signed.verify_with_trust(&trusted_signers)?;
            validate_record(record)?;
        }
        Ok(Self {
            path,
            records: Arc::new(Mutex::new(records)),
            trusted_signers,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn propose(&self, signed: SignedEvolutionProposal) -> Result<String, EvolutionError> {
        signed.verify_with_trust(&self.trusted_signers)?;
        let id = signed.proposal.id.clone();
        let mut records = self.lock_records()?;
        if records.contains_key(&id) {
            return Err(EvolutionError::StateConflict(format!(
                "proposal '{}' already exists",
                id
            )));
        }
        let record = ProposalRecord {
            signed,
            state: ProposalState::Draft,
        };
        validate_record(&record)?;
        records.insert(id.clone(), record);
        self.persist_locked(&records)?;
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Option<ProposalRecord>, EvolutionError> {
        let records = self.lock_records()?;
        Ok(records.get(id).cloned())
    }

    pub fn list(&self) -> Result<Vec<ProposalRecord>, EvolutionError> {
        let records = self.lock_records()?;
        Ok(records.values().cloned().collect())
    }

    pub fn approve(&self, id: &str, approved_by: &str) -> Result<(), EvolutionError> {
        if approved_by.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal(
                "approver id is empty".into(),
            ));
        }
        self.transition(id, |record| match record.state {
            ProposalState::Draft => {
                record.signed.verify()?;
                record.signed.proposal.approved = true;
                record.state = ProposalState::Approved {
                    approved_by: approved_by.to_string(),
                    approved_at_ms: now_ms(),
                };
                Ok(())
            }
            ref state => Err(EvolutionError::StateConflict(format!(
                "cannot approve proposal in state {:?}",
                state
            ))),
        })
    }

    pub fn start_canary(&self, id: &str, run_id: &str) -> Result<(), EvolutionError> {
        if run_id.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal(
                "canary run id is empty".into(),
            ));
        }
        self.transition(id, |record| match record.state {
            ProposalState::Approved { .. } => {
                record.state = ProposalState::Canary {
                    run_id: run_id.to_string(),
                    started_at_ms: now_ms(),
                };
                Ok(())
            }
            ref state => Err(EvolutionError::StateConflict(format!(
                "cannot start canary from state {:?}",
                state
            ))),
        })
    }

    pub fn finalize_canary_report(
        &self,
        id: &str,
        report: CanaryReport,
    ) -> Result<(), EvolutionError> {
        let evidence = report.evidence_digest()?;
        self.transition(id, |record| match record.state {
            ProposalState::Canary { ref run_id, .. } if run_id != &report.run_id => {
                Err(EvolutionError::StateConflict(format!(
                    "canary report run '{}' does not match active run '{}'",
                    report.run_id, run_id
                )))
            }
            ProposalState::Canary { .. } => {
                validate_report_against_proposal(&record.signed.proposal, &report)?;
                if report.passed() {
                    record.state = ProposalState::Applied {
                        evidence,
                        completed_at_ms: now_ms(),
                    };
                } else {
                    record.state = ProposalState::RolledBack {
                        reason: evidence,
                        completed_at_ms: now_ms(),
                    };
                }
                Ok(())
            }
            ref state => Err(EvolutionError::StateConflict(format!(
                "cannot finalize canary report from state {:?}",
                state
            ))),
        })
    }

    pub fn rollback(&self, id: &str, reason: &str) -> Result<(), EvolutionError> {
        if reason.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal(
                "rollback reason is empty".into(),
            ));
        }
        self.transition(id, |record| match record.state {
            ProposalState::Approved { .. }
            | ProposalState::Canary { .. }
            | ProposalState::Applied { .. } => {
                record.state = ProposalState::RolledBack {
                    reason: reason.to_string(),
                    completed_at_ms: now_ms(),
                };
                Ok(())
            }
            ref state => Err(EvolutionError::StateConflict(format!(
                "cannot rollback proposal in state {:?}",
                state
            ))),
        })
    }

    fn transition<F>(&self, id: &str, transition: F) -> Result<(), EvolutionError>
    where
        F: FnOnce(&mut ProposalRecord) -> Result<(), EvolutionError>,
    {
        let mut records = self.lock_records()?;
        let original = records.get(id).cloned().ok_or_else(|| {
            EvolutionError::StateConflict(format!("proposal '{}' does not exist", id))
        })?;
        let transition_result = (|| {
            let record = records.get_mut(id).ok_or_else(|| {
                EvolutionError::StateConflict(format!("proposal '{}' does not exist", id))
            })?;
            transition(record)?;
            record.signed.verify_with_trust(&self.trusted_signers)?;
            validate_record(record)
        })();
        if let Err(error) = transition_result {
            records.insert(id.to_string(), original);
            return Err(error);
        }
        match self.persist_locked(&records) {
            Ok(()) => Ok(()),
            Err(error) => {
                records.insert(id.to_string(), original);
                Err(error)
            }
        }
    }

    fn lock_records(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ProposalRecord>>, EvolutionError> {
        self.records
            .lock()
            .map_err(|_| EvolutionError::Persistence("ledger lock poisoned".into()))
    }

    fn persist_locked(
        &self,
        records: &BTreeMap<String, ProposalRecord>,
    ) -> Result<(), EvolutionError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
        }
        let content = serde_json::to_vec_pretty(records)
            .map_err(|error| EvolutionError::Serialization(error.to_string()))?;
        let stamp = now_ms();
        let temp = self.path.with_extension(format!("json.tmp-{}", stamp));
        fs::write(&temp, content)
            .map_err(|error| EvolutionError::Persistence(error.to_string()))?;
        fs::rename(&temp, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            EvolutionError::Persistence(error.to_string())
        })
    }
}

fn validate_record(record: &ProposalRecord) -> Result<(), EvolutionError> {
    record.signed.verify()?;
    validate_identifier(&record.signed.signer_id, "signer id")?;
    match record.state {
        ProposalState::Draft if record.signed.proposal.approved => Err(
            EvolutionError::InvalidProposal("draft proposal cannot be marked approved".into()),
        ),
        ProposalState::Draft => Ok(()),
        ProposalState::Approved { .. }
        | ProposalState::Canary { .. }
        | ProposalState::Applied { .. }
        | ProposalState::RolledBack { .. }
            if !record.signed.proposal.approved =>
        {
            Err(EvolutionError::InvalidProposal(
                "non-draft proposal must be marked approved".into(),
            ))
        }
        _ => Ok(()),
    }
}

fn validate_report_against_proposal(
    proposal: &EvolutionProposal,
    report: &CanaryReport,
) -> Result<(), EvolutionError> {
    let expected: BTreeSet<&str> = proposal.files.keys().map(String::as_str).collect();
    let actual: BTreeSet<&str> = report.changed_files.keys().map(String::as_str).collect();
    if expected != actual {
        return Err(EvolutionError::InvalidProposal(
            "canary changed-file set does not match the evolution proposal".into(),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<(), EvolutionError> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(EvolutionError::InvalidProposal(format!(
            "{} must be 1 to 256 bytes and contain no control characters",
            field
        )));
    }
    Ok(())
}

fn validate_check(check: &EvaluationCheck) -> Result<(), EvolutionError> {
    validate_identifier(&check.name, "evaluation check name")?;
    if check.passed != (check.exit_code == Some(0)) {
        return Err(EvolutionError::InvalidProposal(
            "evaluation pass state must agree with a zero exit code".into(),
        ));
    }
    if !is_hex_digest(&check.stdout_digest) || !is_hex_digest(&check.stderr_digest) {
        return Err(EvolutionError::InvalidProposal(
            "evaluation output digests must be 64-character hexadecimal values".into(),
        ));
    }
    Ok(())
}

fn valid_relative_path(path: &str) -> bool {
    if path.trim().is_empty() || path == "." {
        return false;
    }
    let path = Path::new(path);
    !path.is_absolute()
        && path.components().all(|component| {
            !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn proposal_content_hash(proposal: &EvolutionProposal) -> Result<String, EvolutionError> {
    let payload = serde_json::to_vec(&(
        &proposal.title,
        &proposal.files,
        &proposal.test_command,
        &proposal.risk,
    ))
    .map_err(|error| EvolutionError::Serialization(error.to_string()))?;
    Ok(hex_digest(&payload))
}

fn signing_payload(
    proposal: &EvolutionProposal,
    signer_id: &str,
    public_key: &[u8; 32],
) -> Result<Vec<u8>, EvolutionError> {
    serde_json::to_vec(&(
        &proposal.id,
        &proposal.title,
        &proposal.files,
        &proposal.test_command,
        &proposal.risk,
        &proposal.content_hash,
        signer_id,
        public_key,
    ))
    .map_err(|error| EvolutionError::Serialization(error.to_string()))
}

fn hex_digest(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    fn signed_proposal() -> SignedEvolutionProposal {
        let files = BTreeMap::from([(String::from("skills/new.md"), String::from("content"))]);
        let proposal = EvolutionProposal::new("new skill", files, "cargo test", "medium").unwrap();
        let key = SigningKey::from_bytes(&[7u8; 32]);
        SignedEvolutionProposal::sign(proposal, "operator:local", &key).unwrap()
    }

    fn trusted_signers() -> TrustedSignerStore {
        let signed = signed_proposal();
        let mut trusted = TrustedSignerStore::default();
        trusted
            .trust_public_key(&signed.signer_id, &signed.public_key)
            .unwrap();
        trusted
    }

    fn open_ledger(path: impl AsRef<Path>) -> EvolutionLedger {
        EvolutionLedger::open_with_trusted_signers(path, trusted_signers()).unwrap()
    }

    fn report_for_run(root: &Path, run_id: &str, check: EvaluationCheck) -> CanaryReport {
        let file = root.join("skills/new.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "content").unwrap();
        CanaryReport::from_workspace(root, run_id, vec![check], &[String::from("skills/new.md")])
            .unwrap()
    }

    #[test]
    fn signature_verifies_and_tampering_fails_closed() {
        let signed = signed_proposal();
        signed.verify().unwrap();
        let mut tampered = signed.clone();
        tampered.proposal.risk = "critical".into();
        assert!(matches!(
            tampered.verify(),
            Err(EvolutionError::InvalidProposal(_))
        ));
        let mut forged_signature = signed.clone();
        forged_signature.signature[0] ^= 1;
        assert!(matches!(
            forged_signature.verify(),
            Err(EvolutionError::InvalidSignature)
        ));
    }

    #[test]
    fn unknown_or_mismatched_signers_are_rejected() {
        let dir = tempdir().unwrap();
        let signed = signed_proposal();
        let unknown = EvolutionLedger::open(dir.path().join("unknown.json")).unwrap();
        assert!(matches!(
            unknown.propose(signed.clone()),
            Err(EvolutionError::UntrustedSigner(_))
        ));

        let mut wrong_key = TrustedSignerStore::default();
        wrong_key
            .trust_public_key(&signed.signer_id, &[9u8; 32])
            .unwrap();
        let mismatched = EvolutionLedger::open_with_trusted_signers(
            dir.path().join("mismatched.json"),
            wrong_key,
        )
        .unwrap();
        assert!(matches!(
            mismatched.propose(signed),
            Err(EvolutionError::UntrustedSigner(_))
        ));
    }

    #[test]
    fn contradictory_checks_and_workspace_escapes_are_rejected() {
        assert!(matches!(
            EvaluationCheck::from_output("cargo test", true, Some(1), "", "failure", 1),
            Err(EvolutionError::InvalidProposal(_))
        ));
        let dir = tempdir().unwrap();
        let check = EvaluationCheck::from_output("cargo test", true, Some(0), "ok", "", 1).unwrap();
        let missing = vec![String::from("missing.txt")];
        assert!(CanaryReport::from_workspace(dir.path(), "run-1", vec![check], &missing).is_err());
    }

    #[test]
    fn canary_report_rejects_unsafe_paths_and_malformed_digests() {
        let check = EvaluationCheck::from_output("cargo test", true, Some(0), "ok", "", 1).unwrap();
        let unsafe_path = BTreeMap::from([(String::from("../escape"), "0".repeat(64))]);
        assert!(matches!(
            CanaryReport::new("run-1", vec![check.clone()], unsafe_path),
            Err(EvolutionError::InvalidProposal(_))
        ));
        let malformed_digest = BTreeMap::from([(String::from("src/lib.rs"), String::from("bad"))]);
        assert!(matches!(
            CanaryReport::new("run-1", vec![check], malformed_digest),
            Err(EvolutionError::InvalidProposal(_))
        ));
    }

    #[test]
    fn lifecycle_persists_approval_canary_and_apply() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("evolution.json");
        let ledger = open_ledger(&path);
        let id = ledger.propose(signed_proposal()).unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::Draft
        ));
        ledger.approve(&id, "reviewer:local").unwrap();
        ledger.start_canary(&id, "run-1").unwrap();
        let check = EvaluationCheck::from_output(
            "cargo test --all-targets",
            true,
            Some(0),
            "37 tests passed",
            "",
            42,
        )
        .unwrap();
        let report = report_for_run(dir.path(), "run-1", check);
        ledger.finalize_canary_report(&id, report).unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::Applied { .. }
        ));
        let reopened = open_ledger(&path);
        assert!(matches!(
            reopened.get(&id).unwrap().unwrap().state,
            ProposalState::Applied { .. }
        ));
    }

    #[test]
    fn mismatched_or_failed_canary_report_cannot_apply() {
        let dir = tempdir().unwrap();
        let ledger = open_ledger(dir.path().join("evolution.json"));
        let id = ledger.propose(signed_proposal()).unwrap();
        ledger.approve(&id, "reviewer:local").unwrap();
        ledger.start_canary(&id, "run-1").unwrap();
        let failed =
            EvaluationCheck::from_output("cargo test", false, Some(1), "", "failure", 10).unwrap();
        let mismatched = report_for_run(dir.path(), "run-2", failed.clone());
        assert!(matches!(
            ledger.finalize_canary_report(&id, mismatched),
            Err(EvolutionError::StateConflict(_))
        ));
        let report = report_for_run(dir.path(), "run-1", failed);
        ledger.finalize_canary_report(&id, report).unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::RolledBack { .. }
        ));
    }

    #[test]
    fn failed_canary_rolls_back_and_draft_cannot_skip_approval() {
        let dir = tempdir().unwrap();
        let ledger = open_ledger(dir.path().join("evolution.json"));
        let id = ledger.propose(signed_proposal()).unwrap();
        assert!(ledger.start_canary(&id, "run-1").is_err());
        ledger.approve(&id, "reviewer:local").unwrap();
        ledger.start_canary(&id, "run-1").unwrap();
        let check = EvaluationCheck::from_output(
            "cargo test",
            false,
            Some(1),
            "",
            "verification failed",
            10,
        )
        .unwrap();
        ledger
            .finalize_canary_report(&id, report_for_run(dir.path(), "run-1", check))
            .unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::RolledBack { .. }
        ));
    }
}
