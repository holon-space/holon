---
id: 2026-09-16-watching-a-nonexistent-block-renders-an-entity-not-an-error
date: 2026-09-16
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Watching a block that does not exist yields a `render_entity` expression where
  the error fallback is expected, so a missing block looks like an ordinary row
  instead of an error surface.
---

## Bug

`holon-integration-tests::store_suite watch_ui::watch_ui_error_recovery_on_nonexistent_block`
fails at `crates/holon-integration-tests/tests/store_suite/watch_ui.rs:191`:

```
    Expected error widget for missing block
      left: "render_entity"
     right: "error"
```

Found by the wave-14 land-gate triage
(`/tmp/holon-land-w14-1789580360/triage/TRIAGE.md` row 35), classified
PRE-EXISTING-ON-MAIN. Identical in both trees — A 1.9 s, B 1.9 s
(`runs/A1-int-nonwindowed.log`, `runs/B1-int-tests.log:284`) — against main
`be08291ccf1b`, so no lane caused it.

The test builds a vault with one `placeholder` block, waits for it to sync,
then watches `block:missing-block` through `watch_ui_first_structure` and
requires the returned render expression to be a `FunctionCall` named `error`.
The SUT returns `render_entity` — the fallback a normal, resolvable block
would get. The test exists precisely because the failure mode is quiet: a
missing block that renders as an ordinary row looks like an empty block, and
nothing downstream complains.

## Root cause

UNATTRIBUTED. The capture fixes the observable at the `struct`-watch seam, not
the mechanism, and two readings fit it:

1. the `ui_watcher` `error_render_expr` fallback regressed or was bypassed for
   the "block does not exist" case, so an unresolvable id takes the ordinary
   entity path; or
2. the fallback was changed deliberately and the test is stale.

Reading 2 is recorded here as live but NOT supported by the neighbouring
invariant. `inv-viewmodel-no-error-widgets`
(`crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_no_error_widgets.rs`)
asserts that no `Error` widget nodes exist in the rendered FOREST, and its doc
names `ui_watcher`'s `error_render_expr` fallback as the thing it reaches
through `SutRenderer::widget_tree_for`. That is the rendered tree, one layer
below the `RenderExpr` this test inspects, so the invariant does not require the
fallback to stop existing — but it does mean "error widgets are unwanted" is not
a safe summary of the product's intent and cannot be used to dismiss reading 1.

## Missing piece

The composed keystone never watches a block that does not exist. Neither
`missing-block` nor `watch_ui_first_structure` appears under
`crates/holon-integration-tests/src/pbt/`, so there is no generated case in
which an unresolvable id is watched, and no composed invariant that judges what
such a watch renders. The behaviour is asserted by one side binary, which is how
it stayed red and unowned.

The ORACLE secondary: "an unresolvable watched id surfaces an error rather than
an entity" is a fail-loud property of exactly the kind the project's error
philosophy makes first-class, and no invariant states it.

## Remedy

OPEN. The discriminator is a bisect of the fallback path — find where
`error_render_expr` is chosen and whether the "no such block" case still reaches
it, since that distinguishes a regression from a stale oracle in one step.

The gap-closing work is a composed transition that watches an id with no row
plus a composed invariant that judges the rendered shape, which would move this
from a single binary under the keystone.
