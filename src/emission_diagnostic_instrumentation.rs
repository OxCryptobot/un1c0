use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::emission_diagnostic_stream::{MAX_STREAM_BYTES, MAX_STREAM_FRAMES};

pub const DIAGNOSTIC_INSTRUMENTATION_VERSION: u8 = 1;
pub const DIAGNOSTIC_TELEMETRY_SCHEMA_VERSION: u8 = 1;
pub const DIAGNOSTIC_TELEMETRY_EVENT_TYPE: &str = "diagnostic_instrumentation_snapshot";
pub const MAX_DIAGNOSTIC_TELEMETRY_SAMPLES: usize = 512;
pub const MAX_DIAGNOSTIC_TELEMETRY_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_SAMPLE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticStage {
    TransportReceive,
    TransportFrameIntegrity,
    StreamShape,
    SnapshotFingerprint,
    NestedReportVerify,
    CanonicalReportSerialize,
    CanonicalStreamSerialize,
    CanonicalBytesReuse,
    ContentHash,
    AttestationShape,
    TrustLookup,
    PublicKeyParse,
    SigningPayloadSerialize,
    Ed25519Verify,
    AggregateAdmission,
    EvidenceCacheLookup,
    EvidenceCacheInsert,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticStageTimings {
    pub transport_receive_ns: u64,
    pub transport_frame_integrity_ns: u64,
    pub stream_shape_ns: u64,
    pub snapshot_fingerprint_ns: u64,
    pub nested_report_verify_ns: u64,
    pub canonical_report_serialize_ns: u64,
    pub canonical_stream_serialize_ns: u64,
    pub canonical_bytes_reuse_ns: u64,
    pub content_hash_ns: u64,
    pub attestation_shape_ns: u64,
    pub trust_lookup_ns: u64,
    pub public_key_parse_ns: u64,
    pub signing_payload_serialize_ns: u64,
    pub ed25519_verify_ns: u64,
    pub aggregate_admission_ns: u64,
    pub evidence_cache_lookup_ns: u64,
    pub evidence_cache_insert_ns: u64,
    pub unattributed_ns: u64,
}

impl DiagnosticStageTimings {
    fn add(&mut self, stage: DiagnosticStage, elapsed_ns: u64) {
        let target = match stage {
            DiagnosticStage::TransportReceive => &mut self.transport_receive_ns,
            DiagnosticStage::TransportFrameIntegrity => &mut self.transport_frame_integrity_ns,
            DiagnosticStage::StreamShape => &mut self.stream_shape_ns,
            DiagnosticStage::SnapshotFingerprint => &mut self.snapshot_fingerprint_ns,
            DiagnosticStage::NestedReportVerify => &mut self.nested_report_verify_ns,
            DiagnosticStage::CanonicalReportSerialize => &mut self.canonical_report_serialize_ns,
            DiagnosticStage::CanonicalStreamSerialize => &mut self.canonical_stream_serialize_ns,
            DiagnosticStage::CanonicalBytesReuse => &mut self.canonical_bytes_reuse_ns,
            DiagnosticStage::ContentHash => &mut self.content_hash_ns,
            DiagnosticStage::AttestationShape => &mut self.attestation_shape_ns,
            DiagnosticStage::TrustLookup => &mut self.trust_lookup_ns,
            DiagnosticStage::PublicKeyParse => &mut self.public_key_parse_ns,
            DiagnosticStage::SigningPayloadSerialize => &mut self.signing_payload_serialize_ns,
            DiagnosticStage::Ed25519Verify => &mut self.ed25519_verify_ns,
            DiagnosticStage::AggregateAdmission => &mut self.aggregate_admission_ns,
            DiagnosticStage::EvidenceCacheLookup => &mut self.evidence_cache_lookup_ns,
            DiagnosticStage::EvidenceCacheInsert => &mut self.evidence_cache_insert_ns,
        };
        *target = target.saturating_add(elapsed_ns);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticStageCounters {
    pub accepted_operations: u64,
    pub rejected_operations: u64,
    pub trust_lookups: u64,
    pub public_key_parses: u64,
    pub signature_verifications: u64,
    pub content_hashes: u64,
    pub frame_integrity_checks: u64,
    pub stale_snapshot_rejections: u64,
    pub replay_gap_rejections: u64,
    pub evidence_cache_hits: u64,
    pub evidence_cache_misses: u64,
    pub evidence_cache_invalidations: u64,
    pub dropped_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticVerificationSample {
    pub schema_version: u8,
    pub frame_count: u16,
    pub stream_bytes: u32,
    pub outcome: VerificationOutcome,
    pub stages: DiagnosticStageTimings,
    pub counters: DiagnosticStageCounters,
    pub unattributed_ns: u64,
    pub end_to_end_ns: u64,
}

impl DiagnosticVerificationSample {
    pub fn contains_sensitive_material(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticInstrumentationSnapshot {
    pub enabled: bool,
    pub completed_operations: u64,
    pub accepted_operations: u64,
    pub rejected_operations: u64,
    pub dropped_samples: u64,
    pub counters: DiagnosticStageCounters,
    pub samples: Vec<DiagnosticVerificationSample>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticTelemetryError {
    Json(String),
    UnsupportedSchemaVersion { version: u8 },
    UnexpectedEventType,
    EnvelopeTooLarge { bytes: usize, maximum: usize },
    TooManySamples { count: usize, maximum: usize },
    InvalidSampleSchema { index: usize, version: u8 },
    InvalidFrameCount { index: usize, count: u16 },
    InvalidStreamBytes { index: usize, bytes: u32 },
    NonCanonicalEncoding,
}

impl std::fmt::Display for DiagnosticTelemetryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "diagnostic telemetry JSON failed: {error}"),
            Self::UnsupportedSchemaVersion { version } => {
                write!(
                    formatter,
                    "unsupported diagnostic telemetry schema version: {version}"
                )
            }
            Self::UnexpectedEventType => {
                formatter.write_str("unexpected diagnostic telemetry event type")
            }
            Self::EnvelopeTooLarge { bytes, maximum } => write!(
                formatter,
                "diagnostic telemetry envelope is {bytes} bytes; maximum is {maximum}"
            ),
            Self::TooManySamples { count, maximum } => write!(
                formatter,
                "diagnostic telemetry contains {count} samples; maximum is {maximum}"
            ),
            Self::InvalidSampleSchema { index, version } => write!(
                formatter,
                "diagnostic telemetry sample {index} has schema version {version}"
            ),
            Self::InvalidFrameCount { index, count } => write!(
                formatter,
                "diagnostic telemetry sample {index} has invalid frame count {count}"
            ),
            Self::InvalidStreamBytes { index, bytes } => write!(
                formatter,
                "diagnostic telemetry sample {index} has invalid stream bytes {bytes}"
            ),
            Self::NonCanonicalEncoding => {
                formatter.write_str("diagnostic telemetry envelope is not canonically encoded")
            }
        }
    }
}

impl std::error::Error for DiagnosticTelemetryError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticTelemetryEnvelope {
    pub schema_version: u8,
    pub event_type: String,
    pub snapshot: DiagnosticInstrumentationSnapshot,
}

impl DiagnosticInstrumentationSnapshot {
    pub fn validate_telemetry(&self) -> Result<(), DiagnosticTelemetryError> {
        if self.samples.len() > MAX_DIAGNOSTIC_TELEMETRY_SAMPLES {
            return Err(DiagnosticTelemetryError::TooManySamples {
                count: self.samples.len(),
                maximum: MAX_DIAGNOSTIC_TELEMETRY_SAMPLES,
            });
        }
        for (index, sample) in self.samples.iter().enumerate() {
            if sample.schema_version != DIAGNOSTIC_INSTRUMENTATION_VERSION {
                return Err(DiagnosticTelemetryError::InvalidSampleSchema {
                    index,
                    version: sample.schema_version,
                });
            }
            if sample.frame_count == 0 || sample.frame_count as usize > MAX_STREAM_FRAMES {
                return Err(DiagnosticTelemetryError::InvalidFrameCount {
                    index,
                    count: sample.frame_count,
                });
            }
            if sample.stream_bytes as usize > MAX_STREAM_BYTES {
                return Err(DiagnosticTelemetryError::InvalidStreamBytes {
                    index,
                    bytes: sample.stream_bytes,
                });
            }
        }
        Ok(())
    }

    pub fn to_versioned_json(&self) -> Result<Vec<u8>, DiagnosticTelemetryError> {
        self.validate_telemetry()?;
        let envelope = DiagnosticTelemetryEnvelope {
            schema_version: DIAGNOSTIC_TELEMETRY_SCHEMA_VERSION,
            event_type: DIAGNOSTIC_TELEMETRY_EVENT_TYPE.to_owned(),
            snapshot: self.clone(),
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| DiagnosticTelemetryError::Json(error.to_string()))?;
        if bytes.len() > MAX_DIAGNOSTIC_TELEMETRY_BYTES {
            return Err(DiagnosticTelemetryError::EnvelopeTooLarge {
                bytes: bytes.len(),
                maximum: MAX_DIAGNOSTIC_TELEMETRY_BYTES,
            });
        }
        Ok(bytes)
    }

    pub fn from_versioned_json(bytes: &[u8]) -> Result<Self, DiagnosticTelemetryError> {
        if bytes.len() > MAX_DIAGNOSTIC_TELEMETRY_BYTES {
            return Err(DiagnosticTelemetryError::EnvelopeTooLarge {
                bytes: bytes.len(),
                maximum: MAX_DIAGNOSTIC_TELEMETRY_BYTES,
            });
        }
        let envelope: DiagnosticTelemetryEnvelope = serde_json::from_slice(bytes)
            .map_err(|error| DiagnosticTelemetryError::Json(error.to_string()))?;
        if envelope.schema_version != DIAGNOSTIC_TELEMETRY_SCHEMA_VERSION {
            return Err(DiagnosticTelemetryError::UnsupportedSchemaVersion {
                version: envelope.schema_version,
            });
        }
        if envelope.event_type != DIAGNOSTIC_TELEMETRY_EVENT_TYPE {
            return Err(DiagnosticTelemetryError::UnexpectedEventType);
        }
        let canonical = serde_json::to_vec(&envelope)
            .map_err(|error| DiagnosticTelemetryError::Json(error.to_string()))?;
        if canonical != bytes {
            return Err(DiagnosticTelemetryError::NonCanonicalEncoding);
        }
        envelope.snapshot.validate_telemetry()?;
        Ok(envelope.snapshot)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticTelemetryCollectError {
    InvalidCapacity,
    QueueFull { entries: usize, maximum: usize },
    Schema(DiagnosticTelemetryError),
}

impl std::fmt::Display for DiagnosticTelemetryCollectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCapacity => {
                formatter.write_str("diagnostic telemetry queue capacity is invalid")
            }
            Self::QueueFull { entries, maximum } => write!(
                formatter,
                "diagnostic telemetry queue has {entries} entries; maximum is {maximum}"
            ),
            Self::Schema(error) => write!(
                formatter,
                "diagnostic telemetry schema rejected sample: {error}"
            ),
        }
    }
}

impl std::error::Error for DiagnosticTelemetryCollectError {}

#[derive(Debug, Clone)]
struct TelemetryCollectorState {
    queue: VecDeque<Vec<u8>>,
    maximum: usize,
}

#[derive(Debug, Clone)]
pub struct DiagnosticTelemetryCollector {
    state: Arc<Mutex<TelemetryCollectorState>>,
}

impl DiagnosticTelemetryCollector {
    pub fn new(maximum: usize) -> Result<Self, DiagnosticTelemetryCollectError> {
        if maximum == 0 || maximum > MAX_DIAGNOSTIC_TELEMETRY_SAMPLES {
            return Err(DiagnosticTelemetryCollectError::InvalidCapacity);
        }
        Ok(Self {
            state: Arc::new(Mutex::new(TelemetryCollectorState {
                queue: VecDeque::with_capacity(maximum.min(64)),
                maximum,
            })),
        })
    }

    pub fn collect(
        &self,
        snapshot: &DiagnosticInstrumentationSnapshot,
    ) -> Result<(), DiagnosticTelemetryCollectError> {
        let bytes = snapshot
            .to_versioned_json()
            .map_err(DiagnosticTelemetryCollectError::Schema)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.queue.len() >= state.maximum {
            return Err(DiagnosticTelemetryCollectError::QueueFull {
                entries: state.queue.len(),
                maximum: state.maximum,
            });
        }
        state.queue.push_back(bytes);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queue
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn maximum(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .maximum
    }
}

#[derive(Debug)]
struct InstrumentationState {
    samples: Mutex<Vec<DiagnosticVerificationSample>>,
    capacity: usize,
    completed_operations: AtomicU64,
    accepted_operations: AtomicU64,
    rejected_operations: AtomicU64,
    dropped_samples: AtomicU64,
    trust_lookups: AtomicU64,
    public_key_parses: AtomicU64,
    signature_verifications: AtomicU64,
    content_hashes: AtomicU64,
    frame_integrity_checks: AtomicU64,
    stale_snapshot_rejections: AtomicU64,
    replay_gap_rejections: AtomicU64,
    evidence_cache_hits: AtomicU64,
    evidence_cache_misses: AtomicU64,
    evidence_cache_invalidations: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct DiagnosticInstrumentation {
    state: Option<Arc<InstrumentationState>>,
}

impl Default for DiagnosticInstrumentation {
    fn default() -> Self {
        Self::disabled()
    }
}

impl DiagnosticInstrumentation {
    pub fn disabled() -> Self {
        Self { state: None }
    }

    pub fn enabled(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            state: Some(Arc::new(InstrumentationState {
                samples: Mutex::new(Vec::with_capacity(capacity.min(DEFAULT_SAMPLE_CAPACITY))),
                capacity,
                completed_operations: AtomicU64::new(0),
                accepted_operations: AtomicU64::new(0),
                rejected_operations: AtomicU64::new(0),
                dropped_samples: AtomicU64::new(0),
                trust_lookups: AtomicU64::new(0),
                public_key_parses: AtomicU64::new(0),
                signature_verifications: AtomicU64::new(0),
                content_hashes: AtomicU64::new(0),
                frame_integrity_checks: AtomicU64::new(0),
                stale_snapshot_rejections: AtomicU64::new(0),
                replay_gap_rejections: AtomicU64::new(0),
                evidence_cache_hits: AtomicU64::new(0),
                evidence_cache_misses: AtomicU64::new(0),
                evidence_cache_invalidations: AtomicU64::new(0),
            })),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.state.is_some()
    }

    pub fn recorder(
        &self,
        frame_count: usize,
        stream_bytes: usize,
    ) -> DiagnosticVerificationRecorder {
        DiagnosticVerificationRecorder {
            instrumentation: self.state.clone(),
            started_at: Instant::now(),
            frame_count: frame_count.min(u16::MAX as usize) as u16,
            stream_bytes: stream_bytes.min(u32::MAX as usize) as u32,
            stages: DiagnosticStageTimings::default(),
            counters: DiagnosticStageCounters::default(),
        }
    }

    pub fn snapshot(&self) -> DiagnosticInstrumentationSnapshot {
        let Some(state) = &self.state else {
            return DiagnosticInstrumentationSnapshot {
                enabled: false,
                completed_operations: 0,
                accepted_operations: 0,
                rejected_operations: 0,
                dropped_samples: 0,
                counters: DiagnosticStageCounters::default(),
                samples: Vec::new(),
            };
        };
        let samples = state
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        DiagnosticInstrumentationSnapshot {
            enabled: true,
            completed_operations: state.completed_operations.load(Ordering::Relaxed),
            accepted_operations: state.accepted_operations.load(Ordering::Relaxed),
            rejected_operations: state.rejected_operations.load(Ordering::Relaxed),
            dropped_samples: state.dropped_samples.load(Ordering::Relaxed),
            counters: DiagnosticStageCounters {
                accepted_operations: state.accepted_operations.load(Ordering::Relaxed),
                rejected_operations: state.rejected_operations.load(Ordering::Relaxed),
                trust_lookups: state.trust_lookups.load(Ordering::Relaxed),
                public_key_parses: state.public_key_parses.load(Ordering::Relaxed),
                signature_verifications: state.signature_verifications.load(Ordering::Relaxed),
                content_hashes: state.content_hashes.load(Ordering::Relaxed),
                frame_integrity_checks: state.frame_integrity_checks.load(Ordering::Relaxed),
                stale_snapshot_rejections: state.stale_snapshot_rejections.load(Ordering::Relaxed),
                replay_gap_rejections: state.replay_gap_rejections.load(Ordering::Relaxed),
                evidence_cache_hits: state.evidence_cache_hits.load(Ordering::Relaxed),
                evidence_cache_misses: state.evidence_cache_misses.load(Ordering::Relaxed),
                evidence_cache_invalidations: state
                    .evidence_cache_invalidations
                    .load(Ordering::Relaxed),
                dropped_samples: state.dropped_samples.load(Ordering::Relaxed),
            },
            samples,
        }
    }
}

pub struct DiagnosticVerificationRecorder {
    instrumentation: Option<Arc<InstrumentationState>>,
    started_at: Instant,
    frame_count: u16,
    stream_bytes: u32,
    stages: DiagnosticStageTimings,
    counters: DiagnosticStageCounters,
}

impl DiagnosticVerificationRecorder {
    pub fn time<T>(&mut self, stage: DiagnosticStage, operation: impl FnOnce() -> T) -> T {
        if self.instrumentation.is_none() {
            return operation();
        }
        let started_at = Instant::now();
        let result = operation();
        self.stages.add(stage, elapsed_ns(started_at));
        result
    }

    pub fn record_elapsed(&mut self, stage: DiagnosticStage, elapsed_ns: u64) {
        if self.instrumentation.is_some() {
            self.stages.add(stage, elapsed_ns);
        }
    }

    pub fn increment(&mut self, counter: DiagnosticCounter) {
        if self.instrumentation.is_none() {
            return;
        }
        match counter {
            DiagnosticCounter::TrustLookup => {
                self.counters.trust_lookups = self.counters.trust_lookups.saturating_add(1)
            }
            DiagnosticCounter::PublicKeyParse => {
                self.counters.public_key_parses = self.counters.public_key_parses.saturating_add(1)
            }
            DiagnosticCounter::SignatureVerification => {
                self.counters.signature_verifications =
                    self.counters.signature_verifications.saturating_add(1)
            }
            DiagnosticCounter::ContentHash => {
                self.counters.content_hashes = self.counters.content_hashes.saturating_add(1)
            }
            DiagnosticCounter::FrameIntegrity => {
                self.counters.frame_integrity_checks =
                    self.counters.frame_integrity_checks.saturating_add(1)
            }
            DiagnosticCounter::StaleSnapshotRejection => {
                self.counters.stale_snapshot_rejections =
                    self.counters.stale_snapshot_rejections.saturating_add(1)
            }
            DiagnosticCounter::ReplayGapRejection => {
                self.counters.replay_gap_rejections =
                    self.counters.replay_gap_rejections.saturating_add(1)
            }
            DiagnosticCounter::EvidenceCacheHit => {
                self.counters.evidence_cache_hits =
                    self.counters.evidence_cache_hits.saturating_add(1)
            }
            DiagnosticCounter::EvidenceCacheMiss => {
                self.counters.evidence_cache_misses =
                    self.counters.evidence_cache_misses.saturating_add(1)
            }
            DiagnosticCounter::EvidenceCacheInvalidation => {
                self.counters.evidence_cache_invalidations =
                    self.counters.evidence_cache_invalidations.saturating_add(1)
            }
        }
    }

    pub fn finish(self, outcome: VerificationOutcome) {
        let Some(state) = self.instrumentation else {
            return;
        };
        let end_to_end_ns = elapsed_ns(self.started_at);
        let stage_total = self
            .stages
            .transport_receive_ns
            .saturating_add(self.stages.transport_frame_integrity_ns)
            .saturating_add(self.stages.stream_shape_ns)
            .saturating_add(self.stages.snapshot_fingerprint_ns)
            .saturating_add(self.stages.nested_report_verify_ns)
            .saturating_add(self.stages.canonical_report_serialize_ns)
            .saturating_add(self.stages.canonical_stream_serialize_ns)
            .saturating_add(self.stages.canonical_bytes_reuse_ns)
            .saturating_add(self.stages.content_hash_ns)
            .saturating_add(self.stages.attestation_shape_ns)
            .saturating_add(self.stages.trust_lookup_ns)
            .saturating_add(self.stages.public_key_parse_ns)
            .saturating_add(self.stages.signing_payload_serialize_ns)
            .saturating_add(self.stages.ed25519_verify_ns)
            .saturating_add(self.stages.aggregate_admission_ns)
            .saturating_add(self.stages.evidence_cache_lookup_ns)
            .saturating_add(self.stages.evidence_cache_insert_ns);
        let sample = DiagnosticVerificationSample {
            schema_version: DIAGNOSTIC_INSTRUMENTATION_VERSION,
            frame_count: self.frame_count,
            stream_bytes: self.stream_bytes,
            outcome,
            stages: self.stages,
            counters: self.counters.clone(),
            unattributed_ns: end_to_end_ns.saturating_sub(stage_total),
            end_to_end_ns,
        };
        state.completed_operations.fetch_add(1, Ordering::Relaxed);
        match outcome {
            VerificationOutcome::Accepted => {
                state.accepted_operations.fetch_add(1, Ordering::Relaxed);
            }
            VerificationOutcome::Rejected => {
                state.rejected_operations.fetch_add(1, Ordering::Relaxed);
            }
        }
        state
            .trust_lookups
            .fetch_add(sample.counters.trust_lookups, Ordering::Relaxed);
        state
            .public_key_parses
            .fetch_add(sample.counters.public_key_parses, Ordering::Relaxed);
        state
            .signature_verifications
            .fetch_add(sample.counters.signature_verifications, Ordering::Relaxed);
        state
            .content_hashes
            .fetch_add(sample.counters.content_hashes, Ordering::Relaxed);
        state
            .frame_integrity_checks
            .fetch_add(sample.counters.frame_integrity_checks, Ordering::Relaxed);
        state
            .stale_snapshot_rejections
            .fetch_add(sample.counters.stale_snapshot_rejections, Ordering::Relaxed);
        state
            .replay_gap_rejections
            .fetch_add(sample.counters.replay_gap_rejections, Ordering::Relaxed);
        state
            .evidence_cache_hits
            .fetch_add(sample.counters.evidence_cache_hits, Ordering::Relaxed);
        state
            .evidence_cache_misses
            .fetch_add(sample.counters.evidence_cache_misses, Ordering::Relaxed);
        state.evidence_cache_invalidations.fetch_add(
            sample.counters.evidence_cache_invalidations,
            Ordering::Relaxed,
        );
        let mut samples = state
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if samples.len() >= state.capacity {
            state.dropped_samples.fetch_add(1, Ordering::Relaxed);
        } else {
            samples.push(sample);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCounter {
    TrustLookup,
    PublicKeyParse,
    SignatureVerification,
    ContentHash,
    FrameIntegrity,
    StaleSnapshotRejection,
    ReplayGapRejection,
    EvidenceCacheHit,
    EvidenceCacheMiss,
    EvidenceCacheInvalidation,
}

fn elapsed_ns(started_at: Instant) -> u64 {
    started_at.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

pub fn redacted_numeric_fields(
    sample: &DiagnosticVerificationSample,
) -> BTreeMap<&'static str, u64> {
    BTreeMap::from([
        ("frame_count", sample.frame_count as u64),
        ("stream_bytes", sample.stream_bytes as u64),
        ("end_to_end_ns", sample.end_to_end_ns),
        ("unattributed_ns", sample.unattributed_ns),
    ])
}
