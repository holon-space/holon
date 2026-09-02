---
id: 2026-09-02-reverse-sync-leg-reuses-the-receiver-audience
date: 2026-09-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  The receiver→owner sync direction attaches the RECEIVER's membership proof and
  verifies against the receiver's own principal, so the owner admits envelopes
  addressed to someone else and no chain naming the owner is ever required.
---

## Bug

Found by code audit during lane `pair-inc0` (own-device pair, Increment 0),
while widening the two-instance slice so both peers author concurrently. The
reverse leg worked immediately, which was suspicious: no certificate naming the
owner as audience exists anywhere in the flow.

The acceptor's whole design rests on the audience being the receiving side. Its
module doc is explicit:

> `MembershipProof::principal` names the **audience** principal the blob is
> destined for … So the receiver verifies *its own* authorization at *its own*
> clock. That is what makes revocation work without any online check.

On the reverse leg neither half of that holds. The blob is stamped with the
receiver's principal, and the owner verifies that same principal rather than
itself. Revocation therefore cannot work in that direction: revoking the
receiver's lease stops the owner from accepting the receiver's data, but nothing
expresses or checks the owner's own authorization to receive.

## Root cause

`TwoInstanceHandle::sync_now`
(`crates/holon-integration-tests/src/pbt/composed/two_instance.rs`) builds ONE
`OutboundAuth` and ONE `AcceptorContext` and reuses both for both directions.
`OutboundAuth.audience` and `AcceptorContext.receiver` are each hardcoded to
`RECEIVER_PRINCIPAL` regardless of which way the round runs, so on a
receiver→owner round the owner's `pull_once` runs with
`ctx.receiver = "receiver"` and `admit`'s claimant check
(`crates/holon-sharing/src/acceptor.rs:113-121`) compares the receiver's
principal against itself and passes.

This is a test-harness shape today, because `sync_once` is wired to no network.
It is recorded as a product concern because the production own-device leg is
bidirectional by definition and the sans-IO `admit` is the shared decision
function both legs will use: whatever calls it has to supply a per-direction
audience, and nothing in the current API makes that obligation visible. The
production iroh leg (`iroh_sync_adapter.rs`) does not go through `admit` at all,
so there is no existing correct implementation to copy.

## Missing piece

The two-instance slice models a ONE-WAY share. Its alphabet had no
receiver-authored write until this lane added one
(`boot_two_instances_with_receiver_caps`), so no case ever exercised a
receiver→owner round carrying real content, and the single-audience shortcut was
never wrong in any generated sequence. That is the COVERAGE gap.

Secondarily ORACLE: even now that the direction is exercised, nothing asserts
that a round is authorized for the side that RECEIVES it. An invariant of the
shape "every admitted envelope's proof names the admitting peer" would have
failed on the first reverse round.

## Remedy

OPEN. Not fixed in this lane.

Fix shape: make the audience a function of the round's direction rather than a
constant — the owner needs its own owner-audience chain (self-issued for an
own-device pair), and `AcceptorContext.receiver` must be the peer actually
pulling. Then add the invariant above so a future single-audience shortcut fails
loud.

Related in the same lane: `2026-09-02-capability-write-is-enforced-nowhere`.
Both are authorization obligations that the current one-way, read-only-only test
shape made unobservable.
