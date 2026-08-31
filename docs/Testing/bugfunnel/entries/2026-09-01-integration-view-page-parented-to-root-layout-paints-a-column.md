---
id: 2026-09-01-integration-view-page-parented-to-root-layout-paints-a-column
date: 2026-09-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `claude-history-view` was authored as a child of `root-layout`, and every
  rendered child of that block IS a layout column, so the app has been painting
  a phantom extra column since the integrations view landed.
---

## Bug

The seeded layout carried a column nobody asked for: `claude-history-view`, the
page `integration.open_default_view` focuses in the MAIN panel, was also being
laid out as a permanent column beside the sidebars and the main panel.

Found during D53.c (lane integ-views4) while authoring the four remaining
default views. Authoring them the same way — as `**` children of `* Holon
Layout` — took the layout from 4 columns to 8, and at that width the synthesizer
refused outright: `render_root_slot` failed with "root slot: synthesizing layout
for perspective block:root-layout", the window never left its loading state, and
two windowed rungs went red with nothing painted. The already-shipped
single-column case had been silently absorbed.

Not found by a test. The A/B that identified it was run by hand:
`lane-logs/d53c-gpui-BASE-A.log` (base assets, green) against
`lane-logs/d53c-gpui-EXP-widgetonly.log` (four views added, red), then
`lane-logs/d53c-bisect-{gcal,gmail,jsonplaceholder,todoist}.log` — each page
alone green, so the trigger is the panel COUNT, not any one page's SQL. Marking
the pages `:WIDGET_ONLY: t` did not help, which is what ruled out the sidebar
hypothesis and pointed at the perspective.

## Root cause

`PerspectiveSpec` builds the layout from the children of the perspective block,
and `PanelSpec::is_displayable` (crates/holon-api/src/perspective.rs:343) counts
a child as a column when it carries EITHER a query source OR a render:

```rust
pub fn is_displayable(&self) -> bool {
    self.source.is_some() || self.render.is_some()
}
```

`layout_dsl` (crates/holon-api/src/perspective.rs:275) then emits one
`live_block("<id>")` cell per displayable panel into `columns(...)` at each
breakpoint. A view page is a render block by construction, so authoring it under
`root-layout` makes it a column by the DSL's own semantics — there is no
property that opts out. `Advice Rules` sits under the same parent and is NOT a
column only because its single child is a `holon_advice_rule_yaml` source, which
is neither a source nor a render for this purpose.

## Missing piece

**No invariant reads the SEEDED layout's column set.** The keystone boots the
seeded layout on every case and renders it, so this is not a generation gap: the
state was reached constantly. Nothing asserts how many columns the layout
synthesized FROM THE SEED has, or which blocks they came from, so an extra one is
invisible to every headless rung — and the windowed rungs tolerated it because
each asserts its own element by id rather than the column inventory.

`layout_dsl_reproduces_bundled_default_shape`
(crates/holon-api/src/perspective.rs:653) does pin a column set, but over a
HAND-BUILT four-child fixture — it proves the synthesizer's arithmetic, not what
the shipped `index.org` actually seeds, so the two never met.

The escape is fully formalizable (`layout_dsl` is a pure function of the
perspective's children), which is what makes this ORACLE rather than PERCEPTION.

## Remedy

All five integration view pages are now top-level `*` headlines in
`assets/default/index.org` — siblings of `* Holon Layout`, not its children —
so they are pages the main panel focuses and never columns. Martin ratified the
placement, including moving the already-landed `claude-history-view`.

The locking rung is in
`crates/holon-app/tests/integrations_section_seed.rs`
(`every_bundled_integration_has_a_seeded_default_view`): for every bundled
sidecar it reads the `default_view` block's `parent_id` and reports a problem
when it is `block:root-layout`, naming the mechanism. Re-authoring a view page
under the layout now fails headlessly instead of shipping a column.

Green after: `lane-logs/d53c-gpui-FINAL.log` (5/5 windowed rungs),
`lane-logs/d53c-app-mcp-GREEN.log` (412 tests), `lane-logs/d53c-keystone-smoke.log`.

**Residual gap (open, not this lane's):** the SEEDED layout's column COUNT still
has no pin. The new rung forbids the one authoring mistake that caused this
escape, but any other route to a spurious or missing column — a `views:`-driven
panel, a perspective edit, a source child appearing on a layout block — stays
unobserved. A column-inventory invariant over the `layout_dsl` synthesized from
the real seed is the follow-up.

Two neighbouring escape classes the same lane left open, recorded here so they
are not rediscovered as new bugs:

- **The two NEW `default` profile_variants are unverified end to end.** Without
  them `render_entity()` resolves no profile and paints nothing, so
  `jsonplaceholder` and `todoist` needed one each. An entity type registers only
  on a REAL connect, and the bundled integrations are never connected in tests,
  so no rung can catch a typo inside those render strings. They are byte-shaped
  like the `gcal`/`gmail` variants that ship today and their columns were
  hand-checked against each entity's schema; that is the whole of the evidence.
- **The sweep does not check SELECTed column names.**
  `every_bundled_integration_has_a_seeded_default_view` asserts the table name and
  the collection wrapper, so a misspelled column in a view's SQL ships silently
  and only surfaces on first connect. All five views were hand-checked against
  their sidecar schemas on 2026-09-01 and are clean.
