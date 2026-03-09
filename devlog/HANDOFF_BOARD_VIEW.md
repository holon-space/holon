# Board View — Handoff

Status: shipped through the design gallery; default `board_view` variant added to
the collection profile; per-instance `lane_field` plumbing in place.

**Update 2026-04-28** — follow-ups landed:
- **#1 drag persistence (full)** — GPUI `board.rs::render` wires both Sortable
  callbacks:
  - `on_insert` → `dispatch_lane_change` resolves the dropped row's profile,
    finds its `set_field` op, and dispatches `set_field(id, lane_field,
    target_lane_title)` so the lane field column persists across reload.
  - `on_reorder` and `on_insert` → `dispatch_sort_key_for_position` reads the
    post-update Sortable items list, computes a fractional-index key with
    `holon::storage::gen_key_between(prev, next)` from the moved card's
    immediate neighbors, and dispatches `set_field(id, "sort_key", key)` so
    within-lane order persists too. No-op when the row carries no `sort_key`
    column or when both neighbors are absent (single-card lane).
  - Cards with no persisted `id` (gallery demo / inline rows) are silently
    no-op'd.
- **#2 stable lane ordering** — `board(lane_order: [...])` arg + lex fallback
- **#3 empty-lane label fallback** — `lane_label_default`, defaults to "No status"
- **lane_field as board prop** — exposed at top level so GPUI can read it
- **row_id on cards** — shadow board attaches `row_id` from the row's `id` column
- **#6 state_accent value-fn** — sage/amber/coral/neutral palette; wired into
  `collection_profile.yaml` `board_view`
- **`lane_width` arg + horizontal scroll** — `board(lane_width: 320)` (or
  `lane_width: col(...)` / `lane_width: some_fn()`) overrides the default
  240px lane width. Resolved through `args.get_f64`, surfaced as a board
  prop, read by the GPUI render. The outer lane row is now a stateful
  `id`'d container with `overflow_x_scroll`, so wide boards (many lanes /
  wide lanes) scroll horizontally instead of clipping.
- **GPUI build is green** — the "pre-existing breakage" was already resolved
  upstream by the time this work landed; rust-analyzer just had stale cache
- **Tests:** 8 widget_gallery board tests + 2 state_accent tests + YAML profile
  parse test all pass; full GPUI build (`cargo build -p holon-gpui
  --all-targets`) green

