---
id: 2026-08-20-streaming-list-drops-named-props-on-item-column
date: 2026-08-20
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A `column` (or `row`) used as a streaming collection's TOP-LEVEL
  `item_template` renders with its named props dropped — `gap` is always 0 and a
  `min_height` authored on it never reaches the gpui builder. The same column
  authored one level deeper (inside a `render_entity` variant) keeps its props.
  So a per-row floor / gap on `list(item_template: column(#{...}, ...))` is
  silently a no-op.
---

## Bug
Restyling the Journals feed (LogSeq look), I authored the day-section floor as
`list(#{... item_template: column(#{min_height: 220}, render_entity(),
divider())})` in `assets/default/Journals.org`. The floor never applied — every
day section painted at its content height (~105px), never 220px. The heading
`style` and `creation_slot` on the SAME template survived; only the OUTER
column's named props vanished.

Found by agent exploration (this lane), not by Martin and not by any automated
test. Windowed measurement in
`frontends/gpui/tests/gpui_journals_logseq_look.rs`.

## Root cause
Instrumented `column::render` (gpui) and the shadow `column` registration:

- `frontends/gpui/src/render/builders/column.rs::render` IS called for the day
  item column (150 calls, `nchildren=2` matching `render_entity()`+`divider()`),
  but every call reads `min_height=None` and `gap=0`.
- `crates/holon-frontend/src/shadow_builders/mod.rs` `column` registration, when
  it runs, sees `named_keys=[]` — the `#{min_height: 220}` map never arrives as
  named args for the item-template column.

The streaming collection interprets each row's `item_template` per row via
`ReactiveView`'s `node_interpret_fn`
(`crates/holon-frontend/src/reactive_view.rs:1156`), which for a non-props-only
widget calls `svc.interpret(item_template, row_ctx)` and keeps
`fresh.props`. The named props on the TOP-LEVEL item column are lost on this
path; a column authored INSIDE a `render_entity` variant (interpreted by normal
recursion) keeps them — that is the workaround this feature shipped with
(`min_height` moved onto a column inside the `embedded_page_expanded` variant,
`block_profile.yaml`). The exact drop point in the streaming per-row
interpretation was not fully localized (out of scope for this feature lane).

## Missing piece
COVERAGE: no PBT asserts that named props on a collection's TOP-LEVEL
`item_template` container (e.g. `list(item_template: column(#{gap|min_height:
N}, ...))`) survive to the rendered node. The keystone drives collections but
never authors a styled item-template container and reads its props back.

## Remedy
- OPEN — NOT fixed here (feature shipped by relocating `min_height` onto a
  deeper column that keeps its props; see `block_profile.yaml`
  `embedded_page_expanded`). The general streaming-item-template prop-drop is a
  separate engine fix.
- A covering PBT should author `list(item_template: row(#{gap: G}, ...))` (or
  `column(#{min_height: N}, ...)`) and assert the rendered container carries the
  prop — red on the current tree, green after the streaming interpret preserves
  top-level item-template named props.
