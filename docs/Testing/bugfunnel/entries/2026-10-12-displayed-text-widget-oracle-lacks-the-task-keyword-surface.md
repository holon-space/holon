---
id: 2026-10-12-displayed-text-widget-oracle-lacks-the-task-keyword-surface
date: 2026-10-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The windowed composed keystone's inv-displayed-text/widget compared the focused editable
  surface, which shows the task keyword by design, against the bare content, so any focused
  block carrying a task_state failed the oracle although the product was right.
---

## Bug
Found by the g28-ab lane while attributing the windowed gate-28 signature. Reproduced
deterministically on main e5487b06 by a 1-transition replay (`NavigateFocus{Main,
block:decision-page}`, 12.6 s, profile `test`), and red twice on integration 8026acd2.

Observed text (run log excerpt, `/viewmodel` arm green):

```
[inv-displayed-text/widget] 1 text widget(s) show stale content at the widget/geometry layer. ...
editable_text@block=block:kd-1
    shown:    "? Which store holds the decision?"
    expected: "Which store holds the decision?"
```

## Root cause
The `editing` template (`assets/default/types/block_profile.yaml:218-221`, `condition:
is_focused`) paints `editable_text`. Its input shows the task-keyword source surface:
`editor_view.rs` `project_authority` -> `task_keyword.rs` `source_projection` returns
`"{keyword} {content}"`. `text_compare.rs` compared that with `==` against the bare
`content` column (`ref_caps/blocks.rs` `block_content`). The SUT was right; the oracle had
no surface projection for this arm. Reachable since the decision corpus (`62ae3c6572`).
Sibling, inverse direction: `2026-07-20-edit-mode-block-shows-stripped-link`.

## Missing piece
Two, in series:

1. `compare_text_to_ref` chose the expected text per BLOCK, so it could not say that one
   block's two surfaces differ: `rendered_text` shows the content column while
   `editable_text` shows the source projection.
2. `#[holon_macros::capmap_adapter]` skips default-bodied trait methods
   (`crates/holon-macros/src/capmap.rs:38-41`), so `impl RefBlockTree for CapMap` did NOT
   forward `editor_surface_text` to the live `ReferenceState` — every composed invariant
   silently read the trait's default body (the content column) instead of the oracle's
   projection. A bridged invariant can only read a REQUIRED cap method.

## Remedy
The `/widget` arm now expects `RefBlockTree::editor_surface_text` for an `editable_text`
widget — the reference's own `source_projection` call, the same production function the
renderer uses — and the content column for every other text widget. `editor_surface_text`
became a REQUIRED `RefBlockTree` method so the CapMap adapter forwards it; the refs that
model no task state answer with their content explicitly. The comparison stays `==`, so a
stale string that happens to carry a keyword prefix still fails.
`focused_task_block_shows_its_keyword_surface`
(`frontends/gpui/tests/gpui_composed_windowed_loop.rs`) pins it: cycle the focused block
through TODO/DOING/DONE/cleared, then navigate to `decision-page` for the `?` keyword.
No known-red row was ever registered for this signature, so none is removed.
