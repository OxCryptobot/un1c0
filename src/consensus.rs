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
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_MEMBERS: usize = 256;
const MAX_BATCH_ENTRIES: usize = 256;
const MAX_KEY_BYTES: usize = 4 * 1024;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_LOG_ENTRIES: usize = 100_000;
const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NONCE_BYTES: usize = 128;
const MAX_CLUSTER_ID_BYTES: usize = 128;
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_CHUNK_BYTES: usize = 64 * 1024;
const MAX_SYNC_CHUNKS: usize = 256;
const MAX_READ_ROUNDS: usize = 1_024;
const MAX_COMPLETED_READ_REQUESTS: usize = 4_096;
const MAX_LEASE_TICKS: u64 = 86_400_000;
const MAX_ELECTION_TICKS: u64 = 86_400_000;
const MAX_REPLICATION_BATCH_BYTES: usize = 512 * 1024;
const DEFAULT_CLUSTER_ID: &str = "legacy-unbound";
const MAX_COMPACTION_DISCARD_ENTRIES: usize = MAX_BATCH_ENTRIES;
const MAX_RETAINED_LOG_ENTRIES: usize = MAX_LOG_ENTRIES;
const MAX_COMPACTION_MANIFEST_BYTES: u64 = 128 * 1024;
const MAX_SNAPSHOT_BANDWIDTH_BYTES: u64 = MAX_SNAPSHOT_BYTES;
const MAX_SNAPSHOT_BANDWIDTH_WINDOW_TICKS: u64 = MAX_ELECTION_TICKS;
const MAX_DURABLE_CONSENSUS_STATE_BYTES: u64 = 128 * 1024;
const MAX_SOCKET_QUOTA_BYTES: u64 = 16 * MAX_FRAME_BYTES as u64;
const MAX_DURABLE_SOCKET_QUEUE_ENTRIES: usize = 256;
const MAX_DURABLE_SOCKET_QUEUE_BYTES: u64 = 16 * 1024 * 1024;

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
    #[error("invalid membership change: {0}")]
    InvalidMembershipChange(String),
    #[error("membership change is not in progress")]
    NoMembershipChange,
    #[error("membership change is already in progress")]
    MembershipChangeInProgress,
    #[error("invalid cluster configuration: {0}")]
    InvalidClusterConfiguration(String),
    #[error("consensus replay detected for nonce")]
    ReplayDetected,
    #[error("consensus transport failed: {0}")]
    Transport(String),
    #[error("consensus frame exceeds the configured bound")]
    FrameTooLarge,
    #[error("invalid snapshot chunk: {0}")]
    InvalidSnapshotChunk(String),
    #[error("snapshot transfer is incomplete")]
    SnapshotTransferIncomplete,
    #[error("incremental state synchronization conflict: {0}")]
    IncrementalSyncConflict(String),
    #[error("invalid leader lease configuration: {0}")]
    InvalidLeaderLease(String),
    #[error("invalid linearizable read request: {0}")]
    InvalidReadRequest(String),
    #[error("read-index round is unknown: {0}")]
    UnknownReadIndex(String),
    #[error("read-index request was already completed: {0}")]
    DuplicateReadRequest(String),
    #[error("linearizable read is not ready: {0}")]
    ReadNotReady(String),
    #[error("leader lease has expired or is unsafe")]
    LeaseExpired,
    #[error("monotonic clock safety is uncertain")]
    ClockUntrusted,
    #[error("invalid election timer configuration: {0}")]
    InvalidElectionTimer(String),
    #[error("peer is not an accepted consensus member: {0}")]
    InvalidPeer(String),
    #[error("replication flow-control violation: {0}")]
    ReplicationFlowControl(String),
    #[error("log compaction violation: {0}")]
    LogCompaction(String),
    #[error("replication requires a snapshot: {0}")]
    SnapshotRequired(String),
    #[error("invalid compaction manifest: {0}")]
    InvalidCompactionManifest(String),
    #[error("compaction persistence failed: {0}")]
    CompactionPersistence(String),
    #[error("invalid snapshot acknowledgement: {0}")]
    InvalidSnapshotAcknowledgement(String),
    #[error("snapshot bandwidth violation: {0}")]
    SnapshotBandwidth(String),
    #[error("snapshot cancellation violation: {0}")]
    SnapshotCancellation(String),
    #[error("durable consensus state violation: {0}")]
    DurableConsensusState(String),
    #[error("authenticated replay epoch mismatch: expected {expected}, received {received}")]
    ReplayEpochMismatch { expected: u64, received: u64 },
    #[error("authenticated envelope term is below the replay floor")]
    StaleReplayTerm,
    #[error("compaction coordination violation: {0}")]
    CompactionCoordination(String),
    #[error("snapshot request violation: {0}")]
    SnapshotRequest(String),
    #[error("socket quota violation: {0}")]
    SocketQuota(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusRole {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StateCommand {
    Set {
        key: String,
        value: String,
    },
    Delete {
        key: String,
    },
    ConfigurationJoint {
        old_members: BTreeSet<String>,
        new_members: BTreeSet<String>,
    },
    ConfigurationFinal {
        members: BTreeSet<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigurationPhase {
    Stable,
    Joint,
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
            Self::ConfigurationJoint {
                old_members,
                new_members,
            } => {
                validate_members(old_members)?;
                validate_members(new_members)?;
            }
            Self::ConfigurationFinal { members } => validate_members(members)?,
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
            Self::ConfigurationJoint { .. } | Self::ConfigurationFinal { .. } => {}
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationFlowConfig {
    pub max_entries_per_batch: usize,
    pub max_batch_bytes: usize,
    pub retry_backoff_ticks: u64,
}

impl Default for ReplicationFlowConfig {
    fn default() -> Self {
        Self {
            max_entries_per_batch: MAX_BATCH_ENTRIES,
            max_batch_bytes: MAX_REPLICATION_BATCH_BYTES,
            retry_backoff_ticks: 25,
        }
    }
}

impl ReplicationFlowConfig {
    pub fn new(
        max_entries_per_batch: usize,
        max_batch_bytes: usize,
        retry_backoff_ticks: u64,
    ) -> Result<Self, ConsensusError> {
        let config = Self {
            max_entries_per_batch,
            max_batch_bytes,
            retry_backoff_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.max_entries_per_batch == 0 || self.max_entries_per_batch > MAX_BATCH_ENTRIES {
            return Err(ConsensusError::ReplicationFlowControl(
                "entries per batch must be between 1 and MAX_BATCH_ENTRIES".into(),
            ));
        }
        if self.max_batch_bytes == 0 || self.max_batch_bytes > MAX_REPLICATION_BATCH_BYTES {
            return Err(ConsensusError::ReplicationFlowControl(
                "batch bytes must be positive and bounded".into(),
            ));
        }
        if self.retry_backoff_ticks == 0 || self.retry_backoff_ticks > MAX_ELECTION_TICKS {
            return Err(ConsensusError::ReplicationFlowControl(
                "retry backoff must be positive and bounded".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationBatch {
    pub batch_id: u64,
    pub term: u64,
    pub leader_id: String,
    pub follower_id: String,
    pub request: AppendEntries,
}

impl ReplicationBatch {
    pub fn validate(&self, config: &ReplicationFlowConfig) -> Result<(), ConsensusError> {
        config.validate()?;
        validate_node_id(&self.leader_id)?;
        validate_node_id(&self.follower_id)?;
        if self.batch_id == 0 || self.term == 0 {
            return Err(ConsensusError::ReplicationFlowControl(
                "batch ID and term must be positive".into(),
            ));
        }
        if self.request.term != self.term
            || self.request.leader_id != self.leader_id
            || self.request.entries.len() > config.max_entries_per_batch
        {
            return Err(ConsensusError::ReplicationFlowControl(
                "batch metadata does not match bounded append request".into(),
            ));
        }
        if self.request.entries.is_empty() {
            return Err(ConsensusError::ReplicationFlowControl(
                "replication batches must contain at least one entry".into(),
            ));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() > config.max_batch_bytes {
            return Err(ConsensusError::ReplicationFlowControl(
                "serialized replication batch exceeds byte bound".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationBatchAck {
    pub batch_id: u64,
    pub follower_id: String,
    pub response: AppendResponse,
}

impl ReplicationBatchAck {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_node_id(&self.follower_id)?;
        if self.batch_id == 0
            || self.response.term == 0
            || self.response.follower_id != self.follower_id
        {
            return Err(ConsensusError::ReplicationFlowControl(
                "acknowledgement metadata is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplicationFlowAction {
    Idle,
    Backpressured { retry_at_tick: Option<u64> },
    Send(ReplicationBatch),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationWindowStatus {
    pub follower_id: String,
    pub in_flight_batch_id: Option<u64>,
    pub last_completed_batch_id: Option<u64>,
    pub retry_at_tick: Option<u64>,
    pub sent_batches: u64,
    pub acknowledged_batches: u64,
    pub rejected_batches: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaderLeaseConfig {
    pub lease_ticks: u64,
    pub max_clock_drift_ticks: u64,
}

impl Default for LeaderLeaseConfig {
    fn default() -> Self {
        Self {
            lease_ticks: 1_000,
            max_clock_drift_ticks: 10,
        }
    }
}

impl LeaderLeaseConfig {
    pub fn new(lease_ticks: u64, max_clock_drift_ticks: u64) -> Result<Self, ConsensusError> {
        let config = Self {
            lease_ticks,
            max_clock_drift_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.lease_ticks == 0 || self.lease_ticks > MAX_LEASE_TICKS {
            return Err(ConsensusError::InvalidLeaderLease(format!(
                "lease ticks must be between 1 and {}",
                MAX_LEASE_TICKS
            )));
        }
        if self.max_clock_drift_ticks >= self.lease_ticks {
            return Err(ConsensusError::InvalidLeaderLease(
                "clock drift must be strictly less than the lease duration".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadIndexRequest {
    pub request_id: String,
    pub term: u64,
    pub leader_id: String,
    pub read_index: u64,
}

impl ReadIndexRequest {
    pub fn new(
        request_id: &str,
        term: u64,
        leader_id: &str,
        read_index: u64,
    ) -> Result<Self, ConsensusError> {
        let request = Self {
            request_id: request_id.to_string(),
            term,
            leader_id: leader_id.to_string(),
            read_index,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_read_request_id(&self.request_id)?;
        validate_node_id(&self.leader_id)?;
        if self.term == 0 {
            return Err(ConsensusError::InvalidReadRequest(
                "read-index term must be positive".into(),
            ));
        }
        if self.read_index > MAX_LOG_ENTRIES as u64 {
            return Err(ConsensusError::InvalidReadRequest(
                "read-index exceeds the bounded log frontier".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadIndexResponse {
    pub request_id: String,
    pub term: u64,
    pub follower_id: String,
    pub read_index: u64,
    pub accepted: bool,
}

impl ReadIndexResponse {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_read_request_id(&self.request_id)?;
        validate_node_id(&self.follower_id)?;
        if self.term == 0 {
            return Err(ConsensusError::InvalidReadRequest(
                "read-index response term must be positive".into(),
            ));
        }
        if self.read_index > MAX_LOG_ENTRIES as u64 {
            return Err(ConsensusError::InvalidReadRequest(
                "read-index response exceeds the bounded log frontier".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusMessage {
    VoteRequest(VoteRequest),
    VoteResponse(VoteResponse),
    AppendEntries(AppendEntries),
    AppendResponse(AppendResponse),
    SnapshotManifest(SnapshotManifest),
    SnapshotChunk(SnapshotChunk),
    StateDelta(StateDelta),
    ReadIndexRequest(ReadIndexRequest),
    ReadIndexResponse(ReadIndexResponse),
    ReplicationBatch(ReplicationBatch),
    ReplicationBatchAck(ReplicationBatchAck),
    SnapshotInstallAck(SnapshotInstallAck),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinearizableReadRequest {
    pub request_id: String,
    pub key: String,
    pub now_tick: u64,
}

impl LinearizableReadRequest {
    pub fn new(request_id: &str, key: &str, now_tick: u64) -> Result<Self, ConsensusError> {
        let request = Self {
            request_id: request_id.to_string(),
            key: key.to_string(),
            now_tick,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_read_request_id(&self.request_id)?;
        validate_key(&self.key).map_err(|error| {
            ConsensusError::InvalidReadRequest(format!("invalid query key: {}", error))
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinearizableReadPlan {
    pub request_id: String,
    pub key: String,
    pub term: u64,
    pub read_index: u64,
    pub lease_fast_path: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadIndexAction {
    Lease(LinearizableReadPlan),
    Quorum(ReadIndexRequest),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ElectionTimerConfig {
    pub election_timeout_ticks: u64,
    pub election_jitter_ticks: u64,
    pub heartbeat_interval_ticks: u64,
    pub failure_detector_ticks: u64,
}

impl Default for ElectionTimerConfig {
    fn default() -> Self {
        Self {
            election_timeout_ticks: 150,
            election_jitter_ticks: 50,
            heartbeat_interval_ticks: 50,
            failure_detector_ticks: 300,
        }
    }
}

impl ElectionTimerConfig {
    pub fn new(
        election_timeout_ticks: u64,
        election_jitter_ticks: u64,
        heartbeat_interval_ticks: u64,
        failure_detector_ticks: u64,
    ) -> Result<Self, ConsensusError> {
        let config = Self {
            election_timeout_ticks,
            election_jitter_ticks,
            heartbeat_interval_ticks,
            failure_detector_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.election_timeout_ticks == 0
            || self.election_timeout_ticks > MAX_ELECTION_TICKS
            || self.heartbeat_interval_ticks == 0
            || self.failure_detector_ticks == 0
        {
            return Err(ConsensusError::InvalidElectionTimer(
                "timer values must be positive and bounded".into(),
            ));
        }
        if self.election_jitter_ticks > MAX_ELECTION_TICKS
            || self
                .election_timeout_ticks
                .checked_add(self.election_jitter_ticks)
                .is_none_or(|value| value > MAX_ELECTION_TICKS)
        {
            return Err(ConsensusError::InvalidElectionTimer(
                "election timeout plus deterministic jitter exceeds the bound".into(),
            ));
        }
        if self.heartbeat_interval_ticks >= self.election_timeout_ticks {
            return Err(ConsensusError::InvalidElectionTimer(
                "heartbeat interval must be less than the election timeout".into(),
            ));
        }
        if self.failure_detector_ticks < self.election_timeout_ticks
            || self.failure_detector_ticks > MAX_ELECTION_TICKS
        {
            return Err(ConsensusError::InvalidElectionTimer(
                "failure detector interval must cover the election timeout and remain bounded"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatPlan {
    pub term: u64,
    pub leader_id: String,
    pub peer_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ElectionTimerAction {
    Idle,
    StartElection(VoteRequest),
    SendHeartbeats(HeartbeatPlan),
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogCompactionConfig {
    pub min_retained_entries: usize,
    pub max_discard_entries: usize,
}

impl Default for LogCompactionConfig {
    fn default() -> Self {
        Self {
            min_retained_entries: 1,
            max_discard_entries: MAX_COMPACTION_DISCARD_ENTRIES,
        }
    }
}

impl LogCompactionConfig {
    pub fn new(
        min_retained_entries: usize,
        max_discard_entries: usize,
    ) -> Result<Self, ConsensusError> {
        let config = Self {
            min_retained_entries,
            max_discard_entries,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.min_retained_entries > MAX_RETAINED_LOG_ENTRIES
            || self.max_discard_entries == 0
            || self.max_discard_entries > MAX_COMPACTION_DISCARD_ENTRIES
        {
            return Err(ConsensusError::LogCompaction(
                "retention and discard bounds are invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionCoordinationConfig {
    pub max_follower_lag_entries: u64,
    pub min_safe_followers: usize,
    pub require_quorum: bool,
}

impl Default for CompactionCoordinationConfig {
    fn default() -> Self {
        Self {
            max_follower_lag_entries: MAX_LOG_ENTRIES as u64,
            min_safe_followers: 1,
            require_quorum: true,
        }
    }
}

impl CompactionCoordinationConfig {
    pub fn new(
        max_follower_lag_entries: u64,
        min_safe_followers: usize,
        require_quorum: bool,
    ) -> Result<Self, ConsensusError> {
        let config = Self {
            max_follower_lag_entries,
            min_safe_followers,
            require_quorum,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.max_follower_lag_entries > MAX_LOG_ENTRIES as u64
            || self.min_safe_followers > MAX_MEMBERS
        {
            return Err(ConsensusError::CompactionCoordination(
                "compaction coordination bounds are invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionFollowerStatus {
    pub follower_id: String,
    pub match_index: u64,
    pub target_index: u64,
    pub lag_entries: u64,
    pub safe_for_compaction: bool,
}

impl CompactionFollowerStatus {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_node_id(&self.follower_id)?;
        if self.match_index > self.target_index
            || self.lag_entries != self.target_index.saturating_sub(self.match_index)
        {
            return Err(ConsensusError::CompactionCoordination(
                "follower status frontier is inconsistent".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionCoordinationPlan {
    pub leader_id: String,
    pub target_index: u64,
    pub target_term: u64,
    pub commit_index: u64,
    pub applied_index: u64,
    pub configuration_phase: ConfigurationPhase,
    pub configuration_hash: String,
    pub safe_followers: Vec<CompactionFollowerStatus>,
    pub blocked_followers: Vec<CompactionFollowerStatus>,
    pub required_safe_followers: usize,
    pub quorum_required: bool,
    pub ready: bool,
    pub plan_hash: String,
}

impl CompactionCoordinationPlan {
    fn content_hash(&self) -> Result<String, ConsensusError> {
        digest_json(&(
            &self.leader_id,
            self.target_index,
            self.target_term,
            self.commit_index,
            self.applied_index,
            self.configuration_phase,
            &self.configuration_hash,
            &self.safe_followers,
            &self.blocked_followers,
            self.required_safe_followers,
            self.quorum_required,
            self.ready,
        ))
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_node_id(&self.leader_id)?;
        if self.target_index == 0 || self.target_term == 0 {
            return Err(ConsensusError::CompactionCoordination(
                "coordination target frontier must be positive".into(),
            ));
        }
        validate_hex_digest(&self.configuration_hash)?;
        for status in self
            .safe_followers
            .iter()
            .chain(self.blocked_followers.iter())
        {
            status.validate()?;
        }
        if self
            .safe_followers
            .iter()
            .any(|status| !status.safe_for_compaction)
            || self
                .blocked_followers
                .iter()
                .any(|status| status.safe_for_compaction)
        {
            return Err(ConsensusError::CompactionCoordination(
                "follower safety classification is inconsistent".into(),
            ));
        }
        if self.ready != (self.safe_followers.len() >= self.required_safe_followers) {
            return Err(ConsensusError::CompactionCoordination(
                "coordination readiness is inconsistent".into(),
            ));
        }
        if self.content_hash()? != self.plan_hash {
            return Err(ConsensusError::CompactionCoordination(
                "coordination plan hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactionCoordinationAction {
    Waiting {
        plan: CompactionCoordinationPlan,
    },
    Compacted {
        plan: CompactionCoordinationPlan,
        snapshot: ConfigurationBoundSnapshot,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotRequestReason {
    CompactedFrontier,
    IncrementalBaseBehind,
    AppendPredecessorCompacted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotRequest {
    pub request_id: String,
    pub follower_id: String,
    pub leader_id: String,
    pub term: u64,
    pub last_included_index: u64,
    pub last_included_term: u64,
    pub configuration_hash: String,
    pub snapshot_sha256: Option<String>,
    pub reason: SnapshotRequestReason,
    pub retry_at_tick: Option<u64>,
    pub request_hash: String,
}

impl SnapshotRequest {
    pub fn new(
        follower_id: &str,
        leader_id: &str,
        term: u64,
        snapshot: Option<&ConfigurationBoundSnapshot>,
        reason: SnapshotRequestReason,
        retry_at_tick: Option<u64>,
    ) -> Result<Self, ConsensusError> {
        validate_node_id(follower_id)?;
        validate_node_id(leader_id)?;
        if follower_id == leader_id || term == 0 {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request identity or term is invalid".into(),
            ));
        }
        let (last_included_index, last_included_term, configuration_hash, snapshot_sha256) =
            if let Some(snapshot) = snapshot {
                snapshot.validate()?;
                let bytes = serde_json::to_vec(snapshot)
                    .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
                (
                    snapshot.last_included_index,
                    snapshot.last_included_term,
                    snapshot.configuration_hash.clone(),
                    Some(digest_bytes(&bytes)),
                )
            } else {
                (0, 0, String::new(), None)
            };
        let mut request = Self {
            request_id: digest_json(&(
                follower_id,
                leader_id,
                term,
                last_included_index,
                last_included_term,
                &configuration_hash,
                &snapshot_sha256,
                reason,
                retry_at_tick,
            ))?,
            follower_id: follower_id.to_string(),
            leader_id: leader_id.to_string(),
            term,
            last_included_index,
            last_included_term,
            configuration_hash,
            snapshot_sha256,
            reason,
            retry_at_tick,
            request_hash: String::new(),
        };
        request.request_hash = request.content_hash()?;
        Ok(request)
    }

    fn content_hash(&self) -> Result<String, ConsensusError> {
        digest_json(&(
            &self.request_id,
            &self.follower_id,
            &self.leader_id,
            self.term,
            self.last_included_index,
            self.last_included_term,
            &self.configuration_hash,
            &self.snapshot_sha256,
            self.reason,
            self.retry_at_tick,
        ))
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_transfer_id(&self.request_id)?;
        validate_node_id(&self.follower_id)?;
        validate_node_id(&self.leader_id)?;
        if self.follower_id == self.leader_id || self.term == 0 {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request identity or term is invalid".into(),
            ));
        }
        if (self.last_included_index == 0) != (self.last_included_term == 0) {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request frontier must be both known or unknown".into(),
            ));
        }
        if self.last_included_index == 0 {
            if !self.configuration_hash.is_empty() || self.snapshot_sha256.is_some() {
                return Err(ConsensusError::SnapshotRequest(
                    "unknown snapshot frontier cannot carry snapshot bindings".into(),
                ));
            }
        } else {
            validate_hex_digest(&self.configuration_hash)?;
        }
        if let Some(snapshot_sha256) = &self.snapshot_sha256 {
            validate_hex_digest(snapshot_sha256)?;
        }
        if self.content_hash()? != self.request_hash {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotRequestAction {
    None,
    Retry { retry_at_tick: u64 },
    Request(SnapshotRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigurationBoundSnapshot {
    pub term: u64,
    pub last_included_index: u64,
    pub last_included_term: u64,
    pub commit_index: u64,
    pub last_applied: u64,
    pub state: BTreeMap<String, String>,
    pub state_hash: String,
    pub configuration_phase: ConfigurationPhase,
    pub members: BTreeSet<String>,
    pub previous_members: Option<BTreeSet<String>>,
    pub configuration_hash: String,
}

impl ConfigurationBoundSnapshot {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.term == 0
            || self.last_included_index == 0
            || self.last_included_term == 0
            || self.last_included_index > self.last_applied
            || self.last_applied > self.commit_index
        {
            return Err(ConsensusError::InvalidSnapshot(
                "configuration-bound snapshot metadata is invalid".into(),
            ));
        }
        validate_members(&self.members)?;
        if let Some(previous) = &self.previous_members {
            validate_members(previous)?;
            if self.configuration_phase != ConfigurationPhase::Joint {
                return Err(ConsensusError::InvalidSnapshot(
                    "previous membership requires joint configuration".into(),
                ));
            }
        }
        let expected_state_hash = digest_json(&self.state)?;
        if expected_state_hash != self.state_hash {
            return Err(ConsensusError::InvalidSnapshot(
                "configuration-bound snapshot state hash mismatch".into(),
            ));
        }
        let expected_configuration_hash = digest_json(&(
            self.configuration_phase,
            &self.members,
            &self.previous_members,
        ))?;
        if expected_configuration_hash != self.configuration_hash {
            return Err(ConsensusError::InvalidSnapshot(
                "configuration-bound snapshot configuration hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplicationCatchUpAction {
    Incremental(StateDelta),
    Snapshot(ConfigurationBoundSnapshot),
    Idle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotInstallReadiness {
    Unknown,
    Receiving,
    Validated,
    DurablyStaged,
    Installed,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotBandwidthConfig {
    pub max_bytes_per_window: u64,
    pub window_ticks: u64,
}

impl Default for SnapshotBandwidthConfig {
    fn default() -> Self {
        Self {
            max_bytes_per_window: MAX_SNAPSHOT_BANDWIDTH_BYTES,
            window_ticks: 1,
        }
    }
}

impl SnapshotBandwidthConfig {
    pub fn new(max_bytes_per_window: u64, window_ticks: u64) -> Result<Self, ConsensusError> {
        let config = Self {
            max_bytes_per_window,
            window_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.max_bytes_per_window == 0
            || self.max_bytes_per_window > MAX_SNAPSHOT_BANDWIDTH_BYTES
        {
            return Err(ConsensusError::SnapshotBandwidth(
                "per-window byte budget must be positive and bounded".into(),
            ));
        }
        if self.window_ticks == 0 || self.window_ticks > MAX_SNAPSHOT_BANDWIDTH_WINDOW_TICKS {
            return Err(ConsensusError::SnapshotBandwidth(
                "bandwidth window must be positive and bounded".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotTransferProgressAction {
    Accepted {
        bytes_sent: u64,
        bytes_remaining: u64,
    },
    Backpressured {
        retry_at_tick: u64,
        available_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotTransferMetrics {
    pub follower_id: String,
    pub active_transfer_id: Option<String>,
    pub snapshot_bytes: u64,
    pub bytes_sent: u64,
    pub bytes_remaining: u64,
    pub bandwidth_window_start_tick: Option<u64>,
    pub bandwidth_window_bytes: u64,
    pub bandwidth_limit_bytes: u64,
    pub bandwidth_window_ticks: u64,
    pub sent_transfers: u64,
    pub acknowledged_transfers: u64,
    pub rejected_transfers: u64,
    pub cancelled_transfers: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotTransferCancellation {
    pub follower_id: String,
    pub transfer_id: String,
    pub retry_at_tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotInstallAck {
    pub transfer_id: String,
    pub follower_id: String,
    pub term: u64,
    pub last_included_index: u64,
    pub last_included_term: u64,
    pub snapshot_sha256: String,
    pub configuration_hash: String,
    pub readiness: SnapshotInstallReadiness,
    pub reason: Option<String>,
}

impl SnapshotInstallAck {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        validate_transfer_id(&self.transfer_id)?;
        validate_node_id(&self.follower_id)?;
        if self.term == 0 || self.last_included_index == 0 || self.last_included_term == 0 {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "term and snapshot frontier must be positive".into(),
            ));
        }
        validate_hex_digest(&self.snapshot_sha256)?;
        validate_hex_digest(&self.configuration_hash)?;
        if self.reason.as_ref().is_some_and(|reason| {
            reason.is_empty()
                || reason.len() > MAX_VALUE_BYTES
                || reason.chars().any(char::is_control)
        }) {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "acknowledgement reason is invalid".into(),
            ));
        }
        if self.readiness == SnapshotInstallReadiness::Rejected && self.reason.is_none() {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "rejected acknowledgement requires a reason".into(),
            ));
        }
        if self.readiness != SnapshotInstallReadiness::Rejected && self.reason.is_some() {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "non-rejected acknowledgement cannot carry a rejection reason".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapshotTransferAction {
    Idle,
    Backpressured {
        retry_at_tick: Option<u64>,
    },
    Send {
        transfer_id: String,
        snapshot: ConfigurationBoundSnapshot,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotReplicationStatus {
    pub follower_id: String,
    pub active_transfer_id: Option<String>,
    pub readiness: SnapshotInstallReadiness,
    pub last_installed_index: u64,
    pub last_installed_term: u64,
    pub retry_at_tick: Option<u64>,
    pub sent_transfers: u64,
    pub acknowledged_transfers: u64,
    pub rejected_transfers: u64,
    pub cancelled_transfers: u64,
    pub metrics: SnapshotTransferMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub transfer_id: String,
    pub term: u64,
    pub commit_index: u64,
    pub last_applied: u64,
    pub total_bytes: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub state_hash: String,
    pub manifest_hash: String,
}

impl SnapshotManifest {
    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.transfer_id.is_empty()
            || self.transfer_id.len() > MAX_CLUSTER_ID_BYTES
            || self.transfer_id.chars().any(char::is_control)
        {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "transfer ID is invalid".into(),
            ));
        }
        if self.term == 0
            || self.chunk_size == 0
            || self.chunk_size as usize > MAX_SNAPSHOT_CHUNK_BYTES
        {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "manifest term or chunk size is invalid".into(),
            ));
        }
        if self.chunk_count == 0 || self.chunk_count as usize > MAX_SYNC_CHUNKS {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "manifest chunk count is outside the configured bound".into(),
            ));
        }
        if self.total_bytes == 0 || self.total_bytes > MAX_SNAPSHOT_BYTES {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "manifest byte count is outside the snapshot bound".into(),
            ));
        }
        let expected_count: u32 = self
            .total_bytes
            .div_ceil(self.chunk_size as u64)
            .try_into()
            .map_err(|_| ConsensusError::InvalidSnapshotChunk("chunk count overflow".into()))?;
        if expected_count != self.chunk_count {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "manifest chunk count does not match total bytes".into(),
            ));
        }
        validate_hex_digest(&self.state_hash)?;
        let expected = digest_json(&(
            &self.transfer_id,
            self.term,
            self.commit_index,
            self.last_applied,
            self.total_bytes,
            self.chunk_size,
            self.chunk_count,
            &self.state_hash,
        ))?;
        if expected != self.manifest_hash {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "manifest hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotChunk {
    pub term: u64,
    pub transfer_id: String,
    pub index: u32,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub chunk_hash: String,
}

impl SnapshotChunk {
    pub fn validate(&self, manifest: &SnapshotManifest) -> Result<(), ConsensusError> {
        manifest.validate()?;
        if self.term != manifest.term
            || self.transfer_id != manifest.transfer_id
            || self.index >= manifest.chunk_count
            || self.bytes.is_empty()
            || self.bytes.len() > manifest.chunk_size as usize
            || self.offset != self.index as u64 * manifest.chunk_size as u64
        {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "chunk identity, offset, or size is invalid".into(),
            ));
        }
        let remaining = manifest.total_bytes.saturating_sub(self.offset);
        if self.bytes.len() as u64 > remaining {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "chunk exceeds the manifest byte frontier".into(),
            ));
        }
        let expected = digest_bytes(&self.bytes);
        if expected != self.chunk_hash {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "chunk hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotChunker {
    manifest: SnapshotManifest,
    chunks: Vec<SnapshotChunk>,
}

impl SnapshotChunker {
    pub fn from_snapshot(
        snapshot: &ReplicatedSnapshot,
        transfer_id: &str,
        chunk_size: usize,
    ) -> Result<Self, ConsensusError> {
        snapshot.validate()?;
        if chunk_size == 0 || chunk_size > MAX_SNAPSHOT_CHUNK_BYTES {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "chunk size exceeds the configured bound".into(),
            ));
        }
        validate_transfer_id(transfer_id)?;
        let bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "serialized snapshot exceeds the configured bound".into(),
            ));
        }
        let chunk_count = bytes.len().div_ceil(chunk_size);
        if chunk_count > MAX_SYNC_CHUNKS {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "snapshot requires too many chunks".into(),
            ));
        }
        let mut chunks = Vec::with_capacity(chunk_count);
        for (index, slice) in bytes.chunks(chunk_size).enumerate() {
            chunks.push(SnapshotChunk {
                term: snapshot.term,
                transfer_id: transfer_id.to_string(),
                index: index as u32,
                offset: index as u64 * chunk_size as u64,
                bytes: slice.to_vec(),
                chunk_hash: digest_bytes(slice),
            });
        }
        let state_hash = snapshot.state_hash.clone();
        let mut manifest = SnapshotManifest {
            transfer_id: transfer_id.to_string(),
            term: snapshot.term,
            commit_index: snapshot.commit_index,
            last_applied: snapshot.last_applied,
            total_bytes: bytes.len() as u64,
            chunk_size: chunk_size as u32,
            chunk_count: chunk_count as u32,
            state_hash,
            manifest_hash: String::new(),
        };
        manifest.manifest_hash = digest_json(&(
            &manifest.transfer_id,
            manifest.term,
            manifest.commit_index,
            manifest.last_applied,
            manifest.total_bytes,
            manifest.chunk_size,
            manifest.chunk_count,
            &manifest.state_hash,
        ))?;
        manifest.validate()?;
        Ok(Self { manifest, chunks })
    }

    pub fn manifest(&self) -> &SnapshotManifest {
        &self.manifest
    }

    pub fn chunks(&self) -> &[SnapshotChunk] {
        &self.chunks
    }

    pub fn chunk(&self, index: u32) -> Option<&SnapshotChunk> {
        self.chunks.get(index as usize)
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotAssembler {
    manifest: SnapshotManifest,
    chunks: BTreeMap<u32, SnapshotChunk>,
}

impl SnapshotAssembler {
    pub fn new(manifest: SnapshotManifest) -> Result<Self, ConsensusError> {
        manifest.validate()?;
        Ok(Self {
            manifest,
            chunks: BTreeMap::new(),
        })
    }

    pub fn accept(&mut self, chunk: SnapshotChunk) -> Result<(), ConsensusError> {
        chunk.validate(&self.manifest)?;
        if let Some(existing) = self.chunks.get(&chunk.index) {
            if existing.chunk_hash != chunk.chunk_hash {
                return Err(ConsensusError::InvalidSnapshotChunk(
                    "duplicate chunk conflicts with an earlier chunk".into(),
                ));
            }
            return Ok(());
        }
        self.chunks.insert(chunk.index, chunk);
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.chunks.len() == self.manifest.chunk_count as usize
    }

    pub fn finish(self) -> Result<ReplicatedSnapshot, ConsensusError> {
        if !self.is_complete() {
            return Err(ConsensusError::SnapshotTransferIncomplete);
        }
        let mut bytes = Vec::with_capacity(self.manifest.total_bytes as usize);
        for index in 0..self.manifest.chunk_count {
            let chunk = self
                .chunks
                .get(&index)
                .ok_or(ConsensusError::SnapshotTransferIncomplete)?;
            bytes.extend_from_slice(&chunk.bytes);
        }
        if bytes.len() as u64 != self.manifest.total_bytes {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "assembled byte count does not match manifest".into(),
            ));
        }
        let snapshot: ReplicatedSnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        snapshot.validate()?;
        if snapshot.term != self.manifest.term
            || snapshot.commit_index != self.manifest.commit_index
            || snapshot.last_applied != self.manifest.last_applied
            || snapshot.state_hash != self.manifest.state_hash
        {
            return Err(ConsensusError::InvalidSnapshotChunk(
                "assembled snapshot metadata does not match manifest".into(),
            ));
        }
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateDelta {
    pub term: u64,
    pub base_index: u64,
    pub target_index: u64,
    pub leader_commit: u64,
    pub entries: Vec<LogEntry>,
    pub delta_hash: String,
}

impl StateDelta {
    pub fn new(
        term: u64,
        base_index: u64,
        leader_commit: u64,
        entries: Vec<LogEntry>,
    ) -> Result<Self, ConsensusError> {
        if term == 0 || entries.is_empty() || entries.len() > MAX_BATCH_ENTRIES {
            return Err(ConsensusError::IncrementalSyncConflict(
                "delta must contain 1 to MAX_BATCH_ENTRIES entries".into(),
            ));
        }
        let target_index = base_index.checked_add(entries.len() as u64).ok_or(
            ConsensusError::IncrementalSyncConflict("delta index overflow".into()),
        )?;
        for (offset, entry) in entries.iter().enumerate() {
            entry.validate(base_index + offset as u64 + 1)?;
        }
        if leader_commit > target_index {
            return Err(ConsensusError::IncrementalSyncConflict(
                "leader commit exceeds the delta frontier".into(),
            ));
        }
        let delta_hash = digest_json(&(term, base_index, target_index, leader_commit, &entries))?;
        Ok(Self {
            term,
            base_index,
            target_index,
            leader_commit,
            entries,
            delta_hash,
        })
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        let expected = Self::new(
            self.term,
            self.base_index,
            self.leader_commit,
            self.entries.clone(),
        )?;
        if expected.target_index != self.target_index
            || expected.leader_commit != self.leader_commit
            || expected.delta_hash != self.delta_hash
        {
            return Err(ConsensusError::IncrementalSyncConflict(
                "state delta hash or frontier mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableConsensusState {
    pub cluster_id: String,
    pub node_id: String,
    pub current_term: u64,
    pub voted_for: Option<String>,
    pub replay_epoch: u64,
    pub replay_term_floor: u64,
    pub state_hash: String,
}

impl DurableConsensusState {
    pub fn new(
        cluster_id: &str,
        node_id: &str,
        current_term: u64,
        voted_for: Option<String>,
        replay_epoch: u64,
        replay_term_floor: u64,
    ) -> Result<Self, ConsensusError> {
        let mut state = Self {
            cluster_id: cluster_id.to_string(),
            node_id: node_id.to_string(),
            current_term,
            voted_for,
            replay_epoch,
            replay_term_floor,
            state_hash: String::new(),
        };
        state.validate_identity()?;
        state.state_hash = state.content_hash()?;
        Ok(state)
    }

    fn validate_identity(&self) -> Result<(), ConsensusError> {
        validate_cluster_id(&self.cluster_id)?;
        validate_node_id(&self.node_id)?;
        if let Some(voted_for) = &self.voted_for {
            validate_node_id(voted_for)?;
        }
        if self.replay_epoch == 0 || self.replay_term_floor == 0 {
            return Err(ConsensusError::DurableConsensusState(
                "replay epoch and term floor must be positive".into(),
            ));
        }
        Ok(())
    }

    fn content_hash(&self) -> Result<String, ConsensusError> {
        digest_json(&(
            &self.cluster_id,
            &self.node_id,
            self.current_term,
            &self.voted_for,
            self.replay_epoch,
            self.replay_term_floor,
        ))
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        self.validate_identity()?;
        validate_hex_digest(&self.state_hash).map_err(|_| {
            ConsensusError::DurableConsensusState("state hash must be a SHA-256 digest".into())
        })?;
        if self.content_hash()? != self.state_hash {
            return Err(ConsensusError::DurableConsensusState(
                "durable consensus state hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DurableConsensusStateStore {
    path: PathBuf,
}

impl DurableConsensusStateStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn staging_path(&self) -> PathBuf {
        self.path.with_extension("state.tmp")
    }

    pub fn save(&self, state: &DurableConsensusState) -> Result<(), ConsensusError> {
        state.validate()?;
        let bytes = serde_json::to_vec(state)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() as u64 > MAX_DURABLE_CONSENSUS_STATE_BYTES {
            return Err(ConsensusError::DurableConsensusState(
                "durable state exceeds the configured byte bound".into(),
            ));
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?;
        let temporary = self.staging_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn recover_staging(&self) -> Result<bool, ConsensusError> {
        let temporary = self.staging_path();
        if !temporary.exists() {
            return Ok(false);
        }
        fs::remove_file(&temporary)
            .map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(true)
    }

    pub fn load(&self) -> Result<DurableConsensusState, ConsensusError> {
        let metadata = fs::metadata(&self.path)
            .map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?;
        if metadata.len() > MAX_DURABLE_CONSENSUS_STATE_BYTES {
            return Err(ConsensusError::DurableConsensusState(
                "durable state exceeds the configured byte bound".into(),
            ));
        }
        let state: DurableConsensusState = serde_json::from_slice(
            &fs::read(&self.path)
                .map_err(|error| ConsensusError::DurableConsensusState(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        state.validate()?;
        Ok(state)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedConsensusEnvelope {
    pub cluster_id: String,
    pub sender_id: String,
    pub term: u64,
    pub replay_epoch: u64,
    pub nonce: String,
    pub message: ConsensusMessage,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ReplayWindow {
    cluster_id: String,
    sender_id: String,
    max_entries: usize,
    replay_epoch: u64,
    min_term: u64,
    next_sequence: u64,
    seen: BTreeMap<String, u64>,
}

impl ReplayWindow {
    pub fn new(
        cluster_id: &str,
        sender_id: &str,
        max_entries: usize,
    ) -> Result<Self, ConsensusError> {
        Self::new_with_epoch(cluster_id, sender_id, max_entries, 1, 1)
    }

    pub fn new_with_epoch(
        cluster_id: &str,
        sender_id: &str,
        max_entries: usize,
        replay_epoch: u64,
        min_term: u64,
    ) -> Result<Self, ConsensusError> {
        validate_cluster_id(cluster_id)?;
        validate_node_id(sender_id)?;
        if max_entries == 0 || max_entries > MAX_LOG_ENTRIES {
            return Err(ConsensusError::InvalidClusterConfiguration(
                "replay window must be between 1 and the log bound".into(),
            ));
        }
        if replay_epoch == 0 || min_term == 0 {
            return Err(ConsensusError::InvalidClusterConfiguration(
                "replay epoch and minimum term must be positive".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            sender_id: sender_id.to_string(),
            max_entries,
            replay_epoch,
            min_term,
            next_sequence: 0,
            seen: BTreeMap::new(),
        })
    }

    pub fn accept(
        &mut self,
        envelope: &AuthenticatedConsensusEnvelope,
        trusted_key: &[u8],
    ) -> Result<(), ConsensusError> {
        envelope.verify_for_cluster_epoch(
            &self.cluster_id,
            &self.sender_id,
            trusted_key,
            self.replay_epoch,
            self.min_term,
        )?;
        if self.seen.contains_key(&envelope.nonce) {
            return Err(ConsensusError::ReplayDetected);
        }
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(ConsensusError::ReplayDetected)?;
        self.seen.insert(envelope.nonce.clone(), self.next_sequence);
        while self.seen.len() > self.max_entries {
            let oldest = self
                .seen
                .iter()
                .min_by_key(|(_, sequence)| **sequence)
                .map(|(nonce, _)| nonce.clone());
            if let Some(oldest) = oldest {
                self.seen.remove(&oldest);
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn contains_nonce(&self, nonce: &str) -> bool {
        self.seen.contains_key(nonce)
    }

    pub fn replay_epoch(&self) -> u64 {
        self.replay_epoch
    }

    pub fn min_term(&self) -> u64 {
        self.min_term
    }
}

impl AuthenticatedConsensusEnvelope {
    pub fn sign(
        sender_id: &str,
        term: u64,
        nonce: &str,
        message: ConsensusMessage,
        signing_key: &SigningKey,
    ) -> Result<Self, ConsensusError> {
        Self::sign_for_cluster_epoch(
            DEFAULT_CLUSTER_ID,
            sender_id,
            term,
            1,
            nonce,
            message,
            signing_key,
        )
    }

    pub fn sign_for_cluster(
        cluster_id: &str,
        sender_id: &str,
        term: u64,
        nonce: &str,
        message: ConsensusMessage,
        signing_key: &SigningKey,
    ) -> Result<Self, ConsensusError> {
        Self::sign_for_cluster_epoch(cluster_id, sender_id, term, 1, nonce, message, signing_key)
    }

    pub fn sign_for_cluster_epoch(
        cluster_id: &str,
        sender_id: &str,
        term: u64,
        replay_epoch: u64,
        nonce: &str,
        message: ConsensusMessage,
        signing_key: &SigningKey,
    ) -> Result<Self, ConsensusError> {
        validate_cluster_id(cluster_id)?;
        validate_node_id(sender_id)?;
        validate_nonce(nonce)?;
        if replay_epoch == 0 {
            return Err(ConsensusError::Unauthenticated(
                "replay epoch must be positive".into(),
            ));
        }
        if term == 0 || message_term(&message) != term {
            return Err(ConsensusError::Unauthenticated(
                "envelope term must be positive and match the message term".into(),
            ));
        }
        let public_key = signing_key.verifying_key().to_bytes().to_vec();
        let mut envelope = Self {
            cluster_id: cluster_id.to_string(),
            sender_id: sender_id.to_string(),
            term,
            replay_epoch,
            nonce: nonce.to_string(),
            message,
            public_key,
            signature: Vec::new(),
        };
        let payload = envelope.payload()?;
        envelope.signature = signing_key.sign(&payload).to_bytes().to_vec();
        Ok(envelope)
    }

    pub fn verify(
        &self,
        expected_sender_id: &str,
        trusted_key: &[u8],
    ) -> Result<(), ConsensusError> {
        self.verify_for_cluster(&self.cluster_id, expected_sender_id, trusted_key)
    }

    pub fn verify_for_cluster(
        &self,
        expected_cluster_id: &str,
        expected_sender_id: &str,
        trusted_key: &[u8],
    ) -> Result<(), ConsensusError> {
        self.verify_for_cluster_epoch(expected_cluster_id, expected_sender_id, trusted_key, 1, 1)
    }

    pub fn verify_for_cluster_epoch(
        &self,
        expected_cluster_id: &str,
        expected_sender_id: &str,
        trusted_key: &[u8],
        expected_replay_epoch: u64,
        min_term: u64,
    ) -> Result<(), ConsensusError> {
        self.verify_cluster_sender(expected_cluster_id, expected_sender_id)?;
        validate_nonce(&self.nonce)?;
        if expected_replay_epoch == 0 || min_term == 0 {
            return Err(ConsensusError::Unauthenticated(
                "expected replay epoch and minimum term must be positive".into(),
            ));
        }
        if self.replay_epoch != expected_replay_epoch {
            return Err(ConsensusError::ReplayEpochMismatch {
                expected: expected_replay_epoch,
                received: self.replay_epoch,
            });
        }
        if self.term < min_term {
            return Err(ConsensusError::StaleReplayTerm);
        }
        if self.term == 0 || message_term(&self.message) != self.term {
            return Err(ConsensusError::Unauthenticated(
                "term or message binding mismatch".into(),
            ));
        }
        if trusted_key != self.public_key.as_slice() {
            return Err(ConsensusError::Unauthenticated(
                "sender public key is not bound to the trusted identity".into(),
            ));
        }
        let key: [u8; 32] =
            self.public_key.as_slice().try_into().map_err(|_| {
                ConsensusError::Unauthenticated("public key must be 32 bytes".into())
            })?;
        let signature: [u8; 64] =
            self.signature.as_slice().try_into().map_err(|_| {
                ConsensusError::Unauthenticated("signature must be 64 bytes".into())
            })?;
        let verifying_key = VerifyingKey::from_bytes(&key)
            .map_err(|_| ConsensusError::Unauthenticated("invalid public key".into()))?;
        verifying_key
            .verify(&self.payload()?, &Signature::from_bytes(&signature))
            .map_err(|_| ConsensusError::Unauthenticated("invalid consensus signature".into()))
    }

    pub fn verify_cluster_sender(
        &self,
        expected_cluster_id: &str,
        expected_sender_id: &str,
    ) -> Result<(), ConsensusError> {
        validate_cluster_id(expected_cluster_id)?;
        validate_node_id(expected_sender_id)?;
        validate_node_id(&self.sender_id)?;
        if self.cluster_id != expected_cluster_id || self.sender_id != expected_sender_id {
            return Err(ConsensusError::Unauthenticated(
                "cluster or sender identity binding mismatch".into(),
            ));
        }
        Ok(())
    }

    fn payload(&self) -> Result<Vec<u8>, ConsensusError> {
        serde_json::to_vec(&(
            &self.cluster_id,
            &self.sender_id,
            self.term,
            self.replay_epoch,
            &self.nonce,
            &self.message,
            &self.public_key,
        ))
        .map_err(|error| ConsensusError::Serialization(error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SocketQuotaConfig {
    pub max_in_flight_bytes_per_peer: u64,
    pub max_receive_bytes_per_window: u64,
    pub receive_window_ticks: u64,
    pub retry_backoff_ticks: u64,
}

impl Default for SocketQuotaConfig {
    fn default() -> Self {
        Self {
            max_in_flight_bytes_per_peer: 4 * MAX_FRAME_BYTES as u64,
            max_receive_bytes_per_window: 4 * MAX_FRAME_BYTES as u64,
            receive_window_ticks: 1,
            retry_backoff_ticks: 1,
        }
    }
}

impl SocketQuotaConfig {
    pub fn new(
        max_in_flight_bytes_per_peer: u64,
        max_receive_bytes_per_window: u64,
        receive_window_ticks: u64,
        retry_backoff_ticks: u64,
    ) -> Result<Self, ConsensusError> {
        let config = Self {
            max_in_flight_bytes_per_peer,
            max_receive_bytes_per_window,
            receive_window_ticks,
            retry_backoff_ticks,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.max_in_flight_bytes_per_peer == 0
            || self.max_in_flight_bytes_per_peer > MAX_SOCKET_QUOTA_BYTES
            || self.max_receive_bytes_per_window == 0
            || self.max_receive_bytes_per_window > MAX_SOCKET_QUOTA_BYTES
            || self.receive_window_ticks == 0
            || self.receive_window_ticks > MAX_ELECTION_TICKS
            || self.retry_backoff_ticks == 0
            || self.retry_backoff_ticks > MAX_ELECTION_TICKS
        {
            return Err(ConsensusError::SocketQuota(
                "socket quota values are outside bounded limits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SocketPeerQuota {
    pub peer_id: String,
    pub in_flight_bytes: u64,
    pub receive_window_start_tick: Option<u64>,
    pub receive_window_bytes: u64,
    pub admitted_frames: u64,
    pub rejected_frames: u64,
    pub backpressured_sends: u64,
    pub backpressured_receives: u64,
}

impl SocketPeerQuota {
    fn new(peer_id: &str) -> Self {
        Self {
            peer_id: peer_id.to_string(),
            in_flight_bytes: 0,
            receive_window_start_tick: None,
            receive_window_bytes: 0,
            admitted_frames: 0,
            rejected_frames: 0,
            backpressured_sends: 0,
            backpressured_receives: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SocketBackpressureAction {
    Admitted {
        frame_bytes: u64,
        available_bytes: u64,
    },
    Backpressured {
        retry_at_tick: u64,
        available_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SocketReceiveAction {
    Received {
        envelope: AuthenticatedConsensusEnvelope,
        frame_bytes: u64,
    },
    Backpressured {
        retry_at_tick: u64,
        available_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SocketTransportMetrics {
    pub peer_id: String,
    pub in_flight_bytes: u64,
    pub receive_window_start_tick: Option<u64>,
    pub receive_window_bytes: u64,
    pub max_in_flight_bytes: u64,
    pub max_receive_bytes_per_window: u64,
    pub receive_window_ticks: u64,
    pub admitted_frames: u64,
    pub rejected_frames: u64,
    pub backpressured_sends: u64,
    pub backpressured_receives: u64,
    pub durable_queue_frames: u64,
    pub durable_queue_bytes: u64,
    pub next_queue_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableSocketQueueFrame {
    pub peer_id: String,
    pub sequence: u64,
    pub frame_bytes: Vec<u8>,
    pub frame_digest: String,
}

impl DurableSocketQueueFrame {
    fn new(peer_id: &str, sequence: u64, frame_bytes: Vec<u8>) -> Result<Self, ConsensusError> {
        validate_node_id(peer_id)?;
        if sequence == 0 || frame_bytes.is_empty() || frame_bytes.len() > MAX_FRAME_BYTES {
            return Err(ConsensusError::SocketQuota(
                "durable queue frame is outside bounded limits".into(),
            ));
        }
        let frame_digest = digest_bytes(&frame_bytes);
        Ok(Self {
            peer_id: peer_id.to_string(),
            sequence,
            frame_bytes,
            frame_digest,
        })
    }

    fn validate(&self) -> Result<(), ConsensusError> {
        validate_node_id(&self.peer_id)?;
        if self.sequence == 0
            || self.frame_bytes.is_empty()
            || self.frame_bytes.len() > MAX_FRAME_BYTES
        {
            return Err(ConsensusError::SocketQuota(
                "durable queue frame is outside bounded limits".into(),
            ));
        }
        validate_hex_digest(&self.frame_digest).map_err(|_| {
            ConsensusError::SocketQuota("durable queue frame digest is invalid".into())
        })?;
        if digest_bytes(&self.frame_bytes) != self.frame_digest {
            return Err(ConsensusError::SocketQuota(
                "durable queue frame digest mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableSocketQueueState {
    pub cluster_id: String,
    pub node_id: String,
    pub replay_epoch: u64,
    pub quota_config: SocketQuotaConfig,
    pub peer_quotas: BTreeMap<String, SocketPeerQuota>,
    pub next_queue_sequences: BTreeMap<String, u64>,
    pub queued_frames: BTreeMap<String, Vec<DurableSocketQueueFrame>>,
    pub state_hash: String,
}

impl DurableSocketQueueState {
    pub fn new(
        cluster_id: &str,
        node_id: &str,
        replay_epoch: u64,
        quota_config: SocketQuotaConfig,
        peer_quotas: BTreeMap<String, SocketPeerQuota>,
        next_queue_sequences: BTreeMap<String, u64>,
        queued_frames: BTreeMap<String, Vec<DurableSocketQueueFrame>>,
    ) -> Result<Self, ConsensusError> {
        let mut state = Self {
            cluster_id: cluster_id.to_string(),
            node_id: node_id.to_string(),
            replay_epoch,
            quota_config,
            peer_quotas,
            next_queue_sequences,
            queued_frames,
            state_hash: String::new(),
        };
        state.validate_identity()?;
        state.state_hash = state.content_hash()?;
        Ok(state)
    }

    fn validate_identity(&self) -> Result<(), ConsensusError> {
        validate_cluster_id(&self.cluster_id)?;
        validate_node_id(&self.node_id)?;
        if self.replay_epoch == 0 {
            return Err(ConsensusError::SocketQuota(
                "durable queue replay epoch must be positive".into(),
            ));
        }
        self.quota_config.validate()?;
        if self.peer_quotas.len() > MAX_MEMBERS
            || self.next_queue_sequences.len() > MAX_MEMBERS
            || self.queued_frames.len() > MAX_MEMBERS
        {
            return Err(ConsensusError::SocketQuota(
                "durable queue peer count exceeds the member bound".into(),
            ));
        }
        let peer_ids: BTreeSet<String> = self.peer_quotas.keys().cloned().collect();
        if peer_ids != self.next_queue_sequences.keys().cloned().collect()
            || peer_ids != self.queued_frames.keys().cloned().collect()
        {
            return Err(ConsensusError::SocketQuota(
                "durable queue peer maps are not identical".into(),
            ));
        }
        let mut total_bytes = 0u64;
        for peer_id in &peer_ids {
            validate_node_id(peer_id)?;
            let quota = self.peer_quotas.get(peer_id).ok_or_else(|| {
                ConsensusError::SocketQuota("durable queue peer quota is missing".into())
            })?;
            if quota.peer_id != *peer_id
                || quota.in_flight_bytes > self.quota_config.max_in_flight_bytes_per_peer
                || quota.receive_window_bytes > self.quota_config.max_receive_bytes_per_window
            {
                return Err(ConsensusError::SocketQuota(
                    "durable queue peer quota is inconsistent".into(),
                ));
            }
            let next_sequence = *self.next_queue_sequences.get(peer_id).ok_or_else(|| {
                ConsensusError::SocketQuota("durable queue sequence is missing".into())
            })?;
            if next_sequence == 0 {
                return Err(ConsensusError::SocketQuota(
                    "durable queue sequence must be positive".into(),
                ));
            }
            let frames = self.queued_frames.get(peer_id).ok_or_else(|| {
                ConsensusError::SocketQuota("durable queue frames are missing".into())
            })?;
            if frames.len() > MAX_DURABLE_SOCKET_QUEUE_ENTRIES {
                return Err(ConsensusError::SocketQuota(
                    "durable queue entry count exceeds the bound".into(),
                ));
            }
            let mut previous_sequence = 0u64;
            let mut peer_bytes = 0u64;
            for frame in frames {
                frame.validate()?;
                if frame.peer_id != *peer_id || frame.sequence <= previous_sequence {
                    return Err(ConsensusError::SocketQuota(
                        "durable queue frame ordering or peer binding is invalid".into(),
                    ));
                }
                if frame.sequence >= next_sequence {
                    return Err(ConsensusError::SocketQuota(
                        "durable queue sequence exceeds the next sequence".into(),
                    ));
                }
                previous_sequence = frame.sequence;
                peer_bytes = peer_bytes
                    .checked_add(frame.frame_bytes.len() as u64)
                    .ok_or_else(|| {
                        ConsensusError::SocketQuota("durable queue byte overflow".into())
                    })?;
            }
            if peer_bytes != quota.in_flight_bytes {
                return Err(ConsensusError::SocketQuota(
                    "durable queue bytes do not match in-flight quota".into(),
                ));
            }
            total_bytes = total_bytes
                .checked_add(peer_bytes)
                .ok_or_else(|| ConsensusError::SocketQuota("durable queue byte overflow".into()))?;
        }
        if total_bytes > MAX_DURABLE_SOCKET_QUEUE_BYTES {
            return Err(ConsensusError::SocketQuota(
                "durable queue exceeds the global byte bound".into(),
            ));
        }
        Ok(())
    }

    fn content_hash(&self) -> Result<String, ConsensusError> {
        digest_json(&(
            &self.cluster_id,
            &self.node_id,
            self.replay_epoch,
            &self.quota_config,
            &self.peer_quotas,
            &self.next_queue_sequences,
            &self.queued_frames,
        ))
    }

    pub fn validate(&self) -> Result<(), ConsensusError> {
        self.validate_identity()?;
        validate_hex_digest(&self.state_hash).map_err(|_| {
            ConsensusError::SocketQuota("durable queue state hash is invalid".into())
        })?;
        if self.content_hash()? != self.state_hash {
            return Err(ConsensusError::SocketQuota(
                "durable queue state hash mismatch".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DurableSocketQueueStore {
    path: PathBuf,
}

impl DurableSocketQueueStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn staging_path(&self) -> PathBuf {
        self.path.with_extension("queue.tmp")
    }

    pub fn save(&self, state: &DurableSocketQueueState) -> Result<(), ConsensusError> {
        state.validate()?;
        let bytes = serde_json::to_vec(state)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() as u64 > MAX_DURABLE_SOCKET_QUEUE_BYTES {
            return Err(ConsensusError::SocketQuota(
                "durable queue state exceeds the byte bound".into(),
            ));
        }
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| ConsensusError::SocketQuota(error.to_string()))?;
        let temporary = self.staging_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| ConsensusError::SocketQuota(error.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| ConsensusError::SocketQuota(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    pub fn recover_staging(&self) -> Result<bool, ConsensusError> {
        let temporary = self.staging_path();
        if !temporary.exists() {
            return Ok(false);
        }
        fs::remove_file(&temporary)
            .map_err(|error| ConsensusError::SocketQuota(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(true)
    }

    pub fn load(&self) -> Result<DurableSocketQueueState, ConsensusError> {
        let metadata = fs::metadata(&self.path)
            .map_err(|error| ConsensusError::SocketQuota(error.to_string()))?;
        if metadata.len() > MAX_DURABLE_SOCKET_QUEUE_BYTES {
            return Err(ConsensusError::SocketQuota(
                "durable queue state exceeds the byte bound".into(),
            ));
        }
        let state: DurableSocketQueueState = serde_json::from_slice(
            &fs::read(&self.path)
                .map_err(|error| ConsensusError::SocketQuota(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        state.validate()?;
        Ok(state)
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSocketTransport {
    cluster_id: String,
    node_id: String,
    trusted_keys: BTreeMap<String, Vec<u8>>,
    replay_windows: BTreeMap<String, ReplayWindow>,
    replay_epoch: u64,
    replay_term_floor: u64,
    replay_window_entries: usize,
    quota_config: SocketQuotaConfig,
    peer_quotas: BTreeMap<String, SocketPeerQuota>,
    next_queue_sequences: BTreeMap<String, u64>,
    durable_queues: BTreeMap<String, Vec<DurableSocketQueueFrame>>,
    max_frame_bytes: usize,
}

impl AuthenticatedSocketTransport {
    pub fn new(
        cluster_id: &str,
        node_id: &str,
        trusted_keys: BTreeMap<String, Vec<u8>>,
        replay_window_entries: usize,
    ) -> Result<Self, ConsensusError> {
        Self::new_with_epoch(
            cluster_id,
            node_id,
            trusted_keys,
            replay_window_entries,
            1,
            1,
        )
    }

    pub fn new_with_epoch(
        cluster_id: &str,
        node_id: &str,
        trusted_keys: BTreeMap<String, Vec<u8>>,
        replay_window_entries: usize,
        replay_epoch: u64,
        replay_term_floor: u64,
    ) -> Result<Self, ConsensusError> {
        Self::new_with_epoch_and_quota(
            cluster_id,
            node_id,
            trusted_keys,
            replay_window_entries,
            replay_epoch,
            replay_term_floor,
            SocketQuotaConfig::default(),
        )
    }

    pub fn new_with_epoch_and_quota(
        cluster_id: &str,
        node_id: &str,
        trusted_keys: BTreeMap<String, Vec<u8>>,
        replay_window_entries: usize,
        replay_epoch: u64,
        replay_term_floor: u64,
        quota_config: SocketQuotaConfig,
    ) -> Result<Self, ConsensusError> {
        validate_cluster_id(cluster_id)?;
        validate_node_id(node_id)?;
        validate_members(&trusted_keys.keys().cloned().collect())?;
        if !trusted_keys.contains_key(node_id) {
            return Err(ConsensusError::InvalidClusterConfiguration(
                "transport node must have a trusted public key".into(),
            ));
        }
        if replay_epoch == 0 || replay_term_floor == 0 {
            return Err(ConsensusError::InvalidClusterConfiguration(
                "replay epoch and term floor must be positive".into(),
            ));
        }
        quota_config.validate()?;
        let mut replay_windows = BTreeMap::new();
        for sender_id in trusted_keys.keys() {
            replay_windows.insert(
                sender_id.clone(),
                ReplayWindow::new_with_epoch(
                    cluster_id,
                    sender_id,
                    replay_window_entries,
                    replay_epoch,
                    replay_term_floor,
                )?,
            );
        }
        let peer_quotas = trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), SocketPeerQuota::new(peer_id)))
            .collect();
        let next_queue_sequences = trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), 1u64))
            .collect();
        let durable_queues = trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), Vec::new()))
            .collect();
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            node_id: node_id.to_string(),
            trusted_keys,
            replay_windows,
            replay_epoch,
            replay_term_floor,
            replay_window_entries,
            quota_config,
            peer_quotas,
            next_queue_sequences,
            durable_queues,
            max_frame_bytes: MAX_FRAME_BYTES,
        })
    }

    pub fn cluster_id(&self) -> &str {
        &self.cluster_id
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn replay_epoch(&self) -> u64 {
        self.replay_epoch
    }

    pub fn replay_term_floor(&self) -> u64 {
        self.replay_term_floor
    }

    pub fn rotate_replay_epoch(
        &mut self,
        replay_epoch: u64,
        replay_term_floor: u64,
    ) -> Result<(), ConsensusError> {
        if replay_epoch <= self.replay_epoch {
            return Err(ConsensusError::DurableConsensusState(
                "replay epoch must increase monotonically".into(),
            ));
        }
        if replay_term_floor == 0 {
            return Err(ConsensusError::DurableConsensusState(
                "replay term floor must be positive".into(),
            ));
        }
        let mut replay_windows = BTreeMap::new();
        for sender_id in self.trusted_keys.keys() {
            replay_windows.insert(
                sender_id.clone(),
                ReplayWindow::new_with_epoch(
                    &self.cluster_id,
                    sender_id,
                    self.replay_window_entries,
                    replay_epoch,
                    replay_term_floor,
                )?,
            );
        }
        self.replay_windows = replay_windows;
        self.replay_epoch = replay_epoch;
        self.replay_term_floor = replay_term_floor;
        self.peer_quotas = self
            .trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), SocketPeerQuota::new(peer_id)))
            .collect();
        self.next_queue_sequences = self
            .trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), 1u64))
            .collect();
        self.durable_queues = self
            .trusted_keys
            .keys()
            .map(|peer_id| (peer_id.clone(), Vec::new()))
            .collect();
        Ok(())
    }

    pub fn socket_quota_config(&self) -> SocketQuotaConfig {
        self.quota_config
    }

    pub fn set_socket_quota_config(
        &mut self,
        quota_config: SocketQuotaConfig,
    ) -> Result<(), ConsensusError> {
        quota_config.validate()?;
        if self.peer_quotas.values().any(|quota| {
            quota.in_flight_bytes > quota_config.max_in_flight_bytes_per_peer
                || quota.receive_window_bytes > quota_config.max_receive_bytes_per_window
        }) {
            return Err(ConsensusError::SocketQuota(
                "new socket quota would invalidate active peer state".into(),
            ));
        }
        self.quota_config = quota_config;
        Ok(())
    }

    pub fn socket_peer_metrics(
        &self,
        peer_id: &str,
    ) -> Result<SocketTransportMetrics, ConsensusError> {
        validate_node_id(peer_id)?;
        let quota = self
            .peer_quotas
            .get(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        let queued_frames = self
            .durable_queues
            .get(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        let durable_queue_bytes = queued_frames
            .iter()
            .map(|frame| frame.frame_bytes.len() as u64)
            .sum();
        Ok(SocketTransportMetrics {
            peer_id: peer_id.to_string(),
            in_flight_bytes: quota.in_flight_bytes,
            receive_window_start_tick: quota.receive_window_start_tick,
            receive_window_bytes: quota.receive_window_bytes,
            max_in_flight_bytes: self.quota_config.max_in_flight_bytes_per_peer,
            max_receive_bytes_per_window: self.quota_config.max_receive_bytes_per_window,
            receive_window_ticks: self.quota_config.receive_window_ticks,
            admitted_frames: quota.admitted_frames,
            rejected_frames: quota.rejected_frames,
            backpressured_sends: quota.backpressured_sends,
            backpressured_receives: quota.backpressured_receives,
            durable_queue_frames: queued_frames.len() as u64,
            durable_queue_bytes,
            next_queue_sequence: *self
                .next_queue_sequences
                .get(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?,
        })
    }

    pub fn admit_send(
        &mut self,
        peer_id: &str,
        frame_bytes: u64,
        now_tick: u64,
    ) -> Result<SocketBackpressureAction, ConsensusError> {
        validate_node_id(peer_id)?;
        if !self.trusted_keys.contains_key(peer_id) {
            return Err(ConsensusError::UnknownMember(peer_id.to_string()));
        }
        if frame_bytes == 0 || frame_bytes as usize > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let quota = self
            .peer_quotas
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        let available = self
            .quota_config
            .max_in_flight_bytes_per_peer
            .saturating_sub(quota.in_flight_bytes);
        if frame_bytes > available {
            quota.backpressured_sends = quota.backpressured_sends.saturating_add(1);
            let retry_at_tick = now_tick
                .checked_add(self.quota_config.retry_backoff_ticks)
                .ok_or_else(|| ConsensusError::SocketQuota("send retry tick overflow".into()))?;
            return Ok(SocketBackpressureAction::Backpressured {
                retry_at_tick,
                available_bytes: available,
            });
        }
        quota.in_flight_bytes = quota
            .in_flight_bytes
            .checked_add(frame_bytes)
            .ok_or_else(|| ConsensusError::SocketQuota("send quota byte overflow".into()))?;
        quota.admitted_frames = quota.admitted_frames.saturating_add(1);
        Ok(SocketBackpressureAction::Admitted {
            frame_bytes,
            available_bytes: available.saturating_sub(frame_bytes),
        })
    }

    pub fn complete_send(&mut self, peer_id: &str, frame_bytes: u64) -> Result<(), ConsensusError> {
        validate_node_id(peer_id)?;
        let quota = self
            .peer_quotas
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        if frame_bytes == 0 || frame_bytes > quota.in_flight_bytes {
            return Err(ConsensusError::SocketQuota(
                "send completion exceeds the admitted peer bytes".into(),
            ));
        }
        quota.in_flight_bytes -= frame_bytes;
        Ok(())
    }

    fn admit_receive(
        &mut self,
        peer_id: &str,
        frame_bytes: u64,
        now_tick: u64,
    ) -> Result<SocketBackpressureAction, ConsensusError> {
        validate_node_id(peer_id)?;
        if frame_bytes == 0 || frame_bytes as usize > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let quota = self
            .peer_quotas
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        if let Some(start_tick) = quota.receive_window_start_tick {
            if now_tick < start_tick {
                return Err(ConsensusError::SocketQuota(
                    "receive quota clock regressed".into(),
                ));
            }
            if now_tick.saturating_sub(start_tick) >= self.quota_config.receive_window_ticks {
                quota.receive_window_start_tick = Some(now_tick);
                quota.receive_window_bytes = 0;
            }
        } else {
            quota.receive_window_start_tick = Some(now_tick);
            quota.receive_window_bytes = 0;
        }
        let available = self
            .quota_config
            .max_receive_bytes_per_window
            .saturating_sub(quota.receive_window_bytes);
        if frame_bytes > available {
            quota.backpressured_receives = quota.backpressured_receives.saturating_add(1);
            let retry_at_tick = now_tick
                .checked_add(self.quota_config.retry_backoff_ticks)
                .ok_or_else(|| ConsensusError::SocketQuota("receive retry tick overflow".into()))?;
            return Ok(SocketBackpressureAction::Backpressured {
                retry_at_tick,
                available_bytes: available,
            });
        }
        quota.receive_window_bytes = quota
            .receive_window_bytes
            .checked_add(frame_bytes)
            .ok_or_else(|| ConsensusError::SocketQuota("receive quota byte overflow".into()))?;
        quota.admitted_frames = quota.admitted_frames.saturating_add(1);
        Ok(SocketBackpressureAction::Admitted {
            frame_bytes,
            available_bytes: available.saturating_sub(frame_bytes),
        })
    }

    pub fn durable_queue_state(&self) -> Result<DurableSocketQueueState, ConsensusError> {
        DurableSocketQueueState::new(
            &self.cluster_id,
            &self.node_id,
            self.replay_epoch,
            self.quota_config,
            self.peer_quotas.clone(),
            self.next_queue_sequences.clone(),
            self.durable_queues.clone(),
        )
    }

    pub fn persist_durable_queue(
        &self,
        store: &DurableSocketQueueStore,
    ) -> Result<(), ConsensusError> {
        let state = self.durable_queue_state()?;
        store.save(&state)
    }

    pub fn restore_durable_queue(
        &mut self,
        state: DurableSocketQueueState,
    ) -> Result<(), ConsensusError> {
        state.validate()?;
        if state.cluster_id != self.cluster_id || state.node_id != self.node_id {
            return Err(ConsensusError::SocketQuota(
                "durable queue identity does not match transport".into(),
            ));
        }
        if state.replay_epoch != self.replay_epoch {
            return Err(ConsensusError::ReplayEpochMismatch {
                expected: self.replay_epoch,
                received: state.replay_epoch,
            });
        }
        let trusted_peers: BTreeSet<String> = self.trusted_keys.keys().cloned().collect();
        if trusted_peers != state.peer_quotas.keys().cloned().collect()
            || trusted_peers != state.next_queue_sequences.keys().cloned().collect()
            || trusted_peers != state.queued_frames.keys().cloned().collect()
        {
            return Err(ConsensusError::SocketQuota(
                "durable queue membership does not match transport".into(),
            ));
        }
        self.set_socket_quota_config(state.quota_config)?;
        self.peer_quotas = state.peer_quotas;
        self.next_queue_sequences = state.next_queue_sequences;
        self.durable_queues = state.queued_frames;
        Ok(())
    }

    pub fn restore_durable_queue_from_store(
        &mut self,
        store: &DurableSocketQueueStore,
    ) -> Result<(), ConsensusError> {
        let state = store.load()?;
        self.restore_durable_queue(state)
    }

    fn durable_queue_bytes(&self) -> Result<u64, ConsensusError> {
        self.durable_queues
            .values()
            .flat_map(|frames| frames.iter())
            .try_fold(0u64, |total, frame| {
                total
                    .checked_add(frame.frame_bytes.len() as u64)
                    .ok_or_else(|| {
                        ConsensusError::SocketQuota("durable queue byte overflow".into())
                    })
            })
    }

    pub fn durable_queue_frame(
        &self,
        peer_id: &str,
    ) -> Result<Option<DurableSocketQueueFrame>, ConsensusError> {
        validate_node_id(peer_id)?;
        let frames = self
            .durable_queues
            .get(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        Ok(frames.first().cloned())
    }

    pub fn enqueue_durable_frame_with_backpressure(
        &mut self,
        store: &DurableSocketQueueStore,
        peer_id: &str,
        envelope: &AuthenticatedConsensusEnvelope,
        now_tick: u64,
    ) -> Result<SocketBackpressureAction, ConsensusError> {
        validate_node_id(peer_id)?;
        if !self.trusted_keys.contains_key(peer_id) {
            return Err(ConsensusError::UnknownMember(peer_id.to_string()));
        }
        if envelope.sender_id != self.node_id {
            return Err(ConsensusError::Unauthenticated(
                "transport cannot enqueue on behalf of another node".into(),
            ));
        }
        envelope.verify_for_cluster_epoch(
            &self.cluster_id,
            &envelope.sender_id,
            self.trusted_keys
                .get(&envelope.sender_id)
                .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?,
            self.replay_epoch,
            self.replay_term_floor,
        )?;
        let frame_bytes = serde_json::to_vec(envelope)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if frame_bytes.len() > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let frame_len = frame_bytes.len() as u64;
        let queue_bytes = self.durable_queue_bytes()?;
        if queue_bytes
            .checked_add(frame_len)
            .ok_or_else(|| ConsensusError::SocketQuota("durable queue byte overflow".into()))?
            > MAX_DURABLE_SOCKET_QUEUE_BYTES
        {
            let quota = self
                .peer_quotas
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            quota.backpressured_sends = quota.backpressured_sends.saturating_add(1);
            let retry_at_tick = now_tick
                .checked_add(self.quota_config.retry_backoff_ticks)
                .ok_or_else(|| ConsensusError::SocketQuota("queue retry tick overflow".into()))?;
            return Ok(SocketBackpressureAction::Backpressured {
                retry_at_tick,
                available_bytes: MAX_DURABLE_SOCKET_QUEUE_BYTES.saturating_sub(queue_bytes),
            });
        }
        let admission = self.admit_send(peer_id, frame_len, now_tick)?;
        if matches!(admission, SocketBackpressureAction::Backpressured { .. }) {
            return Ok(admission);
        }
        let sequence = *self
            .next_queue_sequences
            .get(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        let frame = DurableSocketQueueFrame::new(peer_id, sequence, frame_bytes)?;
        let frames = self
            .durable_queues
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        if frames.len() >= MAX_DURABLE_SOCKET_QUEUE_ENTRIES {
            let quota = self
                .peer_quotas
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            quota.in_flight_bytes = quota.in_flight_bytes.saturating_sub(frame_len);
            quota.admitted_frames = quota.admitted_frames.saturating_sub(1);
            quota.backpressured_sends = quota.backpressured_sends.saturating_add(1);
            let retry_at_tick = now_tick
                .checked_add(self.quota_config.retry_backoff_ticks)
                .ok_or_else(|| ConsensusError::SocketQuota("queue retry tick overflow".into()))?;
            return Ok(SocketBackpressureAction::Backpressured {
                retry_at_tick,
                available_bytes: 0,
            });
        }
        frames.push(frame);
        *self
            .next_queue_sequences
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))? = sequence
            .checked_add(1)
            .ok_or_else(|| ConsensusError::SocketQuota("queue sequence overflow".into()))?;
        if let Err(error) = self.persist_durable_queue(store) {
            let frames = self
                .durable_queues
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            let _ = frames.pop();
            *self
                .next_queue_sequences
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))? = sequence;
            let quota = self
                .peer_quotas
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            quota.in_flight_bytes = quota.in_flight_bytes.saturating_sub(frame_len);
            quota.admitted_frames = quota.admitted_frames.saturating_sub(1);
            return Err(error);
        }
        Ok(admission)
    }

    pub fn acknowledge_durable_frame(
        &mut self,
        store: &DurableSocketQueueStore,
        peer_id: &str,
        sequence: u64,
    ) -> Result<(), ConsensusError> {
        validate_node_id(peer_id)?;
        if sequence == 0 {
            return Err(ConsensusError::SocketQuota(
                "queue sequence must be positive".into(),
            ));
        }
        let frames = self
            .durable_queues
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        let position = frames
            .iter()
            .position(|frame| frame.sequence == sequence)
            .ok_or_else(|| ConsensusError::SocketQuota("queue frame is not present".into()))?;
        if position != 0 {
            return Err(ConsensusError::SocketQuota(
                "durable queue acknowledgements must be FIFO".into(),
            ));
        }
        let frame = frames.remove(position);
        let quota = self
            .peer_quotas
            .get_mut(peer_id)
            .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
        quota.in_flight_bytes = quota
            .in_flight_bytes
            .checked_sub(frame.frame_bytes.len() as u64)
            .ok_or_else(|| ConsensusError::SocketQuota("queue quota underflow".into()))?;
        if let Err(error) = self.persist_durable_queue(store) {
            let frames = self
                .durable_queues
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            frames.insert(position, frame.clone());
            let quota = self
                .peer_quotas
                .get_mut(peer_id)
                .ok_or_else(|| ConsensusError::UnknownMember(peer_id.to_string()))?;
            quota.in_flight_bytes = quota
                .in_flight_bytes
                .checked_add(frame.frame_bytes.len() as u64)
                .ok_or_else(|| ConsensusError::SocketQuota("queue quota overflow".into()))?;
            return Err(error);
        }
        Ok(())
    }

    pub fn replay_window_len(&self, sender_id: &str) -> Result<usize, ConsensusError> {
        validate_node_id(sender_id)?;
        self.replay_windows
            .get(sender_id)
            .map(ReplayWindow::len)
            .ok_or_else(|| ConsensusError::UnknownMember(sender_id.to_string()))
    }

    pub fn send_to_peer_with_backpressure(
        &mut self,
        stream: &mut TcpStream,
        peer_id: &str,
        envelope: &AuthenticatedConsensusEnvelope,
        now_tick: u64,
    ) -> Result<SocketBackpressureAction, ConsensusError> {
        validate_node_id(peer_id)?;
        if !self.trusted_keys.contains_key(peer_id) {
            return Err(ConsensusError::UnknownMember(peer_id.to_string()));
        }
        if envelope.sender_id != self.node_id {
            return Err(ConsensusError::Unauthenticated(
                "transport cannot send on behalf of another node".into(),
            ));
        }
        envelope.verify_for_cluster_epoch(
            &self.cluster_id,
            &envelope.sender_id,
            self.trusted_keys
                .get(&envelope.sender_id)
                .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?,
            self.replay_epoch,
            self.replay_term_floor,
        )?;
        let bytes = serde_json::to_vec(envelope)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let admission = self.admit_send(peer_id, bytes.len() as u64, now_tick)?;
        if matches!(admission, SocketBackpressureAction::Backpressured { .. }) {
            return Ok(admission);
        }
        let length = (bytes.len() as u32).to_be_bytes();
        let result = stream
            .write_all(&length)
            .and_then(|_| stream.write_all(&bytes))
            .and_then(|_| stream.flush())
            .map_err(|error| ConsensusError::Transport(error.to_string()));
        self.complete_send(peer_id, bytes.len() as u64)?;
        result.map(|_| admission)
    }

    pub fn send(
        &self,
        stream: &mut TcpStream,
        envelope: &AuthenticatedConsensusEnvelope,
    ) -> Result<(), ConsensusError> {
        if envelope.sender_id != self.node_id {
            return Err(ConsensusError::Unauthenticated(
                "transport cannot send on behalf of another node".into(),
            ));
        }
        envelope.verify_for_cluster_epoch(
            &self.cluster_id,
            &envelope.sender_id,
            self.trusted_keys
                .get(&envelope.sender_id)
                .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?,
            self.replay_epoch,
            self.replay_term_floor,
        )?;
        let bytes = serde_json::to_vec(envelope)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if bytes.len() > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let length = (bytes.len() as u32).to_be_bytes();
        stream
            .write_all(&length)
            .and_then(|_| stream.write_all(&bytes))
            .and_then(|_| stream.flush())
            .map_err(|error| ConsensusError::Transport(error.to_string()))
    }

    pub fn receive_with_backpressure(
        &mut self,
        stream: &mut TcpStream,
        now_tick: u64,
    ) -> Result<SocketReceiveAction, ConsensusError> {
        let mut length_bytes = [0u8; 4];
        stream
            .read_exact(&mut length_bytes)
            .map_err(|error| ConsensusError::Transport(error.to_string()))?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        if length == 0 || length > self.max_frame_bytes {
            return Err(ConsensusError::FrameTooLarge);
        }
        let mut bytes = vec![0u8; length];
        stream
            .read_exact(&mut bytes)
            .map_err(|error| ConsensusError::Transport(error.to_string()))?;
        let envelope: AuthenticatedConsensusEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let trusted_key = self
            .trusted_keys
            .get(&envelope.sender_id)
            .cloned()
            .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?;
        envelope.verify_for_cluster_epoch(
            &self.cluster_id,
            &envelope.sender_id,
            &trusted_key,
            self.replay_epoch,
            self.replay_term_floor,
        )?;
        let window = self
            .replay_windows
            .get(&envelope.sender_id)
            .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?;
        if window.contains_nonce(&envelope.nonce) {
            return Err(ConsensusError::ReplayDetected);
        }
        let admission = self.admit_receive(&envelope.sender_id, length as u64, now_tick)?;
        if let SocketBackpressureAction::Backpressured {
            retry_at_tick,
            available_bytes,
        } = admission
        {
            return Ok(SocketReceiveAction::Backpressured {
                retry_at_tick,
                available_bytes,
            });
        }
        self.replay_windows
            .get_mut(&envelope.sender_id)
            .ok_or_else(|| ConsensusError::UnknownMember(envelope.sender_id.clone()))?
            .accept(&envelope, &trusted_key)?;
        Ok(SocketReceiveAction::Received {
            envelope,
            frame_bytes: length as u64,
        })
    }

    pub fn receive(
        &mut self,
        stream: &mut TcpStream,
    ) -> Result<AuthenticatedConsensusEnvelope, ConsensusError> {
        match self.receive_with_backpressure(stream, 0)? {
            SocketReceiveAction::Received { envelope, .. } => Ok(envelope),
            SocketReceiveAction::Backpressured { .. } => Err(ConsensusError::SocketQuota(
                "receive is backpressured; use receive_with_backpressure with a retry tick".into(),
            )),
        }
    }

    pub fn listen_once_with_backpressure(
        &mut self,
        listener: &TcpListener,
        now_tick: u64,
    ) -> Result<SocketReceiveAction, ConsensusError> {
        let (mut stream, _) = listener
            .accept()
            .map_err(|error| ConsensusError::Transport(error.to_string()))?;
        self.receive_with_backpressure(&mut stream, now_tick)
    }

    pub fn listen_once(
        &mut self,
        listener: &TcpListener,
    ) -> Result<AuthenticatedConsensusEnvelope, ConsensusError> {
        let (mut stream, _) = listener
            .accept()
            .map_err(|error| ConsensusError::Transport(error.to_string()))?;
        self.receive(&mut stream)
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
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("snapshot")
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

    pub fn recover_staging(&self) -> Result<bool, ConsensusError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let temporary = parent.join(format!(
            ".{}.tmp",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("snapshot")
        ));
        if !temporary.exists() {
            return Ok(false);
        }
        fs::remove_file(&temporary)
            .map_err(|error| ConsensusError::SnapshotPersistence(error.to_string()))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(true)
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactionLifecycle {
    Staged,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionManifest {
    pub cluster_id: String,
    pub source_node_id: String,
    pub term: u64,
    pub last_included_index: u64,
    pub last_included_term: u64,
    pub retained_suffix_start: u64,
    pub retained_suffix_end: u64,
    pub snapshot_state_hash: String,
    pub configuration_hash: String,
    pub snapshot_sha256: String,
    pub lifecycle: CompactionLifecycle,
    pub manifest_hash: String,
}

impl CompactionManifest {
    pub fn new(
        cluster_id: &str,
        source_node_id: &str,
        snapshot: &ConfigurationBoundSnapshot,
        retained_suffix_end: u64,
    ) -> Result<Self, ConsensusError> {
        snapshot.validate()?;
        validate_cluster_id(cluster_id)?;
        validate_node_id(source_node_id)?;
        let retained_suffix_start = snapshot.last_included_index.saturating_add(1);
        if retained_suffix_end < retained_suffix_start.saturating_sub(1) {
            return Err(ConsensusError::InvalidCompactionManifest(
                "retained suffix end precedes snapshot frontier".into(),
            ));
        }
        let snapshot_bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let mut manifest = Self {
            cluster_id: cluster_id.to_string(),
            source_node_id: source_node_id.to_string(),
            term: snapshot.term,
            last_included_index: snapshot.last_included_index,
            last_included_term: snapshot.last_included_term,
            retained_suffix_start,
            retained_suffix_end,
            snapshot_state_hash: snapshot.state_hash.clone(),
            configuration_hash: snapshot.configuration_hash.clone(),
            snapshot_sha256: digest_bytes(&snapshot_bytes),
            lifecycle: CompactionLifecycle::Staged,
            manifest_hash: String::new(),
        };
        manifest.manifest_hash = manifest.content_hash()?;
        manifest.validate(snapshot)?;
        Ok(manifest)
    }

    fn content_hash(&self) -> Result<String, ConsensusError> {
        digest_json(&(
            &self.cluster_id,
            &self.source_node_id,
            self.term,
            self.last_included_index,
            self.last_included_term,
            self.retained_suffix_start,
            self.retained_suffix_end,
            &self.snapshot_state_hash,
            &self.configuration_hash,
            &self.snapshot_sha256,
            self.lifecycle,
        ))
    }

    pub fn validate(&self, snapshot: &ConfigurationBoundSnapshot) -> Result<(), ConsensusError> {
        snapshot.validate()?;
        validate_cluster_id(&self.cluster_id)?;
        validate_node_id(&self.source_node_id)?;
        if self.term != snapshot.term
            || self.last_included_index != snapshot.last_included_index
            || self.last_included_term != snapshot.last_included_term
            || self.retained_suffix_start != snapshot.last_included_index.saturating_add(1)
            || self.snapshot_state_hash != snapshot.state_hash
            || self.configuration_hash != snapshot.configuration_hash
        {
            return Err(ConsensusError::InvalidCompactionManifest(
                "manifest metadata does not match snapshot".into(),
            ));
        }
        if self.retained_suffix_end < self.retained_suffix_start.saturating_sub(1)
            || validate_hex_digest(&self.snapshot_sha256).is_err()
            || self.content_hash()? != self.manifest_hash
        {
            return Err(ConsensusError::InvalidCompactionManifest(
                "manifest frontier or hash is invalid".into(),
            ));
        }
        let snapshot_bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if digest_bytes(&snapshot_bytes) != self.snapshot_sha256 {
            return Err(ConsensusError::InvalidCompactionManifest(
                "snapshot bytes do not match manifest hash".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionRecoveryOutcome {
    NoStaging,
    Finalized,
    Aborted,
}

#[derive(Debug, Clone)]
pub struct DurableCompactionStore {
    snapshot_path: PathBuf,
    manifest_path: PathBuf,
}

impl DurableCompactionStore {
    pub fn new(snapshot_path: impl AsRef<Path>, manifest_path: impl AsRef<Path>) -> Self {
        Self {
            snapshot_path: snapshot_path.as_ref().to_path_buf(),
            manifest_path: manifest_path.as_ref().to_path_buf(),
        }
    }

    pub fn staging_paths(&self) -> (PathBuf, PathBuf) {
        (self.snapshot_staging_path(), self.manifest_staging_path())
    }

    pub fn recovery_paths(&self) -> (PathBuf, PathBuf, PathBuf) {
        (
            self.snapshot_backup_path(),
            self.manifest_backup_path(),
            self.cutover_marker_path(),
        )
    }

    fn snapshot_staging_path(&self) -> PathBuf {
        self.snapshot_path.with_extension("snapshot.tmp")
    }

    fn manifest_staging_path(&self) -> PathBuf {
        self.manifest_path.with_extension("manifest.tmp")
    }

    fn parent_directory(&self) -> &Path {
        self.snapshot_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
    }

    fn snapshot_backup_path(&self) -> PathBuf {
        self.snapshot_path.with_extension("snapshot.bak")
    }

    fn manifest_backup_path(&self) -> PathBuf {
        self.manifest_path.with_extension("manifest.bak")
    }

    fn cutover_marker_path(&self) -> PathBuf {
        self.manifest_path.with_extension("cutover.marker")
    }

    fn sync_directory(&self) -> Result<(), ConsensusError> {
        if let Ok(directory) = OpenOptions::new().read(true).open(self.parent_directory()) {
            directory
                .sync_all()
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        }
        Ok(())
    }

    fn backup_existing_pair(&self) -> Result<(), ConsensusError> {
        let snapshot_exists = self.snapshot_path.exists();
        let manifest_exists = self.manifest_path.exists();
        if !snapshot_exists && !manifest_exists {
            return Ok(());
        }
        if snapshot_exists != manifest_exists {
            return Err(ConsensusError::CompactionPersistence(
                "durable snapshot and manifest are not a pair".into(),
            ));
        }
        if self.snapshot_backup_path().exists() || self.manifest_backup_path().exists() {
            return Ok(());
        }
        if self.load_latest()?.is_none() {
            return Err(ConsensusError::CompactionPersistence(
                "existing durable pair failed validation".into(),
            ));
        }
        fs::rename(&self.snapshot_path, self.snapshot_backup_path())
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        fs::rename(&self.manifest_path, self.manifest_backup_path())
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        self.sync_directory()
    }

    fn cleanup_cutover_artifacts(&self) -> Result<(), ConsensusError> {
        let _ = fs::remove_file(self.snapshot_backup_path());
        let _ = fs::remove_file(self.manifest_backup_path());
        let _ = fs::remove_file(self.cutover_marker_path());
        self.sync_directory()
    }

    fn restore_previous_pair(&self) -> Result<(), ConsensusError> {
        let snapshot_backup = self.snapshot_backup_path();
        let manifest_backup = self.manifest_backup_path();
        if snapshot_backup.exists() && manifest_backup.exists() {
            let _ = fs::remove_file(&self.snapshot_path);
            let _ = fs::remove_file(&self.manifest_path);
            fs::rename(snapshot_backup, &self.snapshot_path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
            fs::rename(manifest_backup, &self.manifest_path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        }
        self.sync_directory()
    }

    fn load_snapshot_file(
        &self,
        path: &Path,
    ) -> Result<ConfigurationBoundSnapshot, ConsensusError> {
        serde_json::from_slice(
            &fs::read(path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))
    }

    fn load_manifest_file(&self, path: &Path) -> Result<CompactionManifest, ConsensusError> {
        serde_json::from_slice(
            &fs::read(path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))
    }

    pub fn stage(
        &self,
        snapshot: &ConfigurationBoundSnapshot,
        manifest: &CompactionManifest,
    ) -> Result<(), ConsensusError> {
        snapshot.validate()?;
        manifest.validate(snapshot)?;
        let snapshot_bytes = serde_json::to_vec(snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let manifest_bytes = serde_json::to_vec(manifest)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if manifest_bytes.len() as u64 > MAX_COMPACTION_MANIFEST_BYTES
            || snapshot_bytes.len() as u64 > MAX_SNAPSHOT_BYTES
        {
            return Err(ConsensusError::InvalidCompactionManifest(
                "staged compaction artifact exceeds its bound".into(),
            ));
        }
        fs::create_dir_all(self.parent_directory())
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        let snapshot_tmp = self.snapshot_staging_path();
        let manifest_tmp = self.manifest_staging_path();
        let mut snapshot_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&snapshot_tmp)
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        if let Err(error) = snapshot_file
            .write_all(&snapshot_bytes)
            .and_then(|_| snapshot_file.sync_all())
        {
            let _ = fs::remove_file(&snapshot_tmp);
            return Err(ConsensusError::CompactionPersistence(error.to_string()));
        }
        let mut manifest_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_tmp)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_file(&snapshot_tmp);
                return Err(ConsensusError::CompactionPersistence(error.to_string()));
            }
        };
        if let Err(error) = manifest_file
            .write_all(&manifest_bytes)
            .and_then(|_| manifest_file.sync_all())
        {
            let _ = fs::remove_file(&snapshot_tmp);
            let _ = fs::remove_file(&manifest_tmp);
            return Err(ConsensusError::CompactionPersistence(error.to_string()));
        }
        Ok(())
    }

    pub fn commit_staged(&self) -> Result<CompactionManifest, ConsensusError> {
        let (snapshot, manifest) = self.load_staged_pair()?;
        let mut committed = manifest.clone();
        committed.lifecycle = CompactionLifecycle::Committed;
        committed.manifest_hash = committed.content_hash()?;
        committed.validate(&snapshot)?;
        let committed_manifest_bytes = serde_json::to_vec(&committed)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        self.backup_existing_pair()?;
        let marker_bytes = serde_json::to_vec(&committed.manifest_hash)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let marker_path = self.cutover_marker_path();
        let mut marker = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&marker_path)
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        marker
            .write_all(&marker_bytes)
            .and_then(|_| marker.sync_all())
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        fs::rename(self.snapshot_staging_path(), &self.snapshot_path)
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        let manifest_tmp = self.manifest_staging_path();
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&manifest_tmp)
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        file.write_all(&committed_manifest_bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        fs::rename(manifest_tmp, &self.manifest_path)
            .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?;
        self.sync_directory()?;
        self.cleanup_cutover_artifacts()?;
        Ok(committed)
    }

    fn load_staged_pair(
        &self,
    ) -> Result<(ConfigurationBoundSnapshot, CompactionManifest), ConsensusError> {
        let snapshot: ConfigurationBoundSnapshot = serde_json::from_slice(
            &fs::read(self.snapshot_staging_path())
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let manifest: CompactionManifest = serde_json::from_slice(
            &fs::read(self.manifest_staging_path())
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        manifest.validate(&snapshot)?;
        Ok((snapshot, manifest))
    }

    pub fn load_latest(
        &self,
    ) -> Result<Option<(ConfigurationBoundSnapshot, CompactionManifest)>, ConsensusError> {
        if !self.snapshot_path.exists() || !self.manifest_path.exists() {
            return Ok(None);
        }
        let snapshot: ConfigurationBoundSnapshot = serde_json::from_slice(
            &fs::read(&self.snapshot_path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        let manifest: CompactionManifest = serde_json::from_slice(
            &fs::read(&self.manifest_path)
                .map_err(|error| ConsensusError::CompactionPersistence(error.to_string()))?,
        )
        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
        if manifest.lifecycle != CompactionLifecycle::Committed {
            return Err(ConsensusError::InvalidCompactionManifest(
                "durable manifest is not committed".into(),
            ));
        }
        manifest.validate(&snapshot)?;
        Ok(Some((snapshot, manifest)))
    }

    pub fn recover_compaction(&self) -> Result<CompactionRecoveryOutcome, ConsensusError> {
        let snapshot_tmp = self.snapshot_staging_path();
        let manifest_tmp = self.manifest_staging_path();
        let marker_exists = self.cutover_marker_path().exists();
        let snapshot_exists = snapshot_tmp.exists();
        let manifest_exists = manifest_tmp.exists();
        if !snapshot_exists && !manifest_exists && !marker_exists {
            return Ok(CompactionRecoveryOutcome::NoStaging);
        }
        if snapshot_exists && manifest_exists && self.load_staged_pair().is_ok() {
            self.commit_staged()?;
            return Ok(CompactionRecoveryOutcome::Finalized);
        }
        if !snapshot_exists && manifest_exists && self.snapshot_path.exists() {
            let snapshot = self.load_snapshot_file(&self.snapshot_path);
            let manifest = self.load_manifest_file(&manifest_tmp);
            if let (Ok(snapshot), Ok(manifest)) = (snapshot, manifest) {
                if manifest.validate(&snapshot).is_ok() {
                    let mut committed = manifest;
                    committed.lifecycle = CompactionLifecycle::Committed;
                    committed.manifest_hash = committed.content_hash()?;
                    committed.validate(&snapshot)?;
                    let bytes = serde_json::to_vec(&committed)
                        .map_err(|error| ConsensusError::Serialization(error.to_string()))?;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&manifest_tmp)
                        .map_err(|error| {
                            ConsensusError::CompactionPersistence(error.to_string())
                        })?;
                    file.write_all(&bytes)
                        .and_then(|_| file.sync_all())
                        .map_err(|error| {
                            ConsensusError::CompactionPersistence(error.to_string())
                        })?;
                    fs::rename(manifest_tmp, &self.manifest_path).map_err(|error| {
                        ConsensusError::CompactionPersistence(error.to_string())
                    })?;
                    self.sync_directory()?;
                    self.cleanup_cutover_artifacts()?;
                    return Ok(CompactionRecoveryOutcome::Finalized);
                }
            }
        }
        let _ = fs::remove_file(snapshot_tmp);
        let _ = fs::remove_file(manifest_tmp);
        if self.snapshot_backup_path().exists() && self.manifest_backup_path().exists() {
            self.restore_previous_pair()?;
        } else {
            let _ = fs::remove_file(self.cutover_marker_path());
            self.sync_directory()?;
        }
        Ok(CompactionRecoveryOutcome::Aborted)
    }
}

#[derive(Debug, Clone)]
struct PeerReplicationFlow {
    next_batch_id: u64,
    in_flight: Option<u64>,
    last_completed: Option<u64>,
    retry_at_tick: Option<u64>,
    sent_batches: u64,
    acknowledged_batches: u64,
    rejected_batches: u64,
}

#[derive(Debug, Clone)]
struct SnapshotReplicationState {
    active_transfer_id: Option<String>,
    snapshot: Option<ConfigurationBoundSnapshot>,
    snapshot_bytes: u64,
    bytes_sent: u64,
    bandwidth_window_start_tick: Option<u64>,
    bandwidth_window_bytes: u64,
    readiness: SnapshotInstallReadiness,
    last_installed_index: u64,
    last_installed_term: u64,
    retry_at_tick: Option<u64>,
    sent_transfers: u64,
    acknowledged_transfers: u64,
    rejected_transfers: u64,
    cancelled_transfers: u64,
}

impl SnapshotReplicationState {
    fn new() -> Self {
        Self {
            active_transfer_id: None,
            snapshot: None,
            snapshot_bytes: 0,
            bytes_sent: 0,
            bandwidth_window_start_tick: None,
            bandwidth_window_bytes: 0,
            readiness: SnapshotInstallReadiness::Unknown,
            last_installed_index: 0,
            last_installed_term: 0,
            retry_at_tick: None,
            sent_transfers: 0,
            acknowledged_transfers: 0,
            rejected_transfers: 0,
            cancelled_transfers: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct ReadIndexRound {
    request_id: String,
    key: String,
    term: u64,
    read_index: u64,
    acknowledgements: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct ConsensusNode {
    id: String,
    members: BTreeSet<String>,
    previous_members: Option<BTreeSet<String>>,
    configuration_phase: ConfigurationPhase,
    joint_config_index: Option<u64>,
    pending_finalization: Option<u64>,
    max_log_entries: usize,
    role: ConsensusRole,
    current_term: u64,
    voted_for: Option<String>,
    replay_epoch: u64,
    replay_term_floor: u64,
    log: Vec<LogEntry>,
    log_base_index: u64,
    log_base_term: u64,
    commit_index: u64,
    last_applied: u64,
    state: BTreeMap<String, String>,
    votes_received: BTreeSet<String>,
    replication_progress: BTreeMap<String, u64>,
    lease_config: LeaderLeaseConfig,
    lease_expiration_tick: Option<u64>,
    last_observed_tick: Option<u64>,
    clock_uncertain: bool,
    election_timer_config: ElectionTimerConfig,
    election_deadline_tick: Option<u64>,
    heartbeat_due_tick: Option<u64>,
    peer_last_heartbeat_tick: BTreeMap<String, u64>,
    replication_flow_config: ReplicationFlowConfig,
    peer_replication_flow: BTreeMap<String, PeerReplicationFlow>,
    compaction_config: LogCompactionConfig,
    compaction_coordination_config: CompactionCoordinationConfig,
    compacted_snapshot: Option<ConfigurationBoundSnapshot>,
    snapshot_bandwidth_config: SnapshotBandwidthConfig,
    snapshot_replication_state: BTreeMap<String, SnapshotReplicationState>,
    read_rounds: BTreeMap<String, ReadIndexRound>,
    completed_read_requests: BTreeSet<String>,
}

impl PeerReplicationFlow {
    fn new() -> Self {
        Self {
            next_batch_id: 1,
            in_flight: None,
            last_completed: None,
            retry_at_tick: None,
            sent_batches: 0,
            acknowledged_batches: 0,
            rejected_batches: 0,
        }
    }
}

impl ConsensusNode {
    pub fn new(
        id: &str,
        members: BTreeSet<String>,
        max_log_entries: usize,
    ) -> Result<Self, ConsensusError> {
        validate_node_id(id)?;
        validate_members(&members)?;
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
            previous_members: None,
            configuration_phase: ConfigurationPhase::Stable,
            joint_config_index: None,
            pending_finalization: None,
            max_log_entries,
            role: ConsensusRole::Follower,
            current_term: 0,
            voted_for: None,
            replay_epoch: 1,
            replay_term_floor: 1,
            log: Vec::new(),
            log_base_index: 0,
            log_base_term: 0,
            commit_index: 0,
            last_applied: 0,
            state: BTreeMap::new(),
            votes_received: BTreeSet::new(),
            replication_progress: BTreeMap::new(),
            lease_config: LeaderLeaseConfig::default(),
            lease_expiration_tick: None,
            last_observed_tick: None,
            clock_uncertain: false,
            election_timer_config: ElectionTimerConfig::default(),
            election_deadline_tick: None,
            heartbeat_due_tick: None,
            peer_last_heartbeat_tick: BTreeMap::new(),
            replication_flow_config: ReplicationFlowConfig::default(),
            peer_replication_flow: BTreeMap::new(),
            compaction_config: LogCompactionConfig::default(),
            compaction_coordination_config: CompactionCoordinationConfig::default(),
            compacted_snapshot: None,
            snapshot_bandwidth_config: SnapshotBandwidthConfig::default(),
            snapshot_replication_state: BTreeMap::new(),
            read_rounds: BTreeMap::new(),
            completed_read_requests: BTreeSet::new(),
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

    pub fn replay_epoch(&self) -> u64 {
        self.replay_epoch
    }

    pub fn replay_term_floor(&self) -> u64 {
        self.replay_term_floor
    }

    pub fn durable_consensus_state(
        &self,
        cluster_id: &str,
    ) -> Result<DurableConsensusState, ConsensusError> {
        DurableConsensusState::new(
            cluster_id,
            &self.id,
            self.current_term,
            self.voted_for.clone(),
            self.replay_epoch,
            self.replay_term_floor,
        )
    }

    pub fn persist_durable_consensus_state(
        &self,
        cluster_id: &str,
        store: &DurableConsensusStateStore,
    ) -> Result<(), ConsensusError> {
        store.save(&self.durable_consensus_state(cluster_id)?)
    }

    pub fn restore_durable_consensus_state(
        &mut self,
        cluster_id: &str,
        state: DurableConsensusState,
    ) -> Result<(), ConsensusError> {
        state.validate()?;
        if state.cluster_id != cluster_id || state.node_id != self.id {
            return Err(ConsensusError::DurableConsensusState(
                "durable state identity does not match this node".into(),
            ));
        }
        if let Some(voted_for) = &state.voted_for {
            if !self.accepted_members().contains(voted_for) {
                return Err(ConsensusError::DurableConsensusState(
                    "durable vote is not for an accepted member".into(),
                ));
            }
        }
        if state.current_term < self.current_term
            || state.replay_epoch < self.replay_epoch
            || state.replay_term_floor < self.replay_term_floor
        {
            return Err(ConsensusError::DurableConsensusState(
                "durable state would roll back local term or replay state".into(),
            ));
        }
        self.current_term = state.current_term;
        self.voted_for = state.voted_for;
        self.replay_epoch = state.replay_epoch;
        self.replay_term_floor = state.replay_term_floor;
        self.step_down_and_invalidate();
        self.votes_received.clear();
        Ok(())
    }

    fn record_current_term_for_replay(&mut self) {
        self.replay_term_floor = self.replay_term_floor.max(self.current_term);
    }

    pub fn advance_replay_epoch(
        &mut self,
        next_epoch: u64,
        next_term_floor: u64,
    ) -> Result<(), ConsensusError> {
        if next_epoch <= self.replay_epoch
            || next_term_floor < self.current_term
            || next_term_floor == 0
        {
            return Err(ConsensusError::DurableConsensusState(
                "replay epoch and term floor must advance monotonically".into(),
            ));
        }
        self.replay_epoch = next_epoch;
        self.replay_term_floor = next_term_floor;
        Ok(())
    }

    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }

    pub fn log_len(&self) -> usize {
        self.last_log_index() as usize
    }

    pub fn quorum_size(&self) -> usize {
        quorum_size(&self.members)
    }

    pub fn configuration_phase(&self) -> ConfigurationPhase {
        self.configuration_phase
    }

    pub fn members(&self) -> BTreeSet<String> {
        self.members.clone()
    }

    pub fn previous_members(&self) -> Option<BTreeSet<String>> {
        self.previous_members.clone()
    }

    pub fn begin_membership_change(
        &mut self,
        proposed_members: BTreeSet<String>,
    ) -> Result<LogEntry, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        if self.configuration_phase != ConfigurationPhase::Stable {
            return Err(ConsensusError::MembershipChangeInProgress);
        }
        validate_members(&proposed_members)?;
        if proposed_members == self.members {
            return Err(ConsensusError::InvalidMembershipChange(
                "proposed membership is unchanged".into(),
            ));
        }
        let old_members = self.members.clone();
        let entry = self.propose(StateCommand::ConfigurationJoint {
            old_members: old_members.clone(),
            new_members: proposed_members.clone(),
        })?;
        self.previous_members = Some(old_members);
        self.members = proposed_members;
        self.configuration_phase = ConfigurationPhase::Joint;
        self.joint_config_index = Some(entry.index);
        self.pending_finalization = None;
        self.invalidate_lease();
        self.read_rounds.clear();
        self.rebuild_replication_progress();
        Ok(entry)
    }

    pub fn finalize_membership_change(&mut self) -> Result<LogEntry, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        let old_members = self
            .previous_members
            .clone()
            .ok_or(ConsensusError::NoMembershipChange)?;
        let joint_index = self
            .joint_config_index
            .ok_or(ConsensusError::NoMembershipChange)?;
        if self.commit_index < joint_index {
            return Err(ConsensusError::InvalidMembershipChange(
                "joint configuration must commit before finalization".into(),
            ));
        }
        let entry = self.propose(StateCommand::ConfigurationFinal {
            members: self.members.clone(),
        })?;
        self.invalidate_lease();
        self.read_rounds.clear();
        self.pending_finalization = Some(entry.index);
        self.rebuild_replication_progress();
        if !self.members.contains(&self.id) && old_members.contains(&self.id) {
            self.step_down_and_invalidate();
            self.voted_for = None;
        }
        Ok(entry)
    }

    pub fn state_value(&self, key: &str) -> Option<&str> {
        self.state.get(key).map(String::as_str)
    }

    pub fn configure_leader_lease(
        &mut self,
        config: LeaderLeaseConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.lease_config = config;
        self.invalidate_lease();
        Ok(())
    }

    pub fn leader_lease_config(&self) -> LeaderLeaseConfig {
        self.lease_config
    }

    pub fn lease_expiration_tick(&self) -> Option<u64> {
        self.lease_expiration_tick
    }

    pub fn clock_is_trusted(&self) -> bool {
        !self.clock_uncertain
    }

    pub fn reanchor_monotonic_clock(&mut self, now_tick: u64) -> Result<(), ConsensusError> {
        if self
            .last_observed_tick
            .is_some_and(|previous| now_tick < previous)
            && !self.clock_uncertain
        {
            return Err(ConsensusError::ClockUntrusted);
        }
        self.last_observed_tick = Some(now_tick);
        self.clock_uncertain = false;
        self.invalidate_lease();
        self.heartbeat_due_tick = None;
        self.reset_election_deadline(now_tick)?;
        self.peer_last_heartbeat_tick.clear();
        Ok(())
    }

    pub fn lease_is_valid(&self, now_tick: u64) -> bool {
        !self.clock_uncertain
            && self.role == ConsensusRole::Leader
            && self.lease_expiration_tick.is_some_and(|expiration| {
                now_tick
                    .checked_add(self.lease_config.max_clock_drift_ticks)
                    .is_some_and(|safe_now| safe_now < expiration)
            })
    }

    pub fn configure_election_timers(
        &mut self,
        config: ElectionTimerConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.election_timer_config = config;
        self.election_deadline_tick = None;
        self.heartbeat_due_tick = None;
        self.peer_last_heartbeat_tick.clear();
        Ok(())
    }

    pub fn election_timer_config(&self) -> ElectionTimerConfig {
        self.election_timer_config
    }

    pub fn election_deadline_tick(&self) -> Option<u64> {
        self.election_deadline_tick
    }

    pub fn record_peer_heartbeat(
        &mut self,
        peer_id: &str,
        now_tick: u64,
    ) -> Result<(), ConsensusError> {
        validate_node_id(peer_id)?;
        if !self.accepted_members().contains(peer_id) || peer_id == self.id {
            return Err(ConsensusError::InvalidPeer(peer_id.to_string()));
        }
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        self.peer_last_heartbeat_tick
            .insert(peer_id.to_string(), now_tick);
        if self.role != ConsensusRole::Leader {
            self.reset_election_deadline(now_tick)?;
        }
        Ok(())
    }

    pub fn peer_is_suspect(
        &mut self,
        peer_id: &str,
        now_tick: u64,
    ) -> Result<bool, ConsensusError> {
        validate_node_id(peer_id)?;
        if !self.accepted_members().contains(peer_id) || peer_id == self.id {
            return Err(ConsensusError::InvalidPeer(peer_id.to_string()));
        }
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        Ok(self
            .peer_last_heartbeat_tick
            .get(peer_id)
            .is_none_or(|last| {
                now_tick.saturating_sub(*last) >= self.election_timer_config.failure_detector_ticks
            }))
    }

    pub fn tick(&mut self, now_tick: u64) -> Result<ElectionTimerAction, ConsensusError> {
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        match self.role {
            ConsensusRole::Leader => {
                let due = self.heartbeat_due_tick.unwrap_or(now_tick);
                if now_tick < due {
                    return Ok(ElectionTimerAction::Idle);
                }
                self.heartbeat_due_tick = Some(
                    now_tick
                        .checked_add(self.election_timer_config.heartbeat_interval_ticks)
                        .ok_or_else(|| {
                            ConsensusError::InvalidElectionTimer(
                                "heartbeat deadline overflow".into(),
                            )
                        })?,
                );
                let peer_ids = self
                    .accepted_members()
                    .into_iter()
                    .filter(|member| member != &self.id)
                    .collect();
                Ok(ElectionTimerAction::SendHeartbeats(HeartbeatPlan {
                    term: self.current_term,
                    leader_id: self.id.clone(),
                    peer_ids,
                }))
            }
            ConsensusRole::Follower | ConsensusRole::Candidate => {
                let Some(deadline) = self.election_deadline_tick else {
                    self.reset_election_deadline(now_tick)?;
                    return Ok(ElectionTimerAction::Idle);
                };
                if now_tick < deadline {
                    return Ok(ElectionTimerAction::Idle);
                }
                let request = self.start_election()?;
                self.reset_election_deadline(now_tick)?;
                Ok(ElectionTimerAction::StartElection(request))
            }
        }
    }

    pub fn prepare_linearizable_read(
        &mut self,
        request: LinearizableReadRequest,
    ) -> Result<ReadIndexAction, ConsensusError> {
        request.validate()?;
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.observe_tick(request.now_tick);
        if self.completed_read_requests.contains(&request.request_id) {
            return Err(ConsensusError::DuplicateReadRequest(request.request_id));
        }
        if self.lease_is_valid(request.now_tick) {
            return Ok(ReadIndexAction::Lease(LinearizableReadPlan {
                request_id: request.request_id,
                key: request.key,
                term: self.current_term,
                read_index: self.commit_index,
                lease_fast_path: true,
            }));
        }
        if let Some(round) = self.read_rounds.get(&request.request_id) {
            if round.key != request.key {
                return Err(ConsensusError::DuplicateReadRequest(request.request_id));
            }
            return Ok(ReadIndexAction::Quorum(ReadIndexRequest::new(
                &round.request_id,
                round.term,
                &self.id,
                round.read_index,
            )?));
        }
        if self.read_rounds.len() >= MAX_READ_ROUNDS {
            return Err(ConsensusError::InvalidReadRequest(
                "too many in-flight read-index rounds".into(),
            ));
        }
        let read_index = self.commit_index;
        let read_request =
            ReadIndexRequest::new(&request.request_id, self.current_term, &self.id, read_index)?;
        let mut acknowledgements = BTreeSet::new();
        acknowledgements.insert(self.id.clone());
        self.read_rounds.insert(
            request.request_id.clone(),
            ReadIndexRound {
                request_id: request.request_id,
                key: request.key,
                term: self.current_term,
                read_index,
                acknowledgements,
            },
        );
        Ok(ReadIndexAction::Quorum(read_request))
    }

    pub fn handle_read_index_request(
        &mut self,
        request: ReadIndexRequest,
    ) -> Result<ReadIndexResponse, ConsensusError> {
        request.validate()?;
        if !self.accepted_members().contains(&request.leader_id) {
            return Err(ConsensusError::UnknownMember(request.leader_id));
        }
        if request.term < self.current_term {
            return Ok(ReadIndexResponse {
                request_id: request.request_id,
                term: self.current_term,
                follower_id: self.id.clone(),
                read_index: request.read_index,
                accepted: false,
            });
        }
        if request.term > self.current_term {
            self.current_term = request.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
            self.votes_received.clear();
        } else {
            self.invalidate_lease();
        }
        self.role = ConsensusRole::Follower;
        Ok(ReadIndexResponse {
            request_id: request.request_id,
            term: self.current_term,
            follower_id: self.id.clone(),
            read_index: request.read_index,
            accepted: request.read_index <= self.commit_index,
        })
    }

    pub fn acknowledge_read_index(
        &mut self,
        response: ReadIndexResponse,
        now_tick: u64,
    ) -> Result<Option<LinearizableReadPlan>, ConsensusError> {
        response.validate()?;
        if !self.accepted_members().contains(&response.follower_id) {
            return Err(ConsensusError::UnknownMember(response.follower_id));
        }
        self.observe_tick(now_tick);
        if response.term > self.current_term {
            self.current_term = response.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
            self.votes_received.clear();
            return Ok(None);
        }
        if self.role != ConsensusRole::Leader || response.term != self.current_term {
            return Ok(None);
        }
        if self.completed_read_requests.contains(&response.request_id) {
            return Err(ConsensusError::DuplicateReadRequest(response.request_id));
        }
        let quorum_reached = {
            let Some(round) = self.read_rounds.get_mut(&response.request_id) else {
                return Err(ConsensusError::UnknownReadIndex(response.request_id));
            };
            if response.read_index != round.read_index || response.term != round.term {
                return Err(ConsensusError::InvalidReadRequest(
                    "read-index response does not match the active round".into(),
                ));
            }
            if response.accepted {
                round.acknowledgements.insert(response.follower_id);
            }
            let acknowledgements = round.acknowledgements.clone();
            response.accepted && self.has_vote_quorum(&acknowledgements)
        };
        if !quorum_reached {
            return Ok(None);
        }
        let round = self
            .read_rounds
            .remove(&response.request_id)
            .expect("active read-index round exists");
        self.install_lease(now_tick);
        Ok(Some(LinearizableReadPlan {
            request_id: round.request_id,
            key: round.key,
            term: round.term,
            read_index: round.read_index,
            lease_fast_path: false,
        }))
    }

    pub fn execute_linearizable_read(
        &mut self,
        plan: LinearizableReadPlan,
        now_tick: u64,
    ) -> Result<Option<String>, ConsensusError> {
        validate_read_request_id(&plan.request_id)?;
        validate_key(&plan.key).map_err(|error| {
            ConsensusError::InvalidReadRequest(format!("invalid query key: {}", error))
        })?;
        self.observe_tick(now_tick);
        if self.role != ConsensusRole::Leader || plan.term != self.current_term {
            return Err(ConsensusError::ReadNotReady(
                "the read plan is from a different leader term".into(),
            ));
        }
        if plan.lease_fast_path && !self.lease_is_valid(now_tick) {
            return Err(ConsensusError::LeaseExpired);
        }
        if plan.read_index > self.commit_index || self.last_applied < plan.read_index {
            return Err(ConsensusError::ReadNotReady(format!(
                "applied index {} is below read index {}",
                self.last_applied, plan.read_index
            )));
        }
        if self.completed_read_requests.contains(&plan.request_id) {
            return Err(ConsensusError::DuplicateReadRequest(plan.request_id));
        }
        let value = self.state.get(&plan.key).cloned();
        self.remember_completed_read(&plan.request_id);
        Ok(value)
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

    pub fn compaction_config(&self) -> LogCompactionConfig {
        self.compaction_config
    }

    pub fn configure_log_compaction(
        &mut self,
        config: LogCompactionConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.compaction_config = config;
        Ok(())
    }

    pub fn configure_compaction_coordination(
        &mut self,
        config: CompactionCoordinationConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.compaction_coordination_config = config;
        Ok(())
    }

    pub fn compaction_coordination_config(&self) -> CompactionCoordinationConfig {
        self.compaction_coordination_config
    }

    pub fn compacted_log_frontier(&self) -> (u64, u64) {
        (self.log_base_index, self.log_base_term)
    }

    pub fn compaction_coordination_plan(
        &self,
        target_index: u64,
    ) -> Result<CompactionCoordinationPlan, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.compaction_coordination_config.validate()?;
        if target_index <= self.log_base_index
            || target_index > self.last_applied
            || target_index > self.commit_index
        {
            return Err(ConsensusError::CompactionCoordination(
                "coordination target must be committed, applied, and beyond the retained frontier"
                    .into(),
            ));
        }
        let target_term = self
            .entry_at(target_index)
            .map(|entry| entry.term)
            .ok_or_else(|| {
                ConsensusError::CompactionCoordination(
                    "coordination target is not present in the retained log".into(),
                )
            })?;
        let configuration_hash = digest_json(&(
            self.configuration_phase,
            &self.members,
            &self.previous_members,
        ))?;
        let remote_quorum = self.required_remote_compaction_quorum();
        let required_safe_followers = if self.compaction_coordination_config.require_quorum {
            remote_quorum.max(self.compaction_coordination_config.min_safe_followers)
        } else {
            self.compaction_coordination_config.min_safe_followers
        };
        let mut safe_followers = Vec::new();
        let mut blocked_followers = Vec::new();
        for follower_id in self.accepted_members() {
            if follower_id == self.id {
                continue;
            }
            let match_index = self
                .replication_progress
                .get(&follower_id)
                .copied()
                .unwrap_or_default()
                .min(target_index);
            let lag_entries = target_index.saturating_sub(match_index);
            let safe_for_compaction =
                lag_entries <= self.compaction_coordination_config.max_follower_lag_entries;
            let status = CompactionFollowerStatus {
                follower_id,
                match_index,
                target_index,
                lag_entries,
                safe_for_compaction,
            };
            if safe_for_compaction {
                safe_followers.push(status);
            } else {
                blocked_followers.push(status);
            }
        }
        let mut plan = CompactionCoordinationPlan {
            leader_id: self.id.clone(),
            target_index,
            target_term,
            commit_index: self.commit_index,
            applied_index: self.last_applied,
            configuration_phase: self.configuration_phase,
            configuration_hash,
            safe_followers,
            blocked_followers,
            required_safe_followers,
            quorum_required: self.compaction_coordination_config.require_quorum,
            ready: false,
            plan_hash: String::new(),
        };
        plan.ready = plan.safe_followers.len() >= plan.required_safe_followers;
        plan.plan_hash = plan.content_hash()?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn coordinate_compaction(
        &mut self,
        target_index: u64,
    ) -> Result<CompactionCoordinationAction, ConsensusError> {
        let plan = self.compaction_coordination_plan(target_index)?;
        if !plan.ready {
            return Ok(CompactionCoordinationAction::Waiting { plan });
        }
        let snapshot = self.compact_committed_log(target_index)?;
        Ok(CompactionCoordinationAction::Compacted { plan, snapshot })
    }

    pub fn retained_log_len(&self) -> usize {
        self.log.len()
    }

    pub fn configuration_bound_snapshot(
        &self,
    ) -> Result<ConfigurationBoundSnapshot, ConsensusError> {
        let last_included_index = self.log_base_index.max(self.last_applied);
        let last_included_term = if last_included_index == self.log_base_index {
            self.log_base_term
        } else {
            self.entry_at(last_included_index)
                .map(|entry| entry.term)
                .ok_or_else(|| {
                    ConsensusError::InvalidSnapshot(
                        "cannot derive configuration-bound snapshot term".into(),
                    )
                })?
        };
        if last_included_index == 0 || last_included_term == 0 {
            return Err(ConsensusError::LogCompaction(
                "configuration-bound snapshot requires an applied log frontier".into(),
            ));
        }
        let mut snapshot = ConfigurationBoundSnapshot {
            term: self.current_term.max(last_included_term),
            last_included_index,
            last_included_term,
            commit_index: self.commit_index,
            last_applied: self.last_applied,
            state: self.state.clone(),
            state_hash: digest_json(&self.state)?,
            configuration_phase: self.configuration_phase,
            members: self.members.clone(),
            previous_members: self.previous_members.clone(),
            configuration_hash: String::new(),
        };
        snapshot.configuration_hash = digest_json(&(
            snapshot.configuration_phase,
            &snapshot.members,
            &snapshot.previous_members,
        ))?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn compact_committed_log(
        &mut self,
        target_index: u64,
    ) -> Result<ConfigurationBoundSnapshot, ConsensusError> {
        if target_index <= self.log_base_index {
            return Err(ConsensusError::LogCompaction(
                "compaction target must advance the retained frontier".into(),
            ));
        }
        if target_index > self.last_applied || target_index > self.commit_index {
            return Err(ConsensusError::LogCompaction(
                "compaction target must be committed and applied".into(),
            ));
        }
        let end_index = self.last_log_index();
        let discard_count = target_index.saturating_sub(self.log_base_index) as usize;
        if discard_count > self.compaction_config.max_discard_entries {
            return Err(ConsensusError::LogCompaction(
                "compaction discard exceeds the configured bound".into(),
            ));
        }
        let retained_after = end_index.saturating_sub(target_index) as usize;
        if retained_after < self.compaction_config.min_retained_entries {
            return Err(ConsensusError::LogCompaction(
                "compaction would violate the retained suffix bound".into(),
            ));
        }
        let boundary = self.entry_at(target_index).cloned().ok_or_else(|| {
            ConsensusError::LogCompaction(
                "compaction target is not present in the retained log".into(),
            )
        })?;
        let snapshot = self.configuration_bound_snapshot()?;
        if snapshot.last_included_index != self.last_applied {
            return Err(ConsensusError::LogCompaction(
                "compaction snapshot frontier does not match the requested target".into(),
            ));
        }
        self.log.drain(..discard_count);
        self.log_base_index = target_index;
        self.log_base_term = boundary.term;
        self.compacted_snapshot = Some(snapshot.clone());
        self.replication_progress
            .insert(self.id.clone(), self.last_log_index());
        Ok(snapshot)
    }

    pub fn install_configuration_bound_snapshot(
        &mut self,
        snapshot: ConfigurationBoundSnapshot,
    ) -> Result<(), ConsensusError> {
        snapshot.validate()?;
        if snapshot.term < self.current_term
            || snapshot.commit_index < self.commit_index
            || snapshot.last_included_index < self.log_base_index
        {
            return Err(ConsensusError::InvalidSnapshot(
                "configuration-bound snapshot is older than local state".into(),
            ));
        }
        self.current_term = snapshot.term;
        self.record_current_term_for_replay();
        self.step_down_and_invalidate();
        self.voted_for = None;
        self.votes_received.clear();
        self.commit_index = snapshot.commit_index;
        self.last_applied = snapshot.last_applied;
        self.state = snapshot.state.clone();
        self.members = snapshot.members.clone();
        self.previous_members = snapshot.previous_members.clone();
        self.configuration_phase = snapshot.configuration_phase;
        self.joint_config_index = None;
        self.pending_finalization = None;
        self.log.clear();
        self.log_base_index = snapshot.last_included_index;
        self.log_base_term = snapshot.last_included_term;
        self.compacted_snapshot = Some(snapshot);
        self.replication_progress.clear();
        self.rebuild_replication_progress();
        Ok(())
    }

    pub fn replication_catch_up_for(
        &self,
        follower_id: &str,
    ) -> Result<ReplicationCatchUpAction, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        let progress = self
            .replication_progress
            .get(follower_id)
            .copied()
            .unwrap_or_default();
        if progress < self.log_base_index {
            return self
                .compacted_snapshot
                .clone()
                .map(ReplicationCatchUpAction::Snapshot)
                .ok_or_else(|| {
                    ConsensusError::SnapshotRequired(
                        "follower is behind the compacted frontier".into(),
                    )
                });
        }
        if progress >= self.last_log_index() {
            return Ok(ReplicationCatchUpAction::Idle);
        }
        self.incremental_delta_for(follower_id).map(|delta| {
            delta.map_or(
                ReplicationCatchUpAction::Idle,
                ReplicationCatchUpAction::Incremental,
            )
        })
    }

    pub fn snapshot_request_for_append(
        &self,
        request: &AppendEntries,
        retry_at_tick: Option<u64>,
    ) -> Result<SnapshotRequestAction, ConsensusError> {
        validate_node_id(&request.leader_id)?;
        if !self.accepted_members().contains(&request.leader_id) {
            return Err(ConsensusError::UnknownMember(request.leader_id.clone()));
        }
        if request.term < self.current_term || request.prev_log_index >= self.log_base_index {
            return Ok(SnapshotRequestAction::None);
        }
        let snapshot_request = SnapshotRequest::new(
            &self.id,
            &request.leader_id,
            request.term.max(self.current_term),
            None,
            SnapshotRequestReason::AppendPredecessorCompacted,
            retry_at_tick,
        )?;
        snapshot_request.validate()?;
        Ok(SnapshotRequestAction::Request(snapshot_request))
    }

    pub fn snapshot_request_for_incremental_delta(
        &self,
        leader_id: &str,
        delta: &StateDelta,
        retry_at_tick: Option<u64>,
    ) -> Result<SnapshotRequestAction, ConsensusError> {
        validate_node_id(leader_id)?;
        if !self.accepted_members().contains(leader_id) {
            return Err(ConsensusError::UnknownMember(leader_id.to_string()));
        }
        if delta.term < self.current_term || delta.base_index >= self.log_base_index {
            return Ok(SnapshotRequestAction::None);
        }
        let snapshot_request = SnapshotRequest::new(
            &self.id,
            leader_id,
            self.current_term.max(delta.term),
            None,
            SnapshotRequestReason::IncrementalBaseBehind,
            retry_at_tick,
        )?;
        snapshot_request.validate()?;
        Ok(SnapshotRequestAction::Request(snapshot_request))
    }

    pub fn handle_snapshot_request(
        &mut self,
        request: SnapshotRequest,
        now_tick: u64,
    ) -> Result<SnapshotTransferAction, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        request.validate()?;
        if request.leader_id != self.id {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request is bound to another leader".into(),
            ));
        }
        self.validate_replication_peer(&request.follower_id)?;
        if request.term != self.current_term {
            return Err(ConsensusError::SnapshotRequest(
                "snapshot request term is stale or ahead of the leader".into(),
            ));
        }
        self.prepare_snapshot_transfer(&request.follower_id, now_tick)
    }

    pub fn set_snapshot_bandwidth_config(
        &mut self,
        config: SnapshotBandwidthConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.snapshot_bandwidth_config = config;
        Ok(())
    }

    pub fn snapshot_bandwidth_config(&self) -> SnapshotBandwidthConfig {
        self.snapshot_bandwidth_config
    }

    pub fn snapshot_transfer_metrics(
        &self,
        follower_id: &str,
    ) -> Result<SnapshotTransferMetrics, ConsensusError> {
        self.validate_replication_peer(follower_id)?;
        let state = self
            .snapshot_replication_state
            .get(follower_id)
            .cloned()
            .unwrap_or_else(SnapshotReplicationState::new);
        Ok(SnapshotTransferMetrics {
            follower_id: follower_id.to_string(),
            active_transfer_id: state.active_transfer_id,
            snapshot_bytes: state.snapshot_bytes,
            bytes_sent: state.bytes_sent,
            bytes_remaining: state.snapshot_bytes.saturating_sub(state.bytes_sent),
            bandwidth_window_start_tick: state.bandwidth_window_start_tick,
            bandwidth_window_bytes: state.bandwidth_window_bytes,
            bandwidth_limit_bytes: self.snapshot_bandwidth_config.max_bytes_per_window,
            bandwidth_window_ticks: self.snapshot_bandwidth_config.window_ticks,
            sent_transfers: state.sent_transfers,
            acknowledged_transfers: state.acknowledged_transfers,
            rejected_transfers: state.rejected_transfers,
            cancelled_transfers: state.cancelled_transfers,
        })
    }

    pub fn snapshot_replication_status(
        &self,
        follower_id: &str,
    ) -> Result<SnapshotReplicationStatus, ConsensusError> {
        self.validate_replication_peer(follower_id)?;
        let state = self
            .snapshot_replication_state
            .get(follower_id)
            .cloned()
            .unwrap_or_else(SnapshotReplicationState::new);
        let metrics = self.snapshot_transfer_metrics(follower_id)?;
        Ok(SnapshotReplicationStatus {
            follower_id: follower_id.to_string(),
            active_transfer_id: state.active_transfer_id,
            readiness: state.readiness,
            last_installed_index: state.last_installed_index,
            last_installed_term: state.last_installed_term,
            retry_at_tick: state.retry_at_tick,
            sent_transfers: state.sent_transfers,
            acknowledged_transfers: state.acknowledged_transfers,
            rejected_transfers: state.rejected_transfers,
            cancelled_transfers: state.cancelled_transfers,
            metrics,
        })
    }

    pub fn prepare_snapshot_transfer(
        &mut self,
        follower_id: &str,
        now_tick: u64,
    ) -> Result<SnapshotTransferAction, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        let snapshot = self.compacted_snapshot.clone().ok_or_else(|| {
            ConsensusError::SnapshotRequired("no compacted snapshot is available".into())
        })?;
        let state = self
            .snapshot_replication_state
            .entry(follower_id.to_string())
            .or_insert_with(SnapshotReplicationState::new);
        if state.last_installed_index >= snapshot.last_included_index {
            return Ok(SnapshotTransferAction::Idle);
        }
        if state.active_transfer_id.is_some()
            || state
                .retry_at_tick
                .is_some_and(|retry_at| now_tick < retry_at)
        {
            return Ok(SnapshotTransferAction::Backpressured {
                retry_at_tick: state.retry_at_tick,
            });
        }
        let snapshot_bytes = serde_json::to_vec(&snapshot)
            .map_err(|error| ConsensusError::Serialization(error.to_string()))?
            .len() as u64;
        if snapshot_bytes == 0 || snapshot_bytes > MAX_SNAPSHOT_BYTES {
            return Err(ConsensusError::SnapshotBandwidth(
                "serialized snapshot is outside the bounded transfer size".into(),
            ));
        }
        let transfer_id = digest_json(&(
            &self.id,
            follower_id,
            self.current_term,
            snapshot.last_included_index,
            &snapshot.state_hash,
            &snapshot.configuration_hash,
        ))?;
        state.active_transfer_id = Some(transfer_id.clone());
        state.snapshot = Some(snapshot.clone());
        state.snapshot_bytes = snapshot_bytes;
        state.bytes_sent = 0;
        state.bandwidth_window_start_tick = None;
        state.bandwidth_window_bytes = 0;
        state.readiness = SnapshotInstallReadiness::Receiving;
        state.retry_at_tick = None;
        state.sent_transfers = state.sent_transfers.saturating_add(1);
        Ok(SnapshotTransferAction::Send {
            transfer_id,
            snapshot,
        })
    }

    pub fn record_snapshot_transfer_progress(
        &mut self,
        follower_id: &str,
        transfer_id: &str,
        bytes: u64,
        now_tick: u64,
    ) -> Result<SnapshotTransferProgressAction, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        validate_transfer_id(transfer_id)?;
        if bytes == 0 || bytes > MAX_SNAPSHOT_CHUNK_BYTES as u64 {
            return Err(ConsensusError::SnapshotBandwidth(
                "progress bytes must be positive and fit the bounded chunk size".into(),
            ));
        }
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        let config = self.snapshot_bandwidth_config;
        let state = self
            .snapshot_replication_state
            .get_mut(follower_id)
            .ok_or_else(|| {
                ConsensusError::SnapshotBandwidth("progress has no active snapshot transfer".into())
            })?;
        if state.active_transfer_id.as_deref() != Some(transfer_id) {
            return Err(ConsensusError::SnapshotBandwidth(
                "progress transfer ID does not match the active transfer".into(),
            ));
        }
        if bytes > state.snapshot_bytes.saturating_sub(state.bytes_sent) {
            return Err(ConsensusError::SnapshotBandwidth(
                "progress exceeds the remaining snapshot bytes".into(),
            ));
        }
        let window_expired = state.bandwidth_window_start_tick.is_some_and(|start| {
            now_tick
                .checked_sub(start)
                .is_some_and(|elapsed| elapsed >= config.window_ticks)
        });
        if state.bandwidth_window_start_tick.is_none() || window_expired {
            state.bandwidth_window_start_tick = Some(now_tick);
            state.bandwidth_window_bytes = 0;
        }
        let available_bytes = config
            .max_bytes_per_window
            .saturating_sub(state.bandwidth_window_bytes);
        if bytes > available_bytes {
            let start = state.bandwidth_window_start_tick.unwrap_or(now_tick);
            let retry_at_tick = start.checked_add(config.window_ticks).ok_or_else(|| {
                ConsensusError::SnapshotBandwidth("bandwidth retry deadline overflow".into())
            })?;
            return Ok(SnapshotTransferProgressAction::Backpressured {
                retry_at_tick,
                available_bytes,
            });
        }
        state.bytes_sent = state.bytes_sent.saturating_add(bytes);
        state.bandwidth_window_bytes = state.bandwidth_window_bytes.saturating_add(bytes);
        Ok(SnapshotTransferProgressAction::Accepted {
            bytes_sent: state.bytes_sent,
            bytes_remaining: state.snapshot_bytes.saturating_sub(state.bytes_sent),
        })
    }

    pub fn cancel_snapshot_transfer(
        &mut self,
        follower_id: &str,
        transfer_id: &str,
        now_tick: u64,
        reason: &str,
    ) -> Result<SnapshotTransferCancellation, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        validate_transfer_id(transfer_id)?;
        if reason.is_empty()
            || reason.len() > MAX_VALUE_BYTES
            || reason.chars().any(char::is_control)
        {
            return Err(ConsensusError::SnapshotCancellation(
                "cancellation reason is empty, oversized, or contains control characters".into(),
            ));
        }
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        let retry_at_tick = now_tick
            .checked_add(self.replication_flow_config.retry_backoff_ticks)
            .ok_or_else(|| {
                ConsensusError::SnapshotCancellation("cancellation retry deadline overflow".into())
            })?;
        let state = self
            .snapshot_replication_state
            .get_mut(follower_id)
            .ok_or_else(|| {
                ConsensusError::SnapshotCancellation(
                    "cancellation has no active snapshot transfer".into(),
                )
            })?;
        if state.active_transfer_id.as_deref() != Some(transfer_id) {
            return Err(ConsensusError::SnapshotCancellation(
                "cancellation transfer ID does not match the active transfer".into(),
            ));
        }
        state.active_transfer_id = None;
        state.snapshot = None;
        state.snapshot_bytes = 0;
        state.bytes_sent = 0;
        state.bandwidth_window_start_tick = None;
        state.bandwidth_window_bytes = 0;
        state.readiness = SnapshotInstallReadiness::Cancelled;
        state.retry_at_tick = Some(retry_at_tick);
        state.cancelled_transfers = state.cancelled_transfers.saturating_add(1);
        Ok(SnapshotTransferCancellation {
            follower_id: follower_id.to_string(),
            transfer_id: transfer_id.to_string(),
            retry_at_tick,
        })
    }

    pub fn acknowledge_snapshot_transfer(
        &mut self,
        acknowledgement: SnapshotInstallAck,
        now_tick: u64,
    ) -> Result<bool, ConsensusError> {
        acknowledgement.validate()?;
        self.validate_replication_peer(&acknowledgement.follower_id)?;
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        if acknowledgement.term > self.current_term {
            self.current_term = acknowledgement.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
            return Ok(false);
        }
        if acknowledgement.term < self.current_term {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "snapshot acknowledgement term is stale".into(),
            ));
        }
        let state = self
            .snapshot_replication_state
            .get(&acknowledgement.follower_id)
            .cloned()
            .ok_or_else(|| {
                ConsensusError::InvalidSnapshotAcknowledgement(
                    "snapshot acknowledgement has no active transfer".into(),
                )
            })?;
        if state.active_transfer_id.as_deref() != Some(acknowledgement.transfer_id.as_str()) {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "snapshot acknowledgement transfer ID does not match".into(),
            ));
        }
        let snapshot = state.snapshot.clone().ok_or_else(|| {
            ConsensusError::InvalidSnapshotAcknowledgement(
                "snapshot acknowledgement has no transfer snapshot".into(),
            )
        })?;
        if acknowledgement.last_included_index != snapshot.last_included_index
            || acknowledgement.last_included_term != snapshot.last_included_term
            || acknowledgement.snapshot_sha256 != digest_json(&snapshot)?
            || acknowledgement.configuration_hash != snapshot.configuration_hash
        {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "snapshot acknowledgement does not bind the active snapshot".into(),
            ));
        }
        let current_readiness = state.readiness;
        if acknowledgement.readiness == SnapshotInstallReadiness::Installed
            && state.bytes_sent < state.snapshot_bytes
        {
            return Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "installed acknowledgement precedes complete byte accounting".into(),
            ));
        }
        let flow = self
            .snapshot_replication_state
            .get_mut(&acknowledgement.follower_id)
            .ok_or_else(|| {
                ConsensusError::InvalidSnapshotAcknowledgement(
                    "snapshot transfer disappeared during acknowledgement".into(),
                )
            })?;
        match acknowledgement.readiness {
            SnapshotInstallReadiness::Receiving
                if current_readiness == SnapshotInstallReadiness::Receiving =>
            {
                Ok(false)
            }
            SnapshotInstallReadiness::Validated
                if matches!(
                    current_readiness,
                    SnapshotInstallReadiness::Receiving | SnapshotInstallReadiness::Validated
                ) =>
            {
                flow.readiness = SnapshotInstallReadiness::Validated;
                Ok(false)
            }
            SnapshotInstallReadiness::DurablyStaged
                if matches!(
                    current_readiness,
                    SnapshotInstallReadiness::Validated | SnapshotInstallReadiness::DurablyStaged
                ) =>
            {
                flow.readiness = SnapshotInstallReadiness::DurablyStaged;
                Ok(false)
            }
            SnapshotInstallReadiness::Installed
                if current_readiness == SnapshotInstallReadiness::DurablyStaged =>
            {
                let installed_index = snapshot.last_included_index;
                let installed_term = snapshot.last_included_term;
                flow.active_transfer_id = None;
                flow.snapshot = None;
                flow.snapshot_bytes = 0;
                flow.bytes_sent = 0;
                flow.bandwidth_window_start_tick = None;
                flow.bandwidth_window_bytes = 0;
                flow.readiness = SnapshotInstallReadiness::Installed;
                flow.last_installed_index = flow.last_installed_index.max(installed_index);
                flow.last_installed_term = installed_term;
                flow.retry_at_tick = None;
                flow.acknowledged_transfers = flow.acknowledged_transfers.saturating_add(1);
                let last_log_index = self.last_log_index();
                let progress = self
                    .replication_progress
                    .entry(acknowledgement.follower_id.clone())
                    .or_default();
                *progress = (*progress).max(installed_index.min(last_log_index));
                Ok(true)
            }
            SnapshotInstallReadiness::Rejected => {
                flow.active_transfer_id = None;
                flow.snapshot = None;
                flow.snapshot_bytes = 0;
                flow.bytes_sent = 0;
                flow.bandwidth_window_start_tick = None;
                flow.bandwidth_window_bytes = 0;
                flow.readiness = SnapshotInstallReadiness::Rejected;
                flow.retry_at_tick = Some(
                    now_tick
                        .checked_add(self.replication_flow_config.retry_backoff_ticks)
                        .ok_or_else(|| {
                            ConsensusError::InvalidSnapshotAcknowledgement(
                                "snapshot retry deadline overflow".into(),
                            )
                        })?,
                );
                flow.rejected_transfers = flow.rejected_transfers.saturating_add(1);
                Ok(false)
            }
            _ => Err(ConsensusError::InvalidSnapshotAcknowledgement(
                "snapshot readiness transition is not monotonic".into(),
            )),
        }
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
        self.record_current_term_for_replay();
        self.step_down_and_invalidate();
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
        self.record_current_term_for_replay();
        self.invalidate_lease();
        self.read_rounds.clear();
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
        if !self.accepted_members().contains(&request.candidate_id) {
            return Err(ConsensusError::UnknownMember(request.candidate_id));
        }
        if request.term > self.current_term {
            self.current_term = request.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
            self.votes_received.clear();
        }
        let mut granted = false;
        if request.term == self.current_term {
            let (last_log_index, last_log_term) = self.last_log_position();
            let up_to_date = request.last_log_term > last_log_term
                || (request.last_log_term == last_log_term
                    && request.last_log_index >= last_log_index);
            if up_to_date
                && (self.voted_for.is_none()
                    || self.voted_for.as_deref() == Some(request.candidate_id.as_str()))
            {
                self.voted_for = Some(request.candidate_id.clone());
                self.step_down_and_invalidate();
                granted = true;
            }
        }
        Ok(VoteResponse {
            term: self.current_term,
            voter_id: self.id.clone(),
            granted,
        })
    }

    pub fn receive_vote_response(
        &mut self,
        response: VoteResponse,
    ) -> Result<bool, ConsensusError> {
        validate_node_id(&response.voter_id)?;
        if !self.members.contains(&response.voter_id) {
            return Err(ConsensusError::UnknownMember(response.voter_id));
        }
        if response.term > self.current_term {
            self.current_term = response.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
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
        if self.has_vote_quorum(&self.votes_received) {
            self.role = ConsensusRole::Leader;
            self.invalidate_lease();
            self.heartbeat_due_tick = None;
            self.voted_for = Some(self.id.clone());
            self.replication_progress.clear();
            self.replication_progress
                .insert(self.id.clone(), self.last_log_index());
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
        let index = self.last_log_index() + 1;
        let entry = LogEntry::new(index, self.current_term, command)?;
        self.log.push(entry.clone());
        self.replication_progress.insert(self.id.clone(), index);
        self.advance_commit_index()?;
        Ok(entry)
    }

    pub fn snapshot_chunker(
        &self,
        transfer_id: &str,
        chunk_size: usize,
    ) -> Result<SnapshotChunker, ConsensusError> {
        SnapshotChunker::from_snapshot(&self.snapshot()?, transfer_id, chunk_size)
    }

    pub fn install_snapshot_stream(
        &mut self,
        manifest: SnapshotManifest,
        chunks: impl IntoIterator<Item = SnapshotChunk>,
    ) -> Result<(), ConsensusError> {
        let mut assembler = SnapshotAssembler::new(manifest)?;
        for chunk in chunks {
            assembler.accept(chunk)?;
        }
        self.install_snapshot(assembler.finish()?)
    }

    pub fn incremental_delta_for(
        &self,
        follower_id: &str,
    ) -> Result<Option<StateDelta>, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        validate_node_id(follower_id)?;
        if !self.accepted_members().contains(follower_id) {
            return Err(ConsensusError::UnknownMember(follower_id.to_string()));
        }
        if follower_id == self.id {
            return Err(ConsensusError::InvalidMessage(
                "leader cannot synchronize itself".into(),
            ));
        }
        let base_index = self
            .replication_progress
            .get(follower_id)
            .copied()
            .unwrap_or_default();
        if base_index < self.log_base_index {
            return Err(ConsensusError::SnapshotRequired(
                "follower is behind the compacted log frontier".into(),
            ));
        }
        let start = (base_index.saturating_sub(self.log_base_index)) as usize;
        let entries: Vec<LogEntry> = self
            .log
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take(MAX_BATCH_ENTRIES)
            .cloned()
            .collect();
        if entries.is_empty() {
            return Ok(None);
        }
        let target_index = base_index + entries.len() as u64;
        StateDelta::new(
            self.current_term,
            base_index,
            self.commit_index.min(target_index),
            entries,
        )
        .map(Some)
    }

    pub fn apply_incremental_delta(&mut self, delta: StateDelta) -> Result<u64, ConsensusError> {
        delta.validate()?;
        if delta.term < self.current_term {
            return Err(ConsensusError::IncrementalSyncConflict(
                "delta term is stale".into(),
            ));
        }
        if delta.term > self.current_term {
            self.current_term = delta.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
        }
        if delta.base_index < self.log_base_index {
            return Err(ConsensusError::SnapshotRequired(
                "delta base is behind the compacted log frontier".into(),
            ));
        }
        if delta.base_index > self.last_log_index() {
            return Err(ConsensusError::IncrementalSyncConflict(
                "delta base is ahead of the local log".into(),
            ));
        }
        for (offset, entry) in delta.entries.iter().cloned().enumerate() {
            let expected_index = delta.base_index + offset as u64 + 1;
            entry.validate(expected_index)?;
            if let Some(existing) = self.entry_at(expected_index) {
                if existing.term != entry.term || existing.command_hash != entry.command_hash {
                    let truncate_at = (expected_index - self.log_base_index - 1) as usize;
                    self.log.truncate(truncate_at);
                }
            }
            if expected_index > self.last_log_index() {
                if self.log.len() >= self.max_log_entries {
                    return Err(ConsensusError::LogLimitReached);
                }
                self.log.push(entry);
            }
        }
        self.step_down_and_invalidate();
        self.votes_received.clear();
        self.commit_index = self
            .commit_index
            .max(delta.leader_commit.min(delta.target_index));
        self.commit_index = self.commit_index.min(self.last_log_index());
        self.apply_committed();
        Ok(delta.target_index)
    }

    pub fn prepare_concurrent_catch_up(
        &self,
        follower_ids: &[String],
    ) -> Result<BTreeMap<String, StateDelta>, ConsensusError> {
        if follower_ids.len() > MAX_MEMBERS {
            return Err(ConsensusError::IncrementalSyncConflict(
                "too many concurrent followers".into(),
            ));
        }
        let mut unique = BTreeSet::new();
        for follower_id in follower_ids {
            validate_node_id(follower_id)?;
            if !unique.insert(follower_id.clone()) {
                return Err(ConsensusError::IncrementalSyncConflict(
                    "duplicate follower in concurrent catch-up".into(),
                ));
            }
        }
        std::thread::scope(|scope| {
            let handles = follower_ids
                .iter()
                .map(|follower_id| {
                    let follower_id = follower_id.clone();
                    scope.spawn(move || {
                        self.incremental_delta_for(&follower_id)
                            .map(|delta| (follower_id, delta))
                    })
                })
                .collect::<Vec<_>>();
            let mut plans = BTreeMap::new();
            for handle in handles {
                let (follower_id, delta) = handle.join().map_err(|_| {
                    ConsensusError::IncrementalSyncConflict("catch-up worker panicked".into())
                })??;
                if let Some(delta) = delta {
                    plans.insert(follower_id, delta);
                }
            }
            Ok(plans)
        })
    }

    pub fn configure_replication_flow(
        &mut self,
        config: ReplicationFlowConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.replication_flow_config = config;
        self.peer_replication_flow.clear();
        Ok(())
    }

    pub fn replication_flow_config(&self) -> ReplicationFlowConfig {
        self.replication_flow_config
    }

    pub fn replication_window_status(
        &self,
        follower_id: &str,
    ) -> Result<ReplicationWindowStatus, ConsensusError> {
        self.validate_replication_peer(follower_id)?;
        let flow = self
            .peer_replication_flow
            .get(follower_id)
            .cloned()
            .unwrap_or_else(PeerReplicationFlow::new);
        Ok(ReplicationWindowStatus {
            follower_id: follower_id.to_string(),
            in_flight_batch_id: flow.in_flight,
            last_completed_batch_id: flow.last_completed,
            retry_at_tick: flow.retry_at_tick,
            sent_batches: flow.sent_batches,
            acknowledged_batches: flow.acknowledged_batches,
            rejected_batches: flow.rejected_batches,
        })
    }

    pub fn prepare_flow_controlled_replication(
        &mut self,
        follower_id: &str,
        now_tick: u64,
    ) -> Result<ReplicationFlowAction, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        if let Some(flow) = self.peer_replication_flow.get(follower_id) {
            if flow.in_flight.is_some()
                || flow
                    .retry_at_tick
                    .is_some_and(|retry_at| now_tick < retry_at)
            {
                return Ok(ReplicationFlowAction::Backpressured {
                    retry_at_tick: flow.retry_at_tick,
                });
            }
        }
        let request = self.append_entries_for_with_limits(
            follower_id,
            self.replication_flow_config.max_entries_per_batch,
        )?;
        if request.entries.is_empty() {
            return Ok(ReplicationFlowAction::Idle);
        }
        let batch_id = self
            .peer_replication_flow
            .get(follower_id)
            .map(|flow| flow.next_batch_id)
            .unwrap_or(1);
        let next_batch_id = batch_id
            .checked_add(1)
            .ok_or_else(|| ConsensusError::ReplicationFlowControl("batch ID overflow".into()))?;
        let batch = ReplicationBatch {
            batch_id,
            term: self.current_term,
            leader_id: self.id.clone(),
            follower_id: follower_id.to_string(),
            request,
        };
        batch.validate(&self.replication_flow_config)?;
        let flow = self
            .peer_replication_flow
            .entry(follower_id.to_string())
            .or_insert_with(PeerReplicationFlow::new);
        flow.next_batch_id = next_batch_id;
        flow.in_flight = Some(batch_id);
        flow.retry_at_tick = None;
        flow.sent_batches = flow.sent_batches.saturating_add(1);
        Ok(ReplicationFlowAction::Send(batch))
    }

    pub fn acknowledge_flow_controlled_replication(
        &mut self,
        acknowledgement: ReplicationBatchAck,
        now_tick: u64,
    ) -> Result<bool, ConsensusError> {
        acknowledgement.validate()?;
        self.validate_replication_peer(&acknowledgement.follower_id)?;
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            return Err(ConsensusError::ClockUntrusted);
        }
        let Some(flow) = self.peer_replication_flow.get(&acknowledgement.follower_id) else {
            return Err(ConsensusError::ReplicationFlowControl(
                "acknowledgement has no active peer window".into(),
            ));
        };
        if flow.in_flight != Some(acknowledgement.batch_id) {
            return Err(ConsensusError::ReplicationFlowControl(
                "acknowledgement does not match the active batch".into(),
            ));
        }
        let committed = self.acknowledge_append(acknowledgement.response.clone())?;
        if self.role != ConsensusRole::Leader || acknowledgement.response.term != self.current_term
        {
            return Ok(committed);
        }
        let flow = self
            .peer_replication_flow
            .get_mut(&acknowledgement.follower_id)
            .ok_or_else(|| {
                ConsensusError::ReplicationFlowControl(
                    "peer flow disappeared during acknowledgement".into(),
                )
            })?;
        flow.in_flight = None;
        if acknowledgement.response.success {
            flow.last_completed = Some(acknowledgement.batch_id);
            flow.retry_at_tick = None;
            flow.acknowledged_batches = flow.acknowledged_batches.saturating_add(1);
        } else {
            flow.retry_at_tick = Some(
                now_tick
                    .checked_add(self.replication_flow_config.retry_backoff_ticks)
                    .ok_or_else(|| {
                        ConsensusError::ReplicationFlowControl("retry deadline overflow".into())
                    })?,
            );
            flow.rejected_batches = flow.rejected_batches.saturating_add(1);
        }
        Ok(committed)
    }

    pub fn append_entries_for(&self, follower_id: &str) -> Result<AppendEntries, ConsensusError> {
        self.append_entries_for_with_limits(follower_id, MAX_BATCH_ENTRIES)
    }

    fn append_entries_for_with_limits(
        &self,
        follower_id: &str,
        max_entries: usize,
    ) -> Result<AppendEntries, ConsensusError> {
        if self.role != ConsensusRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        self.validate_replication_peer(follower_id)?;
        let next_index = self
            .replication_progress
            .get(follower_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        let prev_log_index = next_index.saturating_sub(1);
        if next_index <= self.log_base_index {
            return Err(ConsensusError::SnapshotRequired(
                "follower is behind the compacted log frontier".into(),
            ));
        }
        let prev_log_term = if prev_log_index == self.log_base_index {
            self.log_base_term
        } else {
            self.entry_at(prev_log_index)
                .map(|entry| entry.term)
                .unwrap_or_default()
        };
        let start = next_index.saturating_sub(self.log_base_index + 1) as usize;
        let entries = self
            .log
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take(max_entries)
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

    pub fn handle_replication_batch(
        &mut self,
        batch: ReplicationBatch,
    ) -> Result<ReplicationBatchAck, ConsensusError> {
        batch.validate(&self.replication_flow_config)?;
        if batch.follower_id != self.id {
            return Err(ConsensusError::InvalidPeer(batch.follower_id));
        }
        if !self.accepted_members().contains(&batch.leader_id) {
            return Err(ConsensusError::UnknownMember(batch.leader_id));
        }
        let response = self.handle_append_entries(batch.request)?;
        Ok(ReplicationBatchAck {
            batch_id: batch.batch_id,
            follower_id: self.id.clone(),
            response,
        })
    }

    pub fn handle_append_entries(
        &mut self,
        request: AppendEntries,
    ) -> Result<AppendResponse, ConsensusError> {
        validate_node_id(&request.leader_id)?;
        if !self.accepted_members().contains(&request.leader_id) {
            return Err(ConsensusError::UnknownMember(request.leader_id));
        }
        if request.term < self.current_term {
            return Ok(AppendResponse {
                term: self.current_term,
                follower_id: self.id.clone(),
                success: false,
                match_index: self.last_log_index(),
            });
        }
        if request.term > self.current_term {
            self.current_term = request.term;
            self.record_current_term_for_replay();
            self.voted_for = None;
        }
        self.step_down_and_invalidate();
        self.votes_received.clear();
        if request.prev_log_index < self.log_base_index {
            return Err(ConsensusError::SnapshotRequired(
                "append predecessor is inside the compacted prefix".into(),
            ));
        }
        if request.prev_log_index > self.last_log_index() {
            return Ok(AppendResponse {
                term: self.current_term,
                follower_id: self.id.clone(),
                success: false,
                match_index: self.last_log_index(),
            });
        }
        if request.prev_log_index > 0 {
            let previous_term = if request.prev_log_index == self.log_base_index {
                self.log_base_term
            } else {
                self.entry_at(request.prev_log_index)
                    .map(|entry| entry.term)
                    .unwrap_or_default()
            };
            if previous_term != request.prev_log_term {
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
            if let Some(existing) = self.entry_at(expected_index) {
                if existing.term != entry.term || existing.command_hash != entry.command_hash {
                    let truncate_at = (expected_index - self.log_base_index - 1) as usize;
                    self.log.truncate(truncate_at);
                }
            }
            if expected_index > self.last_log_index() {
                if self.log.len() >= self.max_log_entries {
                    return Err(ConsensusError::LogLimitReached);
                }
                self.log.push(entry);
            }
            expected_index += 1;
        }
        self.commit_index = request.leader_commit.min(self.last_log_index());
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
        if !self.accepted_members().contains(&response.follower_id) {
            return Err(ConsensusError::UnknownMember(response.follower_id));
        }
        if response.term > self.current_term {
            self.current_term = response.term;
            self.record_current_term_for_replay();
            self.step_down_and_invalidate();
            self.voted_for = None;
            return Ok(false);
        }
        if self.role != ConsensusRole::Leader || response.term != self.current_term {
            return Ok(false);
        }
        if response.success {
            let last_log_index = self.last_log_index();
            let progress = self
                .replication_progress
                .entry(response.follower_id)
                .or_default();
            *progress = (*progress).max(response.match_index.min(last_log_index));
        } else if let Some(progress) = self.replication_progress.get_mut(&response.follower_id) {
            *progress = progress.saturating_sub(1);
        }
        self.advance_commit_index()
    }

    fn advance_commit_index(&mut self) -> Result<bool, ConsensusError> {
        let mut changed = false;
        for index in (self.commit_index + 1)..=self.last_log_index() {
            let replicated_members: BTreeSet<String> = self
                .replication_progress
                .iter()
                .filter(|(_, match_index)| **match_index >= index)
                .map(|(member, _)| member.clone())
                .collect();
            let current_term_entry = self
                .entry_at(index)
                .map(|entry| entry.term == self.current_term)
                .unwrap_or(false);
            if self.has_vote_quorum(&replicated_members) && current_term_entry {
                self.commit_index = index;
                changed = true;
            }
        }
        self.apply_committed();
        Ok(changed)
    }

    fn apply_committed(&mut self) {
        let mut configuration_changed = false;
        while self.last_applied < self.commit_index {
            let index = self.last_applied + 1;
            let Some(entry) = self.entry_at(index).cloned() else {
                break;
            };
            match &entry.command {
                StateCommand::ConfigurationJoint {
                    old_members,
                    new_members,
                } => {
                    if self.configuration_phase == ConfigurationPhase::Stable {
                        self.previous_members = Some(old_members.clone());
                        self.members = new_members.clone();
                        self.configuration_phase = ConfigurationPhase::Joint;
                        self.joint_config_index = Some(entry.index);
                        configuration_changed = true;
                    }
                }
                StateCommand::ConfigurationFinal { members } => {
                    self.members = members.clone();
                    self.previous_members = None;
                    self.configuration_phase = ConfigurationPhase::Stable;
                    self.joint_config_index = None;
                    self.pending_finalization = None;
                    configuration_changed = true;
                }
                _ => entry.command.apply(&mut self.state),
            }
            self.last_applied += 1;
        }
        if self
            .pending_finalization
            .is_some_and(|index| index <= self.commit_index)
        {
            self.configuration_phase = ConfigurationPhase::Stable;
            self.previous_members = None;
            self.joint_config_index = None;
            self.pending_finalization = None;
            configuration_changed = true;
        }
        if configuration_changed {
            self.invalidate_lease();
            self.read_rounds.clear();
            self.rebuild_replication_progress();
        }
    }

    fn validate_replication_peer(&self, follower_id: &str) -> Result<(), ConsensusError> {
        validate_node_id(follower_id)?;
        if !self.accepted_members().contains(follower_id) {
            return Err(ConsensusError::InvalidPeer(follower_id.to_string()));
        }
        if follower_id == self.id {
            return Err(ConsensusError::InvalidPeer(follower_id.to_string()));
        }
        Ok(())
    }

    fn accepted_members(&self) -> BTreeSet<String> {
        let mut members = self.members.clone();
        if let Some(previous) = &self.previous_members {
            members.extend(previous.iter().cloned());
        }
        members
    }

    fn required_remote_compaction_quorum(&self) -> usize {
        let current =
            quorum_size(&self.members).saturating_sub(usize::from(self.members.contains(&self.id)));
        let previous = self
            .previous_members
            .as_ref()
            .map(|members| {
                quorum_size(members).saturating_sub(usize::from(members.contains(&self.id)))
            })
            .unwrap_or_default();
        current.max(previous)
    }

    fn has_vote_quorum(&self, voters: &BTreeSet<String>) -> bool {
        let current_votes = voters.intersection(&self.members).count();
        if current_votes < quorum_size(&self.members) {
            return false;
        }
        self.previous_members
            .as_ref()
            .is_none_or(|previous| voters.intersection(previous).count() >= quorum_size(previous))
    }

    fn rebuild_replication_progress(&mut self) {
        let previous = self.replication_progress.clone();
        self.replication_progress.clear();
        for member in self.accepted_members() {
            self.replication_progress.insert(
                member.clone(),
                previous.get(&member).copied().unwrap_or_default(),
            );
        }
        self.replication_progress
            .insert(self.id.clone(), self.last_log_index());
        let accepted_members = self.accepted_members();
        let local_id = self.id.clone();
        self.peer_replication_flow
            .retain(|member, _| member != &local_id && accepted_members.contains(member));
        self.snapshot_replication_state
            .retain(|member, _| member != &local_id && accepted_members.contains(member));
        for member in accepted_members {
            if member != local_id {
                self.peer_replication_flow
                    .entry(member.clone())
                    .or_insert_with(PeerReplicationFlow::new);
                self.snapshot_replication_state
                    .entry(member)
                    .or_insert_with(SnapshotReplicationState::new);
            }
        }
    }

    fn last_log_index(&self) -> u64 {
        self.log_base_index + self.log.len() as u64
    }

    fn entry_at(&self, index: u64) -> Option<&LogEntry> {
        if index <= self.log_base_index {
            return None;
        }
        self.log.get((index - self.log_base_index - 1) as usize)
    }

    fn last_log_position(&self) -> (u64, u64) {
        self.log
            .last()
            .map(|entry| (entry.index, entry.term))
            .unwrap_or((self.log_base_index, self.log_base_term))
    }

    fn observe_tick(&mut self, now_tick: u64) {
        if self
            .last_observed_tick
            .is_some_and(|previous| now_tick < previous)
        {
            self.invalidate_lease();
            self.clock_uncertain = true;
        }
        self.last_observed_tick = Some(now_tick);
        if self.lease_expiration_tick.is_some_and(|expiration| {
            now_tick
                .checked_add(self.lease_config.max_clock_drift_ticks)
                .is_none_or(|safe_now| safe_now >= expiration)
        }) {
            self.invalidate_lease();
        }
    }

    fn install_lease(&mut self, now_tick: u64) {
        self.observe_tick(now_tick);
        if self.clock_uncertain {
            self.invalidate_lease();
            return;
        }
        self.lease_expiration_tick = now_tick.checked_add(self.lease_config.lease_ticks);
        if self.lease_expiration_tick.is_none() {
            self.invalidate_lease();
        }
    }

    fn invalidate_lease(&mut self) {
        self.lease_expiration_tick = None;
    }

    fn reset_election_deadline(&mut self, now_tick: u64) -> Result<(), ConsensusError> {
        let jitter = if self.election_timer_config.election_jitter_ticks == 0 {
            0
        } else {
            let seed = format!("{}:{}", self.id, self.current_term);
            let digest = Sha256::digest(seed.as_bytes());
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&digest[..8]);
            u64::from_be_bytes(bytes) % (self.election_timer_config.election_jitter_ticks + 1)
        };
        let duration = self
            .election_timer_config
            .election_timeout_ticks
            .checked_add(jitter)
            .ok_or_else(|| {
                ConsensusError::InvalidElectionTimer("election deadline overflow".into())
            })?;
        self.election_deadline_tick = Some(now_tick.checked_add(duration).ok_or_else(|| {
            ConsensusError::InvalidElectionTimer("election deadline overflow".into())
        })?);
        Ok(())
    }

    fn step_down_and_invalidate(&mut self) {
        self.role = ConsensusRole::Follower;
        self.invalidate_lease();
        self.heartbeat_due_tick = None;
        self.peer_replication_flow.clear();
        self.snapshot_replication_state.clear();
        self.read_rounds.clear();
    }

    fn remember_completed_read(&mut self, request_id: &str) {
        if self.completed_read_requests.len() >= MAX_COMPLETED_READ_REQUESTS {
            if let Some(oldest) = self.completed_read_requests.iter().next().cloned() {
                self.completed_read_requests.remove(&oldest);
            }
        }
        self.completed_read_requests.insert(request_id.to_string());
    }
}

fn validate_read_request_id(request_id: &str) -> Result<(), ConsensusError> {
    if request_id.trim().is_empty()
        || request_id.len() > MAX_NONCE_BYTES
        || request_id.chars().any(char::is_control)
    {
        return Err(ConsensusError::InvalidReadRequest(
            "read request ID must be bounded and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn message_term(message: &ConsensusMessage) -> u64 {
    match message {
        ConsensusMessage::VoteRequest(value) => value.term,
        ConsensusMessage::VoteResponse(value) => value.term,
        ConsensusMessage::AppendEntries(value) => value.term,
        ConsensusMessage::AppendResponse(value) => value.term,
        ConsensusMessage::SnapshotManifest(value) => value.term,
        ConsensusMessage::SnapshotChunk(value) => value.term,
        ConsensusMessage::StateDelta(value) => value.term,
        ConsensusMessage::ReadIndexRequest(value) => value.term,
        ConsensusMessage::ReadIndexResponse(value) => value.term,
        ConsensusMessage::ReplicationBatch(value) => value.term,
        ConsensusMessage::ReplicationBatchAck(value) => value.response.term,
        ConsensusMessage::SnapshotInstallAck(value) => value.term,
    }
}

fn validate_nonce(nonce: &str) -> Result<(), ConsensusError> {
    if nonce.trim().is_empty()
        || nonce.len() > MAX_NONCE_BYTES
        || nonce.chars().any(char::is_control)
    {
        return Err(ConsensusError::Unauthenticated(
            "consensus nonce must be bounded and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn validate_hex_digest(value: &str) -> Result<(), ConsensusError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConsensusError::InvalidSnapshotChunk(
            "digest must be exactly 64 hexadecimal characters".into(),
        ));
    }
    Ok(())
}

fn validate_transfer_id(id: &str) -> Result<(), ConsensusError> {
    if id.trim().is_empty() || id.len() > MAX_CLUSTER_ID_BYTES || id.chars().any(char::is_control) {
        return Err(ConsensusError::InvalidSnapshotChunk(
            "transfer ID must be 1 to 128 bytes and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn validate_cluster_id(id: &str) -> Result<(), ConsensusError> {
    if id.trim().is_empty() || id.len() > MAX_CLUSTER_ID_BYTES || id.chars().any(char::is_control) {
        return Err(ConsensusError::InvalidClusterConfiguration(
            "cluster ID must be 1 to 128 bytes and contain no control characters".into(),
        ));
    }
    Ok(())
}

fn validate_members(members: &BTreeSet<String>) -> Result<(), ConsensusError> {
    if members.is_empty() || members.len() > MAX_MEMBERS {
        return Err(ConsensusError::InvalidCluster(format!(
            "cluster must contain 1 to {} members",
            MAX_MEMBERS
        )));
    }
    for member in members {
        validate_node_id(member)?;
    }
    Ok(())
}

fn quorum_size(members: &BTreeSet<String>) -> usize {
    members.len() / 2 + 1
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
        assert_eq!(
            leader.snapshot().unwrap().state_hash,
            follower_c.snapshot().unwrap().state_hash
        );
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
