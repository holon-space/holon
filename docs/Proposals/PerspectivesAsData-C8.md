# Perspectives / Layouts as Data (Vision Gap C8)

Status: **primitive landed, reactive render seam deferred** (2026-07-12).

## Goal

A named UI mode ("perspective") should be data, not code: a block declaring its
panels (each with a query), a profile override, and concealment parameters.
Switching modes swaps the active perspective and the reactive layout follows —
without restart. This is the last-ranked (minor) gap in
`VisionGapAnalysis-2026-07-11.md` §3 C8.

## What the substrate already gives us

The bundled default layout (`assets/default/index.org`) is a block
`block:root-layout` whose children are panels. Each panel is a heading block with
a query-source child (`holon_gql` / `holon_sql` / `holon_prql`) and optionally a
`render`-source child, plus layout-hint properties (`region`, `collapse_to`,
`ideal_width`, `column_priority`, `sequence`). Every frontend watches the fixed
`holon_api::root_layout_block_uri()` (`block:root-layout`) and re-renders when it
or its descendants change.

Crucially: **the default layout is already exactly the shape a perspective needs.**
A perspective and the default layout are the same block shape, so there is no
bespoke layout path to delete — the primitive parses both with one code path.

## What landed

`crates/holon-api/src/perspective.rs`:

- `PerspectiveSpec { id, name, panels, profile_override, concealment }`,
  `PanelSpec { id, region, sequence, source, render, ideal_width, collapse_to,
  column_priority }`, `PanelSource { language, query }`, `ConcealmentParams
  { hide_completed, hide_tags }`.
- `PerspectiveSpec::parse(perspective_id, blocks)` — the parse-don't-validate
  boundary. Perspective-level declaration fields are namespaced `perspective_*`
  (`perspective_name`, `perspective_profile`, `perspective_conceal_completed`,
  `perspective_conceal_tags`); an unrecognized `perspective_*` key or a malformed
  value is a **loud error**, never a dropped field. Non-namespaced properties stay
  generic block metadata. Panels are the perspective block's children; each
  panel's source/render come from its own source children — identical to the
  bundled layout.
- `ACTIVE_PERSPECTIVE_PROPERTY` (`"active_perspective"`): a pointer stored as a
  plain property on `block:root-layout`. Absent ⇒ root-layout is itself the active
  perspective (the default). Being a block property, it persists through Loro and
  survives restart exactly like collapse state — the persistence model the task
  called for.
- `set_active_perspective(root_layout, id)` — the state mutation behind the
  `activate_perspective` op.
- `active_perspective_id(...)` / `resolve_active_perspective(root_layout_id,
  blocks)` — follow the pointer and return the active `PerspectiveSpec`.

Unit tests cover panel/query parsing, loud failure on unknown and malformed
fields, concealment parsing, the default-to-root-layout resolution, switching via
the pointer (queries change), and pointer persistence as a block property.

## Deferred, and exactly why

Two pieces remain; both were deferred because they balloon past a "minor" gap.

### 1. `activate_perspective` as a wired entity op

