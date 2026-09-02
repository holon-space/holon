---
id: 2026-09-02-two-instance-binary-is-red-on-main-and-in-no-gate
date: 2026-09-02
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Two tests in the two-instance sharing binary fail on main because the settle
  returns while the receiver's org write-back is still mid-`home_by`-fold, and
  the binary is in no land-gate recipe, so nothing was watching.
---

## Bug

Found by lane `pair-inc0` (own-device pair, Increment 0) while running the
two-instance slice as a lane gate. Three of its ten tests fail:

- `one_way_share_converges_on_the_receiver`
- `one_way_share_converges_on_the_receiver_over_iroh`, its production-transport
  twin — so the red is transport-independent
- the composed `two_instance_composed_pbt` property

Both fail on the same assertion, from
`crates/holon-integration-tests/src/pbt/composed/invariants/two_instance_convergence.rs`
and its hand-written twin at
`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:270`:

```
the receiver's store converged but its ORG files are missing
[block:fe-blocked, block:fe-parent, block:fe-target] —
received state that never reaches disk is lost on restart
```

Reproduced on this lane's base (`main` 89e2efea) at 7 passed / 3 failed, twice,
by a fresh-context verifier on a reverted copy of the tree; and alone, in
`lane-logs/red-alone-traced-run1-2026-09-02.log`. The lane's own base run
(`lane-logs/red-full-run1-2026-09-02.log`) shows 8 passed / 2 failed — the iroh
twin happened to pass that once, which is what a race does.

## Root cause

A settle race, not a write-back defect. The receiver's write-back is working
correctly and has simply not run yet when the assertion reads disk.

The traced run puts the ordering beyond doubt
(`lane-logs/red-alone-traced-run1-2026-09-02.log`, receiver side):

| Time | Event |
|---|---|
| 17:48:01.683 | receiver applies the imported Loro update (12233 bytes) |
| 17:48:01.797 | `LoroProjection` finishes writing 12 ops to SQL (114ms) |
| ~17:48:01.80 | `converge_handle(receiver, 10s)` returns; the assertion panics |
| 17:48:01.875 | the write-back's first pass reaches `block:forward-edge-page` |
| 17:48:01.879 | and `block:structural-page` |

The two write-back log lines arrive AFTER the panic, and each reports a fold
that is progressing, not stalled: `held=1 authority=3` with the difference
already down to two ids. The 10s budget was never spent — `converge_signals`
returned at its first quiet floor, which is 25ms
(`crates/holon-integration-tests/src/test_environment.rs:2921`).

What the settle could not see is the org loop's own pass.
`OrgSyncIdleSignal::mark_progress` (`crates/holon-orgmode/src/di.rs`) advances
the tick only when a pass COMPLETES, and a write-back pass is several authority
reads plus disk. A pass that outlasts the 25ms floor is therefore
indistinguishable from an idle loop, and the settle concludes inside it. Adding
a flag held for the whole pass is what turned the test green.

Two smaller windows in the same chain were closed alongside it, both measured
rather than assumed. `converge_signals`
(`crates/holon-integration-tests/src/pbt/convergence.rs`) watched only Loro
catch-up, the emitted CDC watermark, the reactive apply epoch, and that tick.
Between the block-matview mirror receiving a batch and the org loop being handed
anything sits `LiveData::home_by`
(`crates/holon-api/src/live_data/home_by.rs`), whose fold is one authority read
per block and which reports nothing while it runs. And CDC EMISSION is not
arrival: a second traced run (`lane-logs/fix-attempt2-traced-2026-09-02.log`)
shows the mirror applying its batch at 17:54:57.2108, within a millisecond of
the projection completing, so the quiet window should restart there.

Why those three ids and not others: `fe-parent`/`fe-blocked`/`fe-target` are the
entire contents of one page (`forward-edge-page`), and the run that failed alone
was missing all six expected ids, `structural-page`'s three included. Which
documents happen to have landed by the time the panic fires is a function of
where the fold was, not of anything specific to the forward-edge corpus.

## Missing piece

Two, and they are independent.

1. **The org write-back reported completion, never occupancy.** Every settle
   signal in the chain said "something finished" and none said "something is
   running", so any stage slower than the quiet floor was invisible — the loop's
   own pass above all, and the `home_by` fold beneath it.
2. **`binary(two_instance_composed_pbt)` appears in no `just` gate recipe.** Not
   in `gate-compile` (which only typechecks), not in `keystone-smoke`, not in
   `loro-suite`, not in the land battery. The binary compiles on every workspace
   check and runs in none, so both tests could go red on `main` and stay red
   with nothing reporting it. Same shape as the 25 un-gated `-p holon`
   integration reds recorded on 2026-09-01.

## Remedy

**Settle (done).** The settle now watches occupancy, not just completion, in
three places:

- `OrgSyncIdleSignal::pass_in_flight` (`crates/holon-orgmode/src/di.rs`) — an
  RAII guard held for the whole of each loop pass, on both the file-change and
  the re-render arms. This is the one that flipped the test green.
- `HomeByProgress` (`crates/holon-api/src/live_data/home_by.rs`) — raised the
  moment a source burst is in hand, BEFORE the authority reads, and lowered only
  when the fold parks with nothing queued. The org container hands one handle to
  the supervised write-back stream (so it survives a restart) and publishes it
  as `OrgSyncIdleSignal::writeback_fold_in_flight`.
- a block-matview-mirror stage (`BlockFeed::consumed_seq` advancing), which
  restarts the quiet window at batch ARRIVAL rather than at CDC emission.

`converge_signals` counts each as activity, exactly as it already counts a
not-caught-up Loro.

The mirror stage is deliberately change-detected and NOT
`consumed_seq < cdc_emitted_watermark`: the emitted watermark counts every
commit, including ones whose CDC touched no `block` row, so the mirror trails it
permanently. A catch-up form was tried first and made the settle burn its full
budget and fail loud (`lane-logs/fix-attempt1-alone-2026-09-02.log`).

The binary is green after the change: three full runs, 10 passed / 3 skipped
each (`lane-logs/green-full-run1-2026-09-02.log` and runs 2 and 3 inside
`lane-logs/gates-2026-09-02.log`), against 7 passed / 3 failed on the same base
before it.

**Gate.** DEVELOPMENT.md now lists the binary as a per-weave gate row, with the
two timing facts a recipe has to respect. Writing the recipe itself is the
integration owner's step.
