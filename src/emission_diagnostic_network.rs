use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::emission_diagnostic_attestation::{
    DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
    EmissionDiagnosticAttestationError, VerifiedDiagnosticEvidence,
};
use crate::emission_diagnostic_cache::{DiagnosticEvidenceCache, DiagnosticEvidenceCacheMetrics};
use crate::emission_diagnostic_instrumentation::{
    DiagnosticCounter, DiagnosticInstrumentation, DiagnosticStage, DiagnosticTelemetryCollector,
    VerificationOutcome,
};
use crate::emission_diagnostic_journal::{DiagnosticJournalError, DiagnosticObservationJournal};
use crate::emission_diagnostic_stream::EmissionDiagnosticStream;
use crate::emission_diagnostic_transport::{
    DistributedDiagnosticObservation, DistributedEmissionAggregator,
    EmissionDiagnosticTransportError,
};
use crate::emission_diagnostic_workers::{DiagnosticVerifiedResult, EmissionDiagnosticWorkerError};
use crate::semantic::TargetCapabilityProfile;
use crate::semantic_batch::SemanticUnitId;
use crate::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use crate::walker::Ueg;

pub const MAX_NETWORK_FRAME_BYTES: usize = 320 * 1024;
pub const MAX_NETWORK_BUFFER_BYTES: usize = 1024 * 1024;
pub const MAX_NETWORK_NODES: usize = 8;
pub const MAX_NETWORK_FRAMES_PER_NODE: usize = 64;
pub const MAX_NETWORK_READS_PER_FRAME: usize = 4;
const NETWORK_VERSION: u8 = 1;
const NETWORK_DOMAIN: &[u8] = b"un1c0/phase74/emission-diagnostic-network/v1";
const HANDSHAKE_DOMAIN: &[u8] = b"un1c0/phase74/emission-diagnostic-handshake/v1";
const FRAME_HEADER_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionDiagnosticNetworkError {
    InvalidNodeId,
    InvalidConnectionId,
    InvalidSequence,
    UnsupportedVersion(u8),
    FrameTooLarge { bytes: usize, maximum: usize },
    BufferTooLarge { bytes: usize, maximum: usize },
    NodeLimit { count: usize, maximum: usize },
    FrameLimit { count: usize, maximum: usize },
    Replay { expected: u64, actual: u64 },
    Gap { expected: u64, actual: u64 },
    UnexpectedNode { expected: u64, actual: u64 },
    HandshakeMismatch,
    HandshakeRequired,
    InvalidHandshake,
    VerificationCancelled,
    Journal(DiagnosticJournalError),
    Closed,
    Io(String),
    Json(String),
    Attestation(EmissionDiagnosticAttestationError),
    Transport(EmissionDiagnosticTransportError),
}

impl Display for EmissionDiagnosticNetworkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNodeId => formatter.write_str("network node ID must be non-zero"),
            Self::InvalidConnectionId => {
                formatter.write_str("network connection ID must be non-zero")
            }
            Self::InvalidSequence => formatter.write_str("network sequence must be non-zero"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported network version {version}")
            }
            Self::FrameTooLarge { bytes, maximum } => write!(
                formatter,
                "network frame is {bytes} bytes; maximum is {maximum}"
            ),
            Self::BufferTooLarge { bytes, maximum } => write!(
                formatter,
                "network buffer is {bytes} bytes; maximum is {maximum}"
            ),
            Self::NodeLimit { count, maximum } => {
                write!(formatter, "network has {count} nodes; maximum is {maximum}")
            }
            Self::FrameLimit { count, maximum } => write!(
                formatter,
                "network accepted {count} frames; maximum is {maximum}"
            ),
            Self::Replay { expected, actual } => write!(
                formatter,
                "network replay: expected {expected}, received {actual}"
            ),
            Self::Gap { expected, actual } => write!(
                formatter,
                "network gap: expected {expected}, received {actual}"
            ),
            Self::UnexpectedNode { expected, actual } => write!(
                formatter,
                "network expected node {expected}, received {actual}"
            ),
            Self::HandshakeMismatch => formatter.write_str("network handshake identity mismatch"),
            Self::HandshakeRequired => formatter.write_str("network handshake is required"),
            Self::InvalidHandshake => formatter.write_str("network handshake is invalid"),
            Self::VerificationCancelled => {
                formatter.write_str("network verification result was cancelled")
            }
            Self::Journal(error) => write!(formatter, "network diagnostic journal failed: {error}"),
            Self::Closed => formatter.write_str("network connection is closed"),
            Self::Io(error) => write!(formatter, "network I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "network JSON failed: {error}"),
            Self::Attestation(error) => write!(formatter, "network attestation failed: {error}"),
            Self::Transport(error) => write!(formatter, "network transport failed: {error}"),
        }
    }
}

