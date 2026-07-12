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
step on top of that seam.

## Recommended next increment

Land #1 and #2 together in one change: op + both render-derivation arms calling
`resolve_active_perspective`, with a keystone case that seeds a second perspective,
fires `activate_perspective`, and asserts the main-panel rows switch. Restructuring
`index.org` is **not** required — root-layout stays the default perspective; a
named perspective is simply another block the pointer can select.
