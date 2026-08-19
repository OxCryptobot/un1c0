use ed25519_dalek::SigningKey;
use un1c0::{
    LinkFault, MultiRegionFailoverSimulator, MultiRegionSimulationConfig, ReplayFaultStep,
    ReplayManifest, ReplaySecurityError, ReplayTraceSeal, SecureReplayEngine,
};

fn signing_key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn schedule() -> Vec<ReplayFaultStep> {
    vec![ReplayFaultStep {
        sequence: 1,
        tick: 3,
        from: "node-a1".to_string(),
        to: "node-b1".to_string(),
        fault: LinkFault::Drop,
    }]
}

fn manifest(key: &SigningKey) -> ReplayManifest {
    ReplayManifest::new(
        "phase31-replay",
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
        31,
        "phase31-nonce",
        schedule(),
        key,
    )
    .unwrap()
}

fn simulator() -> MultiRegionFailoverSimulator {
    MultiRegionFailoverSimulator::new(
        MultiRegionSimulationConfig::three_region("phase31-replay", 31).unwrap(),
    )
    .unwrap()
}

#[test]
fn signed_manifest_replay_applies_faults_transactionally() {
    let key = signing_key(41);
    let manifest = manifest(&key);
    let simulator = simulator();
    let seal = SecureReplayEngine::prepare_trace_seal(&simulator, &manifest, &key).unwrap();
    let mut replay_target = simulator;
    let result = SecureReplayEngine::replay(
        &mut replay_target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap();
    assert_eq!(result.applied_steps, 1);
    assert!(result.safety_passed);
    assert_eq!(replay_target.current_tick(), 3);
    assert_eq!(replay_target.events().len(), 1);
}

#[test]
fn missing_manifest_signature_is_rejected_before_mutation() {
    let key = signing_key(42);
    let mut manifest = manifest(&key);
    manifest.signature.clear();
    let mut target = simulator();
    let before = target.trace_digest();
    let seal = ReplayTraceSeal::sign_for(&manifest, &target, &key).unwrap();
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::SignatureRejected(_)));
    assert_eq!(target.trace_digest(), before);
}

#[test]
fn tampered_schedule_is_rejected_by_hash_binding() {
    let key = signing_key(43);
    let mut manifest = manifest(&key);
    manifest.schedule[0].fault = LinkFault::Corrupt;
    let mut target = simulator();
    let before = target.trace_digest();
    let seal = ReplayTraceSeal::sign_for(&manifest, &target, &key).unwrap();
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::ScheduleHashMismatch));
    assert_eq!(target.trace_digest(), before);
}

#[test]
fn cluster_signer_and_epoch_bindings_fail_closed() {
    let key = signing_key(44);
    let manifest = manifest(&key);
    let mut target = simulator();
    let seal = SecureReplayEngine::prepare_trace_seal(&target, &manifest, &key).unwrap();
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "wrong-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::BindingRejected(_)));
}

#[test]
fn non_monotonic_schedule_is_rejected_before_signing() {
    let key = signing_key(45);
    let invalid = vec![
        ReplayFaultStep {
            sequence: 2,
            tick: 4,
            from: "node-a1".to_string(),
            to: "node-b1".to_string(),
            fault: LinkFault::Drop,
        },
        ReplayFaultStep {
            sequence: 1,
            tick: 3,
            from: "node-a1".to_string(),
            to: "node-c1".to_string(),
            fault: LinkFault::Drop,
        },
    ];
    let error = ReplayManifest::new(
        "phase31-replay",
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
        31,
        "phase31-nonce",
        invalid,
        &key,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ReplaySecurityError::SequenceTickViolation(_)
    ));
}

#[test]
fn trace_seal_tampering_is_rejected_without_partial_commit() {
    let key = signing_key(46);
    let manifest = manifest(&key);
    let simulator = simulator();
    let mut seal = SecureReplayEngine::prepare_trace_seal(&simulator, &manifest, &key).unwrap();
    seal.event_digest = "00".repeat(32);
    let mut target = simulator;
    let before = target.trace_digest();
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::TraceSealRejected(_)));
    assert_eq!(target.trace_digest(), before);
}

#[test]
fn wrong_trusted_key_rejects_signed_manifest() {
    let key = signing_key(47);
    let wrong_key = signing_key(48);
    let manifest = manifest(&key);
    let mut target = simulator();
    let seal = SecureReplayEngine::prepare_trace_seal(&target, &manifest, &key).unwrap();
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &wrong_key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        4,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::BindingRejected(_)));
}

#[test]
fn stale_minimum_epoch_and_term_reject_replay() {
    let key = signing_key(49);
    let manifest = manifest(&key);
    let simulator = simulator();
    let seal = SecureReplayEngine::prepare_trace_seal(&simulator, &manifest, &key).unwrap();
    let mut target = simulator;
    let error = SecureReplayEngine::replay(
        &mut target,
        &manifest,
        &seal,
        &key.verifying_key(),
        "un1c0-cluster",
        "replay-signer",
        5,
        2,
    )
    .unwrap_err();
    assert!(matches!(error, ReplaySecurityError::BindingRejected(_)));
}