Richer card extraction / double-render fix (#4) is still open. End-to-end
visual verification (drag a real block between lanes in a running app and
confirm sort_key/lane_field persist after reload) hasn't been done in this
session — the gallery demo cards have no persisted id, so the dispatch path
silently no-ops there. A real document with `view_mode == "board"` is the
next thing to point at.

## What's done

### Render expression

`board(item_template: card(...), lane_field: <string|col-ref>, rows: [...])` is
the canonical shape. Two evaluation paths:

1. **Data-driven** — `item_template` + (effective) `lane_field` present. Rows
   come from `rows: Value::Array(Value::Object)` if given (gallery demo),
   otherwise from `ctx.data_rows` (collection-profile path). Rows are grouped
   by `lane_field` value into `board_lane(title=lane_value, ...cards)` view
   models; cards are interpreted from `item_template` with each row bound.
2. **Static positional** — `board(board_lane(card(...), ...), ...)`. Children
   are interpreted as-is. Unused in default profiles; kept for direct
   composition.

### Files touched

- `crates/holon-frontend/src/shadow_builders/board.rs` — new builder with both
  paths. `lane_field` defaults to `"task_state"` when arg is null/missing, so
  the YAML can pass `col("lane_field")` and any collection without an explicit
  override falls through to the kanban default.
- `crates/holon-frontend/src/shadow_builders/board_lane.rs` — passthrough for
  the static path; positional children become `lane.children`.
- `frontends/gpui/src/render/builders/board.rs` — renderer; iterates lanes,
  wraps each in `Sortable<BoardCard>`, where `BoardCard` is plain `Clone` data
  (id + accent + `Vec<CardLine>`). `extract_lines` walks the card VM's direct
  children and pulls every `text` widget's `content`/`bold`/`size`/`color`
  props. Inlined visual treatment mirrors `card.rs` (tinted bg, accent left
  border, hover shadow, semibold/muted text).
- `crates/holon-frontend/src/widget_gallery.rs` — `board_mode_expr` builds
  `board(item_template: card(text(col("title"), #{bold: true}), text(col("summary"), #{size: 13.0, color: "muted"})), lane_field: "status", rows: [...])`
  with eight inline cards across "To Do", "In Progress", "Done".
- `assets/default/types/collection_profile.yaml` — `board_view` variant,
  priority 1, condition `view_mode == "board"`, render
  `board(#{lane_field: col("lane_field"), item_template: card(text(col("content"), #{bold: true}))})`.

### Tests

- `crates/holon-frontend/src/widget_gallery.rs::tests::board_mode_groups_cards_into_lanes`
  — interprets `board_mode_expr`, asserts the resulting tree is
  `board → board_lane(title=...) → card → text` and lane order is
  `["To Do", "In Progress", "Done"]`.
- `crates/holon/src/type_registry.rs::tests::default_registry_loads_block_and_collection_profiles`
  — already there, validates the YAML still parses with the new variant.

### Important gotchas (saved to memory)

- `LocalEntityScope` cache keys must be stable across re-renders — pointer-derived
  keys (`format!("...-{:p}", node)`) miss the cache every frame because parents
  rebuild their VM tree. Drag flicker traced to this. See
  `~/.claude/projects/-Users-martin-Workspaces-pkm-holon/memory/feedback_local_entity_cache_keys.md`.
- The board cache seed currently hashes `current_row.id` (when rendered inside a
  collection profile) plus a structural fingerprint of lane titles + child
  counts. Two boards with identical lane structure AND no `current_row` would
  still collide — once boards land in real docs, fold the parent block id from
  the live_block context into the seed.
- Sortable invokes `render_item` twice per frame for a dragging item (in-list
  ghost + floating drag preview, see `SortableDragData::render` in gpui-component).
  An earlier attempt to share an `Entity<CardView>` between both placements
  caused flicker; that's why `BoardCard` is plain data and visuals are inlined
  instead of dispatching through `card.rs`. Solving the double-render properly
  is a real follow-up (see below).

## Pre-existing breakage — RESOLVED 2026-04-28

The handoff originally flagged a wave of stale callers
(`EditorView::new` 13 args, `GpuiRenderContext::new` 9 args, etc.) that broke
`cargo build -p holon-gpui`. By the time the next session picked this up the
upstream refactor had already landed end-to-end; only rust-analyzer's diagnostic
cache was stale. `cargo check -p holon-gpui --all-targets` is currently green.

## Open follow-ups (in priority order)

### 1. Drag persistence — DONE 2026-04-28

Both Sortable callbacks are wired in
`frontends/gpui/src/render/builders/board.rs::render`:

- **Cross-lane (`on_insert`)** — `dispatch_lane_change` resolves
  `services.resolve_profile(&{id: row_id})`, finds the row's `set_field` op,
  and dispatches `OperationIntent::set_field(entity, "set_field", row_id,
  lane_field, target_lane_title)` so the lane-field column persists.
- **Within-lane (`on_reorder`)** + **destination position on cross-lane** —
  `dispatch_sort_key_for_position` reads the optimistically-updated
  Sortable items list, looks up the moved card's immediate neighbors, and
  computes a fractional-index key via
  `holon::storage::gen_key_between(prev, next)`. Dispatched as a
  `set_field(id, "sort_key", new_key)` intent.
- Inline cards without persisted id (gallery demo) silently no-op.
- Single-card lanes (no neighbors to bisect) skip sort_key update.

Plumbing: shadow board attaches `row_id` and `sort_key` as card-level props;
`extract_card` pulls both into `BoardCard`. `lane_field` is read once at the
top of `render` from the board-level `lane_field` prop.

### 2. Stable lane ordering — DONE 2026-04-28

`board(lane_order: ["To Do", "In Progress", "Done"])` is now honored by the
shadow builder. Lanes not in `lane_order` are appended in lexicographic order;
when `lane_order` is absent, every lane sorts lexicographically. Demo gallery
passes `lane_order` to keep the workflow visual order. Test:
`widget_gallery::tests::board_lane_order_default_is_lexicographic`.

For task_state specifically, the YAML profile could later thread the document's
`todo_keywords` in (e.g. via a `todo_states()` value-fn) — would let YAML do
`lane_order: todo_states()` without coupling shadow board to the org domain.

### 3. Empty-lane handling — DONE 2026-04-28

Rows with empty/missing `lane_field` value group under the
`lane_label_default` arg (defaults to `"No status"`). Test:
`widget_gallery::tests::board_empty_lane_value_uses_default_label`.

### 4. Richer card extraction (or: solve the entity-double-render problem)

`extract_lines` only pulls direct `text` children. Item templates that use
`render_entity()` (a column/row tree) render empty cards.

**Options:**
- (a) Walk the VM tree recursively for text content. Quick, ugly, doesn't pick
  up icons/badges/state_toggles inside the tree.
- (b) Solve the double-render so we can dispatch the card VM through the proper
  `card.rs` builder. The constraint: Sortable's `render_item` is `Fn(&T,
  usize, &Window, &App)` — only `&` refs, but `super::render` needs `&mut`.
  Rust now denies `&` → `&mut` casts as a hard error
  (`invalid_reference_casting` lint). Two real options:
  - Fork a `super::render`-equivalent that takes `&App` only (limits what
    builders can do — they all use `tc(ctx, ...)` which uses `&mut`).
  - Find a way to give Sortable an `Entity` that Render::render's twice
    correctly. Worth checking if rendering the same Entity at two element-tree
    paths actually fails in current GPUI — I assumed it does but didn't fully
    confirm. If it works, just store `Entity<CardView>` in `BoardCard`.

### 5. End-to-end usability of `lane_field` per-instance override

YAML reads `col("lane_field")` from the collection entity, but no profile yet
WRITES `lane_field` to a collection. So the override is plumbed but has no UX.

Either:
- Add a `lane_field` column to the collection entity's TypeDefinition with a
  default of `task_state`, plus a tiny editor (combo box of column names).
- Or accept that defaults-only is fine for now and revisit when there's a real
  need.

### 6. Card accent from state — DONE 2026-04-28

`state_accent(state_string)` value-fn lives in
`crates/holon-frontend/src/value_fns/state_accent.rs` and is registered in
`shadow_builders/mod.rs::build_shadow_interpreter`. Palette:

| State                                   | Hex      |
| --------------------------------------- | -------- |
| `DONE` / `COMPLETED` / `CLOSED`         | `#7D9D7D` (sage)  |
| `DOING` / `IN PROGRESS` / `NEXT` / `STARTED` | `#D4A373` (amber) |
| `BLOCKED` / `WAIT` / `WAITING` / `HOLD` | `#C97064` (coral) |
| empty / `TODO` / unknown                | `#5A5A55` (neutral) |

