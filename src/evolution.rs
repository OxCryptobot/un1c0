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
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvolutionError {
    #[error("invalid evolution proposal: {0}")]
    InvalidProposal(String),
    #[error("evolution signature verification failed")]
    InvalidSignature,
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
        if signer_id.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal("signer id is empty".into()));
        }
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
        if self.signer_id.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal("signer id is empty".into()));
        }
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
}

impl EvolutionLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EvolutionError> {
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
            validate_record(record)?;
        }
        Ok(Self {
            path,
            records: Arc::new(Mutex::new(records)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn propose(&self, signed: SignedEvolutionProposal) -> Result<String, EvolutionError> {
        signed.verify()?;
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

    pub fn finalize_canary(
        &self,
        id: &str,
        passed: bool,
        evidence: &str,
    ) -> Result<(), EvolutionError> {
        if evidence.trim().is_empty() {
            return Err(EvolutionError::InvalidProposal(
                "canary evidence is empty".into(),
            ));
        }
        self.transition(id, |record| match record.state {
            ProposalState::Canary { .. } if passed => {
                record.state = ProposalState::Applied {
                    evidence: evidence.to_string(),
                    completed_at_ms: now_ms(),
                };
                Ok(())
            }
            ProposalState::Canary { .. } => {
                record.state = ProposalState::RolledBack {
                    reason: evidence.to_string(),
                    completed_at_ms: now_ms(),
                };
                Ok(())
            }
            ref state => Err(EvolutionError::StateConflict(format!(
                "cannot finalize canary from state {:?}",
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
    match record.state {
        ProposalState::Draft if record.signed.proposal.approved => Err(
            EvolutionError::InvalidProposal("draft proposal cannot be marked approved".into()),
        ),
        ProposalState::Draft => Ok(()),
        ProposalState::Approved { .. }
        | ProposalState::Canary { .. }
        | ProposalState::Applied { .. }
        | ProposalState::RolledBack { .. }
            if !record.signed.proposal.approved => Err(EvolutionError::InvalidProposal(
                "non-draft proposal must be marked approved".into(),
            )),
        _ => Ok(()),
    }
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
    use tempfile::tempdir;

    fn signed_proposal() -> SignedEvolutionProposal {
        let files = BTreeMap::from([(String::from("skills/new.md"), String::from("content"))]);
        let proposal = EvolutionProposal::new("new skill", files, "cargo test", "medium").unwrap();
        let key = SigningKey::from_bytes(&[7u8; 32]);
        SignedEvolutionProposal::sign(proposal, "operator:local", &key).unwrap()
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
    }

    #[test]
    fn lifecycle_persists_approval_canary_and_apply() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("evolution.json");
        let ledger = EvolutionLedger::open(&path).unwrap();
        let id = ledger.propose(signed_proposal()).unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::Draft
        ));
        ledger.approve(&id, "reviewer:local").unwrap();
        ledger.start_canary(&id, "run-1").unwrap();
        ledger
            .finalize_canary(&id, true, "all tests passed")
            .unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::Applied { .. }
        ));
        let reopened = EvolutionLedger::open(&path).unwrap();
        assert!(matches!(
            reopened.get(&id).unwrap().unwrap().state,
            ProposalState::Applied { .. }
        ));
    }

    #[test]
    fn failed_canary_rolls_back_and_draft_cannot_skip_approval() {
        let dir = tempdir().unwrap();
        let ledger = EvolutionLedger::open(dir.path().join("evolution.json")).unwrap();
        let id = ledger.propose(signed_proposal()).unwrap();
        assert!(ledger.start_canary(&id, "run-1").is_err());
        ledger.approve(&id, "reviewer:local").unwrap();
        ledger.start_canary(&id, "run-1").unwrap();
        ledger
            .finalize_canary(&id, false, "verification failed")
            .unwrap();
        assert!(matches!(
            ledger.get(&id).unwrap().unwrap().state,
            ProposalState::RolledBack { .. }
        ));
    }
}