Model it on `crates/holon/src/navigation/provider.rs` (`NavigationProvider` is the
same kind of thing — an op that mutates view state the layout reads). The op takes
a `perspective_id` and does one block-property write:
`set_active_perspective` on `block:root-layout` (via the block write path so it
lands in Loro). Register the provider in `crates/holon/src/di/registration.rs`
alongside the other `OperationProvider`s. This is straightforward; it was deferred
only because on its own it would set a property nothing renders yet (see #2) — a
silent no-op, which violates fail-loud. Land it **together with** #2.

### 2. Reactive render consumption (the real cost)

The blocker is that the 3-column render is derived in **two divergent arms**, and
neither is a simple interceptable "columns over children" default:

- **Turso arm** (desktop, keystone): `BlockDomain::render_entity`
  (`crates/holon/src/api/block_domain.rs:70`) derives the render via **profile
  resolution** — the `columns(item_template: live_block())` layout comes out of the
  profile/collection machinery, not a literal in this file.
- **No-Turso arm**: `loro_ui_watcher::derive_render_expr`
  (`crates/holon/src/api/loro_ui_watcher.rs:320`) renders a block with no
  query-source child as a bare `render_entity()` leaf — it does not itself produce
  the 3-column layout.

To make activation swap the live layout without restart, both derivation sites
must, when rendering `block:root-layout`, call
`perspective::resolve_active_perspective(root_layout_id, snapshot_blocks)` and
render **the resolved perspective's panels** as the columns instead of the
root-layout block's own children. Because the pointer lives on `block:root-layout`,
writing it re-fires the existing fixed-URI watch, so no frontend changes are
needed — but the change touches profile-driven render derivation on the Turso arm,
which is why it is a cross-cutting follow-up rather than part of this increment.

Consuming `ConcealmentParams` and `profile_override` in the render is a further
step on top of that seam — see "Relationship to the view-mode-switcher" for what
`profile_override` must drive.

## Relationship to the view-mode-switcher (two axes, one naming trap)

Perspectives and the existing **view-mode-switcher** are different altitudes of
the same render stack; they compose, they do not compete.

- **view-mode-switcher = intra-panel (how ONE result set is drawn).** Built by
  `BlockDomain::view_mode_switcher` (`crates/holon/src/api/block_domain.rs`):
  `render_entity` wraps a single block's query results in a switcher over the
  **collection variants resolved from that entity's profile** — tree / table /
  board — via `resolver.resolve_collection_variants()`, rendered by
  `crates/holon-frontend/src/shadow_builders/view_mode_switcher.rs` as
  `ViewKind::ViewModeSwitcher` (with a `pick_active_variant` fast-path for
  intra-variant switches). Scope: **one collection block** (a single panel's data
  source). The active variant is per-block UI state (rides `ui_generation` /
  `view_mode`), resolved at render — not a persisted, first-class declared mode.
  It answers: *"draw THIS list as a table or a board?"*
- **perspective = inter-panel (which panels, which queries, which profile).**
  `PerspectiveSpec` declares the whole multi-panel layout, persisted as data. It
  answers: *"which panels, showing which queries, under which profile and
  concealment?"*

A perspective is the **outer** container; a view-mode-switcher is **inner** — one
per collection panel. Activating a perspective changes which panels/queries
render; each collection panel *still* renders its own tree/table/board switcher
inside itself. Orthogonal axes: perspective = "which lists," view mode = "how each
list looks." Perspectives therefore do **not** subsume or replace the
view-mode-switcher — they sit above it.

**The bridge: `profile_override` ↔ `resolve_collection_variants`.** The switcher's
available variants and default mode come entirely from the resolved **profile**.
So `PerspectiveSpec.profile_override` is exactly the lever that steers the switcher
across all of a perspective's panels: a "Kanban perspective" is a perspective
whose profile makes collection panels default to `board`; a "Reading perspective"
defaults them to `tree`. The per-panel switcher still lets the user override on top.

**The naming trap to avoid.** "view **mode**" (tree/table/board, per list) and "UI
**mode** / perspective" (named app-wide layout) are two different axes that both
got called "mode." The vision's "three UI modes as adaptable perspectives" (C8)
maps to **perspectives**, not to the tree/table/board switcher; the switcher
already existed and covers only the intra-list display form. Keep the two words
distinct in code and docs (`view_mode` vs `perspective`).

**Render-seam requirement.** The deferred render-seam increment (#2 above) must
make `profile_override` actually drive the panels' resolved variants: when a
panel renders under an active perspective, its collection render must resolve
`resolve_collection_variants` through the perspective's `profile_override` (when
set) rather than only the panel entity's own profile — so switching perspective
re-points which variants/default view mode every collection panel offers, not just
which queries they run.

## Recommended next increment

Land #1 and #2 together in one change: op + both render-derivation arms calling
`resolve_active_perspective`, with a keystone case that seeds a second perspective,
fires `activate_perspective`, and asserts the main-panel rows switch. Restructuring
`index.org` is **not** required — root-layout stays the default perspective; a
named perspective is simply another block the pointer can select.

## RULING (Martin, 2026-07-13): display slots resolve via ordinary queries

Supersedes both the "two composing axes" framing above and the interim
"variant-selection primitive" idea. Ratified model:

- **A display slot's content is resolved by an ordinary query over ordinary
  data** — the same mechanism as the main panel resolving from
  `navigation.focus`. Perspectives and view modes are both just "which block
  does this slot show". No `activate_perspective` op, no variant-selection
  primitive; switching = ordinary `set_field` on whatever data the slot's
  query reads. This makes the active layout rule-drivable for free (clock
  rules, trust-gated proposals) with zero perspective-specific engine code.
- The landed `ACTIVE_PERSPECTIVE_PROPERTY` + `resolve_active_perspective`
  pair is the DEGENERATE case (pointer property = the data; resolve fn = a
  hardcoded query) and should be converged into slot-query resolution when
  the render-seam increment lands.
- **View modes become saved-view blocks** (Tana/Notion shape): a block
  declaring `{source, renderer, params}`; switching view mode = swapping
  which block the panel shows. The ViewModeSwitcher demotes to sugar that
  swaps/mints sibling view blocks over the same source. Typed parse at the
  render boundary (VariantSpec-as-parse-result) remains; profiles supply
  defaults when no explicit view block is chosen.

### Local, non-syncing view state (ruled direction, 2026-07-13)

For per-device view choices: a **separate local-only Turso table**
(`local_ui_state(scope_block_id, key, value)`) behind a `LocalStateStore`
abstraction. NEVER extra columns/rows on replicated block tables — those are
Loro projections under ADR 0025 (sink-truth diffing, reseeds, and tripwires
assume op-grounded content; un-grounded data there gets wiped or trips
conformance). Precedence is decided IN the slot query:
`COALESCE(local override, synced choice, profile default)` — so the default
semantics are "local override wins until cleared" (remote-wins expressible if
ever wanted). Loss of local state on a DB rebuild is consistent with the C2b
"Turso = disclosed ephemeral cache" doctrine — disclosed, not a hazard.

### Sequencing (wave-after-next, after the current land cycle)

1. Re-aim the deferred render-seam increment: both render arms
   (`BlockDomain::render_entity`, `loro_ui_watcher::derive_render_expr`)
   resolve slots via query (pointer-property degenerate case folds in).
2. `LocalStateStore` + `local_ui_state` + COALESCE precedence (small once 1
   exists).
3. Saved-view blocks + switcher-as-sugar; `view_mode` promoted from transient
   UI state to data (synced by default; local override via 2).

### Clarification (Martin Q, 2026-07-13): no query duplication across views

A saved-view block's `source` is a REFERENCE to the query-bearing block, never
a copy — the query lives once (on the collection block, as today) and all
sibling views point at it; editing the query updates every view. Renderer
needs (board group-by, table columns) are view `params` composing over the
shared source; view-local filter/sort refinements are later optional
COMPOSITION, never duplication (a view wanting a different query is a
different source). Views materialize LAZILY: zero view blocks in the default
case (profile-default renderer); the switcher mints one only on a non-default
choice — no block explosion.

### Clarification 2 (Martin Q, 2026-07-13): no view-block duplication across sources; applicability stays shape-universal

Three layers, and view blocks live only in the third:
1. **Variant menu = computed, never stored.** Renderers declare shape
   requirements; profiles resolve applicable variants per entity kind
   (`resolve_collection_variants`, unchanged). Every query block gets the
   full menu with zero per-source artifacts; a new renderer is instantly
   offered everywhere its shape fits.
2. **Plain choice = selection state, not a block.** "table, default params"
   is a property on the scope block (synced) or a `local_ui_state` row
   (local): value = renderer id. No view block minted.
3. **View blocks = source-INDEPENDENT view templates** `{renderer, params,
   shape requirement}` — named parameterizations ("Kanban by assignee"),
   defined once, applicable to every source whose shape fits; selection
   state pairs (scope → template ref). A source-bound saved view is the
   degenerate pairing. No duplication in either direction: queries are never
   copied into views, views are never copied per query.
