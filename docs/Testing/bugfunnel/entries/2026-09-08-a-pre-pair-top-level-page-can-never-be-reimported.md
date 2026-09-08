---
id: 2026-09-08-a-pre-pair-top-level-page-can-never-be-reimported
date: 2026-09-08
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Every top-level page this device wrote before pairing is deferred as an orphan
  because its parent is the root sentinel, which the re-import plan does not
  count as a home — so the undismissable degraded banner is permanent and its
  Retry can never succeed.
---

## Bug

Found by the `dogfood-explorer` gate for D94.a (pair-boot-degraded), port 8720,
fixture `/tmp/dogfood-w10-degraded` (owner store from one boot, archive from a
second boot whose vault held an extra page; marker written by hand — recipe in
`lane-logs/dogfood-w10/report.md`).

The archive held one ordinary page (`phone.org`: `block:phone-page` →
`block:phone-day` → `block:phone-note`) and one block whose parent the owner's
store already had (`block:phone-extra` under `block:owner-root-block`). The
boot deferred THREE blocks:

    [LoroModule] the pair to ownerdevice0000… left 3 block(s) in
    /tmp/dogfood-w10-degraded/vault/.loro/archive/20260908T040800 that the
    owner's store has no parent for: block:phone-note (parent block:phone-day),
    block:phone-day (parent block:phone-page),
    block:phone-page (parent sentinel:no_parent)

The root of the cascade is the last line: `block:phone-page` is a top-level
page, so its parent is the stored root sentinel `sentinel:no_parent`. The
sentinel is not a block, so it is never a key in the adopted store's block map,
so the page is unplaceable — and its whole subtree with it.

That makes the deferral permanent. `device.pair_retry_reimport` refuses
identically on every attempt, and there is nothing the user can do about it:
the parent the refusal names cannot be created, because `sentinel:no_parent` is
a sentinel and not a creatable block. The banner D94.a made undismissable
therefore never lifts.

Proof both ways, on the same live store: retrying refused with the message
above; creating a real block under the sentinel's place with the id
`block:phone-page` and retrying then returned `{"blocks":2,"conflict_copies":0}`,
removed `pairing-in-progress.json`, wrote `pairing.json`, re-imported
`block:phone-day` and `block:phone-note`, and lifted the banner
(`lane-logs/dogfood-w10/06-after-retry-s.png`). The mechanism works; only the
sentinel is not recognised as a home.

Real pairing hits this on the first device that ever wrote a page of its own —
which is the case the feature is for.

## Root cause

`plan_reimport` (`crates/holon-loro/src/device_pairing_op.rs:337`) seeds
`placed` with `adopted.keys()` only, and gates each block on
`placed.contains(snap.block.parent_id.as_str())` (line 355). The stored root
sentinel is a legal parent everywhere else in the system — the owner's own
top-level pages carry it — but it is not a block id, so it is absent from
`adopted` and the guard rejects it. Blocks whose parent is the root sentinel
therefore fall into `plan.orphans` (line 367) unconditionally.

## Missing piece

`crates/holon-loro/tests/pairing_boot_degraded.rs` builds its unplaceable case
out of a journals subtree (`block:2026-09-05` under `block:journals`), i.e. a
parent that is a real block and merely missing. No fixture in the suite puts a
top-level page — the commonest thing a solo device writes — into the archive,
so the sentinel branch is never exercised. `two_instance_composed_pbt.rs`
cannot reach it either: `pair_with_owner` requires an empty receiver.

## Remedy

Open. Treat the root sentinel as always-placed when seeding `placed` in
`plan_reimport`, and pin it with a case in `pairing_boot_degraded.rs` whose
archive holds a top-level page — red first (the page is deferred), green after
(the page and its subtree are re-imported and the marker is removed).
