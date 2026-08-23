use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::emission_diagnostic::EmissionDiagnosticReport;
use crate::emission_diagnostic_instrumentation::{DiagnosticStage, DiagnosticVerificationRecorder};
use crate::emission_diagnostic_serialization::{
    EmissionDiagnosticSerializationError, MAX_SERIALIZED_DIAGNOSTIC_BYTES,
};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_cache::SemanticCacheKey;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;
use crate::TargetBinding;

pub const MAX_STREAM_FRAMES: usize = 32;
pub const MAX_STREAM_BYTES: usize = 256 * 1024;
const STREAM_VERSION: u8 = 1;
const STREAM_DOMAIN: &[u8] = b"un1c0/phase70/emission-diagnostic-stream/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticStreamError {
    Empty,
    InvalidStreamId,
    InvalidVersion(u8),
    TooManyFrames {
        count: usize,
        maximum: usize,
    },
    FrameTooLarge {
        sequence: u64,
        bytes: usize,
        maximum: usize,
    },
    StreamTooLarge {
        bytes: usize,
        maximum: usize,
    },
    SequenceMismatch {
        expected: u64,
        actual: u64,
    },
    InvalidTarget,
    InvalidBatchId,
    InvalidUnitId,
    ContextMismatch {
        field: &'static str,
    },
    FrameEncodingMismatch {
        sequence: u64,
    },
    IntegrityMismatch,
    NonCanonical,
    Json(String),
    Nested {
        sequence: u64,
        error: EmissionDiagnosticSerializationError,
    },
}

