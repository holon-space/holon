---
id: 2026-09-11-drawer-open-matches-ref-fires-in-the-reverse-direction
date: 2026-09-11
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The TUI windowed PBT reds on `inv-drawer-open-matches-ref` in the direction
  the known-red registry does NOT cover — the SUT paints the left sidebar
  CLOSED while the reference says open.
---

## Bug

Found by the `slot-birth` weave gate, `holon-tui::tui_ui_pbt`:

```
crates/holon-integration-tests/src/pbt/composed/harness.rs:1390
reconciled composed sequence diverged from the oracle:
[("inv-drawer-open-matches-ref",
  "[inv-drawer-open-matches-ref] drawer block:default-left-sidebar rendered
   open=false but reference says open=true")]
```

Gate log: `.jj/sw/check.log` (nextest detail in the run's
`w-slot-birth-nextest.38224.log`). The draw was a 7-transition windowed
alphabet run (`BulkExternalAdd`, `SetEdgeField`, `CreateBlockUnderFocus`,
`InstantiateTemplate`, `NavigateFocus`, `RenamePage`, `RehomeEntity`), failing
at step 0.

## Root cause

**Not established.** Two branches, and the evidence does not yet separate them:

1. the SUT genuinely paints the drawer closed when the reference model says it
   should be open — a product defect the invariant correctly caught; or
2. the reference models the drawer's open state wrongly for this draw — an
   oracle defect, which would make this a FALSE-ALARM.

The cause stays open rather than guessed: an unexplained failure stays open in
its class until it is root-caused. What IS established is **attribution**: the
signature is PRE-EXISTING, not this lane's. Population A/B, N=3 per tree,
serialized — base `main` `8c8c564d` 0/3 passing, `slot-birth` tip `3fb916f92311`
0/3 passing (verifier report, section `## weave-gate A/B`). One tip draw panicked
earlier instead, at `driver_input.rs:639` (a `click_entity` target absent from
the registry) — random-draw variance, not a second signature.

## Missing piece

The registry row for `drawer-open-matches-ref`
(`docs/Testing/KeystoneKnownReds.md`) covers ONE direction only — SUT-open /
ref-closed — and says so explicitly: "The direction is always SUT-open/ref-closed;
the reverse (`open=false` vs `open=true`) is NOT covered and must classify as
novel until triaged." This run is that reverse direction, so the classifier
reports it NOVEL and it blocks a gate read.

Nothing in this lane's diff touches drawer state: the lane's changes are the
in-process slot birth, the cell-registry install, one declaration flip and
docs. The TUI failure's context carries no registry-install error of any kind.

## Remedy

OPEN. The A/B has landed and attributed the signature as pre-existing, so it is
now registered as `drawer-open-matches-ref-reverse` in
`docs/Testing/KeystoneKnownReds.md` with that evidence — pass-with-note, which
unblocks the gate read without claiming either branch of the cause. The cause
itself is unowned and unfixed.