impl std::error::Error for EmissionDiagnosticNetworkError {}

impl From<io::Error> for EmissionDiagnosticNetworkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<EmissionDiagnosticAttestationError> for EmissionDiagnosticNetworkError {
    fn from(error: EmissionDiagnosticAttestationError) -> Self {
        Self::Attestation(error)
    }
}

impl From<DiagnosticJournalError> for EmissionDiagnosticNetworkError {
    fn from(error: DiagnosticJournalError) -> Self {
        Self::Journal(error)
    }
}

impl From<EmissionDiagnosticTransportError> for EmissionDiagnosticNetworkError {
    fn from(error: EmissionDiagnosticTransportError) -> Self {
        Self::Transport(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    version: u8,
    node_id: u64,
    connection_id: u64,
    attestation_public_key: [u8; 32],
    handshake_digest: [u8; 32],
}

impl Handshake {
    fn new(
        node_id: u64,
        connection_id: u64,
        attestation_public_key: [u8; 32],
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        if node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if connection_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidConnectionId);
        }
        let mut handshake = Self {
            version: NETWORK_VERSION,
            node_id,
            connection_id,
            attestation_public_key,
            handshake_digest: [0; 32],
        };
        handshake.handshake_digest = handshake.digest();
        Ok(handshake)
    }

    fn verify(&self) -> Result<(), EmissionDiagnosticNetworkError> {
        if self.version != NETWORK_VERSION {
            return Err(EmissionDiagnosticNetworkError::UnsupportedVersion(
                self.version,
            ));
        }
        if self.node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if self.connection_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidConnectionId);
        }
        if self.digest() != self.handshake_digest {
            return Err(EmissionDiagnosticNetworkError::InvalidHandshake);
        }
        Ok(())
    }

    fn digest(&self) -> [u8; 32] {
        let mut hasher = sha2::Sha256::new();
        hasher.update(HANDSHAKE_DOMAIN);
        hasher.update([self.version]);
        hasher.update(self.node_id.to_be_bytes());
        hasher.update(self.connection_id.to_be_bytes());
        hasher.update(self.attestation_public_key);
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkFrame {
    version: u8,
    node_id: u64,
    connection_id: u64,
    sequence: u64,
    attestation: EmissionDiagnosticAttestation,
    frame_digest: [u8; 32],
}

impl NetworkFrame {
    fn new(
        node_id: u64,
        connection_id: u64,
        sequence: u64,
        attestation: EmissionDiagnosticAttestation,
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        let mut frame = Self {
            version: NETWORK_VERSION,
            node_id,
            connection_id,
            sequence,
            attestation,
            frame_digest: [0; 32],
        };
        frame.frame_digest = frame.digest()?;
        Ok(frame)
    }

    fn verify_shape(&self) -> Result<(), EmissionDiagnosticNetworkError> {
        if self.version != NETWORK_VERSION {
            return Err(EmissionDiagnosticNetworkError::UnsupportedVersion(
                self.version,
            ));
        }
        if self.node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if self.connection_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidConnectionId);
        }
        if self.sequence == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidSequence);
        }
        Ok(())
    }

    fn verify_integrity(&self) -> Result<(), EmissionDiagnosticNetworkError> {
        if self.digest()? != self.frame_digest {
            return Err(EmissionDiagnosticNetworkError::InvalidHandshake);
        }
        Ok(())
    }

    fn digest(&self) -> Result<[u8; 32], EmissionDiagnosticNetworkError> {
        let canonical_attestation = self
            .attestation
            .to_json()
            .map_err(EmissionDiagnosticNetworkError::Attestation)?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(NETWORK_DOMAIN);
        hasher.update([self.version]);
        hasher.update(self.node_id.to_be_bytes());
        hasher.update(self.connection_id.to_be_bytes());
        hasher.update(self.sequence.to_be_bytes());
        hasher.update(canonical_attestation);
        Ok(hasher.finalize().into())
    }
}

