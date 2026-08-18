# Phase 24 Baseline Audit Notes

## Observed facts

`AuthenticatedSocketTransport` validates cluster and sender identity, replay epoch, term floor, trusted Ed25519 keys, frame length, and nonce replay. Its `send` method serializes one envelope and writes the frame directly to a caller-owned `TcpStream`; its `receive` method reads one bounded frame, verifies it, and records the nonce in the sender replay window. The transport has a single global `max_frame_bytes` bound but no per-peer byte budget, queued-byte accounting, in-flight frame limit, or typed backpressure result.

Phase 16 already provides consensus-core replication flow control with one in-flight logical batch per peer, bounded entries/bytes, retry timing, and higher-term invalidation. That control is intentionally transport-agnostic and does not know the serialized authenticated socket frame size or the bytes queued for each peer.

The Phase 23 compliance artifact contains 44 passing gates and the audit utility verifies gate values, Phase 15–23 evidence, benchmark structure, metrics commit ancestry, intentional false evidence, secret absence, and lack of cluster mutation.

## Risks

A caller can repeatedly invoke `send` for one peer without a transport-layer quota, allowing unbounded kernel/socket buffering or unfairness between peers. A large but individually valid frame can consume the entire available transport budget. Because the current send path writes immediately, it cannot return a deterministic retry tick or distinguish peer-local backpressure from frame-size, authentication, or I/O failure. Receive-side frame admission also lacks a per-peer byte window, so one sender can dominate bounded processing capacity before replay checks complete.

## Phase 24 design direction

Add `SocketQuotaConfig`, `SocketPeerQuota`, `SocketBackpressureAction`, and `SocketTransportMetrics`. Track queued/in-flight bytes, accepted/rejected frames, backpressured sends, retry ticks, and per-peer limits in the transport. Add quota-aware send and receive admission APIs that calculate exact serialized frame bytes before mutation, use per-peer state, return deterministic retry ticks, and clear/rebuild quota state only through explicit membership or epoch lifecycle operations. Preserve legacy `send`/`receive` behavior through default-compatible wrappers while making production callers use the typed quota path.

## Validation requirements

Tests must cover per-peer isolation, exact frame-byte admission, queue and in-flight byte limits, deterministic retry boundaries, quota release, oversized-frame rejection before state mutation, receive-side quota rejection, replay/authentication ordering, unknown-peer rejection, epoch rotation quota reset, and no socket-thread or scheduler authority inside the consensus core.
