use std::collections::BTreeSet;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

use serde::Serialize;
use un1c0::{
    ConsensusError, ConsensusNode, ConsensusRole, LeaderLeaseConfig, LinearizableReadPlan,
    LinearizableReadRequest, ReadIndexAction, ReadIndexResponse, StateCommand,
};

fn members() -> BTreeSet<String> {
    ["node-a", "node-b", "node-c"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn elected_leader() -> (ConsensusNode, ConsensusNode) {
    let cluster = members();
    let mut leader = ConsensusNode::new("node-a", cluster.clone(), 128).unwrap();
    let mut follower = ConsensusNode::new("node-b", cluster, 128).unwrap();
    let request = leader.start_election().unwrap();
    let response = follower.handle_vote_request(request).unwrap();
    assert!(response.granted);
    assert!(leader.receive_vote_response(response).unwrap());
    assert_eq!(leader.role(), ConsensusRole::Leader);
    (leader, follower)
}

fn committed_leader() -> (ConsensusNode, ConsensusNode) {
    let (mut leader, mut follower) = elected_leader();
    leader
        .propose(StateCommand::Set {
            key: "agent/read".into(),
            value: "linearizable".into(),
        })
        .unwrap();
    assert_eq!(leader.commit_index(), 0);
    let append = leader.append_entries_for("node-b").unwrap();
    let response = follower.handle_append_entries(append).unwrap();
    assert!(leader.acknowledge_append(response).unwrap());
    let commit_notice = leader.append_entries_for("node-b").unwrap();
    follower.handle_append_entries(commit_notice).unwrap();
    assert_eq!(leader.commit_index(), 1);
    assert_eq!(follower.commit_index(), 1);
    (leader, follower)
}

#[test]
fn leader_lease_fast_path_requires_quorum_observation_and_is_clock_safe() {
    let (mut leader, _) = committed_leader();
    leader
        .configure_leader_lease(LeaderLeaseConfig::new(100, 10).unwrap())
        .unwrap();

    let first = LinearizableReadRequest::new("lease-1", "agent/read", 1).unwrap();
    let action = leader.prepare_linearizable_read(first).unwrap();
    assert!(matches!(action, ReadIndexAction::Quorum(_)));
    let request = match action {
        ReadIndexAction::Quorum(request) => request,
        ReadIndexAction::Lease(_) => unreachable!(),
    };
    let response = ReadIndexResponse {
        request_id: request.request_id.clone(),
        term: request.term,
        follower_id: "node-b".into(),
        read_index: request.read_index,
        accepted: true,
    };
    let plan = leader.acknowledge_read_index(response, 1).unwrap().unwrap();
    assert!(!plan.lease_fast_path);
    assert!(leader.lease_is_valid(1));

    let fast = leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("lease-2", "agent/read", 80).unwrap(),
        )
        .unwrap();
    let fast_plan = match fast {
        ReadIndexAction::Lease(plan) => plan,
        ReadIndexAction::Quorum(_) => panic!("valid lease should use the fast path"),
    };
    assert!(fast_plan.lease_fast_path);
    assert_eq!(
        leader.execute_linearizable_read(fast_plan, 80).unwrap(),
        Some("linearizable".into())
    );
    assert!(!leader.lease_is_valid(91));
    assert!(matches!(
        leader.execute_linearizable_read(
            LinearizableReadPlan {
                request_id: "expired".into(),
                key: "agent/read".into(),
                term: leader.current_term(),
                read_index: leader.commit_index(),
                lease_fast_path: true,
            },
            91,
        ),
        Err(ConsensusError::LeaseExpired)
    ));
}

#[test]
fn lease_configuration_rejects_unsafe_drift_and_read_boundaries_are_strict() {
    assert!(matches!(
        LeaderLeaseConfig::new(10, 10),
        Err(ConsensusError::InvalidLeaderLease(_))
    ));
    assert!(matches!(
        LeaderLeaseConfig::new(0, 0),
        Err(ConsensusError::InvalidLeaderLease(_))
    ));

    let (mut leader, _) = committed_leader();
    leader
        .configure_leader_lease(LeaderLeaseConfig::new(20, 5).unwrap())
        .unwrap();
    let action = leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("boundary", "agent/read", 1).unwrap(),
        )
        .unwrap();
    let request = match action {
        ReadIndexAction::Quorum(request) => request,
        ReadIndexAction::Lease(_) => unreachable!(),
    };
    let plan = leader
        .acknowledge_read_index(
            ReadIndexResponse {
                request_id: request.request_id,
                term: request.term,
                follower_id: "node-b".into(),
                read_index: request.read_index,
                accepted: true,
            },
            1,
        )
        .unwrap()
        .unwrap();
    assert!(!plan.lease_fast_path);
    assert!(leader.lease_is_valid(1));
    let fast_plan = match leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("boundary-fast", "agent/read", 2).unwrap(),
        )
        .unwrap()
    {
        ReadIndexAction::Lease(plan) => plan,
        ReadIndexAction::Quorum(_) => panic!("quorum acknowledgement should install a lease"),
    };
    assert!(!leader.lease_is_valid(16));
    assert!(matches!(
        leader.execute_linearizable_read(fast_plan, 16),
        Err(ConsensusError::LeaseExpired)
    ));
}

