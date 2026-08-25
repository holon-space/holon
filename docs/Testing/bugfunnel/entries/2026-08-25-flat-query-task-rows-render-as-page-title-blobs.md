---
id: 2026-08-25-flat-query-task-rows-render-as-page-title-blobs
date: 2026-08-25
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Every row of a flat live-query task list (the vault's Now list) renders as
  one bare h1 text blob — no TODO state toggle, no bullet — because the
  collection tree_view's level-0 rule stamps role page_title on every
  parentless result row.
---

## Bug

Martin dogfooded the vault's Now list (`Projects/Holon/Now.org`, source block
`now-query::src::0`, a `holon_sql` SELECT over task blocks) in the GPUI
frontend: each result task renders as ONE raw multi-line text widget (headline
and body concatenated), with no task-state chip and no bullet. `describe_ui`
shows `view_mode_switcher > tree > tree_item > text`. Verdict: "looks shitty,
but returns data." Found while driving Holon's own development from inside
Holon (the dogfood goal).

## Root cause

A query page without an explicit render spec resolves the collection profile's
`tree_view` variant (`assets/default/types/collection_profile.yaml:30`), whose
rule

```
#{when: eq("level", 0), override: #{role: "page_title", show_bullet: false, show_chevron: false}}
```

is written for a page's own subtree, where the single level-0 row IS the page
block and should render as the page title. A flat cross-vault query returns
rows whose parents are not in the result set, so the tree builder
(`crates/holon-frontend/src/render_interpreter.rs` `shared_tree_build`, and the
streaming driver in `crates/holon-frontend/src/reactive_view.rs`) makes EVERY
row a level-0 root. The rule then stamps `role: "page_title"` on each row, and
`pick_active_variant` matches the block profile's `page_title` variant
(`assets/default/types/block_profile.yaml:71-74`, render =
`text(col("content"), #{style: "h1"})`) — one bare text widget — instead of the
`default` variant that carries the bullet and
`state_toggle(col("task_state"))`.

Same failure family as
`2026-08-18-integrations-section-renders-one-of-four-rows`: the generic layer
silently accepts a rule/template that is only valid for one data shape and
degrades to look "fine".

## Missing piece

- COVERAGE: no keystone seed or transition produces a query page whose
  live-query results are flat, parentless task rows (the journals query page
  in the seed carries an explicit render spec and page-shaped rows), so the
  misfiring level-0 path over task rows is unreachable by generation.
- ORACLE (secondary): `inv-viewmodel-state-toggle-correct` verifies only the
  state toggles that EXIST in the snapshot; no invariant requires that a
  rendered row backed by a task block contains a state toggle at all, so even
  a case reaching this state would stay green.

## Remedy

Close both gaps red-first, then fix (Option A, ruled D19.a):

1. ORACLE side closed keystone-wide:
   `inv-viewmodel-task-rows-have-state-toggle`
   (`src/pbt/invariants/bodies/task_rows_have_state_toggle.rs`, wired in the
   composed catalog) — a rendered `tree_item` row whose ref block has
   non-empty task_state must contain a `state_toggle` in its own row scope,
   exempting focus roots (Main + Sidebar), Page blocks, and layout blocks.
2. COVERAGE side closed by a dedicated rung that reuses the keystone's
   component, snapshot IR, and the SAME core check:
   `tests/frontend_suite/now_query_task_rows_render_structured.rs` boots the
   headless production frontend over a Now-shaped two-file vault, focuses the
   query page via sidebar click, and red-for-the-right-reason'd on both task
   rows rendering without a toggle. Seeding the query page into the COMPOSED
   keystone is deliberately deferred: focusing a query page whose results are
   cross-document rows false-reds `inv-main-panel-rows-match-focus` (its
   set-equality oracle has no concept of query-page results — ref-known
   non-descendants of the focus root are indistinguishable from stale rows).
   Modeling query-page results in the reference is that invariant's own open
   work and the remaining piece of this coverage gap.
3. Fix: tree builders inject an `is_context_root` positional field (row id ==
   the tree's context/virtual-parent id) on both the eager and streaming
   paths; the `tree_view` page_title rule condition becomes
   `eq("is_context_root", 1)` so only a page's own root row gets the title
   treatment. The right sidebar's inline render (`assets/default/index.org`)
   keeps `eq("level", 0)` deliberately: its level-0 rows are pinned subtree
   heads that SHOULD render as headers.