#[derive(Debug)]
pub struct AuthenticatedDiagnosticListener {
    listener: TcpListener,
    expected_node_id: u64,
    max_frame_bytes: usize,
}

impl AuthenticatedDiagnosticListener {
    pub fn bind(
        address: impl ToSocketAddrs,
        expected_node_id: u64,
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        Self::bind_with_limit(address, expected_node_id, MAX_NETWORK_FRAME_BYTES)
    }

    pub fn bind_with_limit(
        address: impl ToSocketAddrs,
        expected_node_id: u64,
        max_frame_bytes: usize,
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        if expected_node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if max_frame_bytes == 0 || max_frame_bytes > MAX_NETWORK_BUFFER_BYTES {
            return Err(EmissionDiagnosticNetworkError::FrameTooLarge {
                bytes: max_frame_bytes,
                maximum: MAX_NETWORK_BUFFER_BYTES,
            });
        }
        Ok(Self {
            listener: TcpListener::bind(address)?,
            expected_node_id,
            max_frame_bytes,
        })
    }

    pub fn local_addr(&self) -> Result<std::net::SocketAddr, EmissionDiagnosticNetworkError> {
        Ok(self.listener.local_addr()?)
    }

    pub fn accept(
        &self,
        verifier: Arc<DiagnosticAttestationVerifier>,
    ) -> Result<AuthenticatedDiagnosticConnection, EmissionDiagnosticNetworkError> {
        let (stream, _) = self.listener.accept()?;
        let mut connection = AuthenticatedDiagnosticConnection::from_stream(
            stream,
            self.expected_node_id,
            verifier,
            self.max_frame_bytes,
        );
        connection.read_handshake()?;
        Ok(connection)
    }
}

#[derive(Debug)]
pub struct AuthenticatedDiagnosticConnection {
    stream: TcpStream,
    expected_node_id: u64,
    verifier: Arc<DiagnosticAttestationVerifier>,
    max_frame_bytes: usize,
    handshake: Option<Handshake>,
    next_sequence: u64,
    sent_frames: usize,
    accepted_frames: usize,
}

impl AuthenticatedDiagnosticConnection {
    pub fn connect(
        address: impl ToSocketAddrs,
        node_id: u64,
        connection_id: u64,
        public_key: [u8; 32],
        verifier: Arc<DiagnosticAttestationVerifier>,
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        let stream = TcpStream::connect(address)?;
        let mut connection = Self::from_stream(stream, node_id, verifier, MAX_NETWORK_FRAME_BYTES);
        let handshake = Handshake::new(node_id, connection_id, public_key)?;
        connection.write_handshake(&handshake)?;
        connection.handshake = Some(handshake);
        Ok(connection)
    }

