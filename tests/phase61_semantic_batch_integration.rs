use std::collections::BTreeSet;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchError, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn fixture(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn unit(value: &str) -> SemanticUnitId {
    SemanticUnitId::new(value).expect("valid unit id")
}

fn function_span(ueg: &Ueg, index: usize) -> (usize, usize) {
    let NodeKind::Lambda(lambda) = &ueg.nodes[index];
    (lambda.source_span.start_byte, lambda.source_span.end_byte)
}

#[test]
fn multi_unit_batch_refreshes_all_units_atomically() {
    let base_a = parse_ueg(&fixture("value + 1"));
    let changed_a = parse_ueg(&fixture("value + 2"));
    let base_b = parse_ueg(&fixture("value * 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit_a = unit("workspace/a.ueg");
    let unit_b = unit("workspace/b.ueg");
    let mut batch_session = SemanticBatchSession::start(
        profile.clone(),
        vec![
            SemanticUnitStart {
                unit: unit_a.clone(),
                ueg: base_a.clone(),
                capacity: 16,
            },
            SemanticUnitStart {
                unit: unit_b.clone(),
                ueg: base_b.clone(),
                capacity: 16,
            },
        ],
    )
    .expect("valid batch session");
    let (a_start, a_end) = function_span(&base_a, 0);
    let (b_start, b_end) = function_span(&base_b, 0);
    let a_manifest = batch_session
        .manifest_for(
            &unit_a,
            vec![SemanticEditRange::new(a_start, a_end).unwrap()],
        )
        .expect("a manifest");
    let b_manifest = batch_session
        .manifest_for(
            &unit_b,
            vec![SemanticEditRange::new(b_start, b_end).unwrap()],
        )
        .expect("b manifest");
    let batch = SemanticEditBatch::new(vec![
        SemanticEditUpdate {
            unit: unit_b.clone(),
            ueg: base_b,
            manifest: b_manifest,
        },
        SemanticEditUpdate {
            unit: unit_a.clone(),
            ueg: changed_a,
            manifest: a_manifest,
        },
    ])
    .expect("valid batch");

    let result = batch_session
        .refresh_batch(&batch, &profile)
        .expect("atomic batch refresh");
    assert_eq!(result.refreshed.len(), 2);
    assert_eq!(
        result
            .refreshed
            .get(&unit_a)
            .expect("a refresh")
            .validation
            .changed_functions,
        BTreeSet::from([0])
    );
    assert!(result
        .refreshed
        .get(&unit_b)
        .expect("b refresh")
        .validation
        .changed_functions
        .is_empty());
    assert!(batch_session.is_valid());
}

#[test]
fn any_unit_failure_invalidates_the_whole_batch_session() {
    let base_a = parse_ueg(&fixture("value + 1"));
    let changed_a = parse_ueg(&fixture("value + 2"));
    let base_b = parse_ueg(&fixture("value * 2"));
    let changed_b = parse_ueg(&fixture("value * 3"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    let unit_a = unit("a.ueg");
    let unit_b = unit("b.ueg");
    let mut batch_session = SemanticBatchSession::start(
        profile.clone(),
        vec![
            SemanticUnitStart {
                unit: unit_a.clone(),
                ueg: base_a.clone(),
                capacity: 16,
            },
            SemanticUnitStart {
                unit: unit_b.clone(),
                ueg: base_b.clone(),
                capacity: 16,
            },
        ],
    )
    .expect("valid batch session");
    let (a_start, a_end) = function_span(&base_a, 0);
    let valid_a = batch_session
        .manifest_for(
            &unit_a,
            vec![SemanticEditRange::new(a_start, a_end).unwrap()],
        )
        .expect("a manifest");
    let foreign = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit_b.clone(),
            ueg: changed_b.clone(),
            capacity: 16,
        }],
    )
    .expect("foreign session");
    let (b_start, b_end) = function_span(&changed_b, 0);
    let stale_b = foreign
        .manifest_for(
            &unit_b,
            vec![SemanticEditRange::new(b_start, b_end).unwrap()],
        )
        .expect("foreign manifest");
    let batch = SemanticEditBatch::new(vec![
        SemanticEditUpdate {
            unit: unit_a,
            ueg: changed_a,
            manifest: valid_a,
        },
        SemanticEditUpdate {
            unit: unit_b,
            ueg: base_b,
            manifest: stale_b,
        },
    ])
    .expect("well-formed but stale batch");

    assert!(matches!(
        batch_session.refresh_batch(&batch, &profile),
        Err(SemanticBatchError::Unit { .. })
    ));
    assert!(!batch_session.is_valid());
}

#[test]
fn batch_identity_and_membership_are_bounded_and_typed() {
    assert!(matches!(
        SemanticUnitId::new("../escape"),
        Err(SemanticBatchError::InvalidUnitId(_))
    ));
    assert!(matches!(
        SemanticUnitId::new("/absolute"),
        Err(SemanticBatchError::InvalidUnitId(_))
    ));

    let profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let base = parse_ueg(&fixture("value + 1"));
    let id = unit("single.ueg");
    let duplicate = SemanticBatchSession::start(
        profile.clone(),
        vec![
            SemanticUnitStart {
                unit: id.clone(),
                ueg: base.clone(),
                capacity: 4,
            },
            SemanticUnitStart {
                unit: id,
                ueg: base,
                capacity: 4,
            },
        ],
    );
    assert!(matches!(
        duplicate,
        Err(SemanticBatchError::DuplicateUnit(_))
    ));

    let empty = SemanticEditBatch::new(Vec::new());
    assert!(matches!(empty, Err(SemanticBatchError::EmptyBatch)));
}
