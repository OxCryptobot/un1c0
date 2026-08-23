use ed25519_dalek::SigningKey;
use un1c0::phase80_rollout::{
    Phase80RolloutError, Phase80RolloutGate, RolloutApprovalAuthority, RolloutApprovalPolicy,
    RolloutGate, RolloutManifest,
};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn manifest(all_pass: bool) -> RolloutManifest {
    RolloutManifest::new(
        "phase80-release",
        [1; 32],
        [2; 32],
        "local-commit",
        vec![
            RolloutGate {
                id: "format".into(),
                passed: all_pass,
            },
            RolloutGate {
                id: "integration-tests".into(),
                passed: all_pass,
            },
            RolloutGate {
                id: "redaction".into(),
                passed: all_pass,
            },
        ],
    )
    .unwrap()
}

fn approval_authority() -> RolloutApprovalAuthority {
    let key = signing_key(81);
    let policy =
        RolloutApprovalPolicy::new("release-approver", 1, key.verifying_key().to_bytes()).unwrap();
    RolloutApprovalAuthority::new(policy, key).unwrap()
}

#[test]
fn staging_dry_run_is_deterministic_non_mutating_and_requires_independent_approval() {
    let gate = Phase80RolloutGate;
    let manifest = manifest(true);
    let first = gate.dry_run(&manifest).unwrap();
    let second = gate.dry_run(&manifest).unwrap();
    assert_eq!(first, second);
    assert!(first.passed);
    assert_eq!(first.mutation_count, 0);
    assert!(!first.external_mutation);

    let authority = approval_authority();
    assert!(matches!(
        gate.authorize(&manifest, &first, None, authority.policy()),
        Err(Phase80RolloutError::ApprovalRequired)
    ));
    let approval = authority.issue(&manifest, &first).unwrap();
    let authorized = gate
        .authorize(&manifest, &first, Some(&approval), authority.policy())
        .unwrap();
    assert_eq!(authorized.release_id, "phase80-release");
    assert_eq!(authorized.approver_id, "release-approver");
}

#[test]
fn failed_staging_gate_cannot_issue_or_authorize_a_rollout() {
    let gate = Phase80RolloutGate;
    let manifest = manifest(false);
    let report = gate.dry_run(&manifest).unwrap();
    assert!(!report.passed);
    assert_eq!(report.mutation_count, 0);
    let authority = approval_authority();
    assert!(matches!(
        authority.issue(&manifest, &report),
        Err(Phase80RolloutError::FailedGate(_))
    ));
}

#[test]
fn mutated_or_reordered_dry_run_evidence_is_rejected_before_approval() {
    let gate = Phase80RolloutGate;
    let manifest = manifest(true);
    let report = gate.dry_run(&manifest).unwrap();
    let authority = approval_authority();

    let mut mutated = report.clone();
    mutated.mutation_count = 1;
    assert!(matches!(
        authority.issue(&manifest, &mutated),
        Err(Phase80RolloutError::MutationDetected)
    ));

    let mut reordered = report.clone();
    reordered.evaluated_gate_ids.reverse();
    assert!(matches!(
        authority.issue(&manifest, &reordered),
        Err(Phase80RolloutError::ReportMismatch("gate order"))
    ));
}

#[test]
fn approval_is_bound_to_manifest_report_signer_and_generation() {
    let gate = Phase80RolloutGate;
    let manifest = manifest(true);
    let report = gate.dry_run(&manifest).unwrap();
    let authority = approval_authority();
    let approval = authority.issue(&manifest, &report).unwrap();

    let mut changed_manifest = manifest.clone();
    changed_manifest.expected_commit = "different-commit".into();
    assert!(matches!(
        gate.authorize(
            &changed_manifest,
            &report,
            Some(&approval),
            authority.policy()
        ),
        Err(Phase80RolloutError::ManifestMismatch)
    ));

    let stale_policy =
        RolloutApprovalPolicy::new("release-approver", 2, authority.policy().public_key).unwrap();
    assert!(matches!(
        gate.authorize(&manifest, &report, Some(&approval), &stale_policy),
        Err(Phase80RolloutError::ApprovalGenerationMismatch)
    ));

    let other_key = signing_key(82);
    let other_policy =
        RolloutApprovalPolicy::new("other-approver", 1, other_key.verifying_key().to_bytes())
            .unwrap();
    assert!(matches!(
        gate.authorize(&manifest, &report, Some(&approval), &other_policy),
        Err(Phase80RolloutError::ApprovalSignerMismatch)
    ));
}

#[test]
fn rollout_manifest_rejects_duplicate_gates_unknown_fields_and_unbounded_gate_sets() {
    assert!(matches!(
        RolloutManifest::new(
            "phase80-release",
            [1; 32],
            [2; 32],
            "local-commit",
            vec![
                RolloutGate {
                    id: "format".into(),
                    passed: true,
                },
                RolloutGate {
                    id: "format".into(),
                    passed: true,
                },
            ],
        ),
        Err(Phase80RolloutError::DuplicateGate(_))
    ));

    let unknown = br#"{"schema_version":1,"release_id":"r","artifact_digest":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"configuration_digest":[2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2],"expected_commit":"c","gates":[{"id":"g","passed":true}],"unexpected":true}"#;
    assert!(serde_json::from_slice::<RolloutManifest>(unknown).is_err());

    let too_many = (0..33)
        .map(|index| RolloutGate {
            id: format!("gate-{index}"),
            passed: true,
        })
        .collect();
    assert!(matches!(
        RolloutManifest::new("r", [1; 32], [2; 32], "c", too_many),
        Err(Phase80RolloutError::TooManyGates)
    ));
}