#[test]
fn clock_regression_requires_explicit_reanchor_before_lease_reuse() {
    let (mut leader, _) = committed_leader();
    leader
        .configure_leader_lease(LeaderLeaseConfig::new(100, 5).unwrap())
        .unwrap();
    let action = leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("clock-1", "agent/read", 10).unwrap(),
        )
        .unwrap();
    let request = match action {
        ReadIndexAction::Quorum(request) => request,
        ReadIndexAction::Lease(_) => unreachable!(),
    };
    leader
        .acknowledge_read_index(
            ReadIndexResponse {
                request_id: request.request_id,
                term: request.term,
                follower_id: "node-b".into(),
                read_index: request.read_index,
                accepted: true,
            },
            10,
        )
        .unwrap();
    assert!(leader.clock_is_trusted());
    let _ = leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("clock-regressed", "agent/read", 5).unwrap(),
        )
        .unwrap();
    assert!(!leader.clock_is_trusted());
    assert!(!leader.lease_is_valid(6));
    leader.reanchor_monotonic_clock(6).unwrap();
    assert!(leader.clock_is_trusted());
    assert!(!leader.lease_is_valid(6));
}

#[test]
fn followers_only_acknowledge_read_index_after_they_have_committed_it() {
    let (leader, mut follower) = elected_leader();
    let request =
        un1c0::ReadIndexRequest::new("follower-read", leader.current_term(), "node-a", 1).unwrap();
    let response = follower.handle_read_index_request(request).unwrap();
    assert!(!response.accepted);

    let mut leader = leader;
    leader
        .propose(StateCommand::Set {
            key: "agent/read".into(),
            value: "ready".into(),
        })
        .unwrap();
    let append = leader.append_entries_for("node-b").unwrap();
    let append_response = follower.handle_append_entries(append).unwrap();
    assert!(append_response.success);
    assert_eq!(follower.commit_index(), 0);
    let commit_notice = un1c0::AppendEntries {
        term: leader.current_term(),
        leader_id: leader.id().into(),
        prev_log_index: 1,
        prev_log_term: leader.current_term(),
        entries: Vec::new(),
        leader_commit: 1,
    };
    follower.handle_append_entries(commit_notice).unwrap();
    assert_eq!(follower.commit_index(), 1);
    let request =
        un1c0::ReadIndexRequest::new("follower-read-2", leader.current_term(), "node-a", 1)
            .unwrap();
    assert!(
        follower
            .handle_read_index_request(request)
            .unwrap()
            .accepted
    );
}

#[test]
fn stale_terms_duplicates_and_unapplied_plans_fail_closed() {
    let (mut leader, _) = committed_leader();
    let action = leader
        .prepare_linearizable_read(
            LinearizableReadRequest::new("round-1", "agent/read", 1).unwrap(),
        )
        .unwrap();
    let request = match action {
        ReadIndexAction::Quorum(request) => request,
        ReadIndexAction::Lease(_) => unreachable!(),
    };
    assert!(leader
        .acknowledge_read_index(
            ReadIndexResponse {
                request_id: request.request_id.clone(),
                term: request.term,
                follower_id: "node-b".into(),
                read_index: request.read_index,
                accepted: false,
            },
            1,
        )
        .unwrap()
        .is_none());
    assert!(matches!(
        leader.acknowledge_read_index(
            ReadIndexResponse {
                request_id: request.request_id.clone(),
                term: request.term,
                follower_id: "node-c".into(),
                read_index: request.read_index + 1,
                accepted: true,
            },
            2,
        ),
        Err(ConsensusError::InvalidReadRequest(_))
    ));
    let plan = leader
        .acknowledge_read_index(
            ReadIndexResponse {
                request_id: request.request_id.clone(),
                term: request.term,
                follower_id: "node-b".into(),
                read_index: request.read_index,
                accepted: true,
            },
            2,
        )
        .unwrap()
        .unwrap();
    leader.execute_linearizable_read(plan, 2).unwrap();

    let duplicate = LinearizableReadRequest::new("round-1", "agent/read", 3).unwrap();
    assert!(matches!(
        leader.prepare_linearizable_read(duplicate),
        Err(ConsensusError::DuplicateReadRequest(_))
    ));
}

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    path: String,
    concurrency: usize,
    operations: usize,
    successful: usize,
    errors: usize,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    throughput_ops_per_sec: f64,
}

