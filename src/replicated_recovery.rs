use crate::disaster_recovery::{
    DisasterRecoveryController, DisasterRecoveryError, DisasterRecoverySnapshot, FailoverAction,
    FailoverProposal, RecoveryPhase,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_MEMBERS: usize = 64;
const MAX_LOG_ENTRIES: usize = 4096;
const MAX_AUTHORITY_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHAOS_EVENTS: usize = 16_384;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ReplicatedRecoveryError {
    #[error("invalid replicated recovery input: {0}")]
    InvalidInput(String),
    #[error("invalid observer membership: {0}")]
    InvalidMembership(String),
    #[error("membership transition is already in progress")]
    MembershipChangeInProgress,
    #[error("membership transition is not in progress")]
    NoMembershipChange,
    #[error("replicated recovery quorum unavailable: {0}")]
    QuorumUnavailable(String),
    #[error("replicated recovery log violation: {0}")]
    LogViolation(String),
    #[error("replicated recovery snapshot failure: {0}")]
    SnapshotFailure(String),
    #[error("fencing token rejected: {0}")]
    FencingTokenRejected(String),
    #[error("disaster recovery controller rejected authority action: {0}")]
    Controller(DisasterRecoveryError),
}

impl From<DisasterRecoveryError> for ReplicatedRecoveryError {
    fn from(value: DisasterRecoveryError) -> Self {
        Self::Controller(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicatedRecoveryConfig {
    pub cluster_id: String,
    pub resource_id: String,
    pub max_members: usize,
    pub max_log_entries: usize,
}

impl ReplicatedRecoveryConfig {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        max_members: usize,
        max_log_entries: usize,
    ) -> Result<Self, ReplicatedRecoveryError> {
        let config = Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            max_members,
            max_log_entries,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ReplicatedRecoveryError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        if self.max_members < 3 || self.max_members > MAX_MEMBERS {
            return Err(ReplicatedRecoveryError::InvalidInput(
                "max member bound must be between 3 and 64".into(),
            ));
        }
        if self.max_log_entries == 0 || self.max_log_entries > MAX_LOG_ENTRIES {
            return Err(ReplicatedRecoveryError::InvalidInput(
                "log bound is outside the safe range".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObserverMembershipPhase {
    Stable,
    Joint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObserverMembership {
    pub epoch: u64,
    pub members: BTreeSet<String>,
    pub previous_members: Option<BTreeSet<String>>,
    pub phase: ObserverMembershipPhase,
    pub transition_index: Option<u64>,
}

impl ObserverMembership {
    pub fn stable(epoch: u64, members: BTreeSet<String>) -> Result<Self, ReplicatedRecoveryError> {
        let membership = Self {
            epoch,
            members,
            previous_members: None,
            phase: ObserverMembershipPhase::Stable,
            transition_index: None,
        };
        membership.validate()?;
        Ok(membership)
    }

    pub fn validate(&self) -> Result<(), ReplicatedRecoveryError> {
        if self.epoch == 0 {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "membership epoch must be positive".into(),
            ));
        }
        validate_members(&self.members)?;
        match self.phase {
            ObserverMembershipPhase::Stable => {
                if self.previous_members.is_some() || self.transition_index.is_some() {
                    return Err(ReplicatedRecoveryError::InvalidMembership(
                        "stable membership cannot carry a previous set or transition index".into(),
                    ));
                }
            }
            ObserverMembershipPhase::Joint => {
                let previous = self.previous_members.as_ref().ok_or_else(|| {
                    ReplicatedRecoveryError::InvalidMembership(
                        "joint membership requires the previous set".into(),
                    )
                })?;
                validate_members(previous)?;
                if self.transition_index.is_none() || previous == &self.members {
                    return Err(ReplicatedRecoveryError::InvalidMembership(
                        "joint membership requires distinct old/new sets and an index".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn required_quorum(&self) -> usize {
        quorum_size(&self.members)
    }

    pub fn quorum_met(&self, acknowledgements: &BTreeSet<String>) -> bool {
        match self.phase {
            ObserverMembershipPhase::Stable => {
                majority_acknowledged(&self.members, acknowledgements)
            }
            ObserverMembershipPhase::Joint => self.previous_members.as_ref().is_some_and(|old| {
                majority_acknowledged(old, acknowledgements)
                    && majority_acknowledged(&self.members, acknowledgements)
            }),
        }
    }

    pub fn voters(&self) -> BTreeSet<String> {
        let mut voters = self.members.clone();
        if let Some(previous) = &self.previous_members {
            voters.extend(previous.iter().cloned());
        }
        voters
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalFencingToken {
    pub cluster_id: String,
    pub resource_id: String,
    pub owner_region_id: String,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub membership_epoch: u64,
    pub fence_epoch: u64,
    pub authority_id: String,
    pub log_index: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct FencingTokenPayload<'a> {
    cluster_id: &'a str,
    resource_id: &'a str,
    owner_region_id: &'a str,
    owner_term: u64,
    ownership_epoch: u64,
    membership_epoch: u64,
    fence_epoch: u64,
    authority_id: &'a str,
    log_index: u64,
    public_key: &'a [u8],
}

impl ExternalFencingToken {
    fn issue(
        config: &ReplicatedRecoveryConfig,
        authority_id: &str,
        owner_region_id: &str,
        owner_term: u64,
        ownership_epoch: u64,
        membership_epoch: u64,
        fence_epoch: u64,
        log_index: u64,
        signing_key: &SigningKey,
    ) -> Result<Self, ReplicatedRecoveryError> {
        let mut token = Self {
            cluster_id: config.cluster_id.clone(),
            resource_id: config.resource_id.clone(),
            owner_region_id: owner_region_id.to_string(),
            owner_term,
            ownership_epoch,
            membership_epoch,
            fence_epoch,
            authority_id: authority_id.to_string(),
            log_index,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
        };
        token.validate_shape()?;
        token.signature = signing_key
            .sign(&token.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(token)
    }

    pub fn verify(
        &self,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
        expected_resource_id: &str,
    ) -> Result<(), ReplicatedRecoveryError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.resource_id != expected_resource_id {
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "cluster or resource binding mismatch".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "token signer is not trusted".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            ReplicatedRecoveryError::FencingTokenRejected("signature encoding is invalid".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| {
                ReplicatedRecoveryError::FencingTokenRejected(
                    "Ed25519 fencing-token verification failed".into(),
                )
            })
    }

    pub fn token_hash(&self) -> String {
        digest_json(self).unwrap_or_default()
    }

    fn validate_shape(&self) -> Result<(), ReplicatedRecoveryError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.owner_region_id, "owner region")?;
        validate_identifier(&self.authority_id, "authority")?;
        if self.owner_term == 0
            || self.ownership_epoch == 0
            || self.membership_epoch == 0
            || self.fence_epoch == 0
            || self.log_index == 0
        {
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "token generations and log index must be positive".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 {
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "token key or signature length is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, ReplicatedRecoveryError> {
        serde_json::to_vec(&FencingTokenPayload {
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            owner_region_id: &self.owner_region_id,
            owner_term: self.owner_term,
            ownership_epoch: self.ownership_epoch,
            membership_epoch: self.membership_epoch,
            fence_epoch: self.fence_epoch,
            authority_id: &self.authority_id,
            log_index: self.log_index,
            public_key: &self.public_key,
        })
        .map_err(|error| ReplicatedRecoveryError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExternalFenceAction {
    Activated(ExternalFencingToken),
    AlreadyActive(ExternalFencingToken),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalFenceState {
    pub resource_id: String,
    pub active_owner_region_id: Option<String>,
    pub accepted_fence_epoch: u64,
    pub accepted_token_hash: Option<String>,
}

impl ExternalFenceState {
    pub fn new(resource_id: &str) -> Result<Self, ReplicatedRecoveryError> {
        validate_identifier(resource_id, "resource")?;
        Ok(Self {
            resource_id: resource_id.to_string(),
            active_owner_region_id: None,
            accepted_fence_epoch: 0,
            accepted_token_hash: None,
        })
    }

    pub fn apply(
        &mut self,
        token: ExternalFencingToken,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
    ) -> Result<ExternalFenceAction, ReplicatedRecoveryError> {
        token.verify(trusted_key, expected_cluster_id, &self.resource_id)?;
        let token_hash = token.token_hash();
        if token.fence_epoch < self.accepted_fence_epoch {
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "fencing token is older than the externally accepted token".into(),
            ));
        }
        if token.fence_epoch == self.accepted_fence_epoch {
            if self.accepted_token_hash.as_deref() == Some(token_hash.as_str()) {
                return Ok(ExternalFenceAction::AlreadyActive(token));
            }
            return Err(ReplicatedRecoveryError::FencingTokenRejected(
                "same fence epoch carries a conflicting token".into(),
            ));
        }
        self.accepted_fence_epoch = token.fence_epoch;
        self.active_owner_region_id = Some(token.owner_region_id.clone());
        self.accepted_token_hash = Some(token_hash);
        Ok(ExternalFenceAction::Activated(token))
    }

    pub fn admit(
        &self,
        token: &ExternalFencingToken,
        trusted_key: &VerifyingKey,
        expected_cluster_id: &str,
    ) -> Result<bool, ReplicatedRecoveryError> {
        token.verify(trusted_key, expected_cluster_id, &self.resource_id)?;
        Ok(self.accepted_fence_epoch == token.fence_epoch
            && self.accepted_token_hash.as_deref() == Some(token.token_hash().as_str())
            && self.active_owner_region_id.as_deref() == Some(token.owner_region_id.as_str()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecoveryAuthorityCommand {
    BeginJointMembership {
        old_members: BTreeSet<String>,
        new_members: BTreeSet<String>,
        new_observer_keys: BTreeMap<String, Vec<u8>>,
        next_epoch: u64,
    },
    FinalizeMembership {
        members: BTreeSet<String>,
        epoch: u64,
    },
    CommitRecovery {
        proposal: FailoverProposal,
        fencing_token: ExternalFencingToken,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryAuthorityLogEntry {
    pub index: u64,
    pub term: u64,
    pub command: RecoveryAuthorityCommand,
    pub entry_hash: String,
}

impl RecoveryAuthorityLogEntry {
    fn new(
        index: u64,
        term: u64,
        command: RecoveryAuthorityCommand,
    ) -> Result<Self, ReplicatedRecoveryError> {
        let mut entry = Self {
            index,
            term,
            command,
            entry_hash: String::new(),
        };
        entry.entry_hash = entry.content_hash()?;
        Ok(entry)
    }

    fn content_hash(&self) -> Result<String, ReplicatedRecoveryError> {
        digest_json(&(self.index, self.term, &self.command))
    }

    fn validate(&self, expected_index: u64) -> Result<(), ReplicatedRecoveryError> {
        if self.index != expected_index || self.term == 0 {
            return Err(ReplicatedRecoveryError::LogViolation(
                "log index or term is not monotonic".into(),
            ));
        }
        if self.content_hash()? != self.entry_hash {
            return Err(ReplicatedRecoveryError::LogViolation(
                "log entry hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplicatedRecoveryAction {
    Appended { index: u64, required: String },
    Committed { index: u64 },
    AlreadyCommitted { index: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicatedRecoveryReport {
    pub cluster_id: String,
    pub authority_id: String,
    pub membership_epoch: u64,
    pub membership_phase: ObserverMembershipPhase,
    pub members: BTreeSet<String>,
    pub previous_members: Option<BTreeSet<String>>,
    pub log_len: usize,
    pub commit_index: u64,
    pub applied_index: u64,
    pub active_fence_epoch: u64,
    pub active_owner_region_id: Option<String>,
    pub safety_passed: bool,
    pub trace_digest: String,
}

#[derive(Debug)]
pub struct ReplicatedRecoveryAuthority {
    config: ReplicatedRecoveryConfig,
    authority_id: String,
    signing_key: SigningKey,
    membership: ObserverMembership,
    observer_keys: BTreeMap<String, Vec<u8>>,
    log: Vec<RecoveryAuthorityLogEntry>,
    acknowledgements: BTreeMap<u64, BTreeSet<String>>,
    commit_index: u64,
    applied_index: u64,
    current_term: u64,
    pending_joint_index: Option<u64>,
    last_fence_epoch: u64,
    active_fencing_token: Option<ExternalFencingToken>,
    controller: DisasterRecoveryController,
    events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicatedRecoverySnapshot {
    pub config: ReplicatedRecoveryConfig,
    pub authority_id: String,
    pub authority_public_key: Vec<u8>,
    pub membership: ObserverMembership,
    pub observer_keys: BTreeMap<String, Vec<u8>>,
    pub log: Vec<RecoveryAuthorityLogEntry>,
    pub acknowledgements: BTreeMap<u64, BTreeSet<String>>,
    pub commit_index: u64,
    pub applied_index: u64,
    pub current_term: u64,
    pub pending_joint_index: Option<u64>,
    pub last_fence_epoch: u64,
    pub active_fencing_token: Option<ExternalFencingToken>,
    pub controller_snapshot: DisasterRecoverySnapshot,
    pub events: Vec<String>,
    pub state_hash: String,
}

impl ReplicatedRecoveryAuthority {
    pub fn new(
        config: ReplicatedRecoveryConfig,
        authority_id: &str,
        signing_key: SigningKey,
        membership: ObserverMembership,
        observer_keys: BTreeMap<String, Vec<u8>>,
        controller: DisasterRecoveryController,
    ) -> Result<Self, ReplicatedRecoveryError> {
        config.validate()?;
        validate_identifier(authority_id, "authority")?;
        membership.validate()?;
        if membership.members.len() > config.max_members {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "membership exceeds authority configuration bound".into(),
            ));
        }
        validate_observer_keys(&observer_keys, &membership.members)?;
        let report = controller.report();
        if report.cluster_id != config.cluster_id {
            return Err(ReplicatedRecoveryError::InvalidInput(
                "controller and authority cluster IDs differ".into(),
            ));
        }
        Ok(Self {
            config,
            authority_id: authority_id.to_string(),
            signing_key,
            membership,
            observer_keys,
            log: Vec::new(),
            acknowledgements: BTreeMap::new(),
            commit_index: 0,
            applied_index: 0,
            current_term: 1,
            pending_joint_index: None,
            last_fence_epoch: 0,
            active_fencing_token: None,
            controller,
            events: Vec::new(),
        })
    }

    pub fn authority_id(&self) -> &str {
        &self.authority_id
    }

    pub fn public_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn membership(&self) -> &ObserverMembership {
        &self.membership
    }

    pub fn log(&self) -> &[RecoveryAuthorityLogEntry] {
        &self.log
    }

    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }

    pub fn applied_index(&self) -> u64 {
        self.applied_index
    }

    pub fn controller(&self) -> &DisasterRecoveryController {
        &self.controller
    }

    pub fn controller_mut(&mut self) -> &mut DisasterRecoveryController {
        &mut self.controller
    }

    pub fn active_fencing_token(&self) -> Option<&ExternalFencingToken> {
        self.active_fencing_token.as_ref()
    }

    pub fn begin_joint_membership(
        &mut self,
        new_members: BTreeSet<String>,
        new_observer_keys: BTreeMap<String, Vec<u8>>,
        next_epoch: u64,
    ) -> Result<ReplicatedRecoveryAction, ReplicatedRecoveryError> {
        if self.membership.phase != ObserverMembershipPhase::Stable {
            return Err(ReplicatedRecoveryError::MembershipChangeInProgress);
        }
        if next_epoch <= self.membership.epoch {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "membership epoch must increase".into(),
            ));
        }
        validate_members(&new_members)?;
        validate_observer_keys(&new_observer_keys, &new_members)?;
        if new_members == self.membership.members {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "joint membership must change the observer set".into(),
            ));
        }
        let old_members = self.membership.members.clone();
        let entry = self.append(RecoveryAuthorityCommand::BeginJointMembership {
            old_members: old_members.clone(),
            new_members,
            new_observer_keys,
            next_epoch,
        })?;
        self.pending_joint_index = Some(entry.index);
        self.events
            .push(format!("joint-membership-appended:{}", entry.index));
        Ok(ReplicatedRecoveryAction::Appended {
            index: entry.index,
            required: format!(
                "old majority {} and new majority",
                quorum_size(&old_members)
            ),
        })
    }

    pub fn finalize_membership(
        &mut self,
    ) -> Result<ReplicatedRecoveryAction, ReplicatedRecoveryError> {
        if self.membership.phase != ObserverMembershipPhase::Joint {
            return Err(ReplicatedRecoveryError::NoMembershipChange);
        }
        let index = self.pending_joint_index.ok_or_else(|| {
            ReplicatedRecoveryError::LogViolation("joint phase has no transition index".into())
        })?;
        if self.commit_index < index {
            return Err(ReplicatedRecoveryError::QuorumUnavailable(
                "joint membership must commit before finalization".into(),
            ));
        }
        let entry = self.append(RecoveryAuthorityCommand::FinalizeMembership {
            members: self.membership.members.clone(),
            epoch: self.membership.epoch,
        })?;
        self.events
            .push(format!("membership-final-appended:{}", entry.index));
        Ok(ReplicatedRecoveryAction::Appended {
            index: entry.index,
            required: format!("new majority {}", quorum_size(&self.membership.members)),
        })
    }

    pub fn prepare_recovery(
        &mut self,
        candidate_region_id: &str,
        owner_term: u64,
        ownership_epoch: u64,
        snapshot_hash: &str,
    ) -> Result<FailoverAction, ReplicatedRecoveryError> {
        self.controller
            .prepare_promotion(
                candidate_region_id,
                owner_term,
                ownership_epoch,
                snapshot_hash,
            )
            .map_err(Into::into)
    }

    pub fn append_recovery_commit(
        &mut self,
        proposal: FailoverProposal,
    ) -> Result<ReplicatedRecoveryAction, ReplicatedRecoveryError> {
        let snapshot = self.controller.snapshot()?;
        if self.membership.phase != ObserverMembershipPhase::Stable
            || snapshot.membership_epoch != self.membership.epoch
            || snapshot.phase != RecoveryPhase::PromotionPrepared
            || snapshot.pending_proposal.as_ref() != Some(&proposal)
        {
            return Err(ReplicatedRecoveryError::LogViolation(
                "recovery commit must reference the controller's exact pending proposal".into(),
            ));
        }
        let index = self.next_index();
        let fence_epoch = self.last_fence_epoch.saturating_add(1);
        let token = ExternalFencingToken::issue(
            &self.config,
            &self.authority_id,
            &proposal.candidate_region_id,
            proposal.owner_term,
            proposal.ownership_epoch,
            self.membership.epoch,
            fence_epoch,
            index,
            &self.signing_key,
        )?;
        let entry = self.append(RecoveryAuthorityCommand::CommitRecovery {
            proposal,
            fencing_token: token,
        })?;
        self.events
            .push(format!("recovery-commit-appended:{}", entry.index));
        Ok(ReplicatedRecoveryAction::Appended {
            index: entry.index,
            required: self.required_quorum_description(),
        })
    }

    pub fn acknowledge(
        &mut self,
        index: u64,
        member_id: &str,
    ) -> Result<(), ReplicatedRecoveryError> {
        validate_identifier(member_id, "member")?;
        let entry = self
            .log
            .iter()
            .find(|entry| entry.index == index)
            .ok_or_else(|| ReplicatedRecoveryError::LogViolation("unknown log index".into()))?;
        if !self.voters_for_entry(entry).contains(member_id) {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "acknowledgement is not from an eligible voter".into(),
            ));
        }
        self.acknowledgements
            .entry(index)
            .or_default()
            .insert(member_id.to_string());
        Ok(())
    }

    pub fn acknowledgement_count(&self, index: u64) -> usize {
        self.acknowledgements.get(&index).map_or(0, BTreeSet::len)
    }

    pub fn quorum_met(&self, index: u64) -> Result<bool, ReplicatedRecoveryError> {
        let entry = self
            .log
            .iter()
            .find(|entry| entry.index == index)
            .ok_or_else(|| ReplicatedRecoveryError::LogViolation("unknown log index".into()))?;
        Ok(self
            .acknowledgements
            .get(&index)
            .is_some_and(|acks| self.quorum_for_entry(entry, acks)))
    }

    pub fn commit_entry(
        &mut self,
        index: u64,
    ) -> Result<ReplicatedRecoveryAction, ReplicatedRecoveryError> {
        if index <= self.commit_index {
            return Ok(ReplicatedRecoveryAction::AlreadyCommitted { index });
        }
        if index != self.applied_index.saturating_add(1) {
            return Err(ReplicatedRecoveryError::LogViolation(
                "entries must commit in contiguous order".into(),
            ));
        }
        let entry = self
            .log
            .iter()
            .find(|entry| entry.index == index)
            .cloned()
            .ok_or_else(|| ReplicatedRecoveryError::LogViolation("unknown log index".into()))?;
        let acknowledgements = self
            .acknowledgements
            .get(&index)
            .cloned()
            .unwrap_or_default();
        if !self.quorum_for_entry(&entry, &acknowledgements) {
            return Err(ReplicatedRecoveryError::QuorumUnavailable(
                self.quorum_description(&entry, &acknowledgements),
            ));
        }
        self.apply_entry(&entry)?;
        self.commit_index = index;
        self.applied_index = index;
        self.events.push(format!("entry-committed:{}", index));
        Ok(ReplicatedRecoveryAction::Committed { index })
    }

    pub fn snapshot(&self) -> Result<ReplicatedRecoverySnapshot, ReplicatedRecoveryError> {
        let mut snapshot = ReplicatedRecoverySnapshot {
            config: self.config.clone(),
            authority_id: self.authority_id.clone(),
            authority_public_key: self.signing_key.verifying_key().to_bytes().to_vec(),
            membership: self.membership.clone(),
            observer_keys: self.observer_keys.clone(),
            log: self.log.clone(),
            acknowledgements: self.acknowledgements.clone(),
            commit_index: self.commit_index,
            applied_index: self.applied_index,
            current_term: self.current_term,
            pending_joint_index: self.pending_joint_index,
            last_fence_epoch: self.last_fence_epoch,
            active_fencing_token: self.active_fencing_token.clone(),
            controller_snapshot: self.controller.snapshot()?,
            events: self.events.clone(),
            state_hash: String::new(),
        };
        snapshot.state_hash = snapshot.computed_hash()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn save_snapshot(
        &self,
        store: &ReplicatedRecoverySnapshotStore,
    ) -> Result<(), ReplicatedRecoveryError> {
        store.save(&self.snapshot()?)
    }

    pub fn load_snapshot(
        store: &ReplicatedRecoverySnapshotStore,
        signing_key: SigningKey,
    ) -> Result<Self, ReplicatedRecoveryError> {
        Self::from_snapshot(store.load()?, signing_key)
    }

    pub fn from_snapshot(
        snapshot: ReplicatedRecoverySnapshot,
        signing_key: SigningKey,
    ) -> Result<Self, ReplicatedRecoveryError> {
        snapshot.validate()?;
        if snapshot.authority_public_key != signing_key.verifying_key().to_bytes() {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "restored authority key does not match snapshot identity".into(),
            ));
        }
        let controller = DisasterRecoveryController::from_snapshot(snapshot.controller_snapshot)?;
        Ok(Self {
            config: snapshot.config,
            authority_id: snapshot.authority_id,
            signing_key,
            membership: snapshot.membership,
            observer_keys: snapshot.observer_keys,
            log: snapshot.log,
            acknowledgements: snapshot.acknowledgements,
            commit_index: snapshot.commit_index,
            applied_index: snapshot.applied_index,
            current_term: snapshot.current_term,
            pending_joint_index: snapshot.pending_joint_index,
            last_fence_epoch: snapshot.last_fence_epoch,
            active_fencing_token: snapshot.active_fencing_token,
            controller,
            events: snapshot.events,
        })
    }

    pub fn report(&self) -> ReplicatedRecoveryReport {
        let safety_passed = self.membership.validate().is_ok()
            && self
                .log
                .iter()
                .enumerate()
                .all(|(offset, entry)| entry.validate(offset as u64 + 1).is_ok())
            && self.commit_index <= self.applied_index
            && self.applied_index == self.log.len() as u64
            && self.controller.report().safety_passed;
        ReplicatedRecoveryReport {
            cluster_id: self.config.cluster_id.clone(),
            authority_id: self.authority_id.clone(),
            membership_epoch: self.membership.epoch,
            membership_phase: self.membership.phase,
            members: self.membership.members.clone(),
            previous_members: self.membership.previous_members.clone(),
            log_len: self.log.len(),
            commit_index: self.commit_index,
            applied_index: self.applied_index,
            active_fence_epoch: self.last_fence_epoch,
            active_owner_region_id: self
                .active_fencing_token
                .as_ref()
                .map(|token| token.owner_region_id.clone()),
            safety_passed,
            trace_digest: self.trace_digest(),
        }
    }

    pub fn trace_digest(&self) -> String {
        digest_json(&(&self.log, &self.events)).unwrap_or_default()
    }

    fn append(
        &mut self,
        command: RecoveryAuthorityCommand,
    ) -> Result<RecoveryAuthorityLogEntry, ReplicatedRecoveryError> {
        if self.log.len() >= self.config.max_log_entries {
            return Err(ReplicatedRecoveryError::LogViolation(
                "recovery authority log is full".into(),
            ));
        }
        let entry = RecoveryAuthorityLogEntry::new(self.next_index(), self.current_term, command)?;
        self.log.push(entry.clone());
        self.acknowledgements.insert(entry.index, BTreeSet::new());
        Ok(entry)
    }

    fn next_index(&self) -> u64 {
        self.log.len() as u64 + 1
    }

    fn voters_for_entry(&self, entry: &RecoveryAuthorityLogEntry) -> BTreeSet<String> {
        match &entry.command {
            RecoveryAuthorityCommand::BeginJointMembership {
                old_members,
                new_members,
                ..
            } => old_members.union(new_members).cloned().collect(),
            RecoveryAuthorityCommand::FinalizeMembership { members, .. } => members.clone(),
            RecoveryAuthorityCommand::CommitRecovery { .. } => self.membership.voters(),
        }
    }

    fn quorum_for_entry(
        &self,
        entry: &RecoveryAuthorityLogEntry,
        acknowledgements: &BTreeSet<String>,
    ) -> bool {
        match &entry.command {
            RecoveryAuthorityCommand::BeginJointMembership {
                old_members,
                new_members,
                ..
            } => {
                majority_acknowledged(old_members, acknowledgements)
                    && majority_acknowledged(new_members, acknowledgements)
            }
            RecoveryAuthorityCommand::FinalizeMembership { members, .. } => {
                majority_acknowledged(members, acknowledgements)
            }
            RecoveryAuthorityCommand::CommitRecovery { .. } => {
                self.membership.quorum_met(acknowledgements)
            }
        }
    }

    fn required_quorum_description(&self) -> String {
        match self.membership.phase {
            ObserverMembershipPhase::Stable => {
                format!("majority {}", quorum_size(&self.membership.members))
            }
            ObserverMembershipPhase::Joint => "old and new majorities".into(),
        }
    }

    fn quorum_description(
        &self,
        entry: &RecoveryAuthorityLogEntry,
        acknowledgements: &BTreeSet<String>,
    ) -> String {
        match &entry.command {
            RecoveryAuthorityCommand::BeginJointMembership {
                old_members,
                new_members,
                ..
            } => format!(
                "joint quorum unavailable: old {}/{} new {}/{}",
                acknowledged_count(old_members, acknowledgements),
                quorum_size(old_members),
                acknowledged_count(new_members, acknowledgements),
                quorum_size(new_members)
            ),
            RecoveryAuthorityCommand::FinalizeMembership { members, .. } => format!(
                "final quorum unavailable: {}/{}",
                acknowledged_count(members, acknowledgements),
                quorum_size(members)
            ),
            RecoveryAuthorityCommand::CommitRecovery { .. } => "recovery quorum unavailable".into(),
        }
    }

    fn apply_entry(
        &mut self,
        entry: &RecoveryAuthorityLogEntry,
    ) -> Result<(), ReplicatedRecoveryError> {
        match &entry.command {
            RecoveryAuthorityCommand::BeginJointMembership {
                old_members,
                new_members,
                new_observer_keys,
                next_epoch,
            } => {
                if self.membership.phase != ObserverMembershipPhase::Stable
                    || &self.membership.members != old_members
                    || *next_epoch <= self.membership.epoch
                {
                    return Err(ReplicatedRecoveryError::InvalidMembership(
                        "committed joint transition does not match current membership".into(),
                    ));
                }
                validate_observer_keys(new_observer_keys, new_members)?;
                self.controller
                    .rotate_observer_membership(*next_epoch, new_observer_keys.clone())?;
                self.membership = ObserverMembership {
                    epoch: *next_epoch,
                    members: new_members.clone(),
                    previous_members: Some(old_members.clone()),
                    phase: ObserverMembershipPhase::Joint,
                    transition_index: Some(entry.index),
                };
                self.observer_keys = new_observer_keys.clone();
                self.pending_joint_index = Some(entry.index);
            }
            RecoveryAuthorityCommand::FinalizeMembership { members, epoch } => {
                if self.membership.phase != ObserverMembershipPhase::Joint
                    || self.membership.members != *members
                    || self.membership.epoch != *epoch
                {
                    return Err(ReplicatedRecoveryError::InvalidMembership(
                        "committed final transition does not match joint state".into(),
                    ));
                }
                if self.controller.report().membership_epoch != *epoch {
                    return Err(ReplicatedRecoveryError::InvalidMembership(
                        "controller membership epoch does not match final authority epoch".into(),
                    ));
                }
                self.membership = ObserverMembership::stable(*epoch, members.clone())?;
                self.pending_joint_index = None;
            }
            RecoveryAuthorityCommand::CommitRecovery {
                proposal,
                fencing_token,
            } => {
                fencing_token.verify(
                    &self.signing_key.verifying_key(),
                    &self.config.cluster_id,
                    &self.config.resource_id,
                )?;
                if fencing_token.log_index != entry.index
                    || fencing_token.membership_epoch != self.membership.epoch
                    || fencing_token.fence_epoch != self.last_fence_epoch + 1
                    || fencing_token.owner_region_id != proposal.candidate_region_id
                {
                    return Err(ReplicatedRecoveryError::FencingTokenRejected(
                        "committed token is not bound to the recovery log entry".into(),
                    ));
                }
                self.controller.commit_promotion(proposal.clone())?;
                self.last_fence_epoch = fencing_token.fence_epoch;
                self.active_fencing_token = Some(fencing_token.clone());
            }
        }
        Ok(())
    }
}

impl ReplicatedRecoverySnapshot {
    fn content_bytes(&self) -> Result<Vec<u8>, ReplicatedRecoveryError> {
        serde_json::to_vec(&(
            &self.config,
            &self.authority_id,
            &self.authority_public_key,
            &self.membership,
            &self.observer_keys,
            &self.log,
            &self.acknowledgements,
            self.commit_index,
            self.applied_index,
            self.current_term,
            self.pending_joint_index,
            self.last_fence_epoch,
            &self.active_fencing_token,
            &self.controller_snapshot,
            &self.events,
        ))
        .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))
    }

    fn computed_hash(&self) -> Result<String, ReplicatedRecoveryError> {
        let mut digest = Sha256::new();
        digest.update(self.content_bytes()?);
        Ok(format!("{:x}", digest.finalize()))
    }

    pub fn validate(&self) -> Result<(), ReplicatedRecoveryError> {
        self.config.validate()?;
        validate_identifier(&self.authority_id, "authority")?;
        if self.authority_public_key.len() != 32 {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority public key length is invalid".into(),
            ));
        }
        let authority_key =
            VerifyingKey::from_bytes(self.authority_public_key.as_slice().try_into().map_err(
                |_| ReplicatedRecoveryError::SnapshotFailure("authority public key length".into()),
            )?)
            .map_err(|_| {
                ReplicatedRecoveryError::SnapshotFailure("authority public key encoding".into())
            })?;
        self.membership.validate()?;
        if self.log.len() > self.config.max_log_entries {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority log exceeds configured bound".into(),
            ));
        }
        validate_observer_keys(&self.observer_keys, &self.membership.members)?;
        for (offset, entry) in self.log.iter().enumerate() {
            entry.validate(offset as u64 + 1)?;
            if let RecoveryAuthorityCommand::CommitRecovery { fencing_token, .. } = &entry.command {
                fencing_token.verify(
                    &authority_key,
                    &self.config.cluster_id,
                    &self.config.resource_id,
                )?;
                if fencing_token.log_index != entry.index {
                    return Err(ReplicatedRecoveryError::SnapshotFailure(
                        "fencing token log index mismatch".into(),
                    ));
                }
            }
        }
        if self.applied_index > self.commit_index || self.commit_index > self.log.len() as u64 {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority commit frontier is invalid".into(),
            ));
        }
        for (index, acknowledgements) in &self.acknowledgements {
            if *index == 0 || *index > self.log.len() as u64 {
                return Err(ReplicatedRecoveryError::SnapshotFailure(
                    "acknowledgement index is outside the log".into(),
                ));
            }
            for member in acknowledgements {
                validate_identifier(member, "member")?;
            }
        }
        if let Some(token) = &self.active_fencing_token {
            token.verify(
                &authority_key,
                &self.config.cluster_id,
                &self.config.resource_id,
            )?;
            if token.fence_epoch != self.last_fence_epoch {
                return Err(ReplicatedRecoveryError::SnapshotFailure(
                    "active token does not match fence frontier".into(),
                ));
            }
        } else if self.last_fence_epoch != 0 {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "fence frontier exists without active token".into(),
            ));
        }
        self.controller_snapshot.validate()?;
        if self.computed_hash()? != self.state_hash {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority snapshot state hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ReplicatedRecoverySnapshotStore {
    path: PathBuf,
}

impl ReplicatedRecoverySnapshotStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn staging_path(&self) -> PathBuf {
        self.path.with_extension("authority.tmp")
    }

    pub fn save(
        &self,
        snapshot: &ReplicatedRecoverySnapshot,
    ) -> Result<(), ReplicatedRecoveryError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        if bytes.len() as u64 > MAX_AUTHORITY_SNAPSHOT_BYTES {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority snapshot exceeds configured bound".into(),
            ));
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        let staging = self.staging_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&staging, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&staging);
        }
        result.map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn load(&self) -> Result<ReplicatedRecoverySnapshot, ReplicatedRecoveryError> {
        let metadata = fs::metadata(&self.path)
            .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        if metadata.len() > MAX_AUTHORITY_SNAPSHOT_BYTES {
            return Err(ReplicatedRecoveryError::SnapshotFailure(
                "authority snapshot exceeds configured bound".into(),
            ));
        }
        let snapshot: ReplicatedRecoverySnapshot = serde_json::from_slice(
            &fs::read(&self.path)
                .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?,
        )
        .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn recover_staging(&self) -> Result<bool, ReplicatedRecoveryError> {
        let staging = self.staging_path();
        if !staging.exists() {
            return Ok(false);
        }
        fs::remove_file(staging)
            .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
        Ok(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChaosFault {
    Drop,
    Delay { until_tick: u64 },
    Duplicate,
    Reorder,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChaosDelivery {
    Delivered,
    Delayed,
    Dropped,
    DuplicateDelivered,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChaosEvent {
    pub sequence: u64,
    pub tick: u64,
    pub from: String,
    pub to: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicatedRecoveryChaosReport {
    pub node_count: usize,
    pub dynamic_partition_steps: usize,
    pub membership_epochs_seen: Vec<u64>,
    pub committed_entries: u64,
    pub active_fence_epoch: u64,
    pub active_owner_region_id: Option<String>,
    pub stale_epoch_rejections: usize,
    pub stale_fence_rejections: usize,
    pub safety_passed: bool,
    pub trace_digest: String,
}

#[derive(Debug)]
pub struct ReplicatedRecoveryChaosSimulator {
    authority: ReplicatedRecoveryAuthority,
    node_ids: BTreeSet<String>,
    faults: BTreeMap<(String, String), ChaosFault>,
    tick: u64,
    sequence: u64,
    dynamic_partition_steps: usize,
    stale_epoch_rejections: usize,
    stale_fence_rejections: usize,
    events: Vec<ChaosEvent>,
}

impl ReplicatedRecoveryChaosSimulator {
    pub fn new(
        authority: ReplicatedRecoveryAuthority,
        node_ids: BTreeSet<String>,
    ) -> Result<Self, ReplicatedRecoveryError> {
        if node_ids.len() < 3 || node_ids.len() > MAX_MEMBERS {
            return Err(ReplicatedRecoveryError::InvalidInput(
                "chaos simulator node count is outside the safe range".into(),
            ));
        }
        for node in &node_ids {
            validate_identifier(node, "node")?;
        }
        Ok(Self {
            authority,
            node_ids,
            faults: BTreeMap::new(),
            tick: 0,
            sequence: 0,
            dynamic_partition_steps: 0,
            stale_epoch_rejections: 0,
            stale_fence_rejections: 0,
            events: Vec::new(),
        })
    }

    pub fn authority(&self) -> &ReplicatedRecoveryAuthority {
        &self.authority
    }

    pub fn authority_mut(&mut self) -> &mut ReplicatedRecoveryAuthority {
        &mut self.authority
    }

    pub fn partition(&mut self, from: &str, to: &str) -> Result<(), ReplicatedRecoveryError> {
        self.set_fault(from, to, ChaosFault::Drop)?;
        self.set_fault(to, from, ChaosFault::Drop)?;
        self.dynamic_partition_steps += 1;
        Ok(())
    }

    pub fn heal(&mut self, from: &str, to: &str) -> Result<(), ReplicatedRecoveryError> {
        validate_identifier(from, "source node")?;
        validate_identifier(to, "destination node")?;
        self.faults.remove(&(from.to_string(), to.to_string()));
        self.faults.remove(&(to.to_string(), from.to_string()));
        self.record_event(from, to, "link healed");
        Ok(())
    }

    pub fn inject_fault(
        &mut self,
        from: &str,
        to: &str,
        fault: ChaosFault,
    ) -> Result<(), ReplicatedRecoveryError> {
        self.set_fault(from, to, fault)
    }

    pub fn advance_tick(&mut self, ticks: u64) {
        self.tick = self.tick.saturating_add(ticks);
        self.record_event("clock", "clock", &format!("advanced:{ticks}"));
    }

    pub fn deliver_ack(
        &mut self,
        leader_id: &str,
        member_id: &str,
        index: u64,
    ) -> Result<ChaosDelivery, ReplicatedRecoveryError> {
        let fault = self
            .faults
            .get(&(leader_id.to_string(), member_id.to_string()))
            .cloned();
        match fault {
            Some(ChaosFault::Drop) => {
                self.record_event(leader_id, member_id, &format!("ack-dropped:{index}"));
                Ok(ChaosDelivery::Dropped)
            }
            Some(ChaosFault::Delay { until_tick }) if self.tick < until_tick => {
                self.record_event(leader_id, member_id, &format!("ack-delayed:{index}"));
                Ok(ChaosDelivery::Delayed)
            }
            Some(ChaosFault::Duplicate) => {
                self.authority.acknowledge(index, member_id)?;
                self.authority.acknowledge(index, member_id)?;
                self.record_event(leader_id, member_id, &format!("ack-duplicated:{index}"));
                Ok(ChaosDelivery::DuplicateDelivered)
            }
            _ => {
                self.authority.acknowledge(index, member_id)?;
                self.record_event(leader_id, member_id, &format!("ack-delivered:{index}"));
                Ok(ChaosDelivery::Delivered)
            }
        }
    }

    pub fn commit(
        &mut self,
        index: u64,
    ) -> Result<ReplicatedRecoveryAction, ReplicatedRecoveryError> {
        let action = self.authority.commit_entry(index)?;
        self.record_event("authority", "replicas", &format!("commit:{index}"));
        Ok(action)
    }

    pub fn reject_stale_epoch(&mut self, observed_epoch: u64) {
        if observed_epoch < self.authority.membership().epoch {
            self.stale_epoch_rejections += 1;
        }
        self.record_event("stale", "authority", &format!("epoch:{observed_epoch}"));
    }

    pub fn reject_stale_fence(&mut self) {
        self.stale_fence_rejections += 1;
        self.record_event("stale", "fence", "stale-token-rejected");
    }

    pub fn report(&self) -> ReplicatedRecoveryChaosReport {
        let authority_report = self.authority.report();
        let mut epochs = vec![authority_report.membership_epoch];
        if let Some(previous) = &authority_report.previous_members {
            if !previous.is_empty() {
                epochs.push(authority_report.membership_epoch.saturating_sub(1));
            }
        }
        epochs.sort_unstable();
        epochs.dedup();
        ReplicatedRecoveryChaosReport {
            node_count: self.node_ids.len(),
            dynamic_partition_steps: self.dynamic_partition_steps,
            membership_epochs_seen: epochs,
            committed_entries: authority_report.commit_index,
            active_fence_epoch: authority_report.active_fence_epoch,
            active_owner_region_id: authority_report.active_owner_region_id,
            stale_epoch_rejections: self.stale_epoch_rejections,
            stale_fence_rejections: self.stale_fence_rejections,
            safety_passed: authority_report.safety_passed && self.events.len() <= MAX_CHAOS_EVENTS,
            trace_digest: digest_json(&self.events).unwrap_or_default(),
        }
    }

    pub fn events(&self) -> &[ChaosEvent] {
        &self.events
    }

    fn set_fault(
        &mut self,
        from: &str,
        to: &str,
        fault: ChaosFault,
    ) -> Result<(), ReplicatedRecoveryError> {
        validate_identifier(from, "source node")?;
        validate_identifier(to, "destination node")?;
        if !self.node_ids.contains(from) || !self.node_ids.contains(to) || from == to {
            return Err(ReplicatedRecoveryError::InvalidInput(
                "fault endpoint is not a distinct simulator node".into(),
            ));
        }
        self.faults
            .insert((from.to_string(), to.to_string()), fault);
        self.record_event(from, to, "fault-injected");
        Ok(())
    }

    fn record_event(&mut self, from: &str, to: &str, detail: &str) {
        if self.events.len() >= MAX_CHAOS_EVENTS {
            return;
        }
        self.sequence = self.sequence.saturating_add(1);
        self.events.push(ChaosEvent {
            sequence: self.sequence,
            tick: self.tick,
            from: from.to_string(),
            to: to.to_string(),
            detail: detail.to_string(),
        });
    }
}

fn quorum_size(members: &BTreeSet<String>) -> usize {
    members.len() / 2 + 1
}

fn majority_acknowledged(members: &BTreeSet<String>, acknowledgements: &BTreeSet<String>) -> bool {
    acknowledged_count(members, acknowledgements) >= quorum_size(members)
}

fn acknowledged_count(members: &BTreeSet<String>, acknowledgements: &BTreeSet<String>) -> usize {
    members.intersection(acknowledgements).count()
}

fn validate_observer_keys(
    observer_keys: &BTreeMap<String, Vec<u8>>,
    members: &BTreeSet<String>,
) -> Result<(), ReplicatedRecoveryError> {
    if observer_keys.len() != members.len()
        || observer_keys.keys().any(|member| !members.contains(member))
    {
        return Err(ReplicatedRecoveryError::InvalidMembership(
            "observer-key registry must exactly match the stable/new membership".into(),
        ));
    }
    for (member, key_bytes) in observer_keys {
        validate_identifier(member, "observer")?;
        if key_bytes.len() != 32 {
            return Err(ReplicatedRecoveryError::InvalidMembership(
                "observer public key length is invalid".into(),
            ));
        }
        VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
            ReplicatedRecoveryError::InvalidMembership("observer public key length".into())
        })?)
        .map_err(|_| {
            ReplicatedRecoveryError::InvalidMembership("observer public key encoding".into())
        })?;
    }
    Ok(())
}

fn validate_members(members: &BTreeSet<String>) -> Result<(), ReplicatedRecoveryError> {
    if members.len() < 3 || members.len() > MAX_MEMBERS {
        return Err(ReplicatedRecoveryError::InvalidMembership(
            "membership must contain between 3 and 64 members".into(),
        ));
    }
    for member in members {
        validate_identifier(member, "member")?;
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ReplicatedRecoveryError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ReplicatedRecoveryError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, ReplicatedRecoveryError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ReplicatedRecoveryError::SnapshotFailure(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}