    pub fn connect_with_timeout(
        address: impl ToSocketAddrs,
        timeout: Duration,
        node_id: u64,
        connection_id: u64,
        public_key: [u8; 32],
        verifier: Arc<DiagnosticAttestationVerifier>,
    ) -> Result<Self, EmissionDiagnosticNetworkError> {
        let address = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| EmissionDiagnosticNetworkError::Io("no socket address".into()))?;
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        let mut connection = Self::from_stream(stream, node_id, verifier, MAX_NETWORK_FRAME_BYTES);
        let handshake = Handshake::new(node_id, connection_id, public_key)?;
        connection.write_handshake(&handshake)?;
        connection.handshake = Some(handshake);
        Ok(connection)
    }

    fn from_stream(
        stream: TcpStream,
        expected_node_id: u64,
        verifier: Arc<DiagnosticAttestationVerifier>,
        max_frame_bytes: usize,
    ) -> Self {
        Self {
            stream,
            expected_node_id,
            verifier,
            max_frame_bytes,
            handshake: None,
            next_sequence: 1,
            sent_frames: 0,
            accepted_frames: 0,
        }
    }

    fn write_handshake(
        &mut self,
        handshake: &Handshake,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let bytes = serde_json::to_vec(handshake)
            .map_err(|error| EmissionDiagnosticNetworkError::Json(error.to_string()))?;
        write_frame(&mut self.stream, &bytes, self.max_frame_bytes)
    }

    fn read_handshake(&mut self) -> Result<(), EmissionDiagnosticNetworkError> {
        if self.handshake.is_some() {
            return Ok(());
        }
        let bytes = read_frame(&mut self.stream, self.max_frame_bytes)?;
        let handshake: Handshake = serde_json::from_slice(&bytes)
            .map_err(|error| EmissionDiagnosticNetworkError::Json(error.to_string()))?;
        handshake.verify()?;
        if handshake.node_id != self.expected_node_id {
            return Err(EmissionDiagnosticNetworkError::HandshakeMismatch);
        }
        if !self.verifier_contains_key(handshake.attestation_public_key) {
            return Err(EmissionDiagnosticNetworkError::HandshakeMismatch);
        }
        self.handshake = Some(handshake);
        Ok(())
    }

    pub fn send_attestation(
        &mut self,
        sequence: u64,
        attestation: &EmissionDiagnosticAttestation,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let handshake = self
            .handshake
            .as_ref()
            .ok_or(EmissionDiagnosticNetworkError::HandshakeRequired)?;
        if sequence == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidSequence);
        }
        if self.sent_frames >= MAX_NETWORK_FRAMES_PER_NODE {
            return Err(EmissionDiagnosticNetworkError::FrameLimit {
                count: self.sent_frames + 1,
                maximum: MAX_NETWORK_FRAMES_PER_NODE,
            });
        }
        if sequence != self.next_sequence {
            return if sequence < self.next_sequence {
                Err(EmissionDiagnosticNetworkError::Replay {
                    expected: self.next_sequence,
                    actual: sequence,
                })
            } else {
                Err(EmissionDiagnosticNetworkError::Gap {
                    expected: self.next_sequence,
                    actual: sequence,
                })
            };
        }
        if attestation.try_public_key()? != handshake.attestation_public_key {
            return Err(EmissionDiagnosticNetworkError::HandshakeMismatch);
        }
        let frame = NetworkFrame::new(
            handshake.node_id,
            handshake.connection_id,
            sequence,
            attestation.clone(),
        )?;
        let bytes = serde_json::to_vec(&frame)
            .map_err(|error| EmissionDiagnosticNetworkError::Json(error.to_string()))?;
        write_frame(&mut self.stream, &bytes, self.max_frame_bytes)?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.sent_frames += 1;
        Ok(())
    }

    pub fn receive_attestation(
        &mut self,
    ) -> Result<EmissionDiagnosticAttestation, EmissionDiagnosticNetworkError> {
        self.receive_attestation_instrumented(&DiagnosticInstrumentation::disabled())
    }

    pub fn receive_attestation_instrumented(
        &mut self,
        instrumentation: &DiagnosticInstrumentation,
    ) -> Result<EmissionDiagnosticAttestation, EmissionDiagnosticNetworkError> {
        if self.handshake.is_none() {
            return Err(EmissionDiagnosticNetworkError::HandshakeRequired);
        }
        if self.accepted_frames >= MAX_NETWORK_FRAMES_PER_NODE {
            return Err(EmissionDiagnosticNetworkError::FrameLimit {
                count: self.accepted_frames + 1,
                maximum: MAX_NETWORK_FRAMES_PER_NODE,
            });
        }
        let mut recorder = instrumentation.recorder(0, 0);
        let result = (|| {
            let bytes = recorder.time(DiagnosticStage::TransportReceive, || {
                read_frame(&mut self.stream, self.max_frame_bytes)
            })?;
            let frame: NetworkFrame = recorder.time(DiagnosticStage::TransportReceive, || {
                serde_json::from_slice(&bytes)
                    .map_err(|error| EmissionDiagnosticNetworkError::Json(error.to_string()))
            })?;
            frame.verify_shape()?;
            recorder.increment(DiagnosticCounter::FrameIntegrity);
            recorder.time(DiagnosticStage::TransportFrameIntegrity, || {
                frame.verify_integrity()
            })?;
            let handshake = self
                .handshake
                .as_ref()
                .ok_or(EmissionDiagnosticNetworkError::HandshakeRequired)?;
            if frame.node_id != handshake.node_id || frame.connection_id != handshake.connection_id
            {
                return Err(EmissionDiagnosticNetworkError::HandshakeMismatch);
            }
            if frame.node_id != self.expected_node_id {
                return Err(EmissionDiagnosticNetworkError::UnexpectedNode {
                    expected: self.expected_node_id,
                    actual: frame.node_id,
                });
            }
            if frame.sequence != self.next_sequence {
                let error = if frame.sequence < self.next_sequence {
                    EmissionDiagnosticNetworkError::Replay {
                        expected: self.next_sequence,
                        actual: frame.sequence,
                    }
                } else {
                    EmissionDiagnosticNetworkError::Gap {
                        expected: self.next_sequence,
                        actual: frame.sequence,
                    }
                };
                recorder.increment(DiagnosticCounter::ReplayGapRejection);
                return Err(error);
            }
            if frame.attestation.try_public_key()? != handshake.attestation_public_key {
                return Err(EmissionDiagnosticNetworkError::HandshakeMismatch);
            }
            self.next_sequence = self.next_sequence.saturating_add(1);
            self.accepted_frames += 1;
            Ok(frame.attestation)
        })();
        recorder.finish(if result.is_ok() {
            VerificationOutcome::Accepted
        } else {
            VerificationOutcome::Rejected
        });
        result
    }

    fn verifier_contains_key(&self, public_key: [u8; 32]) -> bool {
        self.verifier.contains_public_key(&public_key)
    }

    pub fn shutdown(&mut self) -> Result<(), EmissionDiagnosticNetworkError> {
        self.stream.shutdown(std::net::Shutdown::Both)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct MultiNodeDiagnosticReceiver {
    expected_node_ids: BTreeMap<u64, Arc<DiagnosticAttestationVerifier>>,
    aggregators: BTreeMap<u64, DistributedEmissionAggregator>,
    evidence_cache: DiagnosticEvidenceCache,
    journal: Option<DiagnosticObservationJournal>,
    max_nodes: usize,
}

impl MultiNodeDiagnosticReceiver {
    pub fn new() -> Self {
        Self::with_cache(
            DiagnosticEvidenceCache::with_default_budget(128)
                .expect("default diagnostic evidence cache configuration"),
        )
    }

    pub fn with_cache(evidence_cache: DiagnosticEvidenceCache) -> Self {
        Self {
            expected_node_ids: BTreeMap::new(),
            aggregators: BTreeMap::new(),
            evidence_cache,
            journal: None,
            max_nodes: MAX_NETWORK_NODES,
        }
    }

    pub fn cache_metrics(&self) -> DiagnosticEvidenceCacheMetrics {
        self.evidence_cache.metrics()
    }

    pub fn with_journal(mut self, journal: DiagnosticObservationJournal) -> Self {
        self.journal = Some(journal);
        self
    }

    pub fn journal(&self) -> Option<&DiagnosticObservationJournal> {
        self.journal.as_ref()
    }

    pub fn register_node(
        &mut self,
        node_id: u64,
        verifier: Arc<DiagnosticAttestationVerifier>,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        if node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if !self.expected_node_ids.contains_key(&node_id)
            && self.expected_node_ids.len() >= self.max_nodes
        {
            return Err(EmissionDiagnosticNetworkError::NodeLimit {
                count: self.expected_node_ids.len() + 1,
                maximum: self.max_nodes,
            });
        }
        self.expected_node_ids.insert(node_id, verifier);
        self.aggregators
            .entry(node_id)
            .or_insert_with(DistributedEmissionAggregator::new);
        Ok(())
    }

    pub fn registered_nodes(&self) -> usize {
        self.expected_node_ids.len()
    }

    pub fn aggregator(&self, node_id: u64) -> Option<&DistributedEmissionAggregator> {
        self.aggregators.get(&node_id)
    }

    pub fn ingest_attestation(
        &mut self,
        node_id: u64,
        sequence: u64,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let verifier = self
            .expected_node_ids
            .get(&node_id)
            .ok_or(EmissionDiagnosticNetworkError::UnexpectedNode {
                expected: 0,
                actual: node_id,
            })?
            .clone();
        let evidence = verifier.verify_stream_evidence_with_cache(
            attestation,
            stream,
            envelope,
            profile,
            units,
            &self.evidence_cache,
            &DiagnosticInstrumentation::disabled(),
        )?;
        self.ingest_verified(node_id, 1, sequence, evidence, envelope, profile, units)
    }

    pub fn ingest_attestation_instrumented(
        &mut self,
        node_id: u64,
        sequence: u64,
        attestation: &EmissionDiagnosticAttestation,
        stream: &EmissionDiagnosticStream,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        instrumentation: &DiagnosticInstrumentation,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let verifier = self
            .expected_node_ids
            .get(&node_id)
            .ok_or(EmissionDiagnosticNetworkError::UnexpectedNode {
                expected: 0,
                actual: node_id,
            })?
            .clone();
        let evidence = verifier.verify_stream_evidence_with_cache(
            attestation,
            stream,
            envelope,
            profile,
            units,
            &self.evidence_cache,
            instrumentation,
        )?;
        self.ingest_verified(node_id, 1, sequence, evidence, envelope, profile, units)
    }

    pub fn ingest_worker_result(
        &mut self,
        result: DiagnosticVerifiedResult,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        if result.is_cancelled() {
            return Err(EmissionDiagnosticNetworkError::VerificationCancelled);
        }
        let evidence = result.evidence.map_err(|error| match error {
            EmissionDiagnosticWorkerError::Verification(error) => {
                EmissionDiagnosticNetworkError::Attestation(error)
            }
            EmissionDiagnosticWorkerError::Cancelled => {
                EmissionDiagnosticNetworkError::VerificationCancelled
            }
        })?;
        self.ingest_verified(
            result.node_id,
            result.connection_id,
            result.sequence,
            evidence,
            envelope,
            profile,
            units,
        )
    }

    pub fn ingest_verified(
        &mut self,
        node_id: u64,
        connection_id: u64,
        sequence: u64,
        evidence: VerifiedDiagnosticEvidence,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let observation = self.prepare_observation(
            node_id,
            connection_id,
            sequence,
            &evidence,
            envelope,
            profile,
            units,
        )?;
        let aggregator = self
            .aggregators
            .get(&node_id)
            .expect("registered node has aggregator");
        aggregator.validate_verified(&observation)?;
        let journal_checkpoint = self
            .journal
            .as_ref()
            .map(DiagnosticObservationJournal::checkpoint);
        if let Some(journal) = self.journal.as_mut() {
            journal.append(
                node_id,
                connection_id,
                sequence,
                observation.stream().stream_digest(),
            )?;
        }
        let mutation = self
            .aggregators
            .get_mut(&node_id)
            .expect("registered node has aggregator")
            .ingest_verified(observation);
        if let Err(error) = mutation {
            if let (Some(journal), Some(checkpoint)) = (self.journal.as_mut(), journal_checkpoint) {
                journal.rollback_to(checkpoint)?;
            }
            return Err(error.into());
        }
        Ok(())
    }

    pub fn ingest_verified_with_telemetry(
        &mut self,
        node_id: u64,
        connection_id: u64,
        sequence: u64,
        evidence: VerifiedDiagnosticEvidence,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
        instrumentation: &DiagnosticInstrumentation,
        collector: &DiagnosticTelemetryCollector,
    ) -> Result<(), EmissionDiagnosticNetworkError> {
        let result = self.ingest_verified(
            node_id,
            connection_id,
            sequence,
            evidence,
            envelope,
            profile,
            units,
        );
        let _ = collector.collect(&instrumentation.snapshot());
        result
    }

    fn prepare_observation(
        &self,
        node_id: u64,
        connection_id: u64,
        sequence: u64,
        evidence: &VerifiedDiagnosticEvidence,
        envelope: &SemanticSnapshotEnvelope,
        profile: &TargetCapabilityProfile,
        units: &BTreeMap<SemanticUnitId, Ueg>,
    ) -> Result<DistributedDiagnosticObservation, EmissionDiagnosticNetworkError> {
        if node_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidNodeId);
        }
        if connection_id == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidConnectionId);
        }
        if sequence == 0 {
            return Err(EmissionDiagnosticNetworkError::InvalidSequence);
        }
        let verifier = self.expected_node_ids.get(&node_id).ok_or(
            EmissionDiagnosticNetworkError::UnexpectedNode {
                expected: 0,
                actual: node_id,
            },
        )?;
        verifier.verify_evidence_current(evidence)?;
        if !evidence
            .canonical()
            .matches_current_candidates(envelope, profile, units)
        {
            return Err(EmissionDiagnosticNetworkError::Attestation(
                EmissionDiagnosticAttestationError::ContentMismatch,
            ));
        }
        Ok(DistributedDiagnosticObservation::from_verified_parts(
            node_id,
            sequence,
            evidence.canonical().stream().clone(),
        ))
    }
}

impl Default for MultiNodeDiagnosticReceiver {
    fn default() -> Self {
        Self::new()
    }
}

fn write_frame(
    stream: &mut TcpStream,
    bytes: &[u8],
    maximum: usize,
) -> Result<(), EmissionDiagnosticNetworkError> {
    if bytes.is_empty() || bytes.len() > maximum || bytes.len() > u32::MAX as usize {
        return Err(EmissionDiagnosticNetworkError::FrameTooLarge {
            bytes: bytes.len(),
            maximum,
        });
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

fn read_frame(
    stream: &mut TcpStream,
    maximum: usize,
) -> Result<Vec<u8>, EmissionDiagnosticNetworkError> {
    let mut header = [0u8; FRAME_HEADER_BYTES];
    stream.read_exact(&mut header)?;
    let bytes = u32::from_be_bytes(header) as usize;
    if bytes == 0 || bytes > maximum || bytes > MAX_NETWORK_BUFFER_BYTES {
        return Err(EmissionDiagnosticNetworkError::FrameTooLarge {
            bytes,
            maximum: maximum.min(MAX_NETWORK_BUFFER_BYTES),
        });
    }
    let mut payload = vec![0u8; bytes];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}
