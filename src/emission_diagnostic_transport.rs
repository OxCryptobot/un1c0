use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crossbeam_queue::ArrayQueue;
use sha2::{Digest, Sha256};

use crate::emission_diagnostic_stream::{
    EmissionDiagnosticStream, EmissionDiagnosticStreamError, MAX_STREAM_BYTES,
};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_DISTRIBUTED_SOURCES: usize = 8;
pub const MAX_AGGREGATE_FRAMES: usize = MAX_DISTRIBUTED_SOURCES * 32;
pub const MAX_AGGREGATE_BYTES: usize = MAX_AGGREGATE_FRAMES * MAX_STREAM_BYTES;
pub const MAX_TRANSPORT_WAITERS: usize = 64;
const TRANSPORT_DOMAIN: &[u8] = b"un1c0/phase72/emission-diagnostic-transport/v1";
const AGGREGATE_DOMAIN: &[u8] = b"un1c0/phase72/emission-diagnostic-aggregate/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticTransportError {
    InvalidCapacity,
    InvalidSourceId,
    InvalidSequence,
    QueueFull,
    Closed,
    FrameTooLarge {
        bytes: usize,
        maximum: usize,
    },
    TransportIntegrityMismatch,
    Stream(EmissionDiagnosticStreamError),
    Replay {
        source_id: u64,
        expected: u64,
        actual: u64,
    },
    Gap {
        source_id: u64,
        expected: u64,
        actual: u64,
    },
    TooManySources {
        count: usize,
        maximum: usize,
    },
    TooManyFrames {
        count: usize,
        maximum: usize,
    },
    TooManyBytes {
        bytes: usize,
        maximum: usize,
    },
    AggregateInvariant {
        field: &'static str,
    },
}

impl Display for EmissionDiagnosticTransportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCapacity => {
                formatter.write_str("transport capacity must be non-zero and bounded")
            }
            Self::InvalidSourceId => formatter.write_str("transport source ID must be non-zero"),
            Self::InvalidSequence => formatter.write_str("transport sequence must be non-zero"),
            Self::QueueFull => formatter.write_str("transport queue is full"),
            Self::Closed => formatter.write_str("transport is closed"),
            Self::FrameTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "transport frame is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::TransportIntegrityMismatch => {
                formatter.write_str("transport frame integrity digest mismatch")
            }
            Self::Stream(error) => write!(formatter, "transport stream failed: {error}"),
            Self::Replay {
                source_id,
                expected,
                actual,
            } => write!(
                formatter,
                "source {source_id} replayed sequence {actual}; expected {expected}"
            ),
            Self::Gap {
                source_id,
                expected,
                actual,
            } => write!(
                formatter,
                "source {source_id} sent sequence {actual}; expected contiguous {expected}"
            ),
            Self::TooManySources { count, maximum } => {
                write!(
                    formatter,
                    "aggregate has {count} sources; maximum is {maximum}"
                )
            }
            Self::TooManyFrames { count, maximum } => {
                write!(
                    formatter,
                    "aggregate has {count} frames; maximum is {maximum}"
                )
            }
            Self::TooManyBytes { bytes, maximum } => {
                write!(
                    formatter,
                    "aggregate has {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::AggregateInvariant { field } => {
                write!(formatter, "aggregate invariant failed for {field}")
            }
        }
    }
}

impl std::error::Error for EmissionDiagnosticTransportError {}