`collection_profile.yaml::board_view` now uses
`card(#{accent: state_accent(col("task_state"))}, ...)`. End-to-end test:
`widget_gallery::tests::board_state_accent_drives_card_accent_per_row`.

### 7. PBT coverage

`general_e2e_pbt.rs` doesn't generate `view_mode == "board"` collections. Per
`CLAUDE.md` no new PBT files — but the existing `WatchSpec` set could include
"board" as a render-source mutation target. Would surface lane-grouping bugs
the same way `tree`/`table` are exercised today.

## Quick verification when resuming

```
# Confirm the YAML still parses
cargo test -p holon --lib type_registry::tests::default_registry_loads_block_and_collection_profiles

# Confirm the board grouping logic
cargo test -p holon-frontend --lib widget_gallery::tests::board_mode_groups_cards_into_lanes

# Visual check (after the GPUI signature refactor is unblocked)
cargo run -p holon-gpui --example design_gallery
# → click the Board tab → drag cards within and across lanes
```

## Pointers

- Render expression types: `crates/holon-api/src/render_types.rs`
- Render eval (resolve_args, value-fn dispatch): `crates/holon-api/src/render_eval.rs`
- Shadow builder dispatcher: `crates/holon-frontend/src/render_interpreter.rs`
- Card builder (visual reference for inlined treatment in board.rs):
  `frontends/gpui/src/render/builders/card.rs`
- Sortable widget source (already cloned locally):
  `~/.cargo/git/checkouts/gpui-component-584ce4d78d9f7342/8cb8d01/crates/ui/src/sortable/sortable.rs`
- Memory note on the cache-key gotcha:
  `~/.claude/projects/-Users-martin-Workspaces-pkm-holon/memory/feedback_local_entity_cache_keys.md`
