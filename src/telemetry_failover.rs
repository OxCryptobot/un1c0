use crate::recovery_transport::{
    AuthenticatedTransportEnvelope, ReservationPersistenceFault, TransportKeyRegistry,
    TransportMessageKind, WitnessReservationStore, WitnessVoteReservation,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const CONSENSUS_TELEMETRY_DOMAIN: &str = "un1c0/consensus-telemetry/v1";
const MAX_LABELS: usize = 16;
const MAX_METRICS: usize = 32;
const MAX_JOURNAL_EVENTS: usize = 4096;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 64;
const MAX_TTL_TICKS: u64 = 1_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TelemetryFailoverError {
    #[error("invalid telemetry input: {0}")]
    InvalidInput(String),
    #[error("unknown telemetry producer: {0}")]
    UnknownProducer(String),
    #[error("telemetry rejected: {0}")]
    TelemetryRejected(String),
    #[error("telemetry replay rejected: {0}")]
    ReplayRejected(String),
    #[error("telemetry is stale: {0}")]
    StaleTelemetry(String),
    #[error("telemetry conflict: {0}")]
    TelemetryConflict(String),
    #[error("failover orchestration rejected: {0}")]
    OrchestrationRejected(String),
    #[error("failover intent persistence failed: {0}")]
    PersistenceFailed(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConsensusTelemetryKind {
    LeaderHealth,
    WitnessQuorum,
    TransportHealth,
    SnapshotFreshness,
    ExternalFenceReady,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelemetryKeyRegistry {
    keys: BTreeMap<String, Vec<u8>>,
}

impl TelemetryKeyRegistry {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        producer_id: &str,
        verifying_key: &VerifyingKey,
    ) -> Result<(), TelemetryFailoverError> {
        validate_identifier(producer_id, "producer")?;
        let key = verifying_key.to_bytes().to_vec();
        if let Some(existing) = self.keys.get(producer_id) {
            if existing != &key {
                return Err(TelemetryFailoverError::TelemetryRejected(
                    "telemetry producer key rebinding is not allowed".into(),
                ));
            }
            return Ok(());
        }
        self.keys.insert(producer_id.to_string(), key);
        Ok(())
    }

    pub fn key_for(&self, producer_id: &str) -> Result<VerifyingKey, TelemetryFailoverError> {
        let bytes = self
            .keys
            .get(producer_id)
            .ok_or_else(|| TelemetryFailoverError::UnknownProducer(producer_id.to_string()))?;
        let key_bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            TelemetryFailoverError::TelemetryRejected("telemetry key length".into())
        })?;
        VerifyingKey::from_bytes(&key_bytes)
            .map_err(|_| TelemetryFailoverError::TelemetryRejected("telemetry key encoding".into()))
    }
}

impl Default for TelemetryKeyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsensusTelemetryEvent {
    pub domain: String,
    pub protocol_version: u16,
    pub cluster_id: String,
    pub resource_id: String,
    pub producer_id: String,
    pub region_id: String,
    pub authority_epoch: u64,
    pub sequence: u64,
    pub observed_tick: u64,
    pub ttl_ticks: u64,
    pub kind: ConsensusTelemetryKind,
    pub labels: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, u64>,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub event_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct TelemetryPayload<'a> {
    domain: &'a str,
    protocol_version: u16,
    cluster_id: &'a str,
    resource_id: &'a str,
    producer_id: &'a str,
    region_id: &'a str,
    authority_epoch: u64,
    sequence: u64,
    observed_tick: u64,
    ttl_ticks: u64,
    kind: &'a ConsensusTelemetryKind,
    labels: &'a BTreeMap<String, String>,
    metrics: &'a BTreeMap<String, u64>,
    public_key: &'a [u8],
}

impl ConsensusTelemetryEvent {
    pub fn sign(
        cluster_id: &str,
        resource_id: &str,
        producer_id: &str,
        region_id: &str,
        authority_epoch: u64,
        sequence: u64,
        observed_tick: u64,
        ttl_ticks: u64,
        kind: ConsensusTelemetryKind,
        labels: BTreeMap<String, String>,
        metrics: BTreeMap<String, u64>,
        signing_key: &SigningKey,
    ) -> Result<Self, TelemetryFailoverError> {
        let mut event = Self {
            domain: CONSENSUS_TELEMETRY_DOMAIN.to_string(),
            protocol_version: 1,
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            producer_id: producer_id.to_string(),
            region_id: region_id.to_string(),
            authority_epoch,
            sequence,
            observed_tick,
            ttl_ticks,
            kind,
            labels,
            metrics,
            public_key: signing_key.verifying_key().to_bytes().to_vec(),
            signature: vec![0; 64],
            event_hash: "0".repeat(64),
        };
        event.validate_shape()?;
        event.signature = signing_key
            .sign(&event.canonical_payload()?)
            .to_bytes()
            .to_vec();
        event.event_hash = event.content_hash()?;
        Ok(event)
    }

    pub fn verify(
        &self,
        registry: &TelemetryKeyRegistry,
        expected_cluster_id: &str,
        expected_resource_id: &str,
    ) -> Result<(), TelemetryFailoverError> {
        self.validate_shape()?;
        if self.cluster_id != expected_cluster_id || self.resource_id != expected_resource_id {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry cluster/resource binding mismatch".into(),
            ));
        }
        let trusted_key = registry.key_for(&self.producer_id)?;
        if self.public_key != trusted_key.to_bytes() {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry producer key mismatch".into(),
            ));
        }
        let signature = Signature::from_slice(&self.signature).map_err(|_| {
            TelemetryFailoverError::TelemetryRejected("telemetry signature encoding".into())
        })?;
        trusted_key
            .verify(&self.canonical_payload()?, &signature)
            .map_err(|_| TelemetryFailoverError::TelemetryRejected("telemetry signature".into()))?;
        if self.event_hash != self.content_hash()? {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry event hash mismatch".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> &str {
        &self.event_hash
    }

    fn content_hash(&self) -> Result<String, TelemetryFailoverError> {
        digest_json(&(
            &self.domain,
            self.protocol_version,
            &self.cluster_id,
            &self.resource_id,
            &self.producer_id,
            &self.region_id,
            self.authority_epoch,
            self.sequence,
            self.observed_tick,
            self.ttl_ticks,
            &self.kind,
            &self.labels,
            &self.metrics,
            &self.public_key,
            &self.signature,
        ))
    }

    fn validate_shape(&self) -> Result<(), TelemetryFailoverError> {
        if self.domain != CONSENSUS_TELEMETRY_DOMAIN || self.protocol_version != 1 {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry domain or protocol version is invalid".into(),
            ));
        }
        validate_identifier(&self.cluster_id, "cluster")?;
        validate_identifier(&self.resource_id, "resource")?;
        validate_identifier(&self.producer_id, "producer")?;
        validate_identifier(&self.region_id, "region")?;
        if self.authority_epoch == 0
            || self.sequence == 0
            || self.ttl_ticks == 0
            || self.ttl_ticks > MAX_TTL_TICKS
        {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry generation or TTL is invalid".into(),
            ));
        }
        if self.public_key.len() != 32 || self.signature.len() != 64 || self.event_hash.len() != 64
        {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry key, signature, or digest shape is invalid".into(),
            ));
        }
        if self.labels.len() > MAX_LABELS || self.metrics.len() > MAX_METRICS {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry cardinality bound exceeded".into(),
            ));
        }
        for (key, value) in &self.labels {
            validate_label(key, "label key")?;
            validate_label(value, "label value")?;
        }
        for key in self.metrics.keys() {
            validate_label(key, "metric key")?;
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, TelemetryFailoverError> {
        serde_json::to_vec(&TelemetryPayload {
            domain: &self.domain,
            protocol_version: self.protocol_version,
            cluster_id: &self.cluster_id,
            resource_id: &self.resource_id,
            producer_id: &self.producer_id,
            region_id: &self.region_id,
            authority_epoch: self.authority_epoch,
            sequence: self.sequence,
            observed_tick: self.observed_tick,
            ttl_ticks: self.ttl_ticks,
            kind: &self.kind,
            labels: &self.labels,
            metrics: &self.metrics,
            public_key: &self.public_key,
        })
        .map_err(|error| TelemetryFailoverError::InvalidInput(error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TelemetryAdmission {
    Accepted,
    AlreadySeen,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelemetryJournalEntry {
    pub sequence: u64,
    pub event_hash: String,
    pub previous_hash: String,
    pub kind: ConsensusTelemetryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelemetryReceiverReport {
    pub accepted_events: usize,
    pub duplicate_events: usize,
    pub rejected_events: usize,
    pub last_authority_epoch: u64,
    pub last_sequence: u64,
    pub journal_length: usize,
    pub trace_digest: String,
    pub safety_passed: bool,
}

#[derive(Debug, Clone)]
pub struct SecureTelemetryReceiver {
    cluster_id: String,
    resource_id: String,
    max_events: usize,
    frontiers: BTreeMap<String, (u64, u64, String)>,
    seen_hashes: BTreeSet<String>,
    journal: Vec<TelemetryJournalEntry>,
    accepted_events: usize,
    duplicate_events: usize,
    rejected_events: usize,
}

impl SecureTelemetryReceiver {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        max_events: usize,
    ) -> Result<Self, TelemetryFailoverError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        if max_events == 0 || max_events > MAX_JOURNAL_EVENTS {
            return Err(TelemetryFailoverError::InvalidInput(
                "telemetry journal bound is outside the safe range".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            max_events,
            frontiers: BTreeMap::new(),
            seen_hashes: BTreeSet::new(),
            journal: Vec::new(),
            accepted_events: 0,
            duplicate_events: 0,
            rejected_events: 0,
        })
    }

    pub fn admit(
        &mut self,
        event: ConsensusTelemetryEvent,
        registry: &TelemetryKeyRegistry,
        current_tick: u64,
    ) -> Result<TelemetryAdmission, TelemetryFailoverError> {
        let result = self.admit_inner(event, registry, current_tick);
        if result.is_err() {
            self.rejected_events = self.rejected_events.saturating_add(1);
        }
        result
    }

    pub fn report(&self) -> TelemetryReceiverReport {
        let (last_authority_epoch, last_sequence) = self
            .frontiers
            .values()
            .max_by_key(|(epoch, sequence, _)| (*epoch, *sequence))
            .map(|(epoch, sequence, _)| (*epoch, *sequence))
            .unwrap_or((0, 0));
        TelemetryReceiverReport {
            accepted_events: self.accepted_events,
            duplicate_events: self.duplicate_events,
            rejected_events: self.rejected_events,
            last_authority_epoch,
            last_sequence,
            journal_length: self.journal.len(),
            trace_digest: digest_json(&self.journal).unwrap_or_default(),
            safety_passed: self.journal.len() <= self.max_events,
        }
    }

    pub fn journal(&self) -> &[TelemetryJournalEntry] {
        &self.journal
    }

    pub fn journal_integrity(&self) -> bool {
        let mut previous_hash = "0".repeat(64);
        for entry in &self.journal {
            if entry.previous_hash != previous_hash
                || entry.event_hash.len() != 64
                || !entry
                    .event_hash
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return false;
            }
            previous_hash = entry.event_hash.clone();
        }
        true
    }

    fn admit_inner(
        &mut self,
        event: ConsensusTelemetryEvent,
        registry: &TelemetryKeyRegistry,
        current_tick: u64,
    ) -> Result<TelemetryAdmission, TelemetryFailoverError> {
        event.verify(registry, &self.cluster_id, &self.resource_id)?;
        if current_tick > event.observed_tick.saturating_add(event.ttl_ticks) {
            return Err(TelemetryFailoverError::StaleTelemetry(
                "telemetry event exceeded its TTL".into(),
            ));
        }
        if self.seen_hashes.contains(event.digest()) {
            self.duplicate_events = self.duplicate_events.saturating_add(1);
            return Ok(TelemetryAdmission::AlreadySeen);
        }
        let frontier =
            self.frontiers
                .entry(event.producer_id.clone())
                .or_insert((0, 0, String::new()));
        if event.authority_epoch < frontier.0 {
            return Err(TelemetryFailoverError::ReplayRejected(
                "telemetry authority epoch regressed".into(),
            ));
        }
        if event.authority_epoch == frontier.0 && event.sequence < frontier.1 {
            return Err(TelemetryFailoverError::ReplayRejected(
                "telemetry sequence regressed".into(),
            ));
        }
        if event.authority_epoch == frontier.0 && event.sequence == frontier.1 && frontier.1 != 0 {
            return Err(TelemetryFailoverError::TelemetryConflict(
                "same telemetry sequence has a different digest".into(),
            ));
        }
        if self.journal.len() >= self.max_events {
            return Err(TelemetryFailoverError::TelemetryRejected(
                "telemetry journal bound exceeded".into(),
            ));
        }
        let previous_hash = self
            .journal
            .last()
            .map(|entry| entry.event_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));
        self.journal.push(TelemetryJournalEntry {
            sequence: event.sequence,
            event_hash: event.event_hash.clone(),
            previous_hash,
            kind: event.kind,
        });
        frontier.0 = event.authority_epoch;
        frontier.1 = event.sequence;
        frontier.2 = event.event_hash.clone();
        self.seen_hashes.insert(event.event_hash);
        self.accepted_events = self.accepted_events.saturating_add(1);
        Ok(TelemetryAdmission::Accepted)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailoverOrchestrationPhase {
    Idle,
    DetectingFailure,
    CollectingEvidence,
    AwaitingFence,
    Committed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FailoverOrchestrationAction {
    AwaitingExternalFence,
    AlreadyPrepared,
    FenceAdmitted,
    AlreadyFenced,
    Committed,
    AlreadyCommitted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailoverIntent {
    pub operation_id: String,
    pub decision_digest: String,
    pub candidate_region_id: String,
    pub authority_epoch: u64,
    pub prepared_tick: u64,
    pub fence_admitted_tick: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailoverOrchestrationReport {
    pub cluster_id: String,
    pub resource_id: String,
    pub phase: FailoverOrchestrationPhase,
    pub active_region_id: Option<String>,
    pub telemetry_epoch: u64,
    pub telemetry_events: usize,
    pub committed_operation_id: Option<String>,
    pub trace_digest: String,
    pub safety_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DurableFailoverIntent {
    pub intent: FailoverIntent,
    pub state_hash: String,
}

#[derive(Debug, Clone)]
pub struct FailoverIntentStore {
    path: PathBuf,
}

impl FailoverIntentStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn save(&self, intent: &FailoverIntent) -> Result<(), TelemetryFailoverError> {
        let envelope = DurableFailoverIntent {
            intent: intent.clone(),
            state_hash: digest_json(intent)?,
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        }
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        file.sync_all()
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        fs::rename(&staging, &self.path)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        if let Some(parent) = self.path.parent() {
            let directory = OpenOptions::new()
                .read(true)
                .open(parent)
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
            directory
                .sync_all()
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<FailoverIntent>, TelemetryFailoverError> {
        let staging = self.path.with_extension("staging");
        if staging.exists() {
            fs::remove_file(&staging)
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        }
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        if bytes.len() > 64 * 1024 {
            return Err(TelemetryFailoverError::PersistenceFailed(
                "failover intent exceeds size bound".into(),
            ));
        }
        let envelope: DurableFailoverIntent = serde_json::from_slice(&bytes)
            .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        if envelope.state_hash != digest_json(&envelope.intent)? {
            return Err(TelemetryFailoverError::PersistenceFailed(
                "failover intent hash mismatch".into(),
            ));
        }
        validate_identifier(&envelope.intent.operation_id, "operation")?;
        validate_hash(&envelope.intent.decision_digest, "decision digest")?;
        validate_identifier(&envelope.intent.candidate_region_id, "candidate region")?;
        if envelope.intent.authority_epoch == 0 || envelope.intent.prepared_tick == 0 {
            return Err(TelemetryFailoverError::PersistenceFailed(
                "failover intent generation is invalid".into(),
            ));
        }
        Ok(Some(envelope.intent))
    }

    pub fn clear(&self) -> Result<(), TelemetryFailoverError> {
        if self.path.exists() {
            fs::remove_file(&self.path)
                .map_err(|error| TelemetryFailoverError::PersistenceFailed(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FailoverOrchestrator {
    cluster_id: String,
    resource_id: String,
    required_kinds: BTreeSet<ConsensusTelemetryKind>,
    max_event_age_ticks: u64,
    phase: FailoverOrchestrationPhase,
    active_region_id: Option<String>,
    detected_operation_id: Option<String>,
    telemetry: BTreeMap<ConsensusTelemetryKind, ConsensusTelemetryEvent>,
    intent: Option<FailoverIntent>,
    committed_operation_id: Option<String>,
    events: Vec<String>,
}

impl FailoverOrchestrator {
    pub fn new(
        cluster_id: &str,
        resource_id: &str,
        required_kinds: BTreeSet<ConsensusTelemetryKind>,
        max_event_age_ticks: u64,
        initial_region_id: Option<&str>,
    ) -> Result<Self, TelemetryFailoverError> {
        validate_identifier(cluster_id, "cluster")?;
        validate_identifier(resource_id, "resource")?;
        if required_kinds.is_empty() || max_event_age_ticks == 0 {
            return Err(TelemetryFailoverError::InvalidInput(
                "orchestration telemetry requirements are empty or unbounded".into(),
            ));
        }
        Ok(Self {
            cluster_id: cluster_id.to_string(),
            resource_id: resource_id.to_string(),
            required_kinds,
            max_event_age_ticks,
            phase: FailoverOrchestrationPhase::Idle,
            active_region_id: initial_region_id.map(str::to_string),
            detected_operation_id: None,
            telemetry: BTreeMap::new(),
            intent: None,
            committed_operation_id: None,
            events: Vec::new(),
        })
    }

    pub fn ingest(
        &mut self,
        event: ConsensusTelemetryEvent,
        registry: &TelemetryKeyRegistry,
        current_tick: u64,
    ) -> Result<TelemetryAdmission, TelemetryFailoverError> {
        event.verify(registry, &self.cluster_id, &self.resource_id)?;
        if current_tick > event.observed_tick.saturating_add(event.ttl_ticks)
            || current_tick.saturating_sub(event.observed_tick) > self.max_event_age_ticks
        {
            self.phase = FailoverOrchestrationPhase::CollectingEvidence;
            return Err(TelemetryFailoverError::StaleTelemetry(
                "orchestration telemetry is outside the freshness window".into(),
            ));
        }
        if let Some(previous) = self.telemetry.get(&event.kind) {
            if event.authority_epoch < previous.authority_epoch
                || (event.authority_epoch == previous.authority_epoch
                    && event.sequence < previous.sequence)
            {
                return Err(TelemetryFailoverError::ReplayRejected(
                    "orchestration telemetry regressed".into(),
                ));
            }
            if event.authority_epoch == previous.authority_epoch
                && event.sequence == previous.sequence
                && event.event_hash == previous.event_hash
            {
                return Ok(TelemetryAdmission::AlreadySeen);
            }
            if event.authority_epoch == previous.authority_epoch
                && event.sequence == previous.sequence
                && event.event_hash != previous.event_hash
            {
                self.phase = FailoverOrchestrationPhase::Failed;
                return Err(TelemetryFailoverError::TelemetryConflict(
                    "same kind/epoch/sequence has conflicting telemetry".into(),
                ));
            }
        }
        self.telemetry.insert(event.kind, event.clone());
        if self.phase == FailoverOrchestrationPhase::DetectingFailure {
            self.phase = FailoverOrchestrationPhase::CollectingEvidence;
        }
        self.events
            .push(format!("telemetry:{}:{}", event.sequence, event.event_hash));
        Ok(TelemetryAdmission::Accepted)
    }

    pub fn detect_failure(&mut self, operation_id: &str) -> Result<(), TelemetryFailoverError> {
        validate_identifier(operation_id, "operation")?;
        if self.committed_operation_id.is_some() {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "committed orchestration cannot detect another failure".into(),
            ));
        }
        if self.phase == FailoverOrchestrationPhase::Failed {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "failed orchestration requires a new instance".into(),
            ));
        }
        if let Some(existing) = &self.detected_operation_id {
            if existing != operation_id {
                return Err(TelemetryFailoverError::OrchestrationRejected(
                    "failure detection operation identity cannot be rebound".into(),
                ));
            }
            return Ok(());
        }
        if self.phase == FailoverOrchestrationPhase::Idle {
            self.detected_operation_id = Some(operation_id.to_string());
            self.phase = FailoverOrchestrationPhase::DetectingFailure;
            self.events.push(format!("detected:{operation_id}"));
        }
        Ok(())
    }

    pub fn begin_failover_with_store(
        &mut self,
        store: &FailoverIntentStore,
        operation_id: &str,
        decision_digest: &str,
        candidate_region_id: &str,
        authority_epoch: u64,
        current_tick: u64,
    ) -> Result<FailoverOrchestrationAction, TelemetryFailoverError> {
        let before = self.clone();
        let action = self.begin_failover(
            operation_id,
            decision_digest,
            candidate_region_id,
            authority_epoch,
            current_tick,
        )?;
        if action == FailoverOrchestrationAction::AwaitingExternalFence {
            let intent = self.intent.as_ref().ok_or_else(|| {
                TelemetryFailoverError::OrchestrationRejected(
                    "prepared orchestration has no intent".into(),
                )
            })?;
            if let Err(error) = store.save(intent) {
                *self = before;
                return Err(error);
            }
        }
        Ok(action)
    }

    pub fn restore_intent(&mut self, intent: FailoverIntent) -> Result<(), TelemetryFailoverError> {
        validate_identifier(&intent.operation_id, "operation")?;
        validate_hash(&intent.decision_digest, "decision digest")?;
        validate_identifier(&intent.candidate_region_id, "candidate region")?;
        if intent.authority_epoch == 0 || intent.prepared_tick == 0 {
            return Err(TelemetryFailoverError::InvalidInput(
                "failover intent generation is invalid".into(),
            ));
        }
        if self.phase == FailoverOrchestrationPhase::Committed
            || self.committed_operation_id.is_some()
        {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "committed orchestration cannot restore a new intent".into(),
            ));
        }
        self.detected_operation_id = Some(intent.operation_id.clone());
        self.intent = Some(intent.clone());
        self.phase = FailoverOrchestrationPhase::AwaitingFence;
        self.events
            .push(format!("restored:{}", intent.operation_id));
        Ok(())
    }

    pub fn begin_failover(
        &mut self,
        operation_id: &str,
        decision_digest: &str,
        candidate_region_id: &str,
        authority_epoch: u64,
        current_tick: u64,
    ) -> Result<FailoverOrchestrationAction, TelemetryFailoverError> {
        validate_identifier(operation_id, "operation")?;
        validate_hash(decision_digest, "decision digest")?;
        validate_identifier(candidate_region_id, "candidate region")?;
        if authority_epoch == 0 {
            return Err(TelemetryFailoverError::InvalidInput(
                "authority epoch must be positive".into(),
            ));
        }
        if self.detected_operation_id.as_deref() != Some(operation_id) {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "failover operation does not match the detected failure".into(),
            ));
        }
        if self.active_region_id.as_deref() == Some(candidate_region_id) {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "candidate region is already active".into(),
            ));
        }
        if let Some(committed_operation_id) = &self.committed_operation_id {
            if committed_operation_id == operation_id {
                return Ok(FailoverOrchestrationAction::AlreadyCommitted);
            }
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "orchestrator is terminally committed".into(),
            ));
        }
        if !matches!(
            self.phase,
            FailoverOrchestrationPhase::DetectingFailure
                | FailoverOrchestrationPhase::CollectingEvidence
                | FailoverOrchestrationPhase::AwaitingFence
        ) {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "failover intent requires failure detection".into(),
            ));
        }
        if let Some(intent) = &self.intent {
            if intent.operation_id == operation_id
                && intent.decision_digest == decision_digest
                && intent.candidate_region_id == candidate_region_id
            {
                return match self.phase {
                    FailoverOrchestrationPhase::AwaitingFence => {
                        Ok(if intent.fence_admitted_tick.is_some() {
                            FailoverOrchestrationAction::AlreadyFenced
                        } else {
                            FailoverOrchestrationAction::AlreadyPrepared
                        })
                    }
                    FailoverOrchestrationPhase::Committed => {
                        Ok(FailoverOrchestrationAction::AlreadyCommitted)
                    }
                    _ => Err(TelemetryFailoverError::OrchestrationRejected(
                        "existing intent is not retryable".into(),
                    )),
                };
            }
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "conflicting failover intent already exists".into(),
            ));
        }
        self.require_fresh_telemetry(authority_epoch, current_tick)?;
        self.intent = Some(FailoverIntent {
            operation_id: operation_id.to_string(),
            decision_digest: decision_digest.to_string(),
            candidate_region_id: candidate_region_id.to_string(),
            authority_epoch,
            prepared_tick: current_tick,
            fence_admitted_tick: None,
        });
        self.phase = FailoverOrchestrationPhase::AwaitingFence;
        self.events.push(format!("prepared:{operation_id}"));
        Ok(FailoverOrchestrationAction::AwaitingExternalFence)
    }

    pub fn admit_external_fence(
        &mut self,
        operation_id: &str,
        decision_digest: &str,
        current_tick: u64,
    ) -> Result<FailoverOrchestrationAction, TelemetryFailoverError> {
        let intent = self.intent.as_mut().ok_or_else(|| {
            TelemetryFailoverError::OrchestrationRejected("no failover intent is prepared".into())
        })?;
        if intent.operation_id != operation_id || intent.decision_digest != decision_digest {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "external fence does not bind to the prepared decision".into(),
            ));
        }
        if self.phase == FailoverOrchestrationPhase::Committed {
            return Ok(FailoverOrchestrationAction::AlreadyCommitted);
        }
        if self.phase != FailoverOrchestrationPhase::AwaitingFence {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "orchestration phase does not admit an external fence".into(),
            ));
        }
        if intent.fence_admitted_tick.is_some() {
            return Ok(FailoverOrchestrationAction::AlreadyFenced);
        }
        intent.fence_admitted_tick = Some(current_tick);
        self.events.push(format!("fenced:{operation_id}"));
        Ok(FailoverOrchestrationAction::FenceAdmitted)
    }

    pub fn commit(
        &mut self,
        operation_id: &str,
        decision_digest: &str,
    ) -> Result<FailoverOrchestrationAction, TelemetryFailoverError> {
        if self.committed_operation_id.as_deref() == Some(operation_id) {
            return Ok(FailoverOrchestrationAction::AlreadyCommitted);
        }
        let intent = self.intent.as_ref().ok_or_else(|| {
            TelemetryFailoverError::OrchestrationRejected("no failover intent is prepared".into())
        })?;
        if intent.operation_id != operation_id || intent.decision_digest != decision_digest {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "commit does not bind to the prepared decision".into(),
            ));
        }
        if intent.fence_admitted_tick.is_none() {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "external fence admission is required before commit".into(),
            ));
        }
        self.active_region_id = Some(intent.candidate_region_id.clone());
        self.committed_operation_id = Some(intent.operation_id.clone());
        self.phase = FailoverOrchestrationPhase::Committed;
        self.events.push(format!("committed:{operation_id}"));
        Ok(FailoverOrchestrationAction::Committed)
    }

    pub fn fail(&mut self, operation_id: &str, reason: &str) -> Result<(), TelemetryFailoverError> {
        validate_identifier(operation_id, "operation")?;
        validate_identifier(reason, "failure reason")?;
        if self.committed_operation_id.is_some() {
            return Err(TelemetryFailoverError::OrchestrationRejected(
                "committed orchestration cannot be downgraded".into(),
            ));
        }
        self.phase = FailoverOrchestrationPhase::Failed;
        self.events.push(format!("failed:{operation_id}:{reason}"));
        Ok(())
    }

    pub fn report(&self) -> FailoverOrchestrationReport {
        let telemetry_epoch = self
            .telemetry
            .values()
            .map(|event| event.authority_epoch)
            .max()
            .unwrap_or(0);
        FailoverOrchestrationReport {
            cluster_id: self.cluster_id.clone(),
            resource_id: self.resource_id.clone(),
            phase: self.phase,
            active_region_id: self.active_region_id.clone(),
            telemetry_epoch,
            telemetry_events: self.telemetry.len(),
            committed_operation_id: self.committed_operation_id.clone(),
            trace_digest: digest_json(&self.events).unwrap_or_default(),
            safety_passed: self.phase != FailoverOrchestrationPhase::Committed
                || self.committed_operation_id.is_some(),
        }
    }

    fn require_fresh_telemetry(
        &mut self,
        authority_epoch: u64,
        current_tick: u64,
    ) -> Result<(), TelemetryFailoverError> {
        for kind in &self.required_kinds {
            let event = self.telemetry.get(kind).ok_or_else(|| {
                self.phase = FailoverOrchestrationPhase::CollectingEvidence;
                TelemetryFailoverError::OrchestrationRejected(format!(
                    "required telemetry is missing: {kind:?}"
                ))
            })?;
            if event.authority_epoch != authority_epoch
                || current_tick > event.observed_tick.saturating_add(event.ttl_ticks)
                || current_tick.saturating_sub(event.observed_tick) > self.max_event_age_ticks
            {
                self.phase = FailoverOrchestrationPhase::CollectingEvidence;
                return Err(TelemetryFailoverError::StaleTelemetry(format!(
                    "required telemetry is stale or epoch-mismatched: {kind:?}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpochChurnFuzzReport {
    pub iterations: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub panics: usize,
    pub max_connection_epoch: u64,
    pub trace_digest: String,
    pub safety_passed: bool,
}

pub fn fuzz_authenticated_transport_receiver(seed: u64, iterations: usize) -> EpochChurnFuzzReport {
    let signing_key = SigningKey::from_bytes(&seed_bytes(seed));
    let mut registry = TransportKeyRegistry::new();
    let _ = registry.register("fuzz-sender", &signing_key.verifying_key());
    let mut receiver = match crate::recovery_transport::AuthenticatedTransportReceiver::new(
        "fuzz-receiver",
        "cluster-a",
        "resource-a",
        1,
        registry,
    ) {
        Ok(receiver) => receiver,
        Err(_) => return empty_fuzz_report(iterations),
    };
    let mut state = seed.max(1);
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut panics = 0usize;
    let mut max_connection_epoch = 0u64;
    for index in 0..iterations {
        let connection_epoch = next_fuzz_value(&mut state) % 4096 + 1;
        let sequence = next_fuzz_value(&mut state) % 256 + 1;
        max_connection_epoch = max_connection_epoch.max(connection_epoch);
        let payload = vec![
            (next_fuzz_value(&mut state) & 0xff) as u8;
            (next_fuzz_value(&mut state) % 64) as usize
        ];
        let mut envelope = match AuthenticatedTransportEnvelope::sign(
            "cluster-a",
            "resource-a",
            "fuzz-sender",
            "fuzz-receiver",
            connection_epoch,
            sequence,
            &format!("nonce-{index}-{connection_epoch}"),
            TransportMessageKind::WitnessVote,
            payload,
            &signing_key,
        ) {
            Ok(envelope) => envelope,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        match next_fuzz_value(&mut state) % 7 {
            0 => envelope.connection_epoch = 0,
            1 => envelope.sequence = 0,
            2 => envelope
                .payload
                .extend(std::iter::repeat_n(0u8, 300 * 1024)),
            3 => envelope.payload_hash = "0".repeat(64),
            4 => envelope.signature[0] ^= 0x80,
            5 => envelope.receiver_id.push('\n'),
            _ => envelope.sender_id = "unknown-sender".into(),
        }
        let result = catch_unwind(AssertUnwindSafe(|| receiver.receive(envelope)));
        match result {
            Ok(Ok(_)) => accepted += 1,
            Ok(Err(_)) => rejected += 1,
            Err(_) => panics += 1,
        }
    }
    fuzz_report(iterations, accepted, rejected, panics, max_connection_epoch)
}

pub fn fuzz_witness_reservation_store(
    path: impl AsRef<Path>,
    seed: u64,
    iterations: usize,
) -> EpochChurnFuzzReport {
    let mut store = WitnessReservationStore::new(path.as_ref().to_path_buf());
    let mut state = seed.max(1);
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut panics = 0usize;
    let mut max_connection_epoch = 0u64;
    for index in 0..iterations {
        let connection_epoch = next_fuzz_value(&mut state) % 4096 + 1;
        max_connection_epoch = max_connection_epoch.max(connection_epoch);
        let digest = format!("{:064x}", next_fuzz_value(&mut state));
        let mut reservation = match WitnessVoteReservation::new(
            index as u64 + 1,
            "fuzz-witness",
            &digest,
            next_fuzz_value(&mut state) % 64 + 1,
            connection_epoch,
        ) {
            Ok(reservation) => reservation,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        let inject_fault = index % 23 == 0;
        if inject_fault {
            store.inject_fault(match index % 3 {
                0 => ReservationPersistenceFault::BeforeStage,
                1 => ReservationPersistenceFault::AfterStage,
                _ => ReservationPersistenceFault::AfterSyncBeforeRename,
            });
        }
        match next_fuzz_value(&mut state) % 6 {
            0 => reservation.connection_epoch = 0,
            1 => reservation.reservation_hash = "0".repeat(64),
            2 => reservation.proposal_digest = "f".repeat(63),
            3 => reservation.witness_id.push('\t'),
            4 => reservation.round_id = 0,
            _ => {}
        }
        let result = catch_unwind(AssertUnwindSafe(|| store.reserve(reservation)));
        store.clear_fault();
        match result {
            Ok(Ok(_)) => accepted += 1,
            Ok(Err(_)) => rejected += 1,
            Err(_) => panics += 1,
        }
        let snapshot_result = catch_unwind(AssertUnwindSafe(|| store.load_snapshot()));
        if snapshot_result.is_err() {
            panics += 1;
        }
    }
    fuzz_report(iterations, accepted, rejected, panics, max_connection_epoch)
}

fn seed_bytes(seed: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = ((seed.rotate_left((index % 63) as u32) >> ((index % 8) * 8)) as u8)
            .wrapping_add(index as u8);
    }
    bytes
}

fn next_fuzz_value(state: &mut u64) -> u64 {
    *state ^= state.wrapping_shl(13);
    *state ^= state.wrapping_shr(7);
    *state ^= state.wrapping_shl(17);
    *state
}

fn empty_fuzz_report(iterations: usize) -> EpochChurnFuzzReport {
    fuzz_report(iterations, 0, iterations, 0, 0)
}

fn fuzz_report(
    iterations: usize,
    accepted: usize,
    rejected: usize,
    panics: usize,
    max_connection_epoch: u64,
) -> EpochChurnFuzzReport {
    let trace_digest = digest_json(&(iterations, accepted, rejected, panics, max_connection_epoch))
        .unwrap_or_default();
    EpochChurnFuzzReport {
        iterations,
        accepted,
        rejected,
        panics,
        max_connection_epoch,
        trace_digest,
        safety_passed: panics == 0 && accepted.saturating_add(rejected) == iterations,
    }
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, TelemetryFailoverError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| TelemetryFailoverError::InvalidInput(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_identifier(value: &str, label: &str) -> Result<(), TelemetryFailoverError> {
    if value.trim().is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(TelemetryFailoverError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_label(value: &str, label: &str) -> Result<(), TelemetryFailoverError> {
    if value.trim().is_empty()
        || value.len() > MAX_LABEL_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(TelemetryFailoverError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), TelemetryFailoverError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(TelemetryFailoverError::InvalidInput(format!(
            "{label} must be a 64-character hexadecimal digest"
        )));
    }
    Ok(())
}
