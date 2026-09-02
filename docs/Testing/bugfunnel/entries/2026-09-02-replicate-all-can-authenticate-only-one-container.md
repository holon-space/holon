---
id: 2026-09-02-replicate-all-can-authenticate-only-one-container
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  `ContainerRegistry::replicate_all` advertises every container in the
  replication set under ONE `SharedRoster`, but a roster's enrollment proof is
  bound to a single `shared_tree_id`, so no peer could ever enroll into the
  second or later container it advertises.
---

## Bug

Found by the `pair-inc3` lane on 2026-09-02 while building the production leg of
the two-instance transport seam (ruling D71.b). Found by reading
`replicate_all`'s signature against the roster's proof rule, not by a failing
run — the replication set is one container today, so nothing exercises it.

`replicate_all` is the everything-policy fast path: it iterates the replication
set and advertises each container gated by the roster it was handed
(`crates/holon-loro/src/container_registry.rs:235-256`):

```rust
pub async fn replicate_all(
    &self,
    advertiser: &IrohAdvertiser,
    roster: SharedRoster,
) -> Result<Vec<(String, EndpointAddr)>> {
    for container in self.replication_set().await? {
        let addr = advertiser
            .start_share_gated(container.id.clone(), …, roster.clone(), …)
```

One roster, every container. But a roster is not container-agnostic.

## Root cause

`ShareRoster` is constructed against a single `shared_tree_id`
(`crates/holon-loro/src/share_enrollment.rs:485-500`) and the possession proof
is *bound* to it — deliberately, so a proof minted for share A cannot be
replayed to enroll into share B. `share_enrollment.rs:602-608`:

```rust
let expected = self
    .capability_secret
    .prove(challenge, &self.shared_tree_id);
// Constant-time compare via `blake3::Hash: PartialEq`.
if &expected != presented_proof {
    return Err(AuthzReject::BadProof);
}
```

The dialer proves against the container it is dialing —
`sync_doc_initiate_enrolled` takes `shared_tree_id` and passes it to
`CapabilitySecret::prove` (`iroh_sync_adapter.rs:250-267`). So for the FIRST
container in the replication set the two tree ids agree and enrollment
succeeds; for every container after it the acceptor computes its expected proof
over the roster's tree id while the dialer computed one over the container's,
they differ, and the peer is refused with `BadProof`.

The refusal is loud and correct in isolation — the binding is doing exactly the
job it was designed for. The defect is that `replicate_all`'s signature makes
the mismatch unavoidable: it accepts one roster for a set it iterates.

Harmless today. The replication set is the root container alone
(`replication_set` = root plus registered extras, `container_registry.rs:127-137`),
and nothing in production registers an extra container onto the replicate-all
path — a third-party share takes the per-share extract-prune-mount path
instead. It becomes reachable the moment own-device pairing replicates a second
container, which is Inc 7's wiring.

The iroh leg of the two-instance slice refuses to paper over it: it fails the
round loudly when the publisher's replication set is not exactly the root
container, naming the ids and the reason
(`crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs`,
the `set.len() != 1` guard), rather than advertising something no peer could
enroll into.

## Missing piece

**COVERAGE.** No transition in any catalog registers a second container into the
replication set and then drives a sync round over it. The two-instance slice's
alphabet is create / type / share / sync, and `share_container` grants a
membership chain over the root — it never calls
`ContainerRegistry::register_container`. So the state that exposes this is
simply not generatable, on either wire.

It is specifically NOT an oracle gap: if a case did reach that state, the
production leg already goes red for the right reason. The dial returns
`AuthzReject::BadProof`, `IrohTransport::round` returns `Err` for an authorized
dial that failed, and the round fails loudly rather than reporting a converged
pair. The detection exists; the generation does not.

## Keystone repro

The keystone cannot reproduce it — single instance, no advertiser, no
enrollment. The two-instance binary is the right home, and closing the coverage
gap means a transition that registers an extra container into the replication
set, plus a receiver that mounts the same container so the round has a
counterpart.

## Remedy

Open, and the fix is a signature change rather than a patch at a call site.
`replicate_all` should take authorization *per container* — a roster resolved
from the container id, or a roster type that carries the whole replicable set
rather than one tree id. That keeps the anti-replay binding intact (which is
the property worth preserving) while letting one call gate a set.

Sequencing: this must land before Inc 7 replicates more than the root
container, or own-device pairing will silently pair only the first container
and refuse the rest. Until then the lane guard above keeps the two-instance
slice honest about the limitation instead of hiding it.
