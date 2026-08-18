# Phase 28 Baseline Audit: Partition-Aware Queue Ownership Fencing

## Observed Phase 27 behavior

Phase 27 persists a hash-bound ownership lease per queue peer and accepts a signed transfer only when the sender is the previous owner, the recipient is the new owner, the owner term is not stale, the ownership epoch increases, and the prior lease is expired or superseded. A new owner can import a source-bound durable queue snapshot, clear stale acknowledgement evidence during transfer, and retry the FIFO head. The local acknowledgement path rejects commits unless the transport is the current owner.

The Phase 27 integration tests are deterministic local-state tests. They cover quorum waiting/commit, acknowledgement restart recovery, source-bound cross-host restore, successful new-owner retry, and stale or misbound transfer rejection. They do not emulate a real partition, a loss of quorum, a lease-renewal protocol, or simultaneous old/new owner activity across independent machines.

## Partition safety gap

The delivery path checks authenticated payloads and active-delivery state but does not expose a typed local action for loss of ownership quorum or lease expiry before writing a queued frame. The transfer API protects the receiving owner, but a partitioned old-owner process has no durable, explicit fencing observation that can stop future delivery attempts. This is a fail-closed safety gap at the local execution boundary even though the existing acknowledgement commit path is owner-bound.

## Phase 28 scope

Add a bounded, hash-bound `QueueOwnershipFence` record and a typed `QueueOwnershipSafetyAction`. The fence records the peer, owner, term, ownership epoch, observation tick, reachable member count, required quorum, and reason. Persist it with the durable queue state. Add an API that records a signed/externally-authorized quorum-loss observation only after validating the current lease binding and quorum bounds. Delivery must reject or return a fenced action when the local owner has an active fence or the lease is expired. A valid higher-term ownership transfer clears the fence atomically and permits the new owner to retry. Restart must restore the fence and remain fail-closed.

## Non-goals

This phase does not claim to implement a real network failure detector, cross-machine lease clock synchronization, transport quorum, or split-brain prevention. Those remain explicit production boundaries. The phase makes the local durable execution kernel fail closed when an authenticated coordinator or caller reports insufficient quorum and makes the evidence restart-safe.

## Proposed evidence gates

1. `quorum_loss_fences_delivery`
2. `lease_expiry_fences_delivery`
3. `fence_survives_restart_and_rejects_ack`
4. `ownership_transfer_clears_fence_for_failover`

## Selection rationale

This is the highest-leverage next slice because it closes the direct safety gap between Phase 27's ownership transfer contract and the actual durable socket write path without inventing an unverified distributed transport. It also provides a typed seam for a future failure detector and quorum coordinator.
