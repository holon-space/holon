---
id: 2026-09-14-the-windowed-composed-keystone-cannot-place-a-caret-for-splitblock
date: 2026-09-14
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The windowed composed keystone fails whenever a draw seats an Enter keystroke
  on a block the windowed driver renders without an editable surface, so the one
  composed test that runs in a real window stops at its driver rather than
  judging the product.
---

## Bug

Found by the `perception-fixes` verifier on 2026-09-14 while checking that
lane's gate coverage. `holon-gpui::gpui_composed_windowed_loop
general_e2e_composed_pbt_windowed` — the WINDOWED half of the one composed
keystone — fails with:

```
[SplitBlock/keystroke] cannot place the caret … editable surface not projected
by this driver
```

It is pre-existing. The verifier reverted all 24 of the lane's files to base
`be08291ccf1b` and got the same signature, so nothing in that lane elevates it.

Two things kept it invisible. It was never registered in
`docs/Testing/KeystoneKnownReds.md`, so no run could classify it; and the lane's
own windowed gate filtered to twelve named binaries, which excluded this one. A
broad five-crate run on the final tree
(`lane-logs/broad-final-1789410657.log`) is what surfaced it.

This matters more than an ordinary red. Martin's standing rule is that the
keystone PBT is a FULL test of all functionality, and the windowed keystone is
the only rung that composes the whole product inside a real window. While it
stops at a driver limitation, every draw that reaches a `SplitBlock` proves
nothing about the product at all.

## Missing piece

The failure is in the DRIVER, not the product: the message says the surface was
never projected, not that the edit was refused. Compare
`cooklang-read-only-split-block-refusal`, which has a similar shape and is the
opposite case — there a read-only home correctly refuses the write, and the
product behaved. Here the block is editable and the windowed driver has nothing
to type into.

Two hypotheses, neither ruled:

1. The windowed driver never mounts an editor for the drawn target — the block
   renders as a row but the editable surface is created lazily, by a focus or
   click path the composed windowed driver does not drive before it types.
2. The transition's precondition offers a target the WINDOWED projection cannot
   carry, the way `state-toggle-row-absent` offered click targets the main panel
   did not render. The headless keystone would then be right to draw it and the
   windowed one wrong to accept it.

Telling them apart needs one measurement: for a failing draw, ask whether the
target block registers any editable-surface bounds in the windowed registry at
all, before the keystroke is dispatched. Hypothesis 1 predicts it appears after
a focus step; hypothesis 2 predicts it never appears.

## Remedy

Not fixed here, and deliberately so — this lane was scoped to five perception
bugs and fixing a keystone driver on the way past would land an unreviewed
change in the rung everything else is judged by.

Registered as `windowed-splitblock-no-editable-surface` in
`docs/Testing/KeystoneKnownReds.md` so the signature classifies as
pass-with-note instead of blocking every gate read, with the A/B attribution and
this entry linked from the row. The registration is the stopgap; the row carries
no owner yet and should not be closed by absence from a run, because the draw
that seats a keystroke on such a block is probabilistic.

Whoever takes it should also widen the windowed gate filter. A binary excluded
from the filter is a rung nobody reads, which is how this stayed unregistered
while running red.
