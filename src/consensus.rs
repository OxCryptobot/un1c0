//! Bounded, transport-agnostic consensus and state replication contracts.
//!
//! This module deliberately owns no sockets or background threads. Callers transport
//! the typed messages through an approved channel, while this state machine enforces
//! membership, terms, quorum commit, bounded logs, and deterministic state hashes.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_MEMBERS: usize = 256;
const MAX_BATCH_ENTRIES: usize = 256;
const MAX_KEY_BYTES: usize = 4 * 1024;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_LOG_ENTRIES: usize = 100_000;
const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NONCE_BYTES: usize = 128;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConsensusError {
    #[error("invalid consensus node: {0}")]
    InvalidNode(String),
    #[error("invalid consensus cluster: {0}")]
    InvalidCluster(String),
    #[error("consensus node is not the leader")]
    NotLeader,
    #[error("consensus message is from an unknown member: {0}")]
    UnknownMember(String),
    #[error("consensus term overflow")]
    TermOverflow,
    #[error("consensus log limit reached")]
    LogLimitReached,
    #[error("consensus log conflict: {0}")]
    LogConflict(String),
    #[error("invalid consensus message: {0}")]
    InvalidMessage(String),
    #[error("consensus serialization failed: {0}")]
    Serialization(String),
    #[error("invalid consensus snapshot: {0}")]
    InvalidSnapshot(String),
    #[error("consensus snapshot persistence failed: {0}")]
    SnapshotPersistence(String),
    #[error("consensus message authentication failed: {0}")]
    Unauthenticated(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusRole {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StateCommand {
    Set { key: String, value: String },
    Delete { key: String },
}

impl StateCommand {
    fn validate(&self) -> Result<(), ConsensusError> {
        match self {
            Self::Set { key, value } => {
                validate_key(key)?;
                if value.len() > MAX_VALUE_BYTES {
                    return Err(ConsensusError::InvalidMessage(format!(
                        "state value exceeds {} bytes",
                        MAX_VALUE_BYTES
                    )));
                }
            }
            Self::Delete { key } => validate_key(key)?,
        }
        Ok(())
    }

    fn apply(&self, state: &mut BTreeMap<String, String>) {
        match self {
            Self::Set { key, value } => {
                state.insert(key.clone(), value.clone());
            }
            Self::Delete { key } => {
                state.remove(key);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub command: StateCommand,
    pub command_hash: String,
}

impl LogEntry {
    fn new(index: u64, term: u64, command: StateCommand) -> Result<Self, ConsensusError> {
        command.validate()?;
        let command_hash = digest_json(&command)?;
        Ok(Self {
            index,
            term,
            command,
            command_hash,
        })
    }

    fn validate(&self, expected_index: u64) -> Result<(), ConsensusError> {
        if self.index != expected_index {
            return Err(ConsensusError::LogConflict(format!(
                "expected log index {}, received {}",
                expected_index, self.index
            )));
        }
        if self.term == 0 {
            return Err(ConsensusError::InvalidMessage(
                "log entry term must be positive".into(),
            ));
        }
        let expected_hash = digest_json(&self.command)?;
        if expected_hash != self.command_hash {
            return Err(ConsensusError::LogConflict(format!(
                "command hash mismatch at index {}",
                self.index
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoteRequest {
    pub term: u64,
    pub candidate_id: String,
    pub last_log_index: u64,
    pub last_log_term: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoteResponse {
    pub term: u64,
    pub voter_id: String,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendEntries {
    pub term: u64,
    pub leader_id: String,
    pub prev_log_index: u64,
    pub prev_log_term: u64,
    pub entries: Vec<LogEntry>,
    pub leader_commit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendResponse {
    pub term: u64,
    pub follower_id: String,
    pub success: bool,
    pub match_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusMessage {
    VoteRequest(VoteRequest),
    VoteResponse(VoteResponse),
    AppendEntries(AppendEntries),
    AppendResponse(AppendResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicatedSnapshot {
    pub term: u64,
    pub commit_index: u64,
    pub last_applied: u64,
    pub state: BTreeMap<String, String>,
    pub state_hash: String,
}

impl ReplicatedSnapshot {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.term == 0 || self.last_applied > self.commit_index {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot term must be positive and last_applied cannot exceed commit_index".into(),
            ));
        }
        let expected = digest_json(&self.state)?;
        if expected != self.state_hash {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot state hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedConsensusEnvelope {
    pub sender_id: String,
    pub term: u64,
    pub nonce: String,
    pub message: ConsensusMessage,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl AuthenticatedConsensusEnvelope {
    pub fn sign(
        sender_id: &str,
        term: u64,
        nonce: &str,
        message: ConsensusMessage,
        signing_key: &SigningKey,
    ) -> Result<Self, ConsensusError> {
        validate_node_id(sender_id)?;
        validate_nonce(nonce)?;
        if term == 0 || message_term(&message) != term {
            return Err(ConsensusError::Unauthenticated(
                "envelope term must be positive and match the message term".into(),
            ));
        }
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut envelope = Self {
            sender_id: sender_id.to_string(),
            term,
            nonce: nonce.to_string(),
            message,
            public_key,
            signature: Vec::new(),
        };
        let payload = envelope.payload()?;
        envelope.signature = signing_key.sign(&payload).to_bytes().to_vec();
        Ok(envelope)
    }

    pub fn verify(&self, expected_sender_id: &str, trusted_key: &[u8]) -> Result<(), ConsensusError> {
        validate_node_id(expected_sender_id)?;
        validate_node_id(&self.sender_id)?;
        validate_nonce(&self.nonce)?;
        if self.sender_id != expected_sender_id || self.term == 0 || message_term(&self.message) != self.term {
            return Err(ConsensusError::Unauthenticated(
                "sender identity or term binding mismatch".into(),
            ));
        }
        if trusted_key != self.public_key.as_slice() {
            return Err(ConsensusError::Unauthenticated(
                "sender public key is not bound to the trusted identity".into(),
            ));
        }
        let key: [u8; 32] = self
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| ConsensusError::Unauthenticated("public key must be 32 bytes".into()))?;
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ConsensusError::Unauthenticated("signature must be 64 bytes".into()))?;
        let verifying_key = VerifyingKey::from_bytes(&key)
            .map_err(|_| ConsensusError::Unauthenticated("invalid public key".into()))?;
        verifying_key
            .verify(&self.payload()?, &Signature::from_bytes(&signature))
            .map_err(|_| ConsensusError::Unauthenticated("invalid consensus signature".into()))
    }

    fn payload(&self) -> Result<Vec<u8>, ConsensusError> {
        serde_json::to_vec(&(
            &self.sender_id,
            self.term,
            &self.nonce,
            &self.message,
            &self.public_key,
        ))
        .map_err(|error| ConsensusError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct DurableSnapshotStore {
    path: PathBuf,
}

impl DurableSnapshotStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn save(&self, snapshot: &ReplicatedSnapshot) -> Result<(), ConsensusError> {
        snapshot.validate()?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot exceeds the 16 MiB bound".into(),
            ));
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?;
        let temporary = parent.join(format!(
            ".{}.tmp",
            self.path.file_name().and_then(|name| name.to_str()).unwrap_or("snapshot")
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn load(&self) -> Result<ReplicatedSnapshot, ConsensusError> {
        let metadata = fs::metadata(&self.path)
            .map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?;
        if metadata.len() > MAX_SNAPSHOT_BYTES {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot exceeds the 16 MiB bound".into(),
            ));
        }
        let snapshot: ReplicatedSnapshot = serde_json::from_slice(
            &fs::read(&self.path)
                .map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        snapshot.validate()?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone)]
pub struct ConsensusNode {
    id: String,
    members: BTreeSet<String>,
    max_log_entries: usize,
    role: ConsensusRole,
    current_term: u64,
    voted_for: Option<String>,
    log: Vec<LogEntry>,
    commit_index: u64,
    last_applied: u64,
    state: BTreeMap<String, String>,
    votes_received: BTreeSet<String>,
    replication_progress: BTreeMap<String, u64>,
}

impl ConsensusNode {
    pub fn new(
        id: &str,
        members: BTreeSet<String>,
        max_log_entries: usize,
    ) -> Result<Self, ConsensusError> {
        validate_node_id(id)?;
        if members.is_empty() || members.len() > MAX_MEMBERS {
            return Err(ConsensusError::InvalidCluster(format!(
                "cluster must contain 1 to {} members",
                MAX_MEMBERS
            )));
        }
        if !members.contains(id) {
            return Err(ConsensusError::InvalidCluster(format!(
                "node '{}' is not a cluster member",
                id
            )));
        }
        if max_log_entries == 0 || max_log_entries > MAX_LOG_ENTRIES {
            return Err(ConsensusError::InvalidCluster(format!(
                "max log entries must be between 1 and {}",
                MAX_LOG_ENTRIES
            )));
        }
        Ok(Self {
            id: id.to_string(),
            members,
            max_log_entries,
            role: ConsensusRole::Follower,
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            state: BTreeMap::new(),
            votes_received: BTreeSet::new(),
            replication_progress: BTreeMap::new(),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn role(&self) -> ConsensusRole {
        self.role
    }

    pub fn current_term(&self) -> u64 {
        self.current_term
    }

    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }

    pub fn log_len(&self) -> usize {
        self.log.len()
    }

    pub fn quorum_size(&self) -> usize {
        self.members.len() / 2 + 1
    }

    pub fn state_value(&self, key: &str) -> Option<&str> {
        self.state.get(key).map(String::as_str)
    }

    pub fn snapshot(&self) -> Result<ReplicatedSnapshot, ConsensusError> {
        let snapshot = ReplicatedSnapshot {
            term: self.current_term,
            commit_index: self.commit_index,
            last_applied: self.last_applied,
            state: self.state.clone(),
            state_hash: digest_json(&self.state)?,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn install_snapshot(&mut self, snapshot: ReplicatedSnapshot) -> Result<(), ConsensusError> {
        snapshot.validate()?;
        if snapshot.term < self.current_term || snapshot.commit_index < self.commit_index {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot is older than the node commit state".into(),
            ));
        }
        if snapshot.last_applied > self.log.len() as u64 && !self.log.is_empty() {
            return Err(ConsensusError::InvalidSnapshot(
                "snapshot last_applied exceeds the local log frontier".into(),
            ));
        }
        self.current_term = snapshot.term;
        self.role = ConsensusRole::Follower;
        self.voted_for = None;
        self.votes_received.clear();
        self.commit_index = snapshot.commit_index;
        self.last_applied = snapshot.last_applied;
        self.state = snapshot.state;
        self.log.clear();
        self.replication_progress.clear();
        Ok(())
    }

    pub fn start_election(&mut self) -> Result<VoteRequest, ConsensusError> {
        self.current_term = self
            .current_term
            .checked_add(1)
            .ok_or(ConsensusError::TermOverflow)?;
        self.role = ConsensusRole::Candidate;
        self.voted_for = Some(self.id.clone());
        self.votes_received.clear();
        self.votes_received.insert(self.id.clone());
        let (last_log_index, last_log_term) = self.last_log_position();
        Ok(VoteRequest {
            term: self.current_term,
            candidate_id: self.id.clone(),
            last_log_index,
            last_log_term,
        })
    }

    pub fn handle_vote_request(
        &mut self,
        request: VoteRequest,
    ) -> Result<VoteResponse, ConsensusError> {
        validate_node_id(&request.candidate_id)?;
        if !self.members.contains(&request.candidate_id) {
            return Err(ConsensusError::UnknownMember(request.candidate_id));
        }
        if request.term > self.current_term {
            self.current_term = request.term;
            self.role = ConsensusRole::Follower;
            self.voted_for = None;
            self.votes_received.clear();
        }
        let mut granted = false;
        if request.term == self.current_term {
            let (last_log_index, last_log_term) = self.last_log_position();
            let up_to_date = request.last_log_term > last_log_term
                || (request.last_log_term == last_log_term && request.last_log_index >= last_log_index);
            if up_to_date
                && (self.voted_for.is_none()
                    || self.voted_for.as_deref() == Some(request.candidate_id.as_str()))
            {
                self.voted_for = Some(request.candidate_id.clone());
                self.role = ConsensusRole::Follower;
                granted = true;
            }
        }
        Ok(VoteResponse {
            term: self.current_term,
            voter_id: self.id.clone(),
            granted,
        })
    }

    pub fn receive_vote_response(&mut self, response: VoteResponse) -> Result<bool, ConsensusError> {
        validate_node_id(&response.voter_id)?;
        if !self.members.contains(&response.voter_id) {
            return Err(ConsensusError::UnknownMember(response.voter_id));
        }
        if response.term > self.current_term {
            self.current_term = response.term;
            self.role = ConsensusRole::Follower;
            self.voted_for = None;
            self.votes_received.clear();
            return Ok(false);
        }
        if self.role != ConsensusRole::Candidate || response.term != self.current_term {
            return Ok(false);
        }
        if response.granted {
            self.votes_received.insert(response.voter_id);
        }
        if self.votes_received.len() >= self.quorum_size() {
            self.role = ConsensusRole::Leader;
            self.voted_for = Some(self.id.clone());
            self.replication_progress.clear();
            self.replication_progress
                .insert(self.id.clone(), self.log.len() as u64);
            for member in &self.members {
                if member != &self.id {
                    self.replication_progress.insert(member.clone(), 0);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub fn propose(&mut self, command: StateCommand) -> Result<LogEntry, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        if self.log.len() >= self.max_log_entries {
            return Err(ConsensusError::LogLimitReached);
        }
        let index = self.log.len() as u64 + 1;
        let entry = LogEntry::new(index, self.current_term, command)?;
        self.log.push(entry.clone());
        self.replication_progress.insert(self.id.clone(), index);
        self.advance_commit_index()?;
        Ok(entry)
    }

    pub fn append_entries_for(&self, follower_id: &str) -> Result<AppendEntries, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        validate_node_id(follower_id)?;
        if !self.members.contains(follower_id) {
            return Err(ConsensusError::UnknownMember(follower_id.to_string()));
        }
        if follower_id == self.id {
            return Err(ConsensusError::InvalidMessage(
                "leader cannot replicate to itself".into(),
            ));
        }
        let next_index = self
            .replication_progress
            .get(follower_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        let prev_log_index = next_index.saturating_sub(1);
        let prev_log_term = if prev_log_index == 0 {
            0
        } else {
            self.log
                .get(prev_log_index as usize - 1)
                .map(|entry| entry.term)
                .unwrap_or_default()
        };
        let start = next_index.saturating_sub(1) as usize;
        let entries = self
            .log
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take(MAX_BATCH_ENTRIES)
            .cloned()
            .collect();
        Ok(AppendEntries {
            term: self.current_term,
            leader_id: self.id.clone(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: self.commit_index,
        })
    }

    pub fn handle_append_entries(
        &mut self,
        request: AppendEntries,
    ) -> Result<AppendResponse, ConsensusError> {
        validate_node_id(&request.leader_id)?;
        if !self.members.contains(&request.leader_id) {
            return Err(ConsensusError::UnknownMember(request.leader_id));
        }
        if request.term < self.current_term {
            return Ok(AppendResponse {
                term: self.current_term,
                follower_id: self.id.clone(),
                success: false,
                match_index: self.log.len() as u64,
            });
        }
        if request.term > self.current_term {
            self.current_term = request.term;
            self.voted_for = None;
        }
        self.role = ConsensusRole::Follower;
        self.votes_received.clear();
        if request.prev_log_index > self.log.len() as u64 {
            return Ok(AppendResponse {
                term: self.current_term,
                follower_id: self.id.clone(),
                success: false,
                match_index: self.log.len() as u64,
            });
        }
        if request.prev_log_index > 0 {
            let previous = &self.log[request.prev_log_index as usize - 1];
            if previous.term != request.prev_log_term {
                return Ok(AppendResponse {
                    term: self.current_term,
                    follower_id: self.id.clone(),
                    success: false,
                    match_index: request.prev_log_index.saturating_sub(1),
                });
            }
        }
        let mut expected_index = request.prev_log_index + 1;
        for entry in request.entries {
            entry.validate(expected_index)?;
            if expected_index as usize <= self.log.len() {
                let existing = &self.log[expected_index as usize - 1];
                if existing.term != entry.term || existing.command_hash != entry.command_hash {
                    self.log.truncate(expected_index as usize - 1);
                }
            }
            if expected_index as usize > self.log.len() {
                if self.log.len() >= self.max_log_entries {
                    return Err(ConsensusError::LogLimitReached);
                }
                self.log.push(entry);
            }
            expected_index += 1;
        }
        self.commit_index = request.leader_commit.min(self.log.len() as u64);
        self.apply_committed();
        Ok(AppendResponse {
            term: self.current_term,
            follower_id: self.id.clone(),
            success: true,
            match_index: expected_index.saturating_sub(1).max(request.prev_log_index),
        })
    }

    pub fn acknowledge_append(&mut self, response: AppendResponse) -> Result<bool, ConsensusError> {
        validate_node_id(&response.follower_id)?;
        if !self.members.contains(&response.follower_id) {
            return Err(ConsensusError::UnknownMember(response.follower_id));
        }
        if response.term > self.current_term {
            self.current_term = response.term;
            self.role = ConsensusRole::Follower;
            self.voted_for = None;
            return Ok(false);
        }
        if self.role != ConsensusRole::Leader || response.term != self.current_term {
            return Ok(false);
        }
        if response.success {
            let progress = self
                .replication_progress
                .entry(response.follower_id)
                .or_default();
            *progress = (*progress).max(response.match_index.min(self.log.len() as u64));
        } else if let Some(progress) = self.replication_progress.get_mut(&response.follower_id) {
            *progress = progress.saturating_sub(1);
        }
        self.advance_commit_index()
    }

    fn advance_commit_index(&mut self) -> Result<bool, ConsensusError> {
        let mut changed = false;
        for index in (self.commit_index + 1)..=(self.log.len() as u64) {
            let replicated = self
                .replication_progress
                .values()
                .filter(|match_index| **match_index >= index)
                .count();
            let current_term_entry = self
                .log
                .get(index as usize - 1)
                .map(|entry| entry.term == self.current_term)
                .unwrap_or(false);
            if replicated >= self.quorum_size() && current_term_entry {
                self.commit_index = index;
                changed = true;
            }
        }
        self.apply_committed();
        Ok(changed)
    }

    fn apply_committed(&mut self) {
        while self.last_applied < self.commit_index {
            let index = self.last_applied as usize;
            if let Some(entry) = self.log.get(index) {
                entry.command.apply(&mut self.state);
                self.last_applied += 1;
            } else {
                break;
            }
        }
    }

    fn last_log_position(&self) -> (u64, u64) {
        self.log
            .last()
            .map(|entry| (entry.index, entry.term))
            .unwrap_or((0, 0))
    }
}

fn message_term(message: &ConsensusMessage) -> u64 {
    match message {
        ConsensusMessage::VoteRequest(value) => value.term,
        ConsensusMessage::VoteResponse(value) => value.term,
        ConsensusMessage::AppendEntries(value) => value.term,
        ConsensusMessage::AppendResponse(value) => value.term,
    }
}

fn validate_nonce(nonce: &str) -> Result<(), ConsensusError> {
    if nonce.trim().is_empty() || nonce.len() > MAX_NONCE_BYTES || nonce.chars().any(char::is_control) {
        return Err(ConsensusError::Unauthenticated(
            "consensus nonce must be bounded and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn validate_node_id(id: &str) -> Result<(), ConsensusError> {
    if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err(ConsensusError::InvalidNode(
            "node id must be 1 to 256 bytes and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), ConsensusError> {
    if key.trim().is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
        return Err(ConsensusError::InvalidMessage(format!(
            "state key must be 1 to {} bytes and contain no control characters",
            MAX_KEY_BYTES
        )));
    }
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, ConsensusError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{:02x}", byte)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> BTreeSet<String> {
        ["node-a", "node-b", "node-c"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn election_requires_quorum_and_leader_replicates_committed_state() {
        let mut leader = ConsensusNode::new("node-a", members(), 16).unwrap();
        let mut follower_b = ConsensusNode::new("node-b", members(), 16).unwrap();
        let mut follower_c = ConsensusNode::new("node-c", members(), 16).unwrap();
        let request = leader.start_election().unwrap();
        let vote = follower_b.handle_vote_request(request).unwrap();
        assert!(vote.granted);
        assert!(leader.receive_vote_response(vote).unwrap());
        assert_eq!(leader.role(), ConsensusRole::Leader);

        leader
            .propose(StateCommand::Set {
                key: "feature/mcp".into(),
                value: "enabled".into(),
            })
            .unwrap();
        assert_eq!(leader.commit_index(), 0);
        let append_b = leader.append_entries_for("node-b").unwrap();
        let response_b = follower_b.handle_append_entries(append_b).unwrap();
        assert!(leader.acknowledge_append(response_b).unwrap());
        assert_eq!(leader.commit_index(), 1);
        assert_eq!(leader.state_value("feature/mcp"), Some("enabled"));

        let append_c = leader.append_entries_for("node-c").unwrap();
        let response_c = follower_c.handle_append_entries(append_c).unwrap();
        assert!(response_c.success);
        let commit_notice = leader.append_entries_for("node-c").unwrap();
        follower_c.handle_append_entries(commit_notice).unwrap();
        assert_eq!(follower_c.state_value("feature/mcp"), Some("enabled"));
        assert_eq!(leader.snapshot().unwrap().state_hash, follower_c.snapshot().unwrap().state_hash);
    }

    #[test]
    fn stale_terms_conflicts_and_unbounded_commands_fail_closed() {
        let mut leader = ConsensusNode::new("node-a", members(), 1).unwrap();
        let mut follower = ConsensusNode::new("node-b", members(), 1).unwrap();
        let request = leader.start_election().unwrap();
        let vote = follower.handle_vote_request(request.clone()).unwrap();
        leader.receive_vote_response(vote).unwrap();
        let stale = AppendEntries {
            term: 0,
            leader_id: "node-a".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        };
        assert!(!follower.handle_append_entries(stale).unwrap().success);
        let too_large = StateCommand::Set {
            key: "k".into(),
            value: "x".repeat(MAX_VALUE_BYTES + 1),
        };
        assert!(matches!(
            leader.propose(too_large),
            Err(ConsensusError::InvalidMessage(_))
        ));
        leader
            .propose(StateCommand::Set {
                key: "k".into(),
                value: "v".into(),
            })
            .unwrap();
        assert!(matches!(
            leader.propose(StateCommand::Delete { key: "k".into() }),
            Err(ConsensusError::LogLimitReached)
        ));
    }
}
