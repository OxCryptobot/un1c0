use ed25519_dalek::SigningKey;
use std::collections::BTreeMap;
use un1c0::{
    ExternalFenceAction, MultiLeaderChaosDelivery, MultiLeaderChaosSimulator, MultiLeaderConfig,
    MultiLeaderFailoverAuthority, MultiLeaderRecoveryError, RegionalLeader,
    ReplicatedRecoveryConfig, TrustedFencingAuthorityRegistry, WitnessVote,
};

const SNAPSHOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn authority() -> (
    MultiLeaderFailoverAuthority,
    BTreeMap<String, SigningKey>,
    BTreeMap<String, SigningKey>,
) {
    let authority_key = key(1);
    let mut witness_keys = BTreeMap::new();
    let mut witness_public_keys = BTreeMap::new();
    for index in 0..5u8 {
        let id = format!("witness-{}", index + 1);
        let signing_key = key(20 + index);
        witness_public_keys.insert(id.clone(), signing_key.verifying_key().to_bytes().to_vec());
        witness_keys.insert(id, signing_key);
    }
    let multi_config =
        MultiLeaderConfig::new("un1c0-cluster", "recovery-resource", 8, witness_keys.len())
            .unwrap();
    let fencing_config =
        ReplicatedRecoveryConfig::new("un1c0-cluster", "recovery-resource", 8, 128).unwrap();
    let mut authority = MultiLeaderFailoverAuthority::new(
        multi_config,
        fencing_config,
        "authority-a",
        authority_key,
        witness_public_keys,
        1,
        Some("region-a"),
    )
    .unwrap();
    let mut leader_keys = BTreeMap::new();
    for (index, (leader_id, region_id)) in [
        ("leader-a", "region-a"),
        ("leader-b", "region-b"),
        ("leader-c", "region-c"),
    ]
    .into_iter()
    .enumerate()
    {
        let signing_key = key(100 + index as u8);
        let leader = RegionalLeader::new(
            leader_id,
            region_id,
            2 + index as u64,
            2 + index as u64,
            1,
            1,
            SNAPSHOT,
            &signing_key,
        )
        .unwrap();
        authority.register_leader(leader).unwrap();
        leader_keys.insert(leader_id.to_string(), signing_key);
    }
    (authority, leader_keys, witness_keys)
}

fn proposal(
    authority: &mut MultiLeaderFailoverAuthority,
    leader_id: &str,
    round_id: u64,
    leader_keys: &BTreeMap<String, SigningKey>,
) -> un1c0::LeaderFailoverProposal {
    authority
        .begin_round(round_id, leader_id, &leader_keys[leader_id])
        .unwrap()
}

fn vote(
    proposal: &un1c0::LeaderFailoverProposal,
    witness_id: &str,
    witness_keys: &BTreeMap<String, SigningKey>,
) -> WitnessVote {
    WitnessVote::sign(
        proposal.round_id,
        witness_id,
        1,
        proposal,
        &witness_keys[witness_id],
    )
    .unwrap()
}

fn commit_with_quorum(
    authority: &mut MultiLeaderFailoverAuthority,
    proposal: &un1c0::LeaderFailoverProposal,
    witness_keys: &BTreeMap<String, SigningKey>,
) -> un1c0::MultiLeaderDecision {
    for witness_id in ["witness-1", "witness-2", "witness-3"] {
        authority
            .accept_vote(proposal, vote(proposal, witness_id, witness_keys))
            .unwrap();
    }
    authority.arbitrate(proposal).unwrap()
}

#[test]
fn witness_quorum_commits_one_multi_leader_failover_and_external_fence() {
    let (mut authority, leader_keys, witness_keys) = authority();
    let proposal = proposal(&mut authority, "leader-b", 1, &leader_keys);
    let decision = commit_with_quorum(&mut authority, &proposal, &witness_keys);
    let action = authority.admit_decision_externally(&decision).unwrap();
    assert!(matches!(action, ExternalFenceAction::Activated(_)));
    assert_eq!(decision.witness_ids.len(), 3);
    assert_eq!(decision.candidate_region_id, "region-b");
    assert_eq!(
        authority.report().active_region_id.as_deref(),
        Some("region-b")
    );
    assert_eq!(
        authority.report().active_leader_id.as_deref(),
        Some("leader-b")
    );
    assert_eq!(authority.report().accepted_fence_epoch, 1);
    assert!(authority.report().safety_passed);
}

