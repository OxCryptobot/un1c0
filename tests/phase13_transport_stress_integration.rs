use std::collections::BTreeMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use ed25519_dalek::SigningKey;
use un1c0::{
    AuthenticatedConsensusEnvelope, AuthenticatedSocketTransport, ConsensusMessage, VoteRequest,
};

fn keys() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            String::from("node-a"),
            SigningKey::from_bytes(&[51u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
        (
            String::from("node-b"),
            SigningKey::from_bytes(&[52u8; 32])
                .verifying_key()
                .to_bytes()
                .to_vec(),
        ),
    ])
}

fn envelope(nonce: &str) -> AuthenticatedConsensusEnvelope {
    AuthenticatedConsensusEnvelope::sign_for_cluster(
        "cluster-stress",
        "node-a",
        1,
        nonce,
        ConsensusMessage::VoteRequest(VoteRequest {
            term: 1,
            candidate_id: "node-a".into(),
            last_log_index: 0,
            last_log_term: 0,
        }),
        &SigningKey::from_bytes(&[51u8; 32]),
    )
    .unwrap()
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) {
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
    stream.flush().unwrap();
}

#[test]
fn concurrent_valid_and_corrupted_packets_are_classified_without_panic() {
    const VALID: usize = 32;
    const CORRUPTED: usize = 4;
    const TRUNCATED: usize = 4;
    const TOTAL: usize = VALID + CORRUPTED + TRUNCATED;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = AuthenticatedSocketTransport::new("cluster-stress", "node-b", keys(), 128).unwrap();
    let server = thread::spawn(move || {
        let mut receiver = receiver;
        let mut verified = 0usize;
        let mut rejected = 0usize;
        for _ in 0..TOTAL {
            match receiver.listen_once(&listener) {
                Ok(_) => verified += 1,
                Err(_) => rejected += 1,
            }
        }
        (verified, rejected)
    });

    let sender = AuthenticatedSocketTransport::new("cluster-stress", "node-a", keys(), 128).unwrap();
    let mut clients = Vec::with_capacity(TOTAL);
    for index in 0..VALID {
        let sender = sender.clone();
        clients.push(thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            sender
                .send(&mut stream, &envelope(&format!("valid-{index}")))
                .unwrap();
        }));
    }
    for index in 0..CORRUPTED {
        clients.push(thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            let mut corrupted = serde_json::to_vec(&envelope(&format!("corrupt-{index}"))).unwrap();
            let last = corrupted.len() - 2;
            corrupted[last] ^= 0x40;
            write_frame(&mut stream, &corrupted);
        }));
    }
    for index in 0..TRUNCATED {
        clients.push(thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(&100u32.to_be_bytes()).unwrap();
            stream.write_all(format!("truncated-{index}").as_bytes()).unwrap();
        }));
    }
    for client in clients {
        client.join().unwrap();
    }
    let (verified, rejected) = server.join().unwrap();
    assert_eq!(verified, VALID);
    assert_eq!(rejected, CORRUPTED + TRUNCATED);
}
