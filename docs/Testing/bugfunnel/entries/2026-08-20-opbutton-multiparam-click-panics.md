---
id: 2026-08-20-opbutton-multiparam-click-panics
date: 2026-08-20
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Clicking the multi-param integration.set_field op_button panics instead of
  opening a param-collection popup, because no popup host exists at an op_button
  site and no test ever drove a multi-param op_button click.
---

## Bug

Found by Martin dogfooding the live GPUI app: clicking the `set_field`
op_button on an integration row in the Settings modal panics the app with

```
present_op(integration.set_field): multi-param popup activation is not yet wired
for op_button sites; 2 param(s) missing (follow-up to mobile-bar PR)
```

It is the tracked follow-up to the mobile-bar PR, which extracted the op_button
affordance but left multi-param activation unwired.

## Root cause

`present_op` dispatches an operation only when the caller's `ctx_params` already
satisfy every `required_param`; otherwise it takes a fail-loud `panic!` branch
(`crates/holon-frontend/src/reactive.rs:3757-3782`). The op_button click site
supplies only `{ id }` (`frontends/gpui/src/render/builders/op_button.rs:102-104`),
but `integration.set_field` declares three required params — `id` (String,
resolved from context), `field` (`OneOf ["enabled"]`), `value` (`Bool`)
(`crates/holon-app/src/integrations_operations.rs:63-81`). The two unresolved
params make `present_op` panic.

Two capabilities are absent, and both are needed:

- **No popup host at an op_button site.** The only param-collection UI in the app
  is bolted inside the text editor's slash menu (`EditorView`,
  `crates/holon-frontend/src/view_event_handler.rs:130-162`); an op_button has no
  editor and no caret to anchor to.
- **The existing param-collection machinery only collects `EntityId` params.**
  `CommandProvider::on_select` enters collection only when
  `entity_params_needed()` is non-empty and runs an async entity search
  (`crates/holon-frontend/src/command_provider.rs:474-501`); it has no path for
  `OneOf` or `Bool`, and it collects exactly one param then executes — never a
  sequence. `set_field` needs a `OneOf` and a `Bool`, in sequence.

`begin_oauth` was even kept single-param on purpose precisely because this seam
was missing (`crates/holon-app/src/integrations_operations.rs:119-121`).

## Missing piece

**No test drives a MULTI-param op_button dispatch.** The interaction is
generatable in the windowed harness — the `set_field` op_button paints for every
integration row (`ops_of` lists every guard-admitted operation, and `set_field`
carries no guard: `crates/holon-frontend/src/value_fns/ops_of.rs:77-112`) — but
the only windowed rung that clicks an integration op_button
(`frontends/gpui/tests/settings_integrations_ops_windowed.rs`) clicks the
single-param `begin_oauth` ("Configure…") button and never the multi-param
`set_field` one. So the crashing interaction was simply never generated: a
COVERAGE gap.

It is not ENVIRONMENT — the failing path runs in the windowed harness (the button
paints there). It is not ORACLE — a panic trips any rung that reaches it.

The keystone (headless) composed PBT structurally cannot express this: an
op_button is interpreted only under interactive services, and `present_op`
panics by design under headless/stub services (the op_button YAML branch is
gated on an interactive session; `crates/holon-frontend/src/reactive.rs:360-376`
and the stub panic at `:4438`). The covering test is therefore windowed; the
param-collection LOGIC is covered separately by headless unit tests.

## Remedy

New windowed GPUI rung
`frontends/gpui/tests/settings_integrations_setfield_popup_windowed.rs` — clicks
the `set_field` op_button, expects a param-collection popup, picks
`field=enabled` then a `value`, and asserts the enablement mirror flips.
Red-for-the-right-reason today (the click panics). The fix extracts the
param-collection state machine into an editor-independent core, extends it to
`OneOf`/`Bool` and multi-step sequencing, and hosts the overlay at the op_button
site. Fail-loud is preserved: param kinds the popup cannot collect still error
visibly.

**FIXED.** The param-collection state machine is
`crates/holon-frontend/src/param_collection.rs` (`ParamCollector`, extending to
`Bool`/`OneOf` and multi-step sequencing, headless-unit-tested). The op_button
opens an INLINE menu anchored beneath the button
(`frontends/gpui/src/render/builders/op_button.rs`) — not a floating overlay: a
first attempt used `gpui::deferred` + a full-screen backdrop, but in a tightly
packed row list the menu landed on the next row and the click fell through the
deferred layer to that row's button (proven by a diagnostic dump showing the
NEXT provider's popup opening). An in-flow menu hit-tests where it paints. The
terminal pick dispatches through `services.dispatch_intent` (same journal /
latency / failure-toast path a satisfied click uses).

Red-for-the-right-reason, then green
(`frontends/gpui/tests/settings_integrations_setfield_popup_windowed.rs`):

```
RED: thread '...' panicked at crates/holon-frontend/src/reactive.rs:3775:9:
     present_op(integration.set_field): multi-param popup activation is not yet
     wired for op_button sites; 2 param(s) missing (follow-up to mobile-bar PR)
  → surfaced as: clicking "op-button-set_field-integration:gcal" must open a
    param-collection popup, not panic.
  test result: FAILED. 0 passed; 1 failed.

GREEN: test clicking_multi_param_set_field_opens_param_popup_then_dispatches ... ok
       test result: ok. 1 passed; 0 failed.
```

The existing single-param rung
(`settings_integrations_ops_windowed.rs`, `begin_oauth`) still passes — no
regression.
