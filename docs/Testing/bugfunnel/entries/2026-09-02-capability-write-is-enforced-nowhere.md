---
id: 2026-09-02-capability-write-is-enforced-nowhere
date: 2026-09-02
gap: ORACLE
secondary: null
status: FIXED
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

FIXED, in two halves.

**Relay leg — already landed before this lane** (present at `main`
`830d794f878f`). `acceptor::admit` now derives the required capability from who
the subject is relative to the admitter (`acceptor.rs:209`
`required_capability`: subject == admitter is a READ, subject != admitter is
that peer WRITING into my replica), and refuses with
`AdmitDecision::RefuseCapability { principal, missing, held }`. `pull_once`
importing on `Import { .. }` is correct as a result — the check happens inside
`admit`, which is the sole admission decision, so there is nothing left for the
caller to re-check. Covering tests in `holon-sharing/src/acceptor.rs`:

- `a_read_only_peers_write_into_the_owners_store_is_refused`
- `a_third_partys_read_only_chain_cannot_write_into_my_store`
- `a_read_write_peers_write_into_the_owners_store_is_admitted` (the positive)

**Iroh leg — this lane (2026-09-08).** The transport had no capability notion at
all: the enrollment gate answered "is this peer a member" and its `AuthorizedPeer`
witness was then discarded. A share now declares what membership confers
(`iroh_advertiser::ShareAdmission`), the accept loop turns the enrollment result
into a `peer_import::AdmittedPeer` carrying that `Capabilities` value, and both
directions are gated by it: `import_peer_delta` requires `Capability::Write`,
`authorize_peer_read` requires `Capability::Read`. A refusal is a typed loud
`Err` (`PeerAccessRefused`) naming the peer, the container, the missing
capability and what was attempted — never a silent drop (D72.a). Covering tests
in `holon-loro/src/peer_import.rs`:

- `a_read_only_peers_delta_is_refused_naming_the_missing_capability` (and it
  asserts the replica is left untouched)
- `an_admission_conferring_nothing_refuses_both_directions`
- `a_write_only_admission_may_not_read`

Both legs now speak ONE capability type, `holon_api::sharing::Capabilities`,
moved down out of `holon-sharing::policy` (which re-exports it) because
`holon-sharing` depends on `holon-loro` and neither could own it. The set travels
from the decision to the enforcement point as a value; nothing re-parses a
string.

**What is still open, and why it is a different bug.** Enforcement is only as
meaningful as the admission behind it, and the third-party subtree-share
lifecycle still advertises with NO roster — every peer that reaches the endpoint
is admitted as a full writer. That is an *admission* gap, not a capability-check
gap, and it is filed as
`2026-09-08-the-subtree-share-hot-path-advertises-un-gated`. This lane made it
typed and greppable (`ShareAdmission::Ungated`, plus a `warn!` per un-gated
share start) rather than an unmarked `None`.

The program consequence the entry flagged is discharged: DC-3's read-only branch
is now buildable — a read-only phone is `Capabilities::read_only()` on the
share's admission, and its writes are refused loudly at the import.