fn run_benchmark(concurrency: usize, lease_fast_path: bool) -> BenchmarkRow {
    let (mut leader, _) = committed_leader();
    leader
        .configure_leader_lease(LeaderLeaseConfig::new(10_000, 10).unwrap())
        .unwrap();
    if lease_fast_path {
        let action = leader
            .prepare_linearizable_read(
                LinearizableReadRequest::new("warmup", "agent/read", 1).unwrap(),
            )
            .unwrap();
        let request = match action {
            ReadIndexAction::Quorum(request) => request,
            ReadIndexAction::Lease(_) => unreachable!(),
        };
        leader
            .acknowledge_read_index(
                ReadIndexResponse {
                    request_id: request.request_id,
                    term: request.term,
                    follower_id: "node-b".into(),
                    read_index: request.read_index,
                    accepted: true,
                },
                1,
            )
            .unwrap();
    }
    let node = Arc::new(Mutex::new(leader));
    let barrier = Arc::new(Barrier::new(concurrency));
    let operations_per_worker = 128;
    let started = Instant::now();
    let mut handles = Vec::with_capacity(concurrency);
    for worker in 0..concurrency {
        let node = Arc::clone(&node);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            let mut latencies = Vec::with_capacity(operations_per_worker);
            let mut successful = 0;
            let mut errors = 0;
            for operation in 0..operations_per_worker {
                let request_id = format!("bench-{}-{}", worker, operation);
                let now_tick = if lease_fast_path {
                    100 + operation as u64
                } else {
                    20_000 + operation as u64 * 20_000
                };
                let begin = Instant::now();
                let result =
                    (|| {
                        let mut node = node.lock().map_err(|_| {
                            ConsensusError::ReadNotReady("benchmark mutex poisoned".into())
                        })?;
                        let action = node.prepare_linearizable_read(
                            LinearizableReadRequest::new(&request_id, "agent/read", now_tick)?,
                        )?;
                        let plan = match action {
                            ReadIndexAction::Lease(plan) => plan,
                            ReadIndexAction::Quorum(request) => node
                                .acknowledge_read_index(
                                    ReadIndexResponse {
                                        request_id: request.request_id,
                                        term: request.term,
                                        follower_id: "node-b".into(),
                                        read_index: request.read_index,
                                        accepted: true,
                                    },
                                    now_tick,
                                )?
                                .ok_or_else(|| {
                                    ConsensusError::ReadNotReady(
                                        "benchmark quorum did not complete".into(),
                                    )
                                })?,
                        };
                        node.execute_linearizable_read(plan, now_tick)
                    })();
                match result {
                    Ok(Some(value)) if value == "linearizable" => successful += 1,
                    Ok(_) | Err(_) => errors += 1,
                }
                latencies.push(begin.elapsed().as_micros());
            }
            (latencies, successful, errors)
        }));
    }
    let mut latencies = Vec::new();
    let mut successful = 0;
    let mut errors = 0;
    for handle in handles {
        let (mut values, worker_successful, worker_errors) = handle.join().unwrap();
        latencies.append(&mut values);
        successful += worker_successful;
        errors += worker_errors;
    }
    latencies.sort_unstable();
    let duration = started.elapsed().as_secs_f64();
    let percentile = |percent: usize| {
        let index = ((latencies.len() - 1) * percent / 100).min(latencies.len() - 1);
        latencies[index]
    };
    BenchmarkRow {
        path: if lease_fast_path {
            "lease_fast_path".into()
        } else {
            "quorum_read_index".into()
        },
        concurrency,
        operations: latencies.len(),
        successful,
        errors,
        p50_us: percentile(50),
        p95_us: percentile(95),
        p99_us: percentile(99),
        throughput_ops_per_sec: latencies.len() as f64 / duration,
    }
}

#[test]
fn high_load_read_benchmark_covers_lease_and_quorum_paths() {
    let mut rows = Vec::new();
    for concurrency in [1, 2, 4, 8, 16, 32] {
        rows.push(run_benchmark(concurrency, true));
        rows.push(run_benchmark(concurrency, false));
    }
    assert!(rows
        .iter()
        .all(|row| row.successful == row.operations && row.errors == 0));
    let json = serde_json::to_string_pretty(&rows).unwrap();
    std::fs::create_dir_all("benchmarks").unwrap();
    std::fs::write(
        "benchmarks/phase14_read_benchmark.json",
        format!("{json}\n"),
    )
    .unwrap();
}
