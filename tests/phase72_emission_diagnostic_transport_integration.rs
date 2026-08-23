use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_transport::{
    AsyncDiagnosticTransport, DistributedEmissionAggregator, EmissionDiagnosticTransportError,
    MAX_AGGREGATE_FRAMES, MAX_DISTRIBUTED_SOURCES,
};
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

struct Fixture {
    receipt: un1c0::EmissionReceipt,
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
}

struct NoopWaker;

impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn source(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn prepared() -> Fixture {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit = SemanticUnitId::new("workspace/unit.ueg").unwrap();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
    let NodeKind::Lambda(lambda) = &base.nodes[0];
    let range =
        SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap();
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit.clone(),
            ueg: base,
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session.manifest_for(&unit, vec![range]).unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: unit.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&batch_envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(unit, changed)]);
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    Fixture {
        receipt,
        snapshot,
        profile,
        candidates,
    }
}

fn stream(fixture: &Fixture) -> EmissionDiagnosticStream {
    stream_with_frames(fixture, 1)
}

fn stream_with_frames(fixture: &Fixture, frame_count: usize) -> EmissionDiagnosticStream {
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&fixture.receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    EmissionDiagnosticStream::from_repeated_report(
        72,
        &report,
        frame_count,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn poll_once<F>(mut future: Pin<&mut F>) -> Poll<F::Output>
where
    F: Future + ?Sized,
{
    let waker = Waker::from(Arc::new(NoopWaker));
    let mut context = Context::from_waker(&waker);
    future.as_mut().poll(&mut context)
}

#[test]
fn async_transport_round_trips_verified_stream_and_wakes_poll() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture);
    let transport = AsyncDiagnosticTransport::new(2).unwrap();
    let mut receive =
        Box::pin(transport.receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates));
    assert!(matches!(poll_once(receive.as_mut()), Poll::Pending));

    transport.send(11, 1, &diagnostic_stream).unwrap();
    let observation = match poll_once(receive.as_mut()) {
        Poll::Ready(Ok(Some(observation))) => observation,
        other => panic!("open transport returned unexpected state: {other:?}"),
    };
    assert_eq!(observation.source_id(), 11);
    assert_eq!(observation.sequence(), 1);
    assert_eq!(observation.stream(), &diagnostic_stream);
    assert_eq!(transport.len(), 0);

    transport.close();
    assert!(transport.is_closed());
    assert!(matches!(poll_once(receive.as_mut()), Poll::Ready(Ok(None))));
    assert!(matches!(
        transport.send(11, 2, &diagnostic_stream),
        Err(EmissionDiagnosticTransportError::Closed)
    ));
}

#[test]
fn transport_bounds_queue_and_context_fail_closed() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture);
    assert!(matches!(
        AsyncDiagnosticTransport::new(0),
        Err(EmissionDiagnosticTransportError::InvalidCapacity)
    ));
    assert!(matches!(
        AsyncDiagnosticTransport::new(MAX_AGGREGATE_FRAMES + 1),
        Err(EmissionDiagnosticTransportError::InvalidCapacity)
    ));

    let transport = AsyncDiagnosticTransport::new(1).unwrap();
    assert!(matches!(
        transport.send(0, 1, &diagnostic_stream),
        Err(EmissionDiagnosticTransportError::InvalidSourceId)
    ));
    assert!(matches!(
        transport.send(1, 0, &diagnostic_stream),
        Err(EmissionDiagnosticTransportError::InvalidSequence)
    ));
    transport.send(1, 1, &diagnostic_stream).unwrap();
    assert!(matches!(
        transport.send(2, 1, &diagnostic_stream),
        Err(EmissionDiagnosticTransportError::QueueFull)
    ));

    let stale = BTreeMap::from([(
        SemanticUnitId::new("workspace/unit.ueg").unwrap(),
        parse(&source("value + 9")),
    )]);
    assert!(matches!(
        transport.try_receive_for(&fixture.snapshot, &fixture.profile, &stale),
        Err(EmissionDiagnosticTransportError::Stream(_))
    ));
}

#[test]
fn distributed_aggregator_enforces_contiguous_source_sequences_and_bounds() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture);
    let transport = AsyncDiagnosticTransport::new(MAX_DISTRIBUTED_SOURCES + 2).unwrap();
    let mut aggregator = DistributedEmissionAggregator::new();

    for source_id in 1..=MAX_DISTRIBUTED_SOURCES as u64 {
        transport.send(source_id, 1, &diagnostic_stream).unwrap();
        let observation = transport
            .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
            .unwrap()
            .unwrap();
        aggregator
            .ingest(
                observation,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
            )
            .unwrap();
    }
    assert_eq!(aggregator.source_count(), MAX_DISTRIBUTED_SOURCES);
    assert_eq!(aggregator.total_frames(), MAX_DISTRIBUTED_SOURCES);
    let first_digest = aggregator.aggregate_digest();

    transport.send(1, 2, &diagnostic_stream).unwrap();
    let second = transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    aggregator
        .ingest(
            second.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert_eq!(aggregator.total_frames(), MAX_DISTRIBUTED_SOURCES + 1);
    assert_ne!(aggregator.aggregate_digest(), first_digest);
    assert!(matches!(
        aggregator.ingest(
            second,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticTransportError::Replay { .. })
    ));

    transport.send(1, 4, &diagnostic_stream).unwrap();
    let gap = transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    assert!(matches!(
        aggregator.ingest(
            gap,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates
        ),
        Err(EmissionDiagnosticTransportError::Gap { .. })
    ));

    transport.send(99, 1, &diagnostic_stream).unwrap();
    let extra = transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    assert!(matches!(
        aggregator.ingest(
            extra,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticTransportError::TooManySources { .. })
    ));
}

#[test]
fn aggregation_summary_is_deterministic_and_rejects_frame_overflow() {
    let fixture = prepared();
    let diagnostic_stream = stream(&fixture);
    let transport = AsyncDiagnosticTransport::new(MAX_DISTRIBUTED_SOURCES).unwrap();
    let mut first = DistributedEmissionAggregator::new();
    let mut second = DistributedEmissionAggregator::new();
    for source_id in 1..=2 {
        transport.send(source_id, 1, &diagnostic_stream).unwrap();
        let observation = transport
            .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
            .unwrap()
            .unwrap();
        let duplicate = observation.clone();
        first
            .ingest(
                observation,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
            )
            .unwrap();
        second
            .ingest(
                duplicate,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
            )
            .unwrap();
    }
    assert_eq!(first.summary(), second.summary());
    assert_eq!(first.summary().source_sequences.len(), 2);
    assert_eq!(first.total_frames(), 2);

    let wide_stream = stream_with_frames(&fixture, 32);
    let bounded_transport = AsyncDiagnosticTransport::new(1).unwrap();
    let mut bounded = DistributedEmissionAggregator::new();
    for sequence in 1..=MAX_DISTRIBUTED_SOURCES as u64 {
        bounded_transport.send(77, sequence, &wide_stream).unwrap();
        let observation = bounded_transport
            .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
            .unwrap()
            .unwrap();
        bounded
            .ingest(
                observation,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
            )
            .unwrap();
    }
    assert_eq!(bounded.total_frames(), MAX_AGGREGATE_FRAMES);
    bounded_transport
        .send(77, (MAX_DISTRIBUTED_SOURCES + 1) as u64, &wide_stream)
        .unwrap();
    let overflow = bounded_transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    assert!(matches!(
        bounded.ingest(
            overflow,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticTransportError::TooManyFrames { .. })
    ));
}
