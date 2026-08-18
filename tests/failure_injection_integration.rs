use std::collections::BTreeMap;
use std::process::Command;

use sha2::Digest;
use tempfile::tempdir;
use un1c0::{ConsensusNode, DurableSnapshotStore, ReplicatedSnapshot};

#[test]
fn sudden_process_crash_leaves_recoverable_snapshot_stage() {
    let directory = tempdir().unwrap();
    let target = directory.path().join("agent.snapshot.json");
    let helper = std::env::var("CARGO_BIN_EXE_un1c0-failure-injector")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_un1c0_failure_injector"))
        .unwrap();
    let status = Command::new(helper)
        .args(["snapshot-power-loss", target.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(!status.success());
    let store = DurableSnapshotStore::new(&target);
    assert!(store.recover_staging().unwrap());
    assert!(!directory.path().join(".agent.snapshot.json.tmp").exists());
    assert!(!target.exists());

    let snapshot = ReplicatedSnapshot {
        term: 3,
        commit_index: 4,
        last_applied: 4,
        state: BTreeMap::from([(String::from("recovered"), String::from("yes"))]),
        state_hash: {
            let bytes = serde_json::to_vec(&BTreeMap::from([
                (String::from("recovered"), String::from("yes")),
            ]))
            .unwrap();
            sha2::Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{:02x}", byte))
                .collect()
        },
    };
    store.save(&snapshot).unwrap();
    assert_eq!(store.load().unwrap(), snapshot);
}

#[test]
fn invalid_snapshot_installation_rolls_back_without_mutating_state() {
    let members = ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect();
    let mut node = ConsensusNode::new("node-a", members, 16).unwrap();
    let before_term = node.current_term();
    let before_commit = node.commit_index();
    let invalid = ReplicatedSnapshot {
        term: 1,
        commit_index: 1,
        last_applied: 1,
        state: BTreeMap::from([(String::from("unsafe"), String::from("mutation"))]),
        state_hash: "0".repeat(64),
    };
    assert!(node.install_snapshot(invalid).is_err());
    assert_eq!(node.current_term(), before_term);
    assert_eq!(node.commit_index(), before_commit);
    assert!(node.state_value("unsafe").is_none());
}
