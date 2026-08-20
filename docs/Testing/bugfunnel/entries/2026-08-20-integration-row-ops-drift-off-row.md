---
id: 2026-08-20-integration-row-ops-drift-off-row
date: 2026-08-20
gap: ORACLE
secondary: PERCEPTION
status: FIXED
summary: >-
  Settings → Integrations op_buttons drift vertically off their row's baseline
  and sit in a far-right column, so a click aimed at one provider's op can land
  on another provider's — no windowed invariant judged op-button/row alignment.
---

## Bug

Found by the `dogfood-explorer` agent driving the live GPUI app (2026-08-20,
two rounds; screenshots under `/tmp/dogfood-2026-08-20-logs/shots/` and
`shots-round2/`). In Settings → Integrations each row's op_buttons sit in a
far-right column separated from the row label by a wide empty gap, and the
buttons drift vertically off the row's text baseline — the drift growing down
the list. The agent, clicking where gmail's `set_field` toggle visually
appeared, twice dispatched on the WRONG provider: its log shows
`set_field id=integration:jsonplaceholder value=false` when it intended gmail.
The op_buttons are not spatially bound to the row they act on.

## Root cause

The op column is a nested collection (`list(ops_of(col("id")))`) inside the
integration row template
(`assets/default/types/integration_profile.yaml:21`). As a `Nested`-placement
streaming collection it renders through `column::eager_collection_div`
(`frontends/gpui/src/render/builders/column.rs:192`), which stacks its items
`flex_col().w_full()`. Each `op_button` is an icon-over-label stack ~38px tall
(`frontends/gpui/src/render/builders/op_button.rs:75-99`). So a row that offers
more than one op (gcal/gmail carry `set_field` + `begin_oauth`) gets an op
column ~76px tall — twice the single-line label — and the outer row's
`align: center` centres that tall column against the label, leaving each button
19px off the label baseline. `w_full` also stretched each button across the
whole right gutter (measured width 348px), which is the "wide empty gap."

Measured in the windowed harness before the fix (button centre_y vs. row label
baseline):

```
provider          drift_dy   op-button w,h
gcal               -19.0px    348 x 38   (two ops stacked → row 76px tall)
gmail              -16.0px    341 x 38
claude-history      +0.0px    282 x 38   (single op → already centred)
```

Only multi-op rows drift; the down-drifting button of one row creeps toward the
next row's band, which is the mis-click hazard.

## Missing piece

**No invariant judged op-button/row alignment.** The interaction is fully
generatable and the geometry was fully available: the two existing windowed
rungs (`settings_integrations_ops_windowed.rs`,
`settings_integrations_setfield_popup_windowed.rs`) open the modal and every
op_button and row `state_toggle` is registered in the `BoundsRegistry` with
its rect and its row entity (`vm_node.entity == "integration:<provider>"`).
But every rung clicks a button at its *registered centre* (`center_of`), so
the driver structurally cannot mis-click regardless of where the button
visually sits — and no rung asserted that a button's rect lies on its own
row's line. The defect state is reachable and an invariant on it fires, so
this is an **ORACLE** gap, not COVERAGE (the state is generatable) and not
ENVIRONMENT (the failing layout renders in the windowed harness).

Secondary **PERCEPTION**: the ultimate symptom is "the button does not read as
belonging to its row," a visual-association judgement — but its cause (vertical
drift, column pitch) is measurable, so the oracle is expressible and this is
recorded ORACLE-primary.

The keystone (headless) composed PBT cannot express it: op_button layout is
interpreted only under interactive services and there is no headless notion of
rendered rects. The covering test is windowed.

## Remedy

New windowed GPUI rung
`frontends/gpui/tests/settings_integrations_row_op_alignment_windowed.rs` —
opens Settings and, for every on-screen provider row, joins the `set_field`
op_button to its row's leftmost label element (via `vm_node.entity ==
"integration:<p>"`) and asserts (1) the button's vertical centre tracks its
row's label baseline within 6px, and (2) the row whose baseline is nearest each
button is the button's OWN row — the geometric form of "this click cannot land
on another provider."

The fix makes the op collection lay its items along a row instead of stacking
them: a `horizontal` flag on the `list` layout, threaded from the render DSL
(`list(#{..., horizontal: true})`) through `CollectionVariant`
(`crates/holon-frontend/src/reactive_view_model.rs`) into
`eager_collection_div`, which renders `flex_row().items_center()` at content
width when set (`frontends/gpui/src/render/builders/column.rs:192`). The
integration row template sets it
(`assets/default/types/integration_profile.yaml:21`). Multi-op rows are now the
same height as the label and every button sits on the row's line; the default
(false) keeps every other collection — sidebar page tree, outline — stacked.

Red-for-the-right-reason, then green
(`settings_integrations_row_op_alignment_windowed.rs`):

```
RED:  gcal's set_field op_button drifts 19.0px off its row's label baseline
      (button centre_y 798.0, label baseline 817.0).
      test result: FAILED. 0 passed; 1 failed.

GREEN (post-fix geometry): every row drift_dy 0.0px, op-button 96px wide,
      op column 38px tall (ops side by side), nearest_row == own row:
        gcal            +0.0    gmail  +0.0    claude-history  +0.0
        jsonplaceholder +0.0    todoist -3.0 (bottom-edge-clipped 7px row)
      test result: ok. 1 passed; 0 failed.
```

The two existing windowed rung files stay green after the bounds shift:
`settings_integrations_ops_windowed` (1 passed) and
`settings_integrations_setfield_popup_windowed` (4 passed). **FIXED.**