#[test]
fn witness_cannot_vote_for_two_leaders_in_one_round_and_no_conflicting_quorum_commits() {
    let (mut authority, leader_keys, witness_keys) = authority();
    let proposal_b = proposal(&mut authority, "leader-b", 7, &leader_keys);
    let proposal_c = proposal(&mut authority, "leader-c", 7, &leader_keys);
    for witness_id in ["witness-1", "witness-2"] {
        authority
            .accept_vote(&proposal_b, vote(&proposal_b, witness_id, &witness_keys))
            .unwrap();
    }
    assert!(matches!(
        authority.accept_vote(&proposal_c, vote(&proposal_c, "witness-1", &witness_keys)),
        Err(MultiLeaderRecoveryError::SplitBrainRejected(_))
    ));
    assert!(matches!(
        authority.arbitrate(&proposal_b),
        Err(MultiLeaderRecoveryError::QuorumUnavailable(_))
    ));
    assert!(authority.report().split_brain_rejections >= 1);
}

#[test]
fn stale_leader_log_and_wrong_signer_proposals_fail_before_arbitration() {
    let (mut authority, leader_keys, _witness_keys) = authority();
    let proposal_b = proposal(&mut authority, "leader-b", 1, &leader_keys);
    assert!(proposal_b
        .verify(
            &MultiLeaderConfig::new("un1c0-cluster", "recovery-resource", 8, 5).unwrap(),
            &leader_keys["leader-b"].verifying_key()
        )
        .is_ok());
    let mut forged = proposal_b.clone();
    forged.candidate_region_id = "region-c".into();
    assert!(matches!(
        authority.accept_signed_proposal(forged),
        Err(MultiLeaderRecoveryError::ProposalRejected(_))
    ));

    let mut stale = proposal(&mut authority, "leader-c", 2, &leader_keys);
    stale.replicated_log_index = 0;
    assert!(matches!(
        authority.accept_signed_proposal(stale),
        Err(MultiLeaderRecoveryError::ProposalRejected(_))
    ));
}

#[test]
fn external_fence_audit_rejects_domain_tampering_authority_rebinding_and_generation_rollback() {
    let (mut authority, leader_keys, witness_keys) = authority();
    let proposal_b = proposal(&mut authority, "leader-b", 1, &leader_keys);
    let decision_b = commit_with_quorum(&mut authority, &proposal_b, &witness_keys);
    authority.admit_decision_externally(&decision_b).unwrap();
    let mut domain_tampered = decision_b.fencing_token.clone();
    domain_tampered.domain = "wrong-domain".into();
    let mut fence = authority.external_fence().clone();
    let registry = authority.registry().clone();
    assert!(matches!(
        fence.apply_from_registry(domain_tampered, &registry, "un1c0-cluster"),
        Err(un1c0::ReplicatedRecoveryError::FencingTokenRejected(_))
    ));

    let proposal_c = proposal(&mut authority, "leader-c", 2, &leader_keys);
    let decision_c = commit_with_quorum(&mut authority, &proposal_c, &witness_keys);
    authority.admit_decision_externally(&decision_c).unwrap();
    let mut rollback = decision_b.fencing_token.clone();
    rollback.fence_epoch = 0;
    assert!(matches!(
        fence.apply_from_registry(rollback, &registry, "un1c0-cluster"),
        Err(un1c0::ReplicatedRecoveryError::FencingTokenRejected(_))
    ));
    assert_eq!(authority.report().accepted_fence_epoch, 2);
}

#[test]
fn trusted_authority_registry_rejects_key_rebinding_and_unknown_signer() {
    let mut registry = TrustedFencingAuthorityRegistry::new();
    let first = key(200);
    let second = key(201);
    registry
        .register("authority-a", &first.verifying_key())
        .unwrap();
    assert!(registry
        .register("authority-a", &second.verifying_key())
        .is_err());
    assert!(registry.key_for("unknown").is_err());
    assert!(registry.contains("authority-a"));
}

