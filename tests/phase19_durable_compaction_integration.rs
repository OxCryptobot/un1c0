use std::collections::BTreeSet;
use std::fs;

use tempfile::tempdir;
use un1c0::{
    CompactionManifest, CompactionRecoveryOutcome, ConfigurationBoundSnapshot, ConsensusError,
    ConsensusNode, DurableCompactionStore, LogCompactionConfig, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn compacted_snapshot() -> ConfigurationBoundSnapshot {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut follower = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response = follower.handle_vote_request(request).unwrap();
    assert!(leader.receive_vote_response(response).unwrap());
    for index in 0..4 {
        leader
            .propose(StateCommand::Set {
                key: format!("key-{index}"),
                value: "value".into(),
            })
            .unwrap();
    }
    let append = leader.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(append).unwrap();
    leader.acknowledge_append(response).unwrap();
    leader
        .propose(StateCommand::Set {
            key: "uncommitted-tail".into(),
            value: "tail".into(),
        })
        .unwrap();
    leader
        .configure_log_compaction(LogCompactionConfig::new(1, 4).unwrap())
        .unwrap();
    leader.compact_committed_log(4).unwrap()
}

#[test]
fn manifest_binds_snapshot_frontiers_and_hashes() {
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    manifest.validate(&snapshot).unwrap();
    assert_eq!(manifest.last_included_index, snapshot.last_included_index);
    assert_eq!(manifest.configuration_hash, snapshot.configuration_hash);
}

#[test]
fn durable_stage_commit_load_and_recovery_is_idempotent() {
    let directory = tempdir().unwrap();
    let store = DurableCompactionStore::new(
        directory.path().join("snapshot.json"),
        directory.path().join("manifest.json"),
    );
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    store.stage(&snapshot, &manifest).unwrap();
    assert!(store.load_latest().unwrap().is_none());
    let committed = store.commit_staged().unwrap();
    assert_eq!(committed.lifecycle, un1c0::CompactionLifecycle::Committed);
    let loaded = store.load_latest().unwrap().unwrap();
    assert_eq!(loaded.0, snapshot);
    assert_eq!(loaded.1, committed);
    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::NoStaging
    );
}

#[test]
fn partial_staging_aborts_without_destroying_prior_durable_snapshot() {
    let directory = tempdir().unwrap();
    let store = DurableCompactionStore::new(
        directory.path().join("snapshot.json"),
        directory.path().join("manifest.json"),
    );
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    store.stage(&snapshot, &manifest).unwrap();
    store.commit_staged().unwrap();
    let durable_before = store.load_latest().unwrap().unwrap();

    store.stage(&snapshot, &manifest).unwrap();
    let (_, manifest_tmp) = store.staging_paths();
    fs::remove_file(manifest_tmp).unwrap();
    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::Aborted
    );
    assert_eq!(store.load_latest().unwrap().unwrap(), durable_before);
    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::NoStaging
    );
}

#[test]
fn tampered_staged_manifest_is_aborted_without_promotion() {
    let directory = tempdir().unwrap();
    let store = DurableCompactionStore::new(
        directory.path().join("snapshot.json"),
        directory.path().join("manifest.json"),
    );
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    store.stage(&snapshot, &manifest).unwrap();
    let (_, manifest_tmp) = store.staging_paths();
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_tmp).unwrap()).unwrap();
    tampered["snapshot_sha256"] = serde_json::Value::String("0".repeat(64));
    fs::write(&manifest_tmp, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::Aborted
    );
    assert!(store.load_latest().unwrap().is_none());
}

#[test]
fn recovery_finalizes_after_snapshot_rename_before_manifest_rename() {
    let directory = tempdir().unwrap();
    let snapshot_path = directory.path().join("snapshot.json");
    let manifest_path = directory.path().join("manifest.json");
    let store = DurableCompactionStore::new(&snapshot_path, &manifest_path);
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    store.stage(&snapshot, &manifest).unwrap();
    let (snapshot_tmp, _) = store.staging_paths();
    fs::rename(snapshot_tmp, &snapshot_path).unwrap();
    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::Finalized
    );
    assert_eq!(store.load_latest().unwrap().unwrap().0, snapshot);
}

#[test]
fn invalid_manifest_after_snapshot_rename_restores_previous_pair() {
    let directory = tempdir().unwrap();
    let snapshot_path = directory.path().join("snapshot.json");
    let manifest_path = directory.path().join("manifest.json");
    let store = DurableCompactionStore::new(&snapshot_path, &manifest_path);
    let snapshot = compacted_snapshot();
    let manifest = CompactionManifest::new("cluster-a", "node-a", &snapshot, 4).unwrap();
    store.stage(&snapshot, &manifest).unwrap();
    store.commit_staged().unwrap();
    let previous = store.load_latest().unwrap().unwrap();

    store.stage(&snapshot, &manifest).unwrap();
    let (snapshot_tmp, manifest_tmp) = store.staging_paths();
    let (snapshot_backup, manifest_backup, _) = store.recovery_paths();
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_tmp).unwrap()).unwrap();
    tampered["retained_suffix_end"] = serde_json::Value::from(999_u64);
    fs::write(&manifest_tmp, serde_json::to_vec(&tampered).unwrap()).unwrap();
    fs::rename(&snapshot_path, snapshot_backup).unwrap();
    fs::rename(&manifest_path, manifest_backup).unwrap();
    fs::rename(snapshot_tmp, &snapshot_path).unwrap();

    assert_eq!(
        store.recover_compaction().unwrap(),
        CompactionRecoveryOutcome::Aborted
    );
    assert_eq!(store.load_latest().unwrap().unwrap(), previous);
}

#[test]
fn invalid_retained_frontier_is_rejected_before_staging() {
    let directory = tempdir().unwrap();
    let store = DurableCompactionStore::new(
        directory.path().join("snapshot.json"),
        directory.path().join("manifest.json"),
    );
    let snapshot = compacted_snapshot();
    assert!(matches!(
        CompactionManifest::new(
            "cluster-a",
            "node-a",
            &snapshot,
            snapshot.last_included_index.saturating_sub(1),
        ),
        Err(ConsensusError::InvalidCompactionManifest(_))
    ));
    assert!(store.load_latest().unwrap().is_none());
}
