use std::collections::BTreeMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use ed25519_dalek::SigningKey;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedSocketTransport, ConsensusError, ConsensusMessage,
    VoteRequest,
};

fn keys() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            String::from("node-a"),
            SigningKey::from_bytes(&[41u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
        (
            String::from("node-b"),
            SigningKey::from_bytes(&[42u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
    ])
}

fn envelope(cluster_id: &str, nonce: &str) -> AuthenticatedConsensusEnvelope {
    let key = SigningKey::from_bytes(&[41u8; 32]);
    AuthenticatedConsensusEnvelope::sign_for_cluster(
        cluster_id,
        "node-a",
        1,
        nonce,
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &key,
    )
    .unwrap()
}

fn write_frame(stream: &mut TcpStream, envelope: &AuthenticatedConsensusEnvelope) {
    let bytes = serde_json::to_vec(envelope).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
    stream.flush().unwrap();
}

#[test]
fn authenticated_socket_binds_cluster_and_rejects_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = AuthenticatedSocketTransport::new("cluster-alpha", "node-b", keys(), 8).unwrap();
    let worker = thread::spawn(move || {
        let mut receiver = receiver;
        let first = receiver.listen_once(&listener).unwrap();
        assert_eq!(first.cluster_id, "cluster-alpha");
        let replay = receiver.listen_once(&listener).unwrap_err();
        assert_eq!(replay, ConsensusError::ReplayDetected);
        let mismatch = receiver.listen_once(&listener).unwrap_err();
        assert!(matches!(mismatch, ConsensusError::Unauthenticated(_)));
    });

    let sender = AuthenticatedSocketTransport::new("cluster-alpha", "node-a", keys(), 8).unwrap();
    let valid = envelope("cluster-alpha", "nonce-1");
    let mut first_stream = TcpStream::connect(address).unwrap();
    sender.send(&mut first_stream, &valid).unwrap();
    drop(first_stream);
    let mut replay_stream = TcpStream::connect(address).unwrap();
    sender.send(&mut replay_stream, &valid).unwrap();
    drop(replay_stream);

    let mut mismatch_stream = TcpStream::connect(address).unwrap();
    write_frame(&mut mismatch_stream, &envelope("cluster-beta", "nonce-2"));
    drop(mismatch_stream);
    worker.join().unwrap();
}

#[test]
fn transport_rejects_oversized_frames_and_untrusted_sender_keys() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = AuthenticatedSocketTransport::new("cluster-alpha", "node-b", keys(), 8).unwrap();
    let worker = thread::spawn(move || {
        let mut receiver = receiver;
        let oversized = receiver.listen_once(&listener).unwrap_err();
        assert_eq!(oversized, ConsensusError::FrameTooLarge);
        let untrusted = receiver.listen_once(&listener).unwrap_err();
        assert!(matches!(untrusted, ConsensusError::Unauthenticated(_)));
    });

    let mut oversized_stream = TcpStream::connect(address).unwrap();
    oversized_stream
        .write_all(&(1024u32 * 1024 + 1).to_be_bytes())
        .unwrap();
    drop(oversized_stream);

    let mut untrusted_stream = TcpStream::connect(address).unwrap();
    let forged = AuthenticatedConsensusEnvelope::sign_for_cluster(
        "cluster-alpha",
        "node-a",
        1,
        "nonce-untrusted",
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &SigningKey::from_bytes(&[99u8; 32]),
    )
    .unwrap();
    write_frame(&mut untrusted_stream, &forged);
    drop(untrusted_stream);
    worker.join().unwrap();
}