#[test]
fn multi_leader_chaos_delays_drops_duplicates_and_then_arbitrates_one_owner() {
    let (authority, leader_keys, witness_keys) = authority();
    let mut chaos = MultiLeaderChaosSimulator::new(authority);
    let proposal_b = proposal(chaos.authority_mut(), "leader-b", 9, &leader_keys);
    let proposal_c = proposal(chaos.authority_mut(), "leader-c", 9, &leader_keys);
    chaos.partition("leader-b", "witness-5").unwrap();
    chaos.delay("leader-b", "witness-3", 4).unwrap();
    chaos.duplicate("leader-b", "witness-1").unwrap();
    assert_eq!(
        chaos
            .deliver_vote(
                "leader-b",
                "witness-5",
                &proposal_b,
                vote(&proposal_b, "witness-5", &witness_keys)
            )
            .unwrap(),
        MultiLeaderChaosDelivery::Dropped
    );
    assert_eq!(
        chaos
            .deliver_vote(
                "leader-b",
                "witness-3",
                &proposal_b,
                vote(&proposal_b, "witness-3", &witness_keys)
            )
            .unwrap(),
        MultiLeaderChaosDelivery::Delayed
    );
    assert_eq!(
        chaos
            .deliver_vote(
                "leader-b",
                "witness-1",
                &proposal_b,
                vote(&proposal_b, "witness-1", &witness_keys)
            )
            .unwrap(),
        MultiLeaderChaosDelivery::DuplicateDelivered
    );
    chaos.advance_tick(4);
    chaos
        .deliver_vote(
            "leader-b",
            "witness-3",
            &proposal_b,
            vote(&proposal_b, "witness-3", &witness_keys),
        )
        .unwrap();
    chaos
        .authority_mut()
        .accept_vote(&proposal_b, vote(&proposal_b, "witness-2", &witness_keys))
        .unwrap();
    let decision = chaos.authority_mut().arbitrate(&proposal_b).unwrap();
    chaos
        .authority_mut()
        .admit_decision_externally(&decision)
        .unwrap();
    assert!(chaos
        .authority_mut()
        .accept_signed_proposal(proposal_c)
        .is_ok());
    let report = chaos.report();
    assert_eq!(report.partition_steps, 1);
    assert_eq!(report.dropped_votes, 1);
    assert_eq!(report.delayed_votes, 1);
    assert_eq!(report.duplicate_votes, 1);
    assert!(report.safety_passed);
    assert!(!report.trace_digest.is_empty());
}

#[test]
fn external_fence_state_is_unchanged_after_rejected_token() {
    let (mut authority, leader_keys, witness_keys) = authority();
    let proposal_b = proposal(&mut authority, "leader-b", 11, &leader_keys);
    let decision = commit_with_quorum(&mut authority, &proposal_b, &witness_keys);
    authority.admit_decision_externally(&decision).unwrap();
    let before = authority.external_fence().clone();
    let mut bad = decision.fencing_token.clone();
    bad.resource_id = "other-resource".into();
    let registry = authority.registry().clone();
    let mut external = before.clone();
    assert!(external
        .apply_from_registry(bad, &registry, "un1c0-cluster")
        .is_err());
    assert_eq!(external, before);
}

#[test]
fn next_failover_round_waits_for_prior_external_fence_and_exact_decision_identity() {
    let (mut authority, leader_keys, witness_keys) = authority();
    let proposal_b = proposal(&mut authority, "leader-b", 20, &leader_keys);
    let decision_b = commit_with_quorum(&mut authority, &proposal_b, &witness_keys);
    let proposal_c = proposal(&mut authority, "leader-c", 21, &leader_keys);
    for witness_id in ["witness-1", "witness-2", "witness-3"] {
        authority
            .accept_vote(&proposal_c, vote(&proposal_c, witness_id, &witness_keys))
            .unwrap();
    }
    assert!(matches!(
        authority.arbitrate(&proposal_c),
        Err(MultiLeaderRecoveryError::SplitBrainRejected(_))
    ));
    let mut forged_decision = decision_b.clone();
    forged_decision.candidate_region_id = "region-c".into();
    assert!(matches!(
        authority.admit_decision_externally(&forged_decision),
        Err(MultiLeaderRecoveryError::SplitBrainRejected(_))
    ));
    authority.admit_decision_externally(&decision_b).unwrap();
    let decision_c = authority.arbitrate(&proposal_c).unwrap();
    authority.admit_decision_externally(&decision_c).unwrap();
    assert_eq!(
        authority.report().active_region_id.as_deref(),
        Some("region-c")
    );
    assert_eq!(authority.report().accepted_fence_epoch, 2);
}