impl Display for EmissionDiagnosticStreamError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("diagnostic stream must contain at least one frame"),
            Self::InvalidStreamId => formatter.write_str("diagnostic stream ID must be non-zero"),
            Self::InvalidVersion(version) => {
                write!(formatter, "unsupported diagnostic stream version {version}")
            }
            Self::TooManyFrames { count, maximum } => {
                write!(
                    formatter,
                    "diagnostic stream contains {count} frames; maximum is {maximum}"
                )
            }
            Self::FrameTooLarge {
                sequence,
                bytes,
                maximum,
            } => write!(
                formatter,
                "diagnostic stream frame {sequence} is {bytes} bytes; maximum is {maximum}"
            ),
            Self::StreamTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "diagnostic stream is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "diagnostic stream expected sequence {expected}, received {actual}"
            ),
            Self::InvalidTarget => formatter.write_str("diagnostic stream target is invalid"),
            Self::InvalidBatchId => {
                formatter.write_str("diagnostic stream batch ID must be non-zero")
            }
            Self::InvalidUnitId => {
                formatter.write_str("diagnostic stream contains an invalid unit ID")
            }
            Self::ContextMismatch { field } => {
                write!(formatter, "diagnostic stream context mismatch in {field}")
            }
            Self::FrameEncodingMismatch { sequence } => write!(
                formatter,
                "diagnostic stream frame {sequence} encoding does not match its report"
            ),
            Self::IntegrityMismatch => {
                formatter.write_str("diagnostic stream integrity digest mismatch")
            }
            Self::NonCanonical => formatter.write_str("diagnostic stream bytes are not canonical"),
            Self::Json(error) => write!(formatter, "diagnostic stream JSON failed: {error}"),
            Self::Nested { sequence, error } => {
                write!(
                    formatter,
                    "diagnostic stream frame {sequence} failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for EmissionDiagnosticStreamError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticStreamFrame {
    sequence: u64,
    report: EmissionDiagnosticReport,
    encoded: Vec<u8>,
}

impl EmissionDiagnosticStreamFrame {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn report(&self) -> &EmissionDiagnosticReport {
        &self.report
    }

    pub fn encoded_bytes(&self) -> &[u8] {
        &self.encoded
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticStreamTemplate {
    report: EmissionDiagnosticReport,
    encoded: Vec<u8>,
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
}

impl EmissionDiagnosticStreamTemplate {
    pub fn from_report(
        report: &EmissionDiagnosticReport,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticStreamError> {
        report
            .verify_for(envelope, profile, units)
            .map_err(|error| EmissionDiagnosticStreamError::Nested {
                sequence: 1,
                error: EmissionDiagnosticSerializationError::Report(error),
            })?;
        let encoded = report
            .to_json()
            .map_err(|error| EmissionDiagnosticStreamError::Nested { sequence: 1, error })?;
        check_frame_size(1, encoded.len())?;
        Ok(Self {
            report: report.clone(),
            encoded,
            target: report.aggregate().target(),
            batch_id: report.aggregate().batch_id(),
            profile_key: report.aggregate().profile_key(),
            unit_roots: report.aggregate().unit_roots().clone(),
        })
    }

    pub fn build(
        &self,
        stream_id: u64,
        frame_count: usize,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<EmissionDiagnosticStream, EmissionDiagnosticStreamError> {
        if stream_id == 0 {
            return Err(EmissionDiagnosticStreamError::InvalidStreamId);
        }
        if frame_count == 0 {
            return Err(EmissionDiagnosticStreamError::Empty);
        }
        if frame_count > MAX_STREAM_FRAMES {
            return Err(EmissionDiagnosticStreamError::TooManyFrames {
                count: frame_count,
                maximum: MAX_STREAM_FRAMES,
            });
        }
        self.report
            .verify_for(envelope, profile, units)
            .map_err(|error| EmissionDiagnosticStreamError::Nested {
                sequence: 1,
                error: EmissionDiagnosticSerializationError::Report(error),
            })?;
        // `from_report` already produced immutable canonical bytes. After current-state
        // verification succeeds, re-encoding the unchanged report would be redundant.
        let encoded = &self.encoded;
        check_frame_size(1, encoded.len())?;
        let total_frame_bytes = encoded.len().checked_mul(frame_count).ok_or(
            EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: usize::MAX,
                maximum: MAX_STREAM_BYTES,
            },
        )?;
        if total_frame_bytes > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: total_frame_bytes,
                maximum: MAX_STREAM_BYTES,
            });
        }
        let mut frames = Vec::with_capacity(frame_count);
        for index in 0..frame_count {
            frames.push(EmissionDiagnosticStreamFrame {
                sequence: (index + 1) as u64,
                report: self.report.clone(),
                encoded: encoded.to_vec(),
            });
        }
        let mut stream = EmissionDiagnosticStream {
            stream_id,
            target: self.target,
            batch_id: self.batch_id,
            profile_key: self.profile_key,
            unit_roots: self.unit_roots.clone(),
            frames,
            total_frame_bytes,
            stream_digest: [0; 32],
            canonical_payload: Arc::from([]),
            canonical_json: Arc::from([]),
        };
        stream.stream_digest = stream.calculate_digest()?;
        stream.finalize_canonical_bytes()?;
        Ok(stream)
    }

    pub fn encoded_bytes(&self) -> &[u8] {
        &self.encoded
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticStreamSummary {
    pub stream_id: u64,
    pub frame_count: usize,
    pub total_frame_bytes: usize,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub stream_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionDiagnosticStream {
    stream_id: u64,
    target: TargetBinding,
    batch_id: u64,
    profile_key: SemanticCacheKey,
    unit_roots: BTreeMap<SemanticUnitId, SemanticCacheKey>,
    frames: Vec<EmissionDiagnosticStreamFrame>,
    total_frame_bytes: usize,
    stream_digest: [u8; 32],
    canonical_payload: Arc<[u8]>,
    canonical_json: Arc<[u8]>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamEnvelope {
    version: u8,
    stream_id: u64,
    target: String,
    batch_id: u64,
    profile_key: [u8; 32],
    unit_roots: BTreeMap<String, [u8; 32]>,
    frames: Vec<StreamFrameEnvelope>,
    stream_digest: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamFrameEnvelope {
    sequence: u64,
    envelope: Vec<u8>,
}

impl EmissionDiagnosticStream {
    pub fn from_repeated_report(
        stream_id: u64,
        report: &EmissionDiagnosticReport,
        frame_count: usize,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticStreamError> {
        if frame_count == 0 {
            return Err(EmissionDiagnosticStreamError::Empty);
        }
        if frame_count > MAX_STREAM_FRAMES {
            return Err(EmissionDiagnosticStreamError::TooManyFrames {
                count: frame_count,
                maximum: MAX_STREAM_FRAMES,
            });
        }
        let template =
            EmissionDiagnosticStreamTemplate::from_report(report, envelope, profile, units)?;
        template.build(stream_id, frame_count, envelope, profile, units)
    }

    pub fn from_verified_reports(
        stream_id: u64,
        reports: &[EmissionDiagnosticReport],
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticStreamError> {
        if stream_id == 0 {
            return Err(EmissionDiagnosticStreamError::InvalidStreamId);
        }
        if reports.is_empty() {
            return Err(EmissionDiagnosticStreamError::Empty);
        }
        if reports.len() > MAX_STREAM_FRAMES {
            return Err(EmissionDiagnosticStreamError::TooManyFrames {
                count: reports.len(),
                maximum: MAX_STREAM_FRAMES,
            });
        }

        let first = reports.first().expect("non-empty reports");
        first
            .verify_for(envelope, profile, units)
            .map_err(|error| EmissionDiagnosticStreamError::Nested {
                sequence: 1,
                error: EmissionDiagnosticSerializationError::Report(error),
            })?;
        let target = first.aggregate().target();
        let batch_id = first.aggregate().batch_id();
        let profile_key = first.aggregate().profile_key();
        let unit_roots = first.aggregate().unit_roots().clone();
        let first_encoded = first
            .to_json()
            .map_err(|error| EmissionDiagnosticStreamError::Nested { sequence: 1, error })?;
        check_frame_size(1, first_encoded.len())?;
        let mut frames = Vec::with_capacity(reports.len());
        let mut total_frame_bytes = 0usize;

        for (index, report) in reports.iter().enumerate() {
            let sequence = (index + 1) as u64;
            let encoded = if index == 0 || report == first {
                // Equal reports have identical private aggregate and entry state. The first
                // report has already passed current-state verification and canonical encoding,
                // so reusing those bytes avoids repeated semantic work for equivalent frames.
                first_encoded.clone()
            } else {
                report
                    .verify_for(envelope, profile, units)
                    .map_err(|error| EmissionDiagnosticStreamError::Nested {
                        sequence,
                        error: EmissionDiagnosticSerializationError::Report(error),
                    })?;
                if report.aggregate().target() != target {
                    return Err(EmissionDiagnosticStreamError::ContextMismatch { field: "target" });
                }
                if report.aggregate().batch_id() != batch_id {
                    return Err(EmissionDiagnosticStreamError::ContextMismatch {
                        field: "batch_id",
                    });
                }
                if report.aggregate().profile_key() != profile_key {
                    return Err(EmissionDiagnosticStreamError::ContextMismatch {
                        field: "profile_key",
                    });
                }
                if report.aggregate().unit_roots() != &unit_roots {
                    return Err(EmissionDiagnosticStreamError::ContextMismatch {
                        field: "unit_roots",
                    });
                }
                report
                    .to_json()
                    .map_err(|error| EmissionDiagnosticStreamError::Nested { sequence, error })?
            };
            check_frame_size(sequence, encoded.len())?;
            total_frame_bytes = total_frame_bytes.checked_add(encoded.len()).ok_or(
                EmissionDiagnosticStreamError::StreamTooLarge {
                    bytes: usize::MAX,
                    maximum: MAX_STREAM_BYTES,
                },
            )?;
            if total_frame_bytes > MAX_STREAM_BYTES {
                return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                    bytes: total_frame_bytes,
                    maximum: MAX_STREAM_BYTES,
                });
            }
            frames.push(EmissionDiagnosticStreamFrame {
                sequence,
                report: report.clone(),
                encoded,
            });
        }

        let mut stream = Self {
            stream_id,
            target,
            batch_id,
            profile_key,
            unit_roots,
            frames,
            total_frame_bytes,
            stream_digest: [0; 32],
            canonical_payload: Arc::from([]),
            canonical_json: Arc::from([]),
        };
        stream.stream_digest = stream.calculate_digest()?;
        stream.finalize_canonical_bytes()?;
        Ok(stream)
    }

    pub fn from_json_for(
        bytes: &[u8],
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Self, EmissionDiagnosticStreamError> {
        if bytes.len() > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: bytes.len(),
                maximum: MAX_STREAM_BYTES,
            });
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| EmissionDiagnosticStreamError::Json(error.to_string()))?;
        let frame_count = value
            .get("frames")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if frame_count > MAX_STREAM_FRAMES {
            return Err(EmissionDiagnosticStreamError::TooManyFrames {
                count: frame_count,
                maximum: MAX_STREAM_FRAMES,
            });
        }
        let mut wire: StreamEnvelope = serde_json::from_value(value)
            .map_err(|error| EmissionDiagnosticStreamError::Json(error.to_string()))?;
        if wire.version != STREAM_VERSION {
            return Err(EmissionDiagnosticStreamError::InvalidVersion(wire.version));
        }
        if wire.stream_id == 0 {
            return Err(EmissionDiagnosticStreamError::InvalidStreamId);
        }
        if wire.frames.is_empty() {
            return Err(EmissionDiagnosticStreamError::Empty);
        }
        let provided_digest = wire.stream_digest;
        wire.stream_digest = [0; 32];
        let payload = serde_json::to_vec(&wire)
            .map_err(|error| EmissionDiagnosticStreamError::Json(error.to_string()))?;
        if digest_payload(&payload) != provided_digest {
            return Err(EmissionDiagnosticStreamError::IntegrityMismatch);
        }
        wire.stream_digest = provided_digest;
        let canonical = serde_json::to_vec(&wire)
            .map_err(|error| EmissionDiagnosticStreamError::Json(error.to_string()))?;
        if canonical != bytes {
            return Err(EmissionDiagnosticStreamError::NonCanonical);
        }
        if wire.target != profile.target.label() {
            return Err(EmissionDiagnosticStreamError::InvalidTarget);
        }
        if wire.batch_id == 0 {
            return Err(EmissionDiagnosticStreamError::InvalidBatchId);
        }
        if wire.batch_id != envelope.batch_id() {
            return Err(EmissionDiagnosticStreamError::ContextMismatch { field: "batch_id" });
        }
        if wire.profile_key != *envelope.profile_key().as_bytes() {
            return Err(EmissionDiagnosticStreamError::ContextMismatch {
                field: "profile_key",
            });
        }
        let expected_roots = envelope
            .units()
            .iter()
            .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
            .collect::<BTreeMap<_, _>>();
        let mut unit_roots = BTreeMap::new();
        for (unit, root) in &wire.unit_roots {
            let id = SemanticUnitId::new(unit.clone())
                .map_err(|_| EmissionDiagnosticStreamError::InvalidUnitId)?;
            unit_roots.insert(id, SemanticCacheKey::from_bytes(*root));
        }
        if unit_roots != expected_roots {
            return Err(EmissionDiagnosticStreamError::ContextMismatch {
                field: "unit_roots",
            });
        }

        let mut frames = Vec::with_capacity(wire.frames.len());
        let mut total_frame_bytes = 0usize;
        for (index, frame) in wire.frames.into_iter().enumerate() {
            let expected_sequence = (index + 1) as u64;
            if frame.sequence != expected_sequence {
                return Err(EmissionDiagnosticStreamError::SequenceMismatch {
                    expected: expected_sequence,
                    actual: frame.sequence,
                });
            }
            check_frame_size(frame.sequence, frame.envelope.len())?;
            total_frame_bytes = total_frame_bytes.checked_add(frame.envelope.len()).ok_or(
                EmissionDiagnosticStreamError::StreamTooLarge {
                    bytes: usize::MAX,
                    maximum: MAX_STREAM_BYTES,
                },
            )?;
            if total_frame_bytes > MAX_STREAM_BYTES {
                return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                    bytes: total_frame_bytes,
                    maximum: MAX_STREAM_BYTES,
                });
            }
            let report =
                EmissionDiagnosticReport::from_json_for(&frame.envelope, envelope, profile, units)
                    .map_err(|error| EmissionDiagnosticStreamError::Nested {
                        sequence: frame.sequence,
                        error,
                    })?;
            if report.aggregate().target() != profile.target
                || report.aggregate().batch_id() != wire.batch_id
                || report.aggregate().profile_key().as_bytes() != &wire.profile_key
                || report.aggregate().unit_roots() != &unit_roots
            {
                return Err(EmissionDiagnosticStreamError::ContextMismatch {
                    field: "frame_context",
                });
            }
            frames.push(EmissionDiagnosticStreamFrame {
                sequence: frame.sequence,
                report,
                encoded: frame.envelope,
            });
        }

        let stream = Self {
            stream_id: wire.stream_id,
            target: profile.target,
            batch_id: wire.batch_id,
            profile_key: SemanticCacheKey::from_bytes(wire.profile_key),
            unit_roots,
            frames,
            total_frame_bytes,
            stream_digest: provided_digest,
            canonical_payload: Arc::from(payload.into_boxed_slice()),
            canonical_json: Arc::from(canonical.into_boxed_slice()),
        };
        stream.verify_for(envelope, profile, units)?;
        Ok(stream)
    }

    pub fn canonical_payload_bytes(&self) -> Result<Vec<u8>, EmissionDiagnosticStreamError> {
        if self.canonical_payload.is_empty() {
            return Err(EmissionDiagnosticStreamError::Json(
                "canonical stream payload cache is unavailable".to_owned(),
            ));
        }
        if digest_payload(&self.canonical_payload) != self.stream_digest {
            return Err(EmissionDiagnosticStreamError::IntegrityMismatch);
        }
        Ok(self.canonical_payload.to_vec())
    }

    pub fn canonical_payload_digest(payload: &[u8]) -> [u8; 32] {
        digest_payload(payload)
    }

    pub fn canonical_json_bytes(&self) -> &[u8] {
        &self.canonical_json
    }

    pub(crate) fn canonical_json_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.canonical_json)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, EmissionDiagnosticStreamError> {
        if self.canonical_json.is_empty() {
            return Err(EmissionDiagnosticStreamError::Json(
                "canonical stream JSON cache is unavailable".to_owned(),
            ));
        }
        if digest_payload(&self.canonical_payload) != self.stream_digest {
            return Err(EmissionDiagnosticStreamError::IntegrityMismatch);
        }
        if self.canonical_json.len() > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: self.canonical_json.len(),
                maximum: MAX_STREAM_BYTES,
            });
        }
        Ok(self.canonical_json.to_vec())
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticStreamError> {
        self.verify_for_inner(envelope, profile, units, None)
    }

    pub(crate) fn verify_for_instrumented(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        recorder: &mut DiagnosticVerificationRecorder,
    ) -> Result<(), EmissionDiagnosticStreamError> {
        self.verify_for_inner(envelope, profile, units, Some(recorder))
    }

    fn verify_for_inner(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        mut recorder: Option<&mut DiagnosticVerificationRecorder>,
    ) -> Result<(), EmissionDiagnosticStreamError> {
        if self.stream_id == 0 {
            return Err(EmissionDiagnosticStreamError::InvalidStreamId);
        }
        if self.frames.is_empty() {
            return Err(EmissionDiagnosticStreamError::Empty);
        }
        if self.frames.len() > MAX_STREAM_FRAMES {
            return Err(EmissionDiagnosticStreamError::TooManyFrames {
                count: self.frames.len(),
                maximum: MAX_STREAM_FRAMES,
            });
        }
        if self.target != profile.target || self.batch_id != envelope.batch_id() {
            return Err(EmissionDiagnosticStreamError::ContextMismatch {
                field: "stream_context",
            });
        }
        if self.profile_key != envelope.profile_key() {
            return Err(EmissionDiagnosticStreamError::ContextMismatch {
                field: "profile_key",
            });
        }
        let expected_roots = envelope
            .units()
            .iter()
            .map(|(unit, snapshot)| (unit.clone(), snapshot.root_key()))
            .collect::<BTreeMap<_, _>>();
        if self.unit_roots != expected_roots {
            return Err(EmissionDiagnosticStreamError::ContextMismatch {
                field: "unit_roots",
            });
        }
        let mut total_frame_bytes = 0usize;
        for (index, frame) in self.frames.iter().enumerate() {
            let expected_sequence = (index + 1) as u64;
            if frame.sequence != expected_sequence {
                return Err(EmissionDiagnosticStreamError::SequenceMismatch {
                    expected: expected_sequence,
                    actual: frame.sequence,
                });
            }
            if let Some(recorder) = recorder.as_deref_mut() {
                frame
                    .report
                    .verify_for_instrumented(envelope, profile, units, recorder)
                    .map_err(|error| EmissionDiagnosticStreamError::Nested {
                        sequence: frame.sequence,
                        error: EmissionDiagnosticSerializationError::Report(error),
                    })?;
            } else {
                frame
                    .report
                    .verify_for(envelope, profile, units)
                    .map_err(|error| EmissionDiagnosticStreamError::Nested {
                        sequence: frame.sequence,
                        error: EmissionDiagnosticSerializationError::Report(error),
                    })?;
            }
            check_frame_size(frame.sequence, frame.encoded.len())?;
            total_frame_bytes = total_frame_bytes.checked_add(frame.encoded.len()).ok_or(
                EmissionDiagnosticStreamError::StreamTooLarge {
                    bytes: usize::MAX,
                    maximum: MAX_STREAM_BYTES,
                },
            )?;
        }
        if total_frame_bytes != self.total_frame_bytes || total_frame_bytes > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: total_frame_bytes,
                maximum: MAX_STREAM_BYTES,
            });
        }
        let payload = if self.canonical_payload.is_empty() {
            return Err(EmissionDiagnosticStreamError::Json(
                "canonical stream payload cache is unavailable".to_owned(),
            ));
        } else if let Some(recorder) = recorder.as_deref_mut() {
            recorder.time(DiagnosticStage::CanonicalBytesReuse, || {
                self.canonical_payload.to_vec()
            })
        } else {
            self.canonical_payload.to_vec()
        };
        let calculated_digest = if let Some(recorder) = recorder.as_deref_mut() {
            recorder.increment(
                crate::emission_diagnostic_instrumentation::DiagnosticCounter::ContentHash,
            );
            recorder.time(DiagnosticStage::ContentHash, || digest_payload(&payload))
        } else {
            digest_payload(&payload)
        };
        if calculated_digest != self.stream_digest {
            return Err(EmissionDiagnosticStreamError::IntegrityMismatch);
        }
        if self.canonical_json.is_empty() || self.canonical_json.len() > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: self.canonical_json.len(),
                maximum: MAX_STREAM_BYTES,
            });
        }
        Ok(())
    }

    pub fn target(&self) -> TargetBinding {
        self.target
    }

    pub fn batch_id(&self) -> u64 {
        self.batch_id
    }

    pub fn profile_key(&self) -> SemanticCacheKey {
        self.profile_key
    }

    pub fn unit_roots(&self) -> &BTreeMap<SemanticUnitId, SemanticCacheKey> {
        &self.unit_roots
    }

    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn total_frame_bytes(&self) -> usize {
        self.total_frame_bytes
    }

    pub fn stream_digest(&self) -> [u8; 32] {
        self.stream_digest
    }

    pub fn frames(&self) -> &[EmissionDiagnosticStreamFrame] {
        &self.frames
    }

    pub fn summary(&self) -> EmissionDiagnosticStreamSummary {
        EmissionDiagnosticStreamSummary {
            stream_id: self.stream_id,
            frame_count: self.frames.len(),
            total_frame_bytes: self.total_frame_bytes,
            first_sequence: self.frames.first().map_or(0, |frame| frame.sequence),
            last_sequence: self.frames.last().map_or(0, |frame| frame.sequence),
            stream_digest: self.stream_digest,
        }
    }

    fn finalize_canonical_bytes(&mut self) -> Result<(), EmissionDiagnosticStreamError> {
        let payload = self.wire_bytes(self.wire([0; 32]))?;
        if digest_payload(&payload) != self.stream_digest {
            return Err(EmissionDiagnosticStreamError::IntegrityMismatch);
        }
        let json = self.wire_bytes(self.wire(self.stream_digest))?;
        if payload.len() > MAX_STREAM_BYTES || json.len() > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticStreamError::StreamTooLarge {
                bytes: payload.len().max(json.len()),
                maximum: MAX_STREAM_BYTES,
            });
        }
        self.canonical_payload = Arc::from(payload.into_boxed_slice());
        self.canonical_json = Arc::from(json.into_boxed_slice());
        Ok(())
    }

    fn wire(&self, stream_digest: [u8; 32]) -> StreamEnvelope {
        StreamEnvelope {
            version: STREAM_VERSION,
            stream_id: self.stream_id,
            target: self.target.label().to_owned(),
            batch_id: self.batch_id,
            profile_key: *self.profile_key.as_bytes(),
            unit_roots: self
                .unit_roots
                .iter()
                .map(|(unit, root)| (unit.as_str().to_owned(), *root.as_bytes()))
                .collect(),
            frames: self
                .frames
                .iter()
                .map(|frame| StreamFrameEnvelope {
                    sequence: frame.sequence,
                    envelope: frame.encoded.clone(),
                })
                .collect(),
            stream_digest,
        }
    }

    fn wire_bytes(&self, wire: StreamEnvelope) -> Result<Vec<u8>, EmissionDiagnosticStreamError> {
        serde_json::to_vec(&wire)
            .map_err(|error| EmissionDiagnosticStreamError::Json(error.to_string()))
    }

    fn calculate_digest(&self) -> Result<[u8; 32], EmissionDiagnosticStreamError> {
        let payload = self.wire_bytes(self.wire([0; 32]))?;
        Ok(digest_payload(&payload))
    }
}

fn check_frame_size(sequence: u64, bytes: usize) -> Result<(), EmissionDiagnosticStreamError> {
    if bytes > MAX_SERIALIZED_DIAGNOSTIC_BYTES {
        return Err(EmissionDiagnosticStreamError::FrameTooLarge {
            sequence,
            bytes,
            maximum: MAX_SERIALIZED_DIAGNOSTIC_BYTES,
        });
    }
    Ok(())
}

fn digest_payload(payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(STREAM_DOMAIN);
    hasher.update(payload);
    hasher.finalize().into()
}
