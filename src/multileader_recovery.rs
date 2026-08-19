use crate::disaster_recovery::FailoverProposal;
use crate::replicated_recovery::{
    ExternalFenceAction, ExternalFenceState, ExternalFencingToken, ReplicatedRecoveryConfig,
    ReplicatedRecoveryError, TrustedFencingAuthorityRegistry,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_LEADERS: usize = 16;
const MAX_WITNESSES: usize = 32;
const MAX_CHAOS_EVENTS: usize = 16_384;
pub const MULTILEADER_PROPOSAL_DOMAIN: &str = "un1c0/multi-leader-failover-proposal/v1";
pub const WITNESS_VOTE_DOMAIN: &str = "un1c0/witness-arbitration-vote/v1";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MultiLeaderRecoveryError {
    #[error("invalid multi-leader recovery input: {0}")]
    InvalidInput(String),
    #[error("unknown leader: {0}")]
    UnknownLeader(String),
    #[error("unknown witness: {0}")]
    UnknownWitness(String),
    #[error("proposal signer is not trusted: {0}")]
    UntrustedLeader(String),
    #[error("leader proposal rejected: {0}")]
    ProposalRejected(String),
    #[error("witness vote rejected: {0}")]
    VoteRejected(String),
    #[error("witness quorum unavailable: {0}")]
    QuorumUnavailable(String),
    #[error("split-brain decision rejected: {0}")]
    SplitBrainRejected(String),
    #[error("fencing token rejected: {0}")]
    FencingTokenRejected(String),
    #[error("replicated recovery error: {0}")]
    Replicated(ReplicatedRecoveryError),
}

impl From<ReplicatedRecoveryError> for MultiLeaderRecoveryError {
    fn from(value: ReplicatedRecoveryError) -> Self {
        Self::Replicated(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiLeaderConfig {
    pub cluster_id: String,
    pub resource_id: String,
    pub max_leaders: usize,
    pub max_witnesses: usize,
}

impl MultiLeaderConfig {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        max_leaders: usize,
        max_witnesses: usize,
    ) -> Result<Self, MultiLeaderRecoveryError> {
        let config = Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            max_leaders,
            max_witnesses,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), MultiLeaderRecoveryError> {
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        if self.max_leaders == 0 || self.max_leaders > MAX_LEADERS {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "leader bound is outside the safe range".into(),
            ));
        }
        if self.max_witnesses < 3 || self.max_witnesses > MAX_WITNESSES {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "witness bound must be between 3 and 32".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionalLeader {
    pub leader_id: String,
    pub region_id: String,
    pub term: u64,
    pub ownership_epoch: u64,
    pub membership_epoch: u64,
    pub replicated_log_index: u64,
    pub snapshot_hash: String,
    pub public_key: Vec<u8>,
}

impl RegionalLeader {
    pub fn new(
        leader_id: &str,
        region_id: &str,
        term: u64,
        ownership_epoch: u64,
        membership_epoch: u64,
        replicated_log_index: u64,
        snapshot_hash: &str,
        signing_key: &SigningKey,
    ) -> Result<Self, MultiLeaderRecoveryError> {
        let leader = Self {
            leader_id: leader_id.to_string(),
            region_id: region_id.to_string(),
            term,
            ownership_epoch,
            membership_epoch,
            replicated_log_index,
            snapshot_hash: snapshot_hash.to_string(),
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
        };
        leader.validate()?;
        Ok(leader)
    }

    fn validate(&self) -> Result<(), MultiLeaderRecoveryError> {
        validate_identifier(&self.leader_id, "leader")?;
        validate_identifier(&self.region_id, "region")?;
        validate_hash(&self.snapshot_hash, "snapshot")?;
        if self.term == 0
            || self.ownership_epoch == 0
            || self.membership_epoch == 0
            || self.replicated_log_index == 0
            || self.public_key.len() != 32
        {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "leader generations, log index, or key shape is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaderFailoverProposal {
    pub domain: String,
    pub round_id: u64,
    pub cluster_id: String,
    pub resource_id: String,
    pub leader_id: String,
    pub candidate_region_id: String,
    pub owner_term: u64,
    pub ownership_epoch: u64,
    pub membership_epoch: u64,
    pub replicated_log_index: u64,
    pub snapshot_hash: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct LeaderProposalPayload<'a> {
    domain: &'a str,
    round_id: u64,
    cluster_id: &'a str,
    resource_id: &'a str,
    leader_id: &'a str,
    candidate_region_id: &'a str,
    owner_term: u64,
    ownership_epoch: u64,
    membership_epoch: u64,
    replicated_log_index: u64,
    snapshot_hash: &'a str,
    public_key: &'a [u8],
}

impl LeaderFailoverProposal {
    pub fn sign(
        config: &MultiLeaderConfig,
        round_id: u64,
        leader: &RegionalLeader,
        signing_key: &SigningKey,
    ) -> Result<Self, MultiLeaderRecoveryError> {
        if signing_key.verifying_key().to_bytes().to_vec() != leader.public_key {
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "leader public key does not match signing key".into(),
            ));
        }
        let mut proposal = Self {
            domain: MULTILEADER_PROPOSAL_DOMAIN.to_string(),
            round_id,
            cluster_id: config.cluster_id.clone(),
            resource_id: config.resource_id.clone(),
            leader_id: leader.leader_id.clone(),
            candidate_region_id: leader.region_id.clone(),
            owner_term: leader.term,
            ownership_epoch: leader.ownership_epoch,
            membership_epoch: leader.membership_epoch,
            replicated_log_index: leader.replicated_log_index,
            snapshot_hash: leader.snapshot_hash.clone(),
            public_key: leader.public_key.clone(),
            signature: vec![0; 64],
        };
        proposal.signature = signing_key
            .sign(&proposal.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(proposal)
    }

    pub fn verify(
        &self,
        config: &MultiLeaderConfig,
        trusted_key: &VerifyingKey,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.validate_shape()?;
        if self.domain != MULTILEADER_PROPOSAL_DOMAIN
            || self.cluster_id != config.cluster_id
            || self.resource_id != config.resource_id
        {
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "proposal domain or cluster/resource binding mismatch".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(MultiLeaderRecoveryError::UntrustedLeader(
                self.leader_id.clone(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            MultiLeaderRecoveryError::ProposalRejected("proposal signature encoding".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| MultiLeaderRecoveryError::ProposalRejected("proposal signature".into()))
    }

    pub fn digest(&self) -> String {
        digest_json(self).unwrap_or_default()
    }

    pub fn as_failover_proposal(&self) -> FailoverProposal {
        FailoverProposal {
            previous_region_id: String::new(),
            candidate_region_id: self.candidate_region_id.clone(),
            owner_term: self.owner_term,
            ownership_epoch: self.ownership_epoch,
            snapshot_hash: self.snapshot_hash.clone(),
        }
    }

    fn validate_shape(&self) -> Result<(), MultiLeaderRecoveryError> {
        validate_identifier(&self.domain, "proposal domain")?;
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.leader_id, "leader")?;
        validate_identifier(&self.candidate_region_id, "candidate region")?;
        validate_hash(&self.snapshot_hash, "snapshot")?;
        if self.round_id == 0
            || self.owner_term == 0
            || self.ownership_epoch == 0
            || self.membership_epoch == 0
            || self.replicated_log_index == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "proposal generations, index, key, or signature shape is invalid".into(),
            ));
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, MultiLeaderRecoveryError> {
        serde_json::to_vec(&LeaderProposalPayload {
            domain: &self.domain,
            round_id: self.round_id,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            leader_id: &self.leader_id,
            candidate_region_id: &self.candidate_region_id,
            owner_term: self.owner_term,
            ownership_epoch: self.ownership_epoch,
            membership_epoch: self.membership_epoch,
            replicated_log_index: self.replicated_log_index,
            snapshot_hash: &self.snapshot_hash,
            public_key: &self.public_key,
        })
        .map_err(|error| MultiLeaderRecoveryError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WitnessVote {
    pub domain: String,
    pub round_id: u64,
    pub witness_id: String,
    pub witness_membership_epoch: u64,
    pub proposal_digest: String,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct WitnessVotePayload<'a> {
    domain: &'a str,
    round_id: u64,
    witness_id: &'a str,
    witness_membership_epoch: u64,
    proposal_digest: &'a str,
    public_key: &'a [u8],
}

impl WitnessVote {
    pub fn sign(
        round_id: u64,
        witness_id: &str,
        witness_membership_epoch: u64,
        proposal: &LeaderFailoverProposal,
        signing_key: &SigningKey,
    ) -> Result<Self, MultiLeaderRecoveryError> {
        validate_identifier(witness_id, "witness")?;
        if witness_membership_epoch == 0 {
            return Err(MultiLeaderRecoveryError::VoteRejected(
                "witness membership epoch must be positive".into(),
            ));
        }
        let mut vote = Self {
            domain: WITNESS_VOTE_DOMAIN.to_string(),
            round_id,
            witness_id: witness_id.to_string(),
            witness_membership_epoch,
            proposal_digest: proposal.digest(),
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
        };
        vote.signature = signing_key
            .sign(&vote.canonical_payload()?)
            .to_bytes()
            .to_vec();
        Ok(vote)
    }

    pub fn verify(
        &self,
        proposal: &LeaderFailoverProposal,
        trusted_key: &VerifyingKey,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.validate_shape()?;
        if self.domain != WITNESS_VOTE_DOMAIN
            || self.round_id != proposal.round_id
            || self.proposal_digest != proposal.digest()
        {
            return Err(MultiLeaderRecoveryError::VoteRejected(
                "witness vote is not bound to this proposal round and digest".into(),
            ));
        }
        if self.public_key != trusted_key.to_bytes() {
            return Err(MultiLeaderRecoveryError::VoteRejected(
                "witness key mismatch".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            MultiLeaderRecoveryError::VoteRejected("witness signature encoding".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| MultiLeaderRecoveryError::VoteRejected("witness signature".into()))
    }

    fn validate_shape(&self) -> Result<(), MultiLeaderRecoveryError> {
        if self.domain != WITNESS_VOTE_DOMAIN
            || self.round_id == 0
            || self.witness_membership_epoch == 0
            || self.public_key.len() != 32
            || self.signature.len() != 64
        {
            return Err(MultiLeaderRecoveryError::VoteRejected(
                "witness vote shape is invalid".into(),
            ));
        }
        validate_identifier(&self.witness_id, "witness")?;
        validate_hash(&self.proposal_digest, "proposal digest")?;
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, MultiLeaderRecoveryError> {
        serde_json::to_vec(&WitnessVotePayload {
            domain: &self.domain,
            round_id: self.round_id,
            witness_id: &self.witness_id,
            witness_membership_epoch: self.witness_membership_epoch,
            proposal_digest: &self.proposal_digest,
            public_key: &self.public_key,
        })
        .map_err(|error| MultiLeaderRecoveryError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiLeaderDecision {
    pub round_id: u64,
    pub proposal_digest: String,
    pub candidate_region_id: String,
    pub winning_leader_id: String,
    pub witness_ids: BTreeSet<String>,
    pub fencing_token: ExternalFencingToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiLeaderReport {
    pub cluster_id: String,
    pub resource_id: String,
    pub leader_count: usize,
    pub witness_count: usize,
    pub active_region_id: Option<String>,
    pub active_leader_id: Option<String>,
    pub committed_round_id: u64,
    pub accepted_fence_epoch: u64,
    pub split_brain_rejections: usize,
    pub stale_proposal_rejections: usize,
    pub duplicate_vote_rejections: usize,
    pub safety_passed: bool,
    pub trace_digest: String,
}

#[derive(Debug)]
pub struct MultiLeaderFailoverAuthority {
    config: MultiLeaderConfig,
    fencing_config: ReplicatedRecoveryConfig,
    authority_id: String,
    authority_signing_key: SigningKey,
    leaders: BTreeMap<String, RegionalLeader>,
    leader_keys: BTreeMap<String, Vec<u8>>,
    witnesses: BTreeMap<String, Vec<u8>>,
    rounds: BTreeMap<u64, BTreeMap<String, BTreeSet<String>>>,
    proposals: BTreeMap<(u64, String), LeaderFailoverProposal>,
    committed: Option<MultiLeaderDecision>,
    active_leader_id: Option<String>,
    active_region_id: Option<String>,
    committed_log_index: u64,
    membership_epoch: u64,
    last_owner_term: u64,
    last_ownership_epoch: u64,
    last_fence_epoch: u64,
    registry: TrustedFencingAuthorityRegistry,
    external_fence: ExternalFenceState,
    events: Vec<String>,
    split_brain_rejections: usize,
    stale_proposal_rejections: usize,
    duplicate_vote_rejections: usize,
}

impl MultiLeaderFailoverAuthority {
    pub fn new(
        config: MultiLeaderConfig,
        fencing_config: ReplicatedRecoveryConfig,
        authority_id: &str,
        authority_signing_key: SigningKey,
        witnesses: BTreeMap<String, Vec<u8>>,
        membership_epoch: u64,
        initial_region_id: Option<&str>,
    ) -> Result<Self, MultiLeaderRecoveryError> {
        config.validate()?;
        fencing_config
            .validate()
            .map_err(|error| MultiLeaderRecoveryError::Replicated(error))?;
        validate_identifier(authority_id, "authority")?;
        if membership_epoch == 0 {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "membership epoch must be positive".into(),
            ));
        }
        if witnesses.len() < 3 || witnesses.len() > config.max_witnesses {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "witness registry is outside configured bounds".into(),
            ));
        }
        for (witness_id, key_bytes) in &witnesses {
            validate_identifier(witness_id, "witness")?;
            if key_bytes.len() != 32 {
                return Err(MultiLeaderRecoveryError::InvalidInput(
                    "witness key length is invalid".into(),
                ));
            }
        }
        let mut registry = TrustedFencingAuthorityRegistry::new();
        registry.register(authority_id, &authority_signing_key.verifying_key())?;
        let external_fence = ExternalFenceState::new(&config.resource_id)?;
        Ok(Self {
            config,
            fencing_config,
            authority_id: authority_id.to_string(),
            authority_signing_key,
            leaders: BTreeMap::new(),
            leader_keys: BTreeMap::new(),
            witnesses,
            rounds: BTreeMap::new(),
            proposals: BTreeMap::new(),
            committed: None,
            active_leader_id: None,
            active_region_id: initial_region_id.map(str::to_string),
            committed_log_index: 0,
            membership_epoch,
            last_owner_term: 0,
            last_ownership_epoch: 0,
            last_fence_epoch: 0,
            registry,
            external_fence,
            events: Vec::new(),
            split_brain_rejections: 0,
            stale_proposal_rejections: 0,
            duplicate_vote_rejections: 0,
        })
    }

    pub fn register_leader(
        &mut self,
        leader: RegionalLeader,
    ) -> Result<(), MultiLeaderRecoveryError> {
        leader.validate()?;
        if self.leaders.len() >= self.config.max_leaders
            && !self.leaders.contains_key(&leader.leader_id)
        {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "leader registry is full".into(),
            ));
        }
        self.leader_keys
            .insert(leader.leader_id.clone(), leader.public_key.clone());
        self.leaders.insert(leader.leader_id.clone(), leader);
        Ok(())
    }

    pub fn register_witness(
        &mut self,
        witness_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), MultiLeaderRecoveryError> {
        validate_identifier(witness_id, "witness")?;
        if self.witnesses.contains_key(witness_id) {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "witness rebinding requires an explicit membership transition".into(),
            ));
        }
        if self.witnesses.len() >= self.config.max_witnesses {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "witness registry is full".into(),
            ));
        }
        self.witnesses
            .insert(witness_id.to_string(), verifying_key.to_bytes().to_vec());
        Ok(())
    }

    pub fn begin_round(
        &mut self,
        round_id: u64,
        leader_id: &str,
        signing_key: &SigningKey,
    ) -> Result<LeaderFailoverProposal, MultiLeaderRecoveryError> {
        if round_id == 0 {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "arbitration round must be positive".into(),
            ));
        }
        let leader = self
            .leaders
            .get(leader_id)
            .ok_or_else(|| MultiLeaderRecoveryError::UnknownLeader(leader_id.to_string()))?
            .clone();
        if leader.membership_epoch != self.membership_epoch {
            self.stale_proposal_rejections += 1;
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "leader membership epoch is stale".into(),
            ));
        }
        if leader.replicated_log_index < self.committed_log_index {
            self.stale_proposal_rejections += 1;
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "leader replicated log is behind committed authority".into(),
            ));
        }
        if self.active_region_id.as_deref() == Some(leader.region_id.as_str())
            && self.active_leader_id.as_deref() == Some(leader.leader_id.as_str())
        {
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "active leader cannot propose its own failover".into(),
            ));
        }
        if signing_key.verifying_key().to_bytes().to_vec() != leader.public_key {
            return Err(MultiLeaderRecoveryError::UntrustedLeader(
                leader_id.to_string(),
            ));
        }
        let key =
            VerifyingKey::from_bytes(
                leader.public_key.as_slice().try_into().map_err(|_| {
                    MultiLeaderRecoveryError::UntrustedLeader(leader_id.to_string())
                })?,
            )
            .map_err(|_| MultiLeaderRecoveryError::UntrustedLeader(leader_id.to_string()))?;
        let proposal = LeaderFailoverProposal::sign(&self.config, round_id, &leader, &signing_key)?;
        proposal.verify(&self.config, &key)?;
        self.proposals
            .insert((round_id, proposal.digest()), proposal.clone());
        self.events
            .push(format!("proposal:{}:{}", round_id, leader_id));
        Ok(proposal)
    }

    pub fn accept_signed_proposal(
        &mut self,
        proposal: LeaderFailoverProposal,
    ) -> Result<(), MultiLeaderRecoveryError> {
        let key_bytes = self
            .leader_keys
            .get(&proposal.leader_id)
            .ok_or_else(|| MultiLeaderRecoveryError::UnknownLeader(proposal.leader_id.clone()))?;
        let key =
            VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                MultiLeaderRecoveryError::UntrustedLeader(proposal.leader_id.clone())
            })?)
            .map_err(|_| MultiLeaderRecoveryError::UntrustedLeader(proposal.leader_id.clone()))?;
        proposal.verify(&self.config, &key)?;
        let leader = self
            .leaders
            .get(&proposal.leader_id)
            .ok_or_else(|| MultiLeaderRecoveryError::UnknownLeader(proposal.leader_id.clone()))?;
        if proposal.membership_epoch != self.membership_epoch
            || proposal.replicated_log_index < self.committed_log_index
            || proposal.candidate_region_id != leader.region_id
            || proposal.owner_term != leader.term
            || proposal.ownership_epoch != leader.ownership_epoch
        {
            self.stale_proposal_rejections += 1;
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "proposal is stale or not bound to the registered leader state".into(),
            ));
        }
        self.proposals
            .insert((proposal.round_id, proposal.digest()), proposal);
        Ok(())
    }

    pub fn accept_vote(
        &mut self,
        proposal: &LeaderFailoverProposal,
        vote: WitnessVote,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.accept_signed_proposal(proposal.clone())?;
        let key_bytes = self
            .witnesses
            .get(&vote.witness_id)
            .ok_or_else(|| MultiLeaderRecoveryError::UnknownWitness(vote.witness_id.clone()))?;
        let key =
            VerifyingKey::from_bytes(key_bytes.as_slice().try_into().map_err(|_| {
                MultiLeaderRecoveryError::VoteRejected("witness key length".into())
            })?)
            .map_err(|_| MultiLeaderRecoveryError::VoteRejected("witness key encoding".into()))?;
        vote.verify(proposal, &key)?;
        if vote.witness_membership_epoch != self.membership_epoch {
            return Err(MultiLeaderRecoveryError::VoteRejected(
                "witness membership epoch is stale".into(),
            ));
        }
        let digest = proposal.digest();
        let votes = self
            .rounds
            .entry(proposal.round_id)
            .or_default()
            .entry(vote.witness_id.clone())
            .or_default();
        if !votes.is_empty() && !votes.contains(&digest) {
            self.split_brain_rejections += 1;
            self.duplicate_vote_rejections += 1;
            return Err(MultiLeaderRecoveryError::SplitBrainRejected(
                "witness attempted to vote for two proposal digests in one round".into(),
            ));
        }
        if votes.contains(&digest) {
            self.duplicate_vote_rejections += 1;
            return Ok(());
        }
        votes.insert(digest.clone());
        self.events
            .push(format!("vote:{}:{}", proposal.round_id, vote.witness_id));
        Ok(())
    }

    pub fn arbitrate(
        &mut self,
        proposal: &LeaderFailoverProposal,
    ) -> Result<MultiLeaderDecision, MultiLeaderRecoveryError> {
        self.accept_signed_proposal(proposal.clone())?;
        let digest = proposal.digest();
        let witness_ids: BTreeSet<String> = self
            .rounds
            .get(&proposal.round_id)
            .into_iter()
            .flat_map(|votes| votes.iter())
            .filter_map(|(witness_id, digests)| {
                digests.contains(&digest).then_some(witness_id.clone())
            })
            .collect();
        let quorum = self.witnesses.len() / 2 + 1;
        if witness_ids.len() < quorum {
            return Err(MultiLeaderRecoveryError::QuorumUnavailable(format!(
                "proposal has {}/{} witness votes",
                witness_ids.len(),
                quorum
            )));
        }
        let conflicting_quorum = self
            .rounds
            .get(&proposal.round_id)
            .into_iter()
            .flat_map(|votes| votes.values())
            .flat_map(|digests| digests.iter())
            .filter(|other| *other != &digest)
            .filter(|other| {
                self.rounds
                    .get(&proposal.round_id)
                    .into_iter()
                    .flat_map(|votes| votes.values())
                    .filter(|digests| digests.contains(*other))
                    .count()
                    >= quorum
            })
            .count();
        if conflicting_quorum > 0 {
            self.split_brain_rejections += 1;
            return Err(MultiLeaderRecoveryError::SplitBrainRejected(
                "more than one proposal has witness quorum".into(),
            ));
        }
        if proposal.replicated_log_index < self.committed_log_index {
            self.stale_proposal_rejections += 1;
            return Err(MultiLeaderRecoveryError::ProposalRejected(
                "winning proposal is behind the committed log".into(),
            ));
        }
        if let Some(decision) = &self.committed {
            if decision.round_id == proposal.round_id && decision.proposal_digest == digest {
                return Ok(decision.clone());
            }
            if self.external_fence.accepted_fence_epoch < self.last_fence_epoch {
                self.split_brain_rejections += 1;
                return Err(MultiLeaderRecoveryError::SplitBrainRejected(
                    "previous decision is not externally fenced yet".into(),
                ));
            }
            if decision.round_id > proposal.round_id
                || (decision.round_id == proposal.round_id && decision.proposal_digest != digest)
            {
                self.split_brain_rejections += 1;
                return Err(MultiLeaderRecoveryError::SplitBrainRejected(
                    "decision would regress or conflict with an existing round".into(),
                ));
            }
        }
        let fence_epoch = self.last_fence_epoch.saturating_add(1);
        let token = ExternalFencingToken::issue(
            &self.fencing_config,
            &self.authority_id,
            &proposal.candidate_region_id,
            proposal.owner_term,
            proposal.ownership_epoch,
            proposal.membership_epoch,
            fence_epoch,
            proposal.replicated_log_index,
            &self.authority_signing_key,
        )?;
        let decision = MultiLeaderDecision {
            round_id: proposal.round_id,
            proposal_digest: digest,
            candidate_region_id: proposal.candidate_region_id.clone(),
            winning_leader_id: proposal.leader_id.clone(),
            witness_ids,
            fencing_token: token,
        };
        self.committed = Some(decision.clone());
        self.committed_log_index = proposal.replicated_log_index;
        self.last_owner_term = proposal.owner_term;
        self.last_ownership_epoch = proposal.ownership_epoch;
        self.last_fence_epoch = fence_epoch;
        self.events.push(format!(
            "decision:{}:{}",
            proposal.round_id, proposal.leader_id
        ));
        Ok(decision)
    }

    pub fn admit_decision_externally(
        &mut self,
        decision: &MultiLeaderDecision,
    ) -> Result<ExternalFenceAction, MultiLeaderRecoveryError> {
        if self.committed.as_ref() != Some(decision) {
            return Err(MultiLeaderRecoveryError::SplitBrainRejected(
                "external fence admission must reference the exact current decision".into(),
            ));
        }
        let action = self
            .external_fence
            .apply_from_registry(
                decision.fencing_token.clone(),
                &self.registry,
                &self.config.cluster_id,
            )
            .map_err(|error| MultiLeaderRecoveryError::FencingTokenRejected(error.to_string()))?;
        self.active_leader_id = Some(decision.winning_leader_id.clone());
        self.active_region_id = Some(decision.candidate_region_id.clone());
        Ok(action)
    }

    pub fn registry(&self) -> &TrustedFencingAuthorityRegistry {
        &self.registry
    }

    pub fn report(&self) -> MultiLeaderReport {
        let safety_passed = self.active_region_id.is_none() || self.active_leader_id.is_some();
        MultiLeaderReport {
            cluster_id: self.config.cluster_id.clone(),
            resource_id: self.config.resource_id.clone(),
            leader_count: self.leaders.len(),
            witness_count: self.witnesses.len(),
            active_region_id: self.active_region_id.clone(),
            active_leader_id: self.active_leader_id.clone(),
            committed_round_id: self
                .committed
                .as_ref()
                .map_or(0, |decision| decision.round_id),
            accepted_fence_epoch: self.external_fence.accepted_fence_epoch,
            split_brain_rejections: self.split_brain_rejections,
            stale_proposal_rejections: self.stale_proposal_rejections,
            duplicate_vote_rejections: self.duplicate_vote_rejections,
            safety_passed,
            trace_digest: digest_json(&self.events).unwrap_or_default(),
        }
    }

    pub fn committed_decision(&self) -> Option<&MultiLeaderDecision> {
        self.committed.as_ref()
    }

    pub fn external_fence(&self) -> &ExternalFenceState {
        &self.external_fence
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MultiLeaderChaosFault {
    Drop,
    Delay { until_tick: u64 },
    Duplicate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MultiLeaderChaosDelivery {
    Delivered,
    Delayed,
    Dropped,
    DuplicateDelivered,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiLeaderChaosEvent {
    pub sequence: u64,
    pub tick: u64,
    pub source: String,
    pub destination: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultiLeaderChaosReport {
    pub leader_count: usize,
    pub witness_count: usize,
    pub partition_steps: usize,
    pub delivered_votes: usize,
    pub dropped_votes: usize,
    pub delayed_votes: usize,
    pub duplicate_votes: usize,
    pub split_brain_rejections: usize,
    pub safety_passed: bool,
    pub trace_digest: String,
}

#[derive(Debug)]
pub struct MultiLeaderChaosSimulator {
    authority: MultiLeaderFailoverAuthority,
    faults: BTreeMap<(String, String), MultiLeaderChaosFault>,
    tick: u64,
    sequence: u64,
    partition_steps: usize,
    delivered_votes: usize,
    dropped_votes: usize,
    delayed_votes: usize,
    duplicate_votes: usize,
    events: Vec<MultiLeaderChaosEvent>,
}

impl MultiLeaderChaosSimulator {
    pub fn new(authority: MultiLeaderFailoverAuthority) -> Self {
        Self {
            authority,
            faults: BTreeMap::new(),
            tick: 0,
            sequence: 0,
            partition_steps: 0,
            delivered_votes: 0,
            dropped_votes: 0,
            delayed_votes: 0,
            duplicate_votes: 0,
            events: Vec::new(),
        }
    }

    pub fn authority(&self) -> &MultiLeaderFailoverAuthority {
        &self.authority
    }

    pub fn authority_mut(&mut self) -> &mut MultiLeaderFailoverAuthority {
        &mut self.authority
    }

    pub fn partition(
        &mut self,
        source: &str,
        destination: &str,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.set_fault(source, destination, MultiLeaderChaosFault::Drop)?;
        self.partition_steps += 1;
        Ok(())
    }

    pub fn delay(
        &mut self,
        source: &str,
        destination: &str,
        until_tick: u64,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.set_fault(
            source,
            destination,
            MultiLeaderChaosFault::Delay { until_tick },
        )
    }

    pub fn duplicate(
        &mut self,
        source: &str,
        destination: &str,
    ) -> Result<(), MultiLeaderRecoveryError> {
        self.set_fault(source, destination, MultiLeaderChaosFault::Duplicate)
    }

    pub fn heal(&mut self, source: &str, destination: &str) {
        self.faults
            .remove(&(source.to_string(), destination.to_string()));
        self.record(source, destination, "healed");
    }

    pub fn advance_tick(&mut self, ticks: u64) {
        self.tick = self.tick.saturating_add(ticks);
        self.record("clock", "clock", &format!("advanced:{ticks}"));
    }

    pub fn deliver_vote(
        &mut self,
        leader_id: &str,
        witness_id: &str,
        proposal: &LeaderFailoverProposal,
        vote: WitnessVote,
    ) -> Result<MultiLeaderChaosDelivery, MultiLeaderRecoveryError> {
        match self
            .faults
            .get(&(leader_id.to_string(), witness_id.to_string()))
            .cloned()
        {
            Some(MultiLeaderChaosFault::Drop) => {
                self.dropped_votes += 1;
                self.record(leader_id, witness_id, "vote-dropped");
                Ok(MultiLeaderChaosDelivery::Dropped)
            }
            Some(MultiLeaderChaosFault::Delay { until_tick }) if self.tick < until_tick => {
                self.delayed_votes += 1;
                self.record(leader_id, witness_id, "vote-delayed");
                Ok(MultiLeaderChaosDelivery::Delayed)
            }
            Some(MultiLeaderChaosFault::Duplicate) => {
                self.authority.accept_vote(proposal, vote.clone())?;
                self.authority.accept_vote(proposal, vote)?;
                self.duplicate_votes += 1;
                self.delivered_votes += 1;
                self.record(leader_id, witness_id, "vote-duplicated");
                Ok(MultiLeaderChaosDelivery::DuplicateDelivered)
            }
            _ => {
                self.authority.accept_vote(proposal, vote)?;
                self.delivered_votes += 1;
                self.record(leader_id, witness_id, "vote-delivered");
                Ok(MultiLeaderChaosDelivery::Delivered)
            }
        }
    }

    pub fn report(&self) -> MultiLeaderChaosReport {
        let authority_report = self.authority.report();
        MultiLeaderChaosReport {
            leader_count: authority_report.leader_count,
            witness_count: authority_report.witness_count,
            partition_steps: self.partition_steps,
            delivered_votes: self.delivered_votes,
            dropped_votes: self.dropped_votes,
            delayed_votes: self.delayed_votes,
            duplicate_votes: self.duplicate_votes,
            split_brain_rejections: authority_report.split_brain_rejections,
            safety_passed: authority_report.safety_passed && self.events.len() <= MAX_CHAOS_EVENTS,
            trace_digest: digest_json(&self.events).unwrap_or_default(),
        }
    }

    pub fn events(&self) -> &[MultiLeaderChaosEvent] {
        &self.events
    }

    fn set_fault(
        &mut self,
        source: &str,
        destination: &str,
        fault: MultiLeaderChaosFault,
    ) -> Result<(), MultiLeaderRecoveryError> {
        validate_identifier(source, "source")?;
        validate_identifier(destination, "destination")?;
        if source == destination {
            return Err(MultiLeaderRecoveryError::InvalidInput(
                "fault endpoints must differ".into(),
            ));
        }
        self.faults
            .insert((source.to_string(), destination.to_string()), fault);
        self.record(source, destination, "fault-injected");
        Ok(())
    }

    fn record(&mut self, source: &str, destination: &str, detail: &str) {
        if self.events.len() >= MAX_CHAOS_EVENTS {
            return;
        }
        self.sequence = self.sequence.saturating_add(1);
        self.events.push(MultiLeaderChaosEvent {
            sequence: self.sequence,
            tick: self.tick,
            source: source.to_string(),
            destination: destination.to_string(),
            detail: detail.to_string(),
        });
    }
}

fn validate_identifier(value: &str, label: &str) -> Result<(), MultiLeaderRecoveryError> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(MultiLeaderRecoveryError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), MultiLeaderRecoveryError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(MultiLeaderRecoveryError::InvalidInput(format!(
            "{label} must be a 64-character hexadecimal digest"
        )));
    }
    Ok(())
}

pub(crate) fn digest_json<T: Serialize>(value: &T) -> Result<String, MultiLeaderRecoveryError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| MultiLeaderRecoveryError::InvalidInput(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}
