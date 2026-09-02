---
id: 2026-09-02-two-instance-binary-is-red-on-main-and-in-no-gate
date: 2026-09-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Two tests in the two-instance sharing binary fail on main — the receiver's org
  write-back never drains for blocks it received over the transport — and the
  binary is in no land-gate recipe, so nothing was watching.
---

## Bug

Found by lane `pair-inc0` (own-device pair, Increment 0) while running the
two-instance slice as a lane gate. Two of its five tests fail:

- `one_way_share_converges_on_the_receiver`
- the composed `two_instance_composed_pbt`

Both fail on the same assertion, from
`crates/holon-integration-tests/src/pbt/composed/invariants/two_instance_convergence.rs`:

```
the receiver's store converged but its ORG files are missing
[block:fe-blocked, block:fe-parent, block:fe-target] —
received state that never reaches disk is lost on restart
```

The receiver's SQL store has the shared blocks; its org files do not. The
assertion's own wording states the consequence: state that never reaches disk is
lost on the next boot.

## Root cause

Not established. Attribution IS established, and it is not this lane:

- The same two tests fail in `.claude/worktrees/main-baseline`, whose working
  copy is empty on `main` 50f878cc and which carries none of this lane's code
  (3 passed / 2 failed, lane log `baseline-two-instance.log`).
- Running only the five ORIGINAL tests in the lane tree, with the lane's four
  new tests filtered out, reproduces both failures (`gate-orig-only.log`).
- A/B on the one production-visible change the lane makes to that path: with
  `Capabilities::read_only()` restored in place of `read_write()`, the failures
  are identical (`gate-t2-readonly.log`).

Observed mechanism, not yet root-caused: each failing test PASSES when run alone
and fails inside the binary, so it is load- and order-sensitive. The receiver
logs the write-back being deferred rather than lost:

```
[FileSyncController] write-back SKIPPED: the holder's membership does not match
the authority's, so this render would project a partially-folded document over
disk. The diff that resolves it is already in flight and will re-trigger the
render.
doc=block:structural-page difference=block:c1@…,block:c2@…,block:parent@… held=0 authority=3
```

So the write-back is waiting for a membership fold that has not landed, and the
test's 10s converge budget expires first. Whether the fold eventually arrives
under no time pressure is not measured here.

## Missing piece

`binary(two_instance_composed_pbt)` appears in no `just` gate recipe. Not in
`gate-compile` (which only typechecks), not in `keystone-smoke`, not in
`loro-suite`, not in the land battery. The binary compiles on every workspace
check and runs in none, so both tests could go red on `main` and stay red with
nothing reporting it.

This is the same shape as the 25 un-gated `-p holon` integration reds recorded
on 2026-09-01: the escape is not a missing assertion — the assertion exists, is
precise, and is correct — it is that no gate executes it.

## Remedy

OPEN.

Two separable pieces:

1. **Gate it.** Put the two-instance binary in a land-gate recipe so the red is
   visible. Note the timing hazard first: the binary needs the 20-minute
   `slow-timeout` override this lane added to `.config/nextest.toml`, and the
   two failing tests are contention-sensitive, so a gate that runs them beside
   the rest of the suite will need a concurrency pin or a test-group.
2. **Fix the write-back.** Establish whether the deferred fold ever completes
   for imported blocks, or whether an imported block's membership authority is
   never satisfied on the receiving side.

The lane did not fix either: it owns the convergence question, and gating a
binary it found red is a decision for the integration owner, not a side effect
of an experiment.
