---
id: 2026-08-26-render-failure-invisible-warn-and-root-only-oracle
date: 2026-08-26
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A per-block render failure renders an error widget that no automated gate
  can see: the only invariant watching for error widgets walks the root
  layout tree alone, and the production render-failure site logs at warn.
---

## Bug

A `render_entity` failure inside a per-block live tree — the proven recipe is
a `live_query` whose materialized-view DDL the engine refuses — is **invisible
to every automated gate**. The user sees a red error box in place of the
block; the keystone PBT, the composed invariant catalog and the error-capture
oracles all stay green.

Found OUTSIDE an automated test, by adversarial verification during the D35
parity lane (verifier probe, not a running test): a query block whose DDL was
refused rendered an error widget, and `inv-viewmodel-no-error-widgets`
returned `Some(0)` on the same tick.

Two independent facts had to hold for the escape, and both did:

1. **Product.** `crates/holon/src/api/ui_watcher.rs:294` — on
   `render_entity` failure the watcher emits `error_render_expr(...)` (a
   legitimate, wanted, VISIBLE fallback) and logs the cause with
   `tracing::warn!`. A render failure is not a warning; at `warn` it is below
   every error-capture oracle's threshold, so `SpanCollector`-based gates
   never see it either.
2. **Oracle.** `inv-viewmodel-no-error-widgets`
   (`crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_no_error_widgets.rs`)
   counts error nodes through `SutViewSelection::headless_error_node_count`,
   whose only implementation of substance
   (`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1992`)
   watches `holon_api::root_layout_block_uri()` and interprets **that tree
   alone**. Error widgets living inside per-block live trees — exactly where
   a failed block render puts them — are outside its reach.

This is an ORACLE gap in the strict sense: the interaction is generatable and
was generated, the invariant existed and ran, and it reported `Ok`.

## Root cause

The invariant's reach was defined by a capability that snapshots one tree,
while the render pipeline produces a *forest*: the root layout plus one live
tree per watched block. `live_block` nodes inside the root snapshot are
references, not inlined subtrees, so a root-only walk terminates precisely
where per-block rendering — and per-block failure — begins.

The gap was already documented at a lower altitude but never closed. The
parity corpus grew a per-block `renders no error widget` STEP
(`Assertion::NoErrorWidget` → `no_error_widget_caps`,
`crates/holon-integration-tests/src/pbt/fixtures/assert.rs:330`) whose own
doc comment states the invariant "walks only from `root_layout_block_uri()`",
and `tests/fixtures/logseq-parity/queries.feature:78-82` carries the same
warning inline: that scenario is satisfiable by a rendered FAILURE, because
the error widget's message quotes the SQL that failed and so contains the
needle text the positive assertion looks for. A per-scenario STEP is opt-in;
the invariant is the thing that runs everywhere.

Collateral: `2026-08-22-backlinks-section-not-observable-headless.md:60`
reasons "`inv-viewmodel-no-error-widgets` is in the composed catalog and did
not fire, so an error widget is unlikely". That inference is REFUTED for
per-block trees — the invariant's silence was never evidence about them.

## Missing piece

No enumeration of per-block live trees in the invariant. The walk existed
elsewhere — `inv-editable-text-has-draggable`
(`.../invariants/bodies/editable_text_has_draggable.rs:70-84`) BFS-es
`live_block` references through `SutRenderer::widget_tree_for` — but
`inv-viewmodel-no-error-widgets` was never given the same reach, and the
render-failure log level kept the second, independent detection channel
(error capture) shut.

## Remedy

Both halves closed in this lane:

- `inv-viewmodel-no-error-widgets` now walks the whole rendered forest: the
  root count via `headless_error_node_count`, plus a BFS over per-block live
  trees via `SutRenderer::widget_tree_for`, reusing the enumeration
  `inv-editable-text-has-draggable` established. `Needs` gains
  `SutRenderer`; both real slices (frontend, window) already register it.
  Failure messages now quote the offending block id and the error widget's
  message, so a red names the failing render instead of a bare count.
- `ui_watcher.rs` render-failure site raised `tracing::warn!` →
  `tracing::error!`. The error widget stays — the fallback is wanted and
  disclosed; what changes is that the cause is now at a level error-capture
  oracles read.

Pinned by a teeth test that reds for the right reason before the widening:
an error widget planted in a per-block tree with a clean root, which the
old body reported as `Ok`.
