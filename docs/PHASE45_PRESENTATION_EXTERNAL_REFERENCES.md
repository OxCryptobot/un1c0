# External references for Phase 45 architecture presentation

1. Rust `std::sync::mpsc` documentation: https://doc.rust-lang.org/std/sync/mpsc/index.html

The official Rust documentation describes `mpsc` as multi-producer, single-consumer FIFO channel communication. It distinguishes asynchronous unbounded channels from synchronous bounded channels created by `sync_channel`, whose pending-message storage has a fixed size. It also documents typed errors on disconnected send/receive operations and `SyncSender` cloning for multiple producers.

2. The Rust Programming Language, message passing: https://doc.rust-lang.org/book/ch16-02-message-passing.html

The official Rust book explains transferring ownership through channels, multiple producers sending to one receiver, and the importance of handling send/receive failures rather than assuming the other endpoint remains alive. It emphasizes that channel ownership rules prevent use-after-send concurrency errors.

3. RFC 8032, Edwards-Curve Digital Signature Algorithm (EdDSA): https://datatracker.ietf.org/doc/html/rfc8032

RFC 8032 defines EdDSA and the Ed25519 verification procedure. It specifies parsing public-key/signature inputs, computing the verification hash, checking the group equation, and rejecting malformed or invalid signatures. The Phase 45 implementation uses the repository’s Ed25519 verification library calls over canonical request and acknowledgement payloads; it does not implement cryptographic primitives itself.

These external sources support general concurrency and signature-verification concepts only. Repository-specific claims are grounded in `src/ownership_bound_cas_verifier.rs`, `src/ownership_bound_cas.rs`, `src/replicated_durability.rs`, the Phase 44/45 integration tests, and the committed sanitized benchmark artifacts.
