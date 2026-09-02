---
id: 2026-09-02-capability-write-is-enforced-nowhere
date: 2026-09-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The sync acceptor gates admission on Capability::Read alone, so a peer holding
  a read-only certificate has its writes imported exactly like a read-write
  peer's — Capability::Write is defined, delegated and intersected, but never
  checked.
---

## Bug

Found by code audit during lane `pair-inc0` (own-device pair, Increment 0),
while granting the two-instance slice's receiver a read-write certificate so it
could author concurrently. Granting it changed nothing observable, which is the
tell.

`Capabilities` carries three capabilities and `Capability::Write` is threaded
through issuance, delegation and chain intersection
(`crates/holon-sharing/src/policy.rs`). No admission path reads it. A peer
issued `Capabilities::read_only()` can push arbitrary state and the receiving
side imports it.

Security-adjacent: the certificate is the artifact a user would reason about
when sharing a page read-only with a third party. It currently describes an
intent the system does not enforce.

## Root cause

`admit` in `crates/holon-sharing/src/acceptor.rs:130-140` is the sole admission
decision for an inbound envelope, and its capability clause tests exactly one
capability:

```rust
Ok(capabilities) if !capabilities.contains(Capability::Read) => {
    AdmitDecision::RefuseCapability { principal: claimant }
}
Ok(capabilities) => AdmitDecision::Import { capabilities },
```

`AdmitDecision::Import` carries the effective `capabilities` back to the caller,
so the information reaches the orchestrator — but `pull_once`
(`crates/holon-sharing/src/sync.rs:210-235`) matches only on
`AdmitDecision::Import { .. }` and discards the payload, importing the blob
regardless of what the chain conferred.

The doc comment on `RefuseCapability` says "membership holds but confers no read
capability over the selector", which is an accurate description of the code and
an incomplete description of what a capability set is for.

## Missing piece

No invariant or unit test asserts the negative: that a read-only chain's writes
are REFUSED. The acceptor's own unit tests cover malformed proofs, bad
signatures, wrong principals and lapsed leases — every refusal path except this
one. Because `Capabilities::read_only()` was the only value ever issued in
tests, "read-only" and "read-write" were indistinguishable by construction and
no oracle could tell them apart.

This is why it reads as ORACLE and not COVERAGE: the interaction is entirely
generatable today — the two-instance slice drives a read-only peer's writes
across the transport in every case. Nothing looks at whether they should have
been admitted.

## Remedy

OPEN. Not fixed in this lane, which owns convergence, not authorization.

The lane's report flags a program consequence: the plan's DC-3 decision card
("is the phone a full writer from day one?") assumes read-only is an available
setting. It is not — choosing a read-only phone requires building this
enforcement first, so both branches of DC-3 cost the same enforcement work.

Fix shape: `pull_once` must consult the `capabilities` that
`AdmitDecision::Import` already hands it, and refuse an update-bearing envelope
from a chain without `Capability::Write`. Add the missing refusal to the
acceptor's unit tests as the red, and add a two-instance invariant that a
read-only receiver's writes never reach the owner.

Related in the same lane: `2026-09-02-reverse-sync-leg-reuses-the-receiver-audience`.
