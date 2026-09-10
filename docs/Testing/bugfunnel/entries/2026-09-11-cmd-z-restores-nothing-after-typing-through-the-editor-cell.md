---
id: 2026-09-11-cmd-z-restores-nothing-after-typing-through-the-editor-cell
date: 2026-09-11
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  With the editor-cell registry installed at GPUI start-up, typing commits
  through the Loro text cell and cmd+z restores nothing — the store keeps the
  typed suffix.
---

## Bug

Found by the `slot-birth` weave gate, then reproduced in isolation.

`frontends/gpui/tests/undo_survives_blur_windowed.rs:310` —

```
assertion `left == right` failed: cmd+z must restore the pre-edit content in the store
  left: "alpha twoQQ"
 right: "alpha two"
```

The test clicks a row, types `QQ` with real keystrokes, asserts the typed
suffix reached the store (it does), presses a real cmd+z, and finds the store
unchanged. Deterministic, alone, at this tree: `lane-logs/red-3.log` — the
`_loro` arm FAILED while `_sqlonly` and `_sqlonly_after_rebuild` both PASSED.

A second windowed test in the same family, `live_promotion_windowed`'s
`the_focused_row_paints_its_vault_syntax_and_cmd_z_walks_the_text_back`, HANGS
instead of asserting: it presses cmd+z twice, each followed by a 30 s settle
(`frontends/gpui/tests/live_promotion_windowed.rs:283-318`), so an undo that
never lands exhausts the budget. The weave gate killed it at its 120 s cap
(`TIMEOUT [120.043s]`); run alone in this workspace the binary was still alive
after 13m43s.

## Root cause

Not established in full, and deliberately not investigated further here — a
scout is establishing how undo is implemented (dispatcher journal vs Loro
`UndoManager`).

What IS established: the two legs commit through different funnels, and only
one of them is undoable.

- **No cell (SqlOnly):** the keystroke funnel dispatches `block.set_field`,
  which carries an inverse — undo has an entry to take back. Both `_sqlonly`
  arms pass.
- **Cell (Loro):** the per-keystroke write goes through the Loro text CRDT and
  no `set_field` is dispatched. The sibling suite states this as the design:
  "Under Loro the blur write is deliberately dropped (a per-keystroke cell
  writer already committed)"
  (`frontends/gpui/tests/task_keyword_blur_windowed.rs:349-352`).

## Missing piece

Nothing exercised the cell leg's undo behaviour, because until 2026-09-11
NOTHING ran the cell leg for keystrokes:

- the user-launched GPUI app never installed the editor-cell registry
  (`2026-09-10-user-launched-gpui-app-never-installed-the-editor-cell-registry`),
  so production typed through the on-blur `set_field` funnel;
- the windowed fixture installed none either, so the `_loro` arms of these
  suites were testing the same no-cell funnel their `_sqlonly` siblings test.

The oracle was never absent — this very assertion existed and passes on the
no-cell leg. The failing path did not run in either environment.

## Consequence

The `slot-birth` lane installs the registry at GPUI start-up, so the shipping
app now takes the cell leg for every keystroke. On that leg, as measured here,
**cmd+z after typing restores nothing**.

## Where the red lives now

D113.a keeps the GPUI app and the windowed fixtures on the no-cell leg, so
`undo_survives_blur_windowed`'s `_loro` arm runs GREEN today — it exercises the
on-blur `set_field` funnel, which carries an inverse. **It becomes the
`cell-undo` lane's red the moment that lane flips the fixture default**, which
is why nothing about the test was changed: the assertion that caught this is the
one that will prove the fix.

## Remedy

OPEN — no fix is claimed and no test was weakened. Escalated to Martin as a
product decision: whether undo must cover per-keystroke cell writes (and by
which mechanism), or whether the cell leg's undo is expected to behave
differently. Disabling the registry in the tests to restore green is
explicitly NOT the remedy: it would hide the behaviour users would get.
