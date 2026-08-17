use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;
use un1c0::{
    CanaryReport, EvaluationCheck, EvolutionError, EvolutionLedger, EvolutionProposal,
    ProposalState, SignedEvolutionProposal, TrustedSignerStore,
};

fn signed_proposal() -> SignedEvolutionProposal {
    let files = BTreeMap::from([(String::from("skills/new.md"), String::from("content"))]);
    let proposal = EvolutionProposal::new("new skill", files, "cargo test --all-targets", "medium")
        .expect("valid proposal");
    let key = SigningKey::from_bytes(&[7u8; 32]);
    SignedEvolutionProposal::sign(proposal, "operator:integration", &key).expect("signed proposal")
}

fn trusted_signers(signed: &SignedEvolutionProposal) -> TrustedSignerStore {
    let mut trusted = TrustedSignerStore::default();
    trusted
        .trust_public_key(&signed.signer_id, &signed.public_key)
        .expect("valid trusted key");
    trusted
}

fn write_changed_file(root: &std::path::Path) {
    let path = root.join("skills/new.md");
    fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
    fs::write(path, "content").expect("write changed file");
}

#[test]
fn signing_status_is_valid_and_fail_closed() {
    let signed = signed_proposal();
    let trusted = trusted_signers(&signed);
    signed
        .verify_with_trust(&trusted)
        .expect("signature verifies");

    let mut forged = signed.clone();
    forged.signature[0] ^= 1;
    assert!(matches!(
        forged.verify(),
        Err(EvolutionError::InvalidSignature)
    ));

    let directory = tempdir().expect("temporary ledger directory");
    let untrusted =
        EvolutionLedger::open(directory.path().join("untrusted.json")).expect("empty ledger opens");
    assert!(matches!(
        untrusted.propose(signed.clone()),
        Err(EvolutionError::UntrustedSigner(_))
    ));

    let mut rebound = TrustedSignerStore::default();
    rebound
        .trust_public_key(&signed.signer_id, &[9u8; 32])
        .expect("different key is syntactically valid");
    let mismatched = EvolutionLedger::open_with_trusted_signers(
        directory.path().join("mismatched.json"),
        rebound,
    )
    .expect("empty ledger opens");
    assert!(matches!(
        mismatched.propose(signed),
        Err(EvolutionError::UntrustedSigner(_))
    ));
}

#[test]
fn signed_proposal_persists_through_applied_canary_lifecycle() {
    let directory = tempdir().expect("temporary workspace");
    write_changed_file(directory.path());
    let signed = signed_proposal();
    let ledger_path = directory.path().join("evolution.json");
    let ledger = EvolutionLedger::open_with_trusted_signers(&ledger_path, trusted_signers(&signed))
        .expect("open trusted ledger");
    let id = ledger.propose(signed).expect("proposal accepted");
    assert!(matches!(
        ledger.get(&id).unwrap().unwrap().state,
        ProposalState::Draft
    ));

    ledger
        .approve(&id, "reviewer:integration")
        .expect("approve proposal");
    ledger
        .start_canary(&id, "run-integration")
        .expect("start canary");
    let check = EvaluationCheck::from_output(
        "cargo test --all-targets",
        true,
        Some(0),
        "all integration gates passed",
        "",
        1,
    )
    .expect("valid evaluation check");
    let report = CanaryReport::from_workspace(
        directory.path(),
        "run-integration",
        vec![check],
        &[String::from("skills/new.md")],
    )
    .expect("workspace-bound canary report");
    ledger
        .finalize_canary_report(&id, report)
        .expect("apply passing canary");
    assert!(matches!(
        ledger.get(&id).unwrap().unwrap().state,
        ProposalState::Applied { .. }
    ));

    let reopened = EvolutionLedger::open_with_trusted_signers(
        &ledger_path,
        trusted_signers(&signed_proposal()),
    )
    .expect("reopen persisted ledger");
    assert!(matches!(
        reopened.get(&id).unwrap().unwrap().state,
        ProposalState::Applied { .. }
    ));
}

#[test]
fn failed_canary_is_persisted_as_rollback() {
    let directory = tempdir().expect("temporary workspace");
    write_changed_file(directory.path());
    let signed = signed_proposal();
    let ledger = EvolutionLedger::open_with_trusted_signers(
        directory.path().join("evolution.json"),
        trusted_signers(&signed),
    )
    .expect("open trusted ledger");
    let id = ledger.propose(signed).expect("proposal accepted");
    ledger
        .approve(&id, "reviewer:integration")
        .expect("approve proposal");
    ledger
        .start_canary(&id, "run-failed")
        .expect("start canary");
    let check = EvaluationCheck::from_output(
        "cargo test --all-targets",
        false,
        Some(1),
        "",
        "verification failed",
        1,
    )
    .expect("valid failed evaluation check");
    let report = CanaryReport::from_workspace(
        directory.path(),
        "run-failed",
        vec![check],
        &[String::from("skills/new.md")],
    )
    .expect("workspace-bound failed report");
    ledger
        .finalize_canary_report(&id, report)
        .expect("persist rollback");
    assert!(matches!(
        ledger.get(&id).unwrap().unwrap().state,
        ProposalState::RolledBack { .. }
    ));
}
