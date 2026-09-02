---
id: 2026-09-02-blobsig-is-unkeyed-so-a-forged-sender-can-replay-a-chain
date: 2026-09-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `BlobSig` is an unkeyed blake3 hash of the envelope's canonical bytes, so it
  authenticates no sender — anyone who can write to a relay can recompute it,
  replay a captured membership chain under a forged sender, and have the
  envelope admitted with that chain's capabilities.
---

## Bug

Found by code audit during lane `pair-inc5` (own-device pair, Increment 5),
while building `Capability::Write` enforcement in the acceptor. The enforcement
work made the gap unavoidable: the acceptor now decides which capability an
envelope needs from *who the subject is*, which is only meaningful if the
sender is who they claim to be. They are not verified to be.

Threat model, as recorded in the lane report:

> Principals are named by owner-signed lease certs, verified at the admitter's
> clock; revocation is non-renewal. Anyone holding an owner-issued cert can
> present it, and chains travel in the clear on the relay, so any peer pulling
> a container log sees every other peer's chain. `BlobSig` is an unkeyed blake3
> hash, not a signature. This layer authenticates NO sender. Capability
> enforcement is therefore only as strong as the transport's own peer
> authentication.

Concretely: peer B, holding only a read-only cert, pulls a container log,
captures peer C's chain out of an envelope the owner addressed to C, builds its
own payload with `audience = C` and that chain, recomputes the hash, and pushes.
C admits it. B has written into C's replica using C's credentials. The same
shape lets any relay-adjacent party inject content under any captured chain.

## Exploit shape (reproduced by adversarial verification)

The `pair-inc5` verifier reproduced this as a passing probe against an
UNMODIFIED copy of the lane tree, and it is sharper than the paragraph above.
The attacker needs **no certificate at all**:

```
CHAIN-SUBSTITUTION DECISION = Import { capabilities: Capabilities({Read}) }
    PASS attacker_writes_into_b_using_bs_own_read_only_chain
```

Mallory holds nothing. She lifts peer B's owner-issued **read-only** chain off
the relay, sets `audience = "peer-b"`, attaches an arbitrary CRDT payload, and
recomputes the unkeyed hash. At B: the audience matches, and because the chain
she chose names B, `subject == admitter`, so the acceptor demands only `Read` —
which B's own chain of course confers. Attacker-chosen state lands in B's
replica.

The sharp consequence: the capability required is a function of an
**attacker-chosen field** (which chain is attached), so an attacker simply
selects the chain that lowers the requirement. `subject == admitter -> Read` is
strictly easier to satisfy than the old unconditional `Read` check, so against a
relay-capable attacker the new capability rule does not merely fail to help —
it supplies the bypass. Its real reach is: it stops an honest peer presenting
its own chain, and stops nobody who tampers.

A second shape with the same root: if a cert is ever issued with the **owner**
as grantee, replaying it at the owner makes `subject == admitter`, so a write
into the owner's own replica passes on `Read` alone. No such cert is issued
today; issuing one silently disarms the gate, which is why issuance wants a loud
assertion.

```
OWNER-GRANTEE DECISION = Import { capabilities: Capabilities({Read}) }
    PASS owner_grantee_chain_replayed_at_the_owner_needs_only_read
```

The control case behaves as designed — a third party presenting its OWN
read-only chain is refused:

```
THIRD-PARTY DECISION = RefuseCapability { principal: Principal("mallory"), missing: Write, held: Capabilities({Read}) }
```

## Root cause

`blob_canonical_bytes` (`crates/holon-sharing/src/acceptor.rs:122-134`) is a
plain blake3 digest over container, sender, audience, selector, epoch, chain
and payload. `admit` (`crates/holon-sharing/src/acceptor.rs:138-152`) compares
`env.sig.0` against a locally recomputed digest of the same bytes:

```rust
let expected = blob_canonical_bytes(env);
if env.sig.0 != expected { return AdmitDecision::RefuseSig { .. }; }
```

There is no key, so "the signature verifies" means only "the bytes are
self-consistent". The doc comment on `BlobSig`
(`crates/holon-loro/src/sync_transport.rs:105-108`) claims it is a signature
"under the CONTAINER key — the reason a relay cannot inject content it did not
receive from a member", which is not what the code does. Lane `pair-inc5`
corrected the acceptor's module docs to state the limitation plainly; the
mechanism itself is unchanged.

`env.sender` is a `StablePeerId` carried in the clear and bound only by that
same unkeyed digest. Nothing maps a principal to a peer id, so even a keyed
check of the sender would need a binding that does not exist yet.

## Missing piece

The two-instance slice has no adversarial alphabet. `InMemoryRelay`
(`crates/holon-loro/src/sync_transport.rs`) is an honest store-and-forward log,
both peers are honest, and every envelope the harness produces is built by
`push_once` from a real chain. There is no transition that tampers with a blob,
re-addresses one, or mints one from captured material, so no generated sequence
can reach the state where a forged sender is admitted. That is the COVERAGE
gap, and it is why the acceptor's refusal paths are all unit-tested rather than
property-driven.

Secondarily ORACLE: even given such a transition, no invariant says "every
admitted envelope was authored by the device the subject's chain belongs to".
The convergence oracle would read a successful injection as convergence.

## Keystone repro

The keystone `general_e2e_composed_pbt` cannot reproduce this — it is
single-instance and never touches `SyncTransport`. The right home is the
two-instance slice, and closing the gap means adding a hostile-relay transition
(tamper / re-address / replay-with-captured-chain) plus the invariant above.
Both are blocked on the remedy below: with no keyed signature there is nothing
for the invariant to assert.

## Remedy

OPEN, deferred by ruling **D75.a** (Martin, 2026-09-02).

Scope of the deferral: own-device pairing ships over **direct iroh/QUIC only**,
where the transport authenticates the peer by keyed node identity and the
enrollment fingerprint. On that path the unkeyed `BlobSig` is not load-bearing —
no untrusted party can place bytes in front of the acceptor. The exposure is
the **relay-backed path** (an HTTPS blind relay, or any third-party
store-and-forward), which is not shipping.

Ruled remedy, required BEFORE any relay-backed or third-party share lands:

1. `BlobSig` becomes a real **Ed25519 signature over `blob_canonical_bytes`**,
   with the signer being the **chain's terminal grantee** (the subject).
2. `admit` verifies that signature against the subject's key rather than
   recomputing a digest, so a captured chain is useless without the matching
   private key. This is what makes the attached chain the attacker's OWN chain
   rather than one they chose, which is the premise every capability check here
   silently assumes.
3. Once (1) and (2) hold, add the hostile-relay transition and the
   "every admitted envelope was authored by its subject" invariant to the
   two-instance slice, closing both gaps recorded here.

Related, from the same lane: `2026-09-02-capability-write-is-enforced-nowhere`
and `2026-09-02-reverse-sync-leg-reuses-the-receiver-audience`. Lane `pair-inc5`
did harden the canonical bytes to cover `auth.chain`, which stops a chain swap
by anyone who cannot also recompute the digest — it does not help against a
party who can, which is anyone.