impl From<EmissionDiagnosticStreamError> for EmissionDiagnosticTransportError {
    fn from(error: EmissionDiagnosticStreamError) -> Self {
        Self::Stream(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedDiagnosticObservation {
    source_id: u64,
    sequence: u64,
    stream: EmissionDiagnosticStream,
}

impl DistributedDiagnosticObservation {
    pub fn source_id(&self) -> u64 {
        self.source_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn stream(&self) -> &EmissionDiagnosticStream {
        &self.stream
    }

    pub(crate) fn from_verified_parts(
        source_id: u64,
        sequence: u64,
        stream: EmissionDiagnosticStream,
    ) -> Self {
        Self {
            source_id,
            sequence,
            stream,
        }
    }
}

#[derive(Debug)]
struct TransportFrame {
    source_id: u64,
    sequence: u64,
    bytes: Vec<u8>,
    digest: [u8; 32],
}

#[derive(Debug)]
pub struct AsyncDiagnosticTransport {
    queue: Arc<ArrayQueue<TransportFrame>>,
    closed: AtomicBool,
    lifecycle_gate: Mutex<()>,
    waiters: Mutex<Vec<Waker>>,
}

impl AsyncDiagnosticTransport {
    pub fn new(capacity: usize) -> Result<Self, EmissionDiagnosticTransportError> {
        if capacity == 0 || capacity > MAX_AGGREGATE_FRAMES {
            return Err(EmissionDiagnosticTransportError::InvalidCapacity);
        }
        Ok(Self {
            queue: Arc::new(ArrayQueue::new(capacity)),
            closed: AtomicBool::new(false),
            lifecycle_gate: Mutex::new(()),
            waiters: Mutex::new(Vec::new()),
        })
    }

    pub fn send(
        &self,
        source_id: u64,
        sequence: u64,
        stream: &EmissionDiagnosticStream,
    ) -> Result<(), EmissionDiagnosticTransportError> {
        if source_id == 0 {
            return Err(EmissionDiagnosticTransportError::InvalidSourceId);
        }
        if sequence == 0 {
            return Err(EmissionDiagnosticTransportError::InvalidSequence);
        }
        let bytes = stream.to_json()?;
        if bytes.len() > MAX_STREAM_BYTES {
            return Err(EmissionDiagnosticTransportError::FrameTooLarge {
                bytes: bytes.len(),
                maximum: MAX_STREAM_BYTES,
            });
        }
        let _gate = self
            .lifecycle_gate
            .lock()
            .expect("transport lifecycle mutex poisoned");
        if self.closed.load(Ordering::Acquire) {
            return Err(EmissionDiagnosticTransportError::Closed);
        }
        let digest = transport_digest(source_id, sequence, &bytes);
        self.queue
            .push(TransportFrame {
                source_id,
                sequence,
                bytes,
                digest,
            })
            .map_err(|_| EmissionDiagnosticTransportError::QueueFull)?;
        self.wake_waiters();
        Ok(())
    }

    pub fn try_receive_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<Option<DistributedDiagnosticObservation>, EmissionDiagnosticTransportError> {
        self.queue
            .pop()
            .map(|frame| self.decode_frame(frame, envelope, profile, units))
            .transpose()
    }

    pub fn poll_receive_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<DistributedDiagnosticObservation>, EmissionDiagnosticTransportError>>
    {
        if let Some(frame) = self.queue.pop() {
            return Poll::Ready(self.decode_frame(frame, envelope, profile, units).map(Some));
        }
        if self.closed.load(Ordering::Acquire) {
            return Poll::Ready(Ok(None));
        }
        self.register_waiter(context.waker());
        if let Some(frame) = self.queue.pop() {
            return Poll::Ready(self.decode_frame(frame, envelope, profile, units).map(Some));
        }
        if self.closed.load(Ordering::Acquire) {
            Poll::Ready(Ok(None))
        } else {
            Poll::Pending
        }
    }

    pub fn receive_for<'a>(
        &'a self,
        envelope: &'a SemanticSnapshotEnvelope,
        profile: &'a TargetCapabilityProfile,
        units: &'a BTreeMap<SemanticUnitId, Ueg>,
    ) -> impl Future<
        Output = Result<Option<DistributedDiagnosticObservation>, EmissionDiagnosticTransportError>,
    > + 'a {
        std::future::poll_fn(move |context| {
            self.poll_receive_for(envelope, profile, units, context)
        })
    }

    pub fn close(&self) {
        let _gate = self
            .lifecycle_gate
            .lock()
            .expect("transport lifecycle mutex poisoned");
        self.closed.store(true, Ordering::Release);
        self.wake_waiters();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    fn decode_frame(
        &self,
        frame: TransportFrame,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<DistributedDiagnosticObservation, EmissionDiagnosticTransportError> {
        if transport_digest(frame.source_id, frame.sequence, &frame.bytes) != frame.digest {
            return Err(EmissionDiagnosticTransportError::TransportIntegrityMismatch);
        }
        let stream =
            EmissionDiagnosticStream::from_json_for(&frame.bytes, envelope, profile, units)?;

        Ok(DistributedDiagnosticObservation {
            source_id: frame.source_id,
            sequence: frame.sequence,
            stream,
        })
    }

    fn register_waiter(&self, waker: &Waker) {
        let mut waiters = self
            .waiters
            .lock()
            .expect("transport waiter mutex poisoned");
        if waiters.iter().any(|existing| existing.will_wake(waker)) {
            return;
        }
        if waiters.len() >= MAX_TRANSPORT_WAITERS {
            waiters.remove(0);
        }
        waiters.push(waker.clone());
    }

    fn wake_waiters(&self) {
        let waiters = {
            let mut guard = self
                .waiters
                .lock()
                .expect("transport waiter mutex poisoned");
            std::mem::take(&mut *guard)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedEmissionAggregateSummary {
    pub source_count: usize,
    pub total_frames: usize,
    pub total_frame_bytes: usize,
    pub source_sequences: BTreeMap<u64, u64>,
    pub aggregate_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedObservation {
    sequence: u64,
    stream: EmissionDiagnosticStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributedEmissionAggregator {
    observations: BTreeMap<u64, AcceptedObservation>,
    total_frames: usize,
    total_frame_bytes: usize,
    aggregate_digest: [u8; 32],
}

impl Default for DistributedEmissionAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl DistributedEmissionAggregator {
    pub fn new() -> Self {
        let mut aggregate = Self {
            observations: BTreeMap::new(),
            total_frames: 0,
            total_frame_bytes: 0,
            aggregate_digest: [0; 32],
        };
        aggregate.aggregate_digest = aggregate.calculate_digest();
        aggregate
    }

    pub fn ingest(
        &mut self,
        observation: DistributedDiagnosticObservation,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticTransportError> {
        if observation.source_id == 0 {
            return Err(EmissionDiagnosticTransportError::InvalidSourceId);
        }
        if observation.sequence == 0 {
            return Err(EmissionDiagnosticTransportError::InvalidSequence);
        }
        observation.stream.verify_for(envelope, profile, units)?;
        self.ingest_verified(observation)
    }

    pub(crate) fn validate_verified(
        &self,
        observation: &DistributedDiagnosticObservation,
    ) -> Result<(usize, usize), EmissionDiagnosticTransportError> {
        let expected = self
            .observations
            .get(&observation.source_id)
            .map_or(1, |accepted| {
                accepted.sequence.checked_add(1).unwrap_or(u64::MAX)
            });
        if observation.sequence < expected {
            return Err(EmissionDiagnosticTransportError::Replay {
                source_id: observation.source_id,
                expected,
                actual: observation.sequence,
            });
        }
        if observation.sequence > expected {
            return Err(EmissionDiagnosticTransportError::Gap {
                source_id: observation.source_id,
                expected,
                actual: observation.sequence,
            });
        }
        if !self.observations.contains_key(&observation.source_id)
            && self.observations.len() >= MAX_DISTRIBUTED_SOURCES
        {
            return Err(EmissionDiagnosticTransportError::TooManySources {
                count: self.observations.len() + 1,
                maximum: MAX_DISTRIBUTED_SOURCES,
            });
        }
        let next_total_frames = self
            .total_frames
            .checked_add(observation.stream.frame_count())
            .ok_or(EmissionDiagnosticTransportError::TooManyFrames {
                count: usize::MAX,
                maximum: MAX_AGGREGATE_FRAMES,
            })?;
        if next_total_frames > MAX_AGGREGATE_FRAMES {
            return Err(EmissionDiagnosticTransportError::TooManyFrames {
                count: next_total_frames,
                maximum: MAX_AGGREGATE_FRAMES,
            });
        }
        let next_total_bytes = self
            .total_frame_bytes
            .checked_add(observation.stream.total_frame_bytes())
            .ok_or(EmissionDiagnosticTransportError::TooManyBytes {
                bytes: usize::MAX,
                maximum: MAX_AGGREGATE_BYTES,
            })?;
        if next_total_bytes > MAX_AGGREGATE_BYTES {
            return Err(EmissionDiagnosticTransportError::TooManyBytes {
                bytes: next_total_bytes,
                maximum: MAX_AGGREGATE_BYTES,
            });
        }
        Ok((next_total_frames, next_total_bytes))
    }

    pub(crate) fn ingest_verified(
        &mut self,
        observation: DistributedDiagnosticObservation,
    ) -> Result<(), EmissionDiagnosticTransportError> {
        let (next_total_frames, next_total_bytes) = self.validate_verified(&observation)?;
        let source_id = observation.source_id;
        self.observations.insert(
            source_id,
            AcceptedObservation {
                sequence: observation.sequence,
                stream: observation.stream,
            },
        );
        self.total_frames = next_total_frames;
        self.total_frame_bytes = next_total_bytes;
        self.aggregate_digest = self.calculate_digest();
        Ok(())
    }

    pub fn summary(&self) -> DistributedEmissionAggregateSummary {
        DistributedEmissionAggregateSummary {
            source_count: self.observations.len(),
            total_frames: self.total_frames,
            total_frame_bytes: self.total_frame_bytes,
            source_sequences: self
                .observations
                .iter()
                .map(|(source, accepted)| (*source, accepted.sequence))
                .collect(),
            aggregate_digest: self.aggregate_digest,
        }
    }

    pub fn source_count(&self) -> usize {
        self.observations.len()
    }

    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    pub fn total_frame_bytes(&self) -> usize {
        self.total_frame_bytes
    }

    pub fn aggregate_digest(&self) -> [u8; 32] {
        self.aggregate_digest
    }

    pub fn verify_for(
        &self,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticTransportError> {
        let mut total_frames = 0usize;
        let mut total_frame_bytes = 0usize;
        for (source_id, observation) in &self.observations {
            if *source_id == 0 || observation.sequence == 0 {
                return Err(EmissionDiagnosticTransportError::AggregateInvariant {
                    field: "source_sequence",
                });
            }
            observation.stream.verify_for(envelope, profile, units)?;
            total_frames = total_frames
                .checked_add(observation.stream.frame_count())
                .ok_or(EmissionDiagnosticTransportError::AggregateInvariant {
                    field: "total_frames_overflow",
                })?;
            total_frame_bytes = total_frame_bytes
                .checked_add(observation.stream.total_frame_bytes())
                .ok_or(EmissionDiagnosticTransportError::AggregateInvariant {
                    field: "total_frame_bytes_overflow",
                })?;
        }
        if total_frames != self.total_frames {
            return Err(EmissionDiagnosticTransportError::AggregateInvariant {
                field: "total_frames",
            });
        }
        if total_frame_bytes != self.total_frame_bytes {
            return Err(EmissionDiagnosticTransportError::AggregateInvariant {
                field: "total_frame_bytes",
            });
        }
        if self.aggregate_digest != self.calculate_digest() {
            return Err(EmissionDiagnosticTransportError::AggregateInvariant {
                field: "aggregate_digest",
            });
        }
        Ok(())
    }

    fn calculate_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(AGGREGATE_DOMAIN);
        for (source_id, observation) in &self.observations {
            hasher.update(source_id.to_be_bytes());
            hasher.update(observation.sequence.to_be_bytes());
            hasher.update(observation.stream.stream_digest());
        }
        hasher.update((self.total_frames as u64).to_be_bytes());
        hasher.update((self.total_frame_bytes as u64).to_be_bytes());
        hasher.finalize().into()
    }
}

fn transport_digest(source_id: u64, sequence: u64, bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(TRANSPORT_DOMAIN);
    hasher.update(source_id.to_be_bytes());
    hasher.update(sequence.to_be_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}
