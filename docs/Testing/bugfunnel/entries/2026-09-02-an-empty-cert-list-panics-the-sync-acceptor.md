---
id: 2026-09-02-an-empty-cert-list-panics-the-sync-acceptor
date: 2026-09-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A membership proof carrying the two bytes `[]` decoded to a well-formed EMPTY
  cert list, which slipped past the byte-level emptiness check and then panicked
  the acceptor on an `expect` asserting a postcondition that was never true —
  reachable with no certificate, key or lease.
---

## Bug

Found by adversarial verification of lane `pair-inc5` (own-device pair,
Increment 5), Rev 2, by a fresh-context verifier probing the acceptor's refusal
paths after the capability and container fixes landed.

`admit` is the trust boundary for every inbound sync envelope. An attacker sends
a proof whose `chain` field is the two bytes `[]`. That is valid JSON and
decodes to a zero-length `Vec<MembershipCert>`, so the guard against an EMPTY
chain — which tested the raw BYTE string, not the decoded list — passes it
through. The subject is then taken from the chain's last cert, and there is no
last cert.

The attacker needs no certificate, no key and no lease. Only the container and
the audience must match, and both are fields they choose. In `pull_once` the
panic unwinds out of the sync round, so one crafted envelope aborts the round
for every container behind it. Denial of service at the boundary whose whole job
is to survive hostile input.

Reproduced (`lane-logs/red-empty-cert-list-panic-2026-09-02.log`):

```
FAIL acceptor::tests::a_chain_that_decodes_to_zero_certs_refuses_instead_of_panicking
thread '...' panicked at crates/holon-sharing/src/acceptor.rs:191:10:
parse_chain rejects an empty chain
Summary: 20 tests run: 19 passed, 1 failed
```

The panic message is the defect stated out loud: the code asserts a
postcondition of `parse_chain` that `parse_chain` did not actually establish.

## Root cause

`parse_chain` (`crates/holon-sharing/src/acceptor.rs`) rejected only
`auth.chain.is_empty()` — zero BYTES — and then handed any successfully decoded
vector back as a `MembershipChain`, empty or not:

```rust
serde_json::from_slice::<Vec<MembershipCert>>(&auth.chain)
    .map(MembershipChain::new)
```

`admit` then did:

```rust
let subject = chain.certs.last()
    .expect("parse_chain rejects an empty chain")
    .grantee.clone();
```

Two representations of "empty" (no bytes, and a decoded list of length zero)
with a check for only the first. The `expect` documented an invariant that lived
nowhere in the type or the parser, so it was a comment with a crash attached
rather than an assertion.

Lane `pair-inc5` introduced the subject extraction when it moved the claimant
off the wire and onto the signed chain; the empty-byte check predates it. The
combination is what made the panic reachable.

## Missing piece

The two-instance slice has no way to produce a hostile envelope. Every proof it
sends is built by `push_once` from a real `MembershipChain` via `encode_chain`,
and the one empty chain the harness does construct (the unshared case) never
reaches the wire, because `push_once` short-circuits on
`auth.chain.certs.is_empty()` and reports the container as `unauthorized`. So no
generated sequence can put a malformed, truncated or adversarial proof in front
of `admit` at all.

That is the COVERAGE gap, and it is the same gap that hid
`2026-09-02-a-capability-is-checked-against-a-sender-chosen-selector`: the
acceptor's negative space is reachable only by hand-written unit tests, and only
for the cases somebody thought of. A boundary that parses attacker-controlled
bytes wants generated input, not enumerated input.

## Keystone repro

The keystone `general_e2e_composed_pbt` cannot reproduce it — single instance,
no `SyncTransport`. The proper closure is a generator over malformed proofs
(empty list, truncated JSON, wrong types, deep nesting) driven at `admit`, which
is pure and sans-IO and therefore ideal to fuzz. Recorded as the shape to build;
not built in this lane.

## Remedy

FIXED in lane `pair-inc5`, parse-don't-validate: `parse_chain` now returns
`(MembershipChain, Principal)` — the chain AND the subject it terminates at — so
the subject's existence is a RESULT of the parse rather than an assumption about
it. A chain with no terminal grantee cannot yield a subject, both empty
representations take the same typed refusal, and there is no caller-side unwrap
left to be wrong about. The `expect` is gone rather than corrected.

Both cases now return `AdmitDecision::RefuseMalformedProof` with the "carries an
EMPTY chain — an unproven claim, not a claim to be trusted" message, which lands
in `SyncReport.refusals` like every other refusal.

Pinned by `a_chain_that_decodes_to_zero_certs_refuses_instead_of_panicking`,
alongside the pre-existing `empty_chain_refuses` which covers the zero-byte
case. Green after the fix: 20 of 20 in the acceptor library, 388 of 388 across
`holon-sharing` and `holon-loro`.
