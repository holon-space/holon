---
id: 2026-08-25-flat-query-task-rows-render-as-page-title-blobs
date: 2026-08-25
gap: COVERAGE
secondary: ORACLE
status: PARTIAL
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

4. That last exemption left the class OPEN on every other path to the
   `page_title` role, which is why this entry is PARTIAL rather than FIXED. A
   landing gate shrank a 3-step keystone case — create a block, pin it to the
   right sidebar, cycle it to DOING — onto exactly it: the pinned head is a
   level-0 row, gets `role: "page_title"`, and the `page_title` block variant
   drew the content and nothing else, so pinning a task removed the affordance
   the pin exists to keep in view. Measured: replacing the rule's role string
   turns the case green (`lane-logs/ptr-probe1.log`), so the role → variant
   chain is the cause.

   Closed at the variant instead of at any one rule, so a task keeps its state
   control under EVERY path to the title role — pinned head, zoomed-in block,
   flat query row. `assets/default/types/block_profile.yaml` adds a computed
   `has_task_state` (`task_state` bound AND non-empty — `is_task` alone is not
   that test, since a bound empty string is `!= ()`) and ranks two title
   variants against it:

   - `page_title_task`, priority 3, condition
     `role == "page_title" && has_task_state`, render
     `row(#{gap: 2}, state_toggle(...), text(... h1))`.
   - `page_title`, priority 2, condition `role == "page_title"` — the bare
     role predicate, render UNCHANGED
     (`text(col("content"), #{style: "h1"})`). `pairing_conflict` moves 3 → 4
     to free the rank.

   The variants are ordered by RANK and NOT split on mutually exclusive
   predicates, and that is the whole load-bearing point. `task_state` is a
   property, never a declared column, so on a block that never was a task it
   is UNBOUND — and `eval_condition`
   (`crates/holon-api/src/entity_profile.rs`) treats a condition over an
   unbound undeclared column as a silent non-match, warning only for columns
   that ARE declared. A computed field inherits that. So the negated form
   `role == "page_title" && !has_task_state` does not read "not a task": it
   matches NOTHING on a never-task block, which then falls through to
   `embedded_page` or `default`/`editing` and loses its h1. Keeping the bare
   role predicate on the lower-ranked variant makes it the catch-all, so a
   never-task title matches it whatever `has_task_state` does.

   Pinned by two tests:

   - the keystone case `pinning-a-task-to-the-right-sidebar-keeps-its-state-toggle`
     (`hand-authored-regressions/keystone.jsonl`; red `lane-logs/ptr-red2.log`,
     green `lane-logs/ptr-green2.log`) — a pinned task keeps its toggle;
   - the rung
     `crates/holon-integration-tests/tests/frontend_suite/page_title_survives_a_never_task_block.rs`
     — a pinned head that never carried a task state renders as a BARE h1
     `text`: the h1 positively, and no `state_toggle`. The second half is the
     only test of the `has_task_state` gate itself; it cannot be a keystone
     invariant, because "a non-task row draws no state_toggle" is false in
     general — the `default` and `editing` outline variants draw one on every
     row. Arm table: `lane-logs/ptr-rung16-orig.log` (main's profile, green),
     `lane-logs/ptr-rung16-v2.log` (negated predicate, red: the head rendered
     `state_toggle` + `editable_text` and no h1),
     `lane-logs/ptr-rung16-v3.log` (this profile, green),
     `lane-logs/ptr-rung16-mut1.log` (`has_task_state` dropped from
     `page_title_task`, red: scope `["row", "state_toggle", "text"]` — the h1
     is there and so is a task control that does not belong).

   The rung reads the `style` keyword off the snapshot, which required
   carrying it: `ViewKind::Text` dropped `#{style: "h1"}` on the way from the
   builder props into the typed ViewModel, so no headless consumer could tell
   a title from body text. It now holds `style: Option<String>`, unresolved —
   the pixel scale is each platform's render-time resolution through
   `render_eval::text_style_font_size`. `frontends/dioxus-web` now resolves it
   the way GPUI does, so the browser frontend paints a title as a title
   wherever the role puts it rather than only where `index.html`'s positional
   document-title rule reaches.

   Two oracles moved with it:

   - `inv-viewmodel-task-rows-have-state-toggle` exempted Main and Sidebar
     focus roots because they "render as page-title headers by design". A
     header is a presentation and a task state is data, so the exemption is
     gone; only `Page` blocks and layout blocks remain exempt.
   - `inv-viewmodel-tree-virtual-slots` used "a state_toggle in the focus
     root's subtree" as its fingerprint for "the default variant fired instead
     of page_title". That proxy is false for a task focus root, which now
     draws one by design, so it discriminates on
     `rendered_text`/`editable_text` — the same variant set, since every
     variant that draws a toggle also draws one of those.
