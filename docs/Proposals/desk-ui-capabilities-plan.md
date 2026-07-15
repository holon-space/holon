# Implementation plan: desk UI capabilities

Companion to [ADR 0026](../adr/0026-attention-environment-architecture.md)
slices 1, 4, and 5 (desk visual prototype, desk UI v1, zoom/chrome). This plan
decomposes the **frontend-stack capability gaps** that block the Attention
Environment desk and similar rich spatial UIs — not the data model (ADR 0026
§1–§5 is ordinary blocks) and not the focus-rekeying gate (ADR 0016, referenced).

**Scope: plan only.** No implementation here. Each phase names its riskiest
assumption and a de-risking spike. Phases are risk-first.

## Status (2026-07-15)

**OPEN (designed, senior-reviewed 2026-07-14, not implemented).** All 8 phases are
open. The plan was code-grounded and reviewed; it corrects two wrong gap claims and
resolves three design questions against the current codebase. No rendering, spatial
layout, or theme code has been written for the desk.

Still open:
- All 8 phases; P1-P4 are the critical path for desk v1
- P2/P6/P8 are desk-independent and could ship first

## Code-grounded corrections to the assumed gap list

The assignment listed eight gaps; two were wrong versus the code, and the plan
reflects the corrected state:

- **Drag-and-drop is NOT "none anywhere."** `draggable` + `drop_zone` +
  `board`/`board_lane` builders exist (`crates/holon-frontend/src/shadow_builders/{draggable,drop_zone,board,board_lane}.rs`),
  dispatch `move_block`, and are wired in **both GPUI**
  (`frontends/gpui/src/render/builders/{board,draggable,drop_zone}.rs`,
  `frontends/gpui/src/render/drag.rs`) **and dioxus-web**
  (`frontends/dioxus-web/src/dnd.rs`, `render/builders/{draggable,drop_zone}.rs`).
  The real gap is that DnD is **lane-reorder only** (gpui-component
  `SortableState` per lane) — no spatial cross-zone drop with capacity
  enforcement, no origin-aware ghost/fixtures. Reframed as Phase 4.
- **`board` is lane-kanban, not spatial.** `LANE_WIDTH_PX`/`CARD_BG` are
  hardcoded constants; lanes are a horizontal strip, cards sorted within a
  lane. There is no `surface` widget that places children by position data
  (zone + ordinal, later x/y). Confirms the spatial-container gap (Phase 1).
- **`ThemeColors`** (`crates/holon-frontend/src/theme.rs:7-22`) is a flat
  16-field `Rgba8` struct with `default_dark()`; no token ramp, no
  state-conditional variant layer. `design_gallery.rs` hardcodes `BG`/`SURFACE`
  hex literals. Confirms the token gap (Phase 2).

## Senior review 2026-07-14 — validated answers + corrections

The plan's three open questions were code-validated; two design flaws fixed:

1. **`move_block` carries `parent_id` positionally — CONFIRMED.**
   `crates/holon-api/src/block_write_field.rs:90`: "express the move
   positionally via `move_block {{ id, parent_id, after_block_id }}`." P4's
   reuse bet is sound; the spike only checks the anchor's optionality
   (append-at-end when `after_block_id` absent).
2. **No typed-`Token` `Value` variant exists — P2 reworded.** `Value`
   (`crates/holon-api/src/lib.rs:212-226`) has no token variant and is
   flutter_rust_bridge-constrained (DateTime/Json already ship as strings).
   Design: token NAME as `Value::String` on the wire, parsed ONCE at the
   builder boundary into a typed `Token` enum — parse-don't-validate at the
   boundary, no scattered string matching. A `Value::Token` variant is
   rejected (FRB + serialization blast radius).
3. **No prior-rect delta exists — P5 committed to registry-diff.**
   `ElementInfo` (`crates/holon-frontend/src/geometry.rs:14`) is a
   current-state bounds registry persisted between frames; the animator
   captures "prior" from the registry before re-render overwrites it. The
   hoped-for pipeline-emitted spatial delta does not exist and is not planned.
4. **P7 contradiction fixed.** "Written through ops (undoable)" and "zoom is
   UI state, data unchanged" were both claimed. Resolution: v1
   `current_level` is per-frontend UI state on `UiState` (the `focused_block`
   pattern) — NOT an engine op, not synced, not undoable. Ops/undo/cross-
   device zoom deferred until a real requirement exists.
5. **P6 real risk named.** No `element_mount_id` exists; local state lives on
   the `ReactiveViewModel` node (existing Mutable pattern). The gallery's
   `view_cache` comment documents that re-interpretation REBUILDS nodes and
   resets Mutables — the spike must prove survival across re-interpretation,
   not just per-mount independence.
6. **P1 additions.** (a) Surface/zone position data must round-trip through
   the serialized worker envelope for dioxus-web — a protocol revision (ADR
   0016 lesson), added to the gate. (b) Time-axis `direction` is a surface
   field from day one (ideation ratified locale-flippable direction).
7. **P4 note.** Spike reads `capacity` from expression literals; production
   reads zone-block properties — a named dependency at the data-model
   track-merge (slice 4), not a silent assumption.

## Phase overview

| Ph | Capability | Size | Riskiest assumption | Spike |
|----|-----------|------|---------------------|-------|
| 1 | Spatial `surface` + zones-as-data | L | GPUI can clip+bound a container whose children carry position data, without per-child layout code | Render one 3-zone surface in GPUI with clip + non-overlap assert |
| 2 | Design tokens + state-conditional style | M | A token layer can feed all 4 frontends without a CSS/string boundary | Token struct → gpui/dioxus/tui/worker each resolve; headless assert |
| 3 | Z-axis / layering primitives | M | Overlays + drag ghosts can be element-tree siblings, not OS windows | One overlay + one ghost on a surface; `describe_ui` z-order |
| 4 | Spatial DnD (extend existing) | M | `move_block` intent can carry a target *zone* (parent_id) + ordinal, reusing the shipped intent path | Drop card zone→zone; assert `move_block` parent_id changes |
| 5 | State-transition animation | M | CDC delta can drive an animation without a timer (event-time) | Settle a card after a CDC update; no `Instant::now` in the path |
| 6 | Local widget-state bindings in expressions | S | A render-expression can read a per-mount `Mutable<bool>` without threading it through `BuilderServices` | focus-settle toggle expressed, not hand-wired |
| 7 | Zoom/chrome state machine | M | Chrome retraction is a pure projection of `current_level` (no separate mode store) | 3 levels, sidebar retracts on level flag; assert no mode field |
| 8 | Garnish: glow/gradient/typography/icon-chip/instrument seam | S–M | GPUI has gradient/box-shadow (else layered-rect fallback) | one radial glow; fallback visible if absent |

Phases 1–4 are the desk's hard dependencies. Phase 7 is ADR 0026 slice 5.
Phases 2, 6, 8 are **desk-independent wins** (tokens help boards/dashboards
today; local state unblocks several hand-wired toggles; garnish is polish).
Phase 3 and 5 are desk-coupled but independently shippable.

---

## Phase 1 — Spatial `surface` primitive + zones-as-data (L)

**ADR 0026 mapping:** slices 1, 4 (desk visual prototype, desk UI v1). This is
the biggest gap and the reason `design_gallery.rs` bypassed render expressions
(its `// hardcoded layout` comment, line 439).

**Riskiest assumption:** GPUI can host a bounded, clipping container whose
children are placed by **position data** (zone id + ordinal now; x/y later per
ADR 0026 §2 v2) without per-child layout code in the renderer. Today every
container builder (`columns`, `list`, `board`) lays out children by flex/sort
order, not by zone assignment.

**Spike (throwaway, GPUI only):** render one `surface` with three `zone`
children (`center`, `shore`, `wake`), each holding two cards placed by
ordinal. Assert via `describe_ui` that (a) the surface bounds equal its
declared size, not the sum of children; (b) children clipped at the surface
edge; (c) no two cards overlap. No data model, no ops — pure layout proof.

### Design

A `surface` widget is a **bounded, non-scrolling container**. Children are
`zone` sub-containers; cards are children of zones. Position is data:

```
surface {
  zones: [
    zone(role: center,  axis: horizontal, capacity: 5) { ...cards... }
    zone(role: arrival, axis: vertical,   capacity: 3) { ...cards... }
  ]
}
```

- `surface` clips (`overflow: hidden`); it never scrolls. Zone-internal scroll
  is a separate concern (Phase 8 edge-fade) — a zone MAY scroll, the surface
  MAY NOT.
- `zone` `role` is a **boundary-parsed closed enum** (`Center | Arrival | Exit
  | Free | Fixture`) following the `TaskState` open-label + closed-role pattern
  referenced in ADR 0026 §2. Logic branches on role, never the display name.
- `capacity` is data on the zone; **the engine never enforces it** (never-moves-
  your-stuff invariant). The UI is the forcing function (Phase 4 DnD refuses
  over-capacity drops visibly). `describe_ui` exposes `capacity` and current
  count so an agent can assert it.
- Ordinal within a zone = the card's consolidator-minted `sort_key` (ADR 0026
  §1). The renderer reads it as data; reordering is `move_block`, not a layout
  recalculation. v2 freeform `x,y` are additive zone-child properties the
  zones-first renderer ignores until a later phase — no schema break.

### Expression-syntax sketch (capability 1)

Consistent with the `board(...) { board_lane(...) { ... } }` nesting style
(`shadow_builders/board.rs`, `board_lane.rs`):

```
surface(bounded: true) {
  zone(role: "center", axis: "horizontal", capacity: 5) {
    row ...  // cards = children, ordered by sort_key
  }
  zone(role: "arrival", capacity: 3) { row ... }
}
```

`bounded` defaults true (the whole point). `axis` defaults to the zone role's
conventional axis. `surface` also carries `direction: past_left | past_right`
(default by locale; the ideation ratified a flippable time axis) — zone order
and gradient direction derive from it; hard-coding left=past is forbidden. Children of a zone are interpreted as cards and placed by
ordinal; the zone never reflows past `capacity` (overflow cards are a visible
"full" state, not a scroll — distinct from zone-internal card-list scroll).

### Gate

- **Build/tests:** `cargo nextest run -p holon-frontend` green; new
  `surface_layout_tests` (bounded, clip, non-overlap, capacity-exposed).
- **Existing PBT invariants green:** `inv-viewmodel-entity-ids-subset-of-data`,
  `inv-main-panel-rows-match-focus` (the desk panel uses its own
  `ReactiveRowProvider` per ADR 0026 §1 row-key safety — surface does not
  change that contract).
- **New invariant `inv-surface-bounded-non-overlap`:** every `surface`'s
  reported bounds (via `ElementInfo`, `geometry.rs:14`) are ≤ declared size,
  and no two zone-children share overlapping screen rects. Exposed through
  `describe_ui` (MCP) so agents assert bounded + non-overlap + capacity.
- **Cross-frontend:** GPUI full; dioxus-web full (CSS `position: absolute`
  inside a `position: relative` clip container) — NOTE dioxus-web renders from
  the serialized worker snapshot, so zone/position/capacity data must
  round-trip through the `WatchEnvelope`; treat as a protocol revision with a
  round-trip test (ADR 0016 §7 lesson), part of this phase's gate; tui renders
  zones as labeled columns with a `[FULL n/c]` marker (visible, not faked);
  worker serializes the surface tree faithfully (see NOTE — it is the
  dioxus-web carrier, not N/A).

**De-risks:** whether `ElementInfo` bounds are accurate enough for the
non-overlap assertion (the spike's `describe_ui` check IS this de-risk).

**Desk-independent win:** no — surfaces are spatial-only. But `zone` as a
data-driven sub-container with parsed role generalizes to dashboards.

---

## Phase 2 — Design tokens + state-conditional styling (M)

**ADR 0026 mapping:** slice 1 (visual prototype fidelity). The reference image
(Holon Desk Dark) shows a state-inverted theme: a focused card flips to
ivory-on-dark. `ThemeColors` (`theme.rs:7-22`) is a flat 16-field `Rgba8`
struct with `default_dark()`; `design_gallery.rs` hardcodes `BG`/`SURFACE` hex.
There is no token ramp, no semantic layer, no state-conditional variant.

**Riskiest assumption:** a single token definition can feed all four frontends
(gpui `Hsla`, dioxus CSS vars, tui `Style`, worker/`describe_ui` text) without
a string boundary that loses type safety (parse-don't-validate: tokens are an
enum/newtype at the boundary, not a `String`).

**Spike (throwaway):** define a `Token` enum (`Surface(elev: u8)` |
`Text(role: TextRole)` | `Accent(state: CardState)` | `Border(state)`) and a
`TokenSet`. Resolve it in gpui and tui only; assert the headless interpreter
can report token names through `describe_ui`. If a frontend must lose
information, it must do so **visibly** (tui ignores `elev`; worker reports the
name).

### Design

- **Token ramp, not flat struct.** Replace the 16 flat `Rgba8` fields with a
  `TokenSet` of named semantic tokens. `ThemeColors` becomes one *resolution*
  of the tokens, not the source. Existing call sites migrate incrementally;
  the flat struct can stay as a compatibility shim during the phase.
- **State-conditional styling = a function of data.** `Accent(state: CardState)`
  where `CardState ∈ { Idle, Focused, Arrived, DueSoon, Pinned }`. The
  reference image's focused-card inversion is `Accent(Focused)` resolving to
  ivory-on-dark in the dark theme. The render expression picks the state from
  row data; the token resolves it per theme. **No per-frontend styling logic.**
- **Full theme variants** are just two `TokenSet` resolutions (dark, light) +
  the state-conditional axis. The "inverted" look is `Accent(Focused)` in the
  dark set, not a separate theme.
- **Patina / opacity** (ADR 0026 context: "time-critical fades in") is a token
  channel: `Opacity(state: CardState) -> f32`. `DueSoon` resolves to 1.0;
  `Idle` in a focus-active desk resolves to a dimmed value — driven by the
  same `CardState`, not a separate dim feature.

### Expression-syntax sketch (capability 3)

```
card(accent: accent_of(this.state), patina: opacity_of(this.state)) {
  text this.title { color: text_of(this.state) }
}
```

`accent_of` / `opacity_of` / `text_of` are value-fns (`ValueFn` trait,
`render_interpreter.rs:71`) that read a row field and emit the token NAME as a
`Value::String` (senior-review resolution 2: `Value` has no token variant and
is FRB-constrained, `holon-api/src/lib.rs:212`). The builder parses the name
ONCE at its boundary into the typed `Token` enum — unknown name = loud error
(`inv-tokens-resolve-loudly`), never a silent default — then resolves through
the active `TokenSet`. Parse-don't-validate holds at the boundary; no
scattered token-name matching past it, and no hex in expressions.

### Gate

- **Build/tests:** `cargo nextest run -p holon-frontend` green; new
  `token_resolution_tests` (each `CardState` resolves distinctly in dark +
  light; tui degrades visibly).
- **Existing PBT green:** `inv-org-render-fixed-point`, keystone
  `general_e2e_composed_pbt` (tokens are display-only; org round-trip
  unaffected).
- **New invariant `inv-tokens-resolve-loudly`:** every `Token` used in an
  expression resolves in the active `TokenSet` or fails loud (no silent
  default-to-background, per fail-loud). Exposed via `describe_ui` as the
  resolved token name + frontend-specific value.
- **Cross-frontend:** gpui `Hsla`; dioxus-web CSS custom properties; tui
  `Style` (loses elevation, keeps color + bold); worker reports token names.

**De-risks:** the `ValueFn`-emits-`Token` path — confirm `ResolvedArgs`
(`render_interpreter.rs:29`) can carry a typed token without string round-trip.

**Desk-independent win: YES (high).** Tokens help boards, dashboards, and the
existing gallery immediately. This phase can ship before Phase 1 and should.

---

## Phase 3 — Z-axis / layering primitives (M)

**ADR 0026 mapping:** slice 4 (drag ghosts), slice 1 (overlays, dim layers,
floating fixtures like the calendar). The reference image shows layered
fixtures and a radial glow halo sitting above the desk plane.

**Riskiest assumption:** overlays, floating fixtures, dim layers, and drag
ghosts can all be **element-tree siblings** of the surface (rendered after it,
higher z) rather than OS-level windows or a separate GPUI overlay window.
GPUI's element tree must support a z-ordered sibling without breaking the
`ElementInfo`/`describe_ui` tree walk.

**Spike (throwaway, GPUI):** render a `surface` with one `overlay` (a dim
layer covering it) and one `floating` fixture (calendar chip in a corner).
Assert via `describe_ui` that (a) the overlay's rect covers the surface; (b)
the floating fixture's rect is inside the surface bounds but not a child of a
zone; (c) z-order is overlay < floating (later sibling paints above).

### Design

Four primitives, all element-tree siblings, z = paint order:

- **`overlay`** — a full-surface dim/blur layer. Painted above zones, below
  floating fixtures. Used for focus-contract dimming (Phase 7) and modal
  backdrops.
- **`floating`** — a fixture positioned by anchor (corner / zone-edge), not by
  zone ordinal. The calendar, the "next arrival" chip. It is NOT a zone child;
  it is a surface child with `position: floating`.
- **`dim_layer`** — a token-driven (Phase 2 `Opacity`) translucent rect. Same
  paint mechanism as overlay but semantic: "calm the rest while X is centered."
- **`drag_ghost`** — the floating preview of a card being dragged. Today the
  `board` builder gets this from gpui-component `SortableDragData`
  (`gpui/src/render/builders/board.rs` comment on double-render). The ghost
  must be a first-class primitive so non-board surfaces (Phase 1) get DnD
  ghosts too (Phase 4).

### Gate

- **Build/tests:** new `layering_tests` (overlay covers surface; floating
  inside surface but outside zones; z-order = sibling order).
- **Existing PBT green:** keystone (layering is display-only; no data change).
- **New invariant `inv-layer-z-order-acyclic`:** z-order is a total order
  equal to sibling paint order; no element claims a z above a later sibling.
  Exposed via `describe_ui` (each element reports `z: <sibling-index>`).
- **Cross-frontend:** gpui full (element tree); dioxus-web full (`z-index`
  CSS); tui — overlays render as a dimmed overlay char (`░`), floating
  fixtures render inline at the corner cell (visible approximation, not
  faked); worker reports the layer list.

**De-risks:** whether `describe_ui`'s tree walk (`geometry.rs` `ElementInfo`)
already captures sibling order as z, or needs a new field. The spike's
`describe_ui` z-order assertion IS this de-risk.

**Desk-independent win:** moderate — overlays/dim layers are general UI; drag
ghosts are board-coupled.

---

## Phase 4 — Spatial drag-and-drop (extend existing) (M)

**ADR 0026 mapping:** slice 4 (sweep, zone→zone moves, capacity forcing
function). DnD **already exists** (correction above): `draggable` +
`drop_zone` dispatch `move_block` in GPUI and dioxus-web. The gap is that it
is **lane-reorder only** (`SortableState` per `board_lane`), not spatial
cross-zone with capacity and origin-awareness.

**Riskiest assumption:** the shipped `move_block` intent can carry a **target
zone's `parent_id`** (not just an after-sibling anchor) so a card dropped into
a zone reparents to that zone block — reusing the existing op path, not a new
op.

**Spike (throwaway, GPUI):** two `zone`s from Phase 1; drag a card from zone A
to zone B; assert the dispatched `move_block` intent's target parent_id = zone
B's block id, and the card's `parent_id` changes in Loro/Turso. This reuses
`build_drop_intent` (`dnd.rs`, `render/drag.rs` `DraggedBlock`) — the change is
the *target resolution*, not the intent shape.

### Design

- **Drop → op mapping is already data.** `drop_zone`'s `op` prop defaults to
  `move_block` (`shadow_builders/drop_zone.rs:8`). For the desk, the drop zone
  is the **zone block**; the op is `move_block` with the zone as the new
  parent. No new op.
- **Capacity as a forcing function (UI, not engine).** A zone at `capacity`
  refuses drops **visibly** (drop highlight turns red / "full" token from
  Phase 2). The engine never auto-evicts (never-moves-your-stuff). The user
  must displace something first. `describe_ui` exposes `capacity` + `count` so
  agents assert the refusal.
- **Origin-aware from day one.** A drag ghost (Phase 3 `drag_ghost`) shows the
  card's origin zone; dropping back on the origin is a no-op (not a `move_block`).
  The existing `DraggedBlock` carries `row_id` + `parent_id`
  (`gpui/src/render/builders/board.rs` `BoardCard`); origin = `parent_id`. A
  drop whose target parent equals the origin parent is suppressed at intent
  build time, not silently.
- **Cross-zone vs intra-zone.** Intra-zone reorder = existing `SortableState`
  path (after-sibling anchor). Cross-zone = parent_id change. Both are
  `move_block`; the difference is whether the target is a sibling or a parent.

### Gate

- **Build/tests:** extend `inv-editable-text-has-draggable` and the headless
  drop walker (`user_driver.rs` `drop_entity`) to cover cross-zone drops; new
  `spatial_dnd_tests` (cross-zone reparents; origin drop is no-op; capacity
  refusal visible).
- **Existing PBT green:** keystone (DnD is ops; data model unchanged).
- **New invariant `inv-dnd-capacity-refusal-visible`:** a drop onto a full
  zone produces no `move_block` AND surfaces a "full" state in `describe_ui`
  (not a silent no-op — fail-loud).
- **Cross-frontend:** GPUI full; dioxus-web full (HTML5 DnD already wired);
  tui — DnD is keyboard-driven (select card, `m`, select zone, enter); worker
  N/A (ops-only, no gesture).

**De-risks (largely RESOLVED by senior review 1):** the intent's canonical
form is `move_block { id, parent_id, after_block_id }`
(`block_write_field.rs:90`) — `parent_id` is first-class, the reuse bet holds.
Remaining spike check: whether `after_block_id` may be omitted for
append-at-end. Also (review 7): the spike reads `capacity` from expression
literals; production reads it from zone-block properties — a named dependency
on the data-model track at the slice-4 merge.

**Desk-independent win:** YES — spatial DnD upgrades every `board`/kanban use
case to cross-lane reparenting with capacity.

---

## Phase 5 — State-transition animation (M)

**ADR 0026 mapping:** slice 4 (card settle), slice 5 (chrome retraction
animation). The reference image implies smooth card placement. Today **no
animation is exposed** in the render-expression vocabulary.

**Hard constraint (non-negotiable):** animation must be **state-transition-
driven**, never timer-driven. A CDC delta (a card arrived, a zone changed) is
the trigger; the animation interpolates from the prior `ElementInfo` bounds to
the new ones. This preserves event-time semantics (ADR 0026 §3): a timer that
mutates state would violate "nothing leaves the shore unwitnessed" and the
never-moves-your-stuff invariant. `Instant::now` must not appear in any
animation path — the clock is data (ADR 0024), and a wall-clock tick never
mutates state.

**Riskiest assumption:** the reactive pipeline can emit a "this element moved
from rect A to rect B" delta that an animator can consume, without the
animator polling. Today `VecDiff::UpdateAt` (`reactive.rs:598`) signals a row
change; the *spatial* delta (old rect → new rect) is not emitted.

**Spike (throwaway, GPUI):** on a CDC update that moves a card's ordinal,
interpolate its position over 150ms from old rect to new. Assert (a) the
animation completes and the final rect matches the data rect; (b) no
`Instant::now` in the animator — it reads a normalized `t ∈ [0,1]` advanced by
the GPUI frame clock, which is a render signal, not a state mutation; (c) a
mid-animation CDC update restarts the interpolation from the current
interpolated position (no jump).

### Design

- **Animation = interpolation between two `ElementInfo` snapshots.** The
  animator holds the prior rect; on a `VecDiff`/CDC delta it tweens to the new
  rect. State is never written by the animator; it only reads render-frame
  `t`.
- **Transition vocabulary (new builders):** `transition(property: position |
  opacity | scale, duration_ms: u32, easing: ease_out)`. Applied as a wrapper
  around a card. The `property` names what to interpolate; the animator
  captures prior/new from `ElementInfo` diffs.
- **Interruptible.** A new delta mid-tween restarts from the current
  interpolated value, not the start. This is what makes it feel physical and
  what keeps it event-time-correct (a late-arriving delta is not lost).
- **No state mutation.** The animator is a pure render-time effect. If the
  clock (ADR 0024) is the only allowed time source, and the animator reads
  only frame `t`, then animation cannot violate event-time. A regression test
  asserts: animating a card does not produce a Loro/Turso write.

### Gate

- **Build/tests:** new `transition_tests` (interpolates; interruptible; zero
  writes); a `grep`-based guard that `Instant::now` / `SystemTime::now` does
  not appear in `shadow_builders/transition*` or the animator module.
- **Existing PBT green:** keystone; `inv-no-observed-errors` (animation panics
  would surface).
- **New invariant `inv-animation-event-time`:** an animation cycle produces
  zero ops/zero writes; its only input is frame `t` + the element rect delta.
  Exposed via `describe_ui` as `transitioning: bool` per element (agents can
  assert settle completes).
- **Cross-frontend:** gpui full (frame clock); dioxus-web full (CSS
  transitions — but the trigger is still the CDC delta, not a CSS `:hover`);
  tui — no animation (terminal can't), degrades to instant placement
  **visibly** (a `[settled]` flash, not a fake tween); worker N/A.

**De-risks (RESOLVED by senior review 3):** the reactive pipeline does NOT
emit prior rects and none are planned. The animator captures "prior" from the
existing `ElementInfo` bounds registry (`geometry.rs:14`, persisted between
frames) before re-render overwrites it — registry-diff is the committed
design, not a fallback. The spike validates registry read-before-overwrite
ordering, which is the remaining unknown.

**Desk-independent win:** moderate — transitions polish boards/lists too, but
the event-time discipline is desk-specific.

---

## Phase 6 — Local widget-state bindings in expressions (S)

**ADR 0026 mapping:** cross-cutting (enables several hand-wired toggles). The
focus-settle toggle in the gallery had to be hand-wired Rust because a
render-expression cannot read a per-mount `Mutable<bool>`. This is the gap.

**Riskiest assumption:** a render expression can declare and read a per-mount
local `Mutable<bool>` (a `local_state` value-fn + a `toggle` op) without
threading the state through `BuilderServices` (which is shared/global, not
per-mount).

**Spike (throwaway, headless):** express the focus-settle toggle as
`if(local_state("settle"), card(...), row(...))` + an `op_button(op:
toggle_local("settle"))`. Assert (a) clicking the button flips the local
state; (b) the card/row swaps; (c) the state is per-mount (two instances of
the expression have independent state); (d) zero Loro/Turso writes (it is
local, not canonical).

### Design

- **`local_state(name: &str) -> bool` value-fn** reads a `Mutable<bool>`
  stored ON the `ReactiveViewModel` node (the existing per-render-slot Mutable
  pattern, Model.md Cell-vs-Mutable cut) keyed by `name`. There is no
  `element_mount_id` concept (senior review 5); the node IS the mount.
  **The real risk:** re-interpretation rebuilds ViewModel nodes and resets
  their Mutables — the gallery's `view_cache` comment documents exactly this
  failure. Survival across re-interpretation (persist the node, or re-seed
  from a keyed side-table like the editor entity cache) is the spike's primary
  question, above per-mount independence.
- **`toggle_local(name)` op** flips it. Dispatched through the existing op
  path but routed to the local mutable, not the engine. It produces no
  `move_block`/no write — it is a UI-only state change.
- **Scope: booleans first.** Local strings/numbers are a later phase if a
  concrete need appears. The focus-settle toggle, expand-collapse local state,
  and palette-open state are all booleans.
- **`describe_ui` exposure:** local state is reported per element so an agent
  can assert the toggle took effect. It is explicitly marked `local: true` so
  agents do not confuse it with canonical state.

### Gate

- **Build/tests:** new `local_state_tests` (toggle; per-mount independence;
  zero writes).
- **Existing PBT green:** keystone (local state is display-only).
- **New invariant `inv-local-state-no-writes`:** a `toggle_local` op produces
  zero Loro/Turso writes (tripwire against accidentally serializing UI state).
- **Cross-frontend:** all four — local state lives in the ViewModel layer
  (`reactive_view_model.rs`), so gpui/dioxus/tui/worker all see it identically
  through `describe_ui`.

**De-risks:** whether the ViewModel mount identity is stable enough to key
local state across re-renders. The spike's per-mount-independence assertion IS
this de-risk.

**Desk-independent win: YES.** Every hand-wired Rust toggle in the gallery and
boards becomes an expression. Cheap, high leverage.

---

## Phase 7 — Zoom / chrome state machine (M)

**ADR 0026 mapping:** slice 5 (zoom state machine + chrome retraction +
focus-contract query gate). ADR 0026 §6 specifies zoom levels as an **ordered
config list** with `current_level: index`; chrome is *derived* from the level's
flags, not stored. The focus-contract query gate is a data/query concern (ADR
0026 §4), already scoped — this phase covers only the **frontend state machine
+ derived chrome**.

**Riskiest assumption:** chrome retraction (sidebar → library at triage levels,
sidebar → transient palette at focus levels) is a **pure projection of
`current_level`** — there is no separate `mode` field, no separate `palette_open`
store. One organ (sidebar = launcher = palette), one state machine.

**Spike (throwaway, GPUI):** three levels (`desk`, `page`, `interior`) with
flags `chrome: triage|sidebar|palette`. Drive `current_level` via a zoom op.
Assert (a) the sidebar retracts/expands as the flag changes; (b) there is no
`mode` field anywhere — the renderer reads `levels[current_level].chrome`; (c)
`describe_ui` reports `current_level` + the active chrome flag, not a mode
name.

### Design

- **Levels as an open list (ADR 0026 §6).** v1 ships the list as a hard-coded
  default data structure (Holon has no config-file system today). The
  commitment "data, not code" = flag-reading logic, not an enum matched by
  name. Adding the debated intermediate "peripheral-awareness" level = a list
  entry.
- **`current_level: index` is per-frontend UI state on `UiState`** (the
  `focused_block` pattern) — NOT an engine op, not synced, not undoable
  (senior review 4: the earlier "written through ops" claim contradicted the
  "data unchanged" gate; zooming one screen must not zoom another device).
  Transitions: zoom in/out = ±1, jump = set. Promoting zoom to an
  attributable op is deferred until a real cross-device/undo requirement
  exists.
- **Derived chrome.** Sidebar retraction/palette mode is a function
  `chrome_of(levels[current_level])`. No `sidebar_mode` field. The sidebar
  widget reads the chrome flag and renders as library / palette / hidden. One
  organ, three projections.
- **Zoom extends the existing block↔page gesture.** The desk is one more
  zoom-out past the document root (ADR 0026 §6). At page level Holon stays the
  LogSeq-class outliner — adoption de-risk.

### Gate

- **Build/tests:** new `zoom_state_tests` (level transitions; chrome projects
  from flag; no `mode` field exists — a `grep` guard).
- **Existing PBT green:** keystone (zoom is UI state; data unchanged).
- **New invariant `inv-zoom-chrome-derived`:** for every `current_level`, the
  rendered chrome equals `levels[current_level].chrome` with no intervening
  `mode` field. Exposed via `describe_ui` (`current_level`, `chrome_flag`).
- **Cross-frontend:** all four — `current_level` is ViewModel state; chrome
  projection is per-frontend (gpui sidebar retracts; dioxus-web CSS; tui
  collapses the sidebar pane; worker reports the level).

**De-risks:** whether the existing block↔page zoom gesture can be extended by
one level without restructuring. The spike reuses the gesture path. ADR 0026
§6 already asserts this is the adoption de-risk.

**Focus-contract query gate (ADR 0026 §4) is NOT in this phase** — it is a
query/SQL concern (clock-join, matview-safe), gated on the ADR 0024 clock
beat. Listed here for mapping only; it degrades visibly to interaction-driven
advance without the clock beat.

**Desk-independent win:** moderate — the zoom axis is desk-specific, but
derived-chrome-from-flags is a general UI-state pattern.

---

## Phase 8 — Garnish: glow, gradient, typography, icon chips, instrument seam (S–M)

**ADR 0026 mapping:** slice 1 visual fidelity. Folded from the reference image.
Each item is marked **v1-critical** or **garnish**. None blocks the desk
shipping; v1-critical items affect whether the desk *reads* as the Attention
Environment.

**Riskiest assumption (collective):** GPUI has the primitive support (gradients,
box-shadow) or a layered-rect fallback exists; absent custom-widget kinds
(instrument seam) degrade **visibly** (a placeholder), not silently.

### Items

- **Icon chips (v1-critical).** A name-based icon registry: `icon(name:
  "calendar")`. Per-frontend resolution: gpui loads a glyph/svg; dioxus-web
  uses an icon font/SVG; tui emits a glyph or `[icon:name]` text; worker
  reports the name. Absent name = visible `[?]` placeholder, not a blank.
  **Spike:** one icon (`calendar`) across all four frontends; assert the
  placeholder renders for a missing name.
- **Instrument-widget seam (v1-critical).** Registered custom widget kinds
  (e.g. a `clock` instrument). ADR 0026 §2 fixtures like `calendar` are zones;
  but an *instrument* (live clock, sparkline) is a custom widget kind
  registered with the interpreter (`RenderInterpreter` builder registry,
  `render_interpreter.rs:136`). Absent registration = a visible placeholder
  block `[instrument:clock unavailable]`, not a silent omission (fail-loud).
  **Spike:** register a no-op `clock` builder; assert the placeholder shows
  when the builder is absent.
- **Typography token ramp (v1-critical).** Per-frontend resolution: gpui font
  loading (display serif family); dioxus-web CSS `font-family`; tui ignores
  family, keeps size/weight. This rides Phase 2's token system —
  `Text(role: Display | Heading | Body | Caption)` resolves to a
  `(family, size, weight)` triple per frontend. **Spike:** one serif display
  token in gpui + tui; assert tui keeps size, drops family visibly.
- **Radial glow halos (garnish).** The reference image shows a focused-card
  glow. **Spike:** check GPUI gradient/box-shadow support; if absent, layered
  translucent rects (Phase 3) as a visible fallback (a soft rect halo, not the
  crisp gradient). Mark the fallback in `describe_ui` so agents know.
- **Hue-shift surface gradients (garnish).** Cool→warm surface tint. Token-
  driven (Phase 2): `Surface(elev)` resolves to a gradient in gpui/dioxus, a
  flat color in tui.
- **Zone-internal scroll + edge fade (garnish, but distinct).** A zone MAY
  scroll its card list (the `surface` itself never scrolls — Phase 1). Edge
  fade = a `dim_layer` (Phase 3) at the zone's scroll boundary. **Spike:** a
  zone with 10 cards in a capacity-5 viewport; assert scroll + fade, surface
  bounds unchanged.
- **Ornament primitives (garnish).** Dashed slots, dividers — new leaf
  builders `divider(style: dashed)` `slot()`. Cheap; no de-risk needed.

### Gate

- **Build/tests:** `icon_registry_tests`, `instrument_placeholder_tests`,
  `typography_token_tests`; fallback-visibility tests for glow/gradient.
- **Existing PBT green:** keystone.
- **New invariant `inv-instrument-absent-visible`:** a referenced instrument
  widget kind with no registered builder renders a placeholder string
  containing the kind name (fail-loud, not blank).
- **Cross-frontend:** icon/typography/instrument degrade visibly per frontend
  per the matrix; worker reports names/placeholders.

**Desk-independent win:** icons + typography tokens help every UI; the
instrument seam is desk/dashboard-specific.

---

## Cross-frontend degradation matrix

"Visibly degrade" is the floor (fail-loud philosophy); silent no-ops are
forbidden. Worker = MCP/`describe_ui` only (no visual surface); it always
*reports* the capability faithfully.

| Capability | GPUI | dioxus-web | tui | worker (MCP) |
|-----------|------|-----------|-----|--------------|
| P1 surface/zone | full (clip + abs-pos) | full (CSS clip + relative) | zones as labeled cols, `[FULL n/c]` marker | reports tree + capacity |
| P2 tokens | full (`Hsla`) | full (CSS vars) | color+bold, drops elevation | reports token names |
| P3 layering | full (element z) | full (`z-index`) | overlay char `░`, floating at corner cell | reports layer list + z |
| P4 spatial DnD | full (drag ghost) | full (HTML5 DnD) | keyboard (`m` then enter) | ops-only, no gesture |
| P5 animation | full (frame clock) | full (CSS transition, CDC-triggered) | instant + `[settled]` flash | reports `transitioning` |
| P6 local state | full | full | full | reports `local: true` state |
| P7 zoom/chrome | full (sidebar retract) | full (CSS) | sidebar pane collapses | reports level + chrome flag |
| P8 icon | glyph/svg | icon font/svg | glyph or `[icon:n]` | reports name |
| P8 typography | serif display load | CSS family | size+weight, drops family | reports role |
| P8 instrument | registered builder | registered builder | placeholder `[instrument:k]` | reports kind/placeholder |
| P8 glow/gradient | gradient or layered-rect fallback | CSS gradient | flat color | reports fallback flag |

---

## Non-goals

- **Freeform x/y canvas.** ADR 0026 §2 v2 defers coordinates; v1 is
  zones-first. The `surface` places by zone + ordinal. x/y are additive
  properties ignored by the v1 renderer — not built here.
- **OS gestures / native switcher takeover.** ADR 0026 §7 lists these as
  progressive enhancement, shell-crate work. Not a frontend-stack capability.
- **Vignette overlay / OS shell presence.** ADR 0026 slices 8–10 (shell,
  launcher, window sets, vignette). Out of scope — this plan is the
  *in-app* desk UI, not the OS shell.
- **Focus re-keying (ADR 0016).** Referenced, not re-planned. Editable desk
  cards (P2 transclusion) ride ADR 0016; v1 cards are non-editable summaries
  (ADR 0026 §1).
- **Focus-contract query gate.** ADR 0026 §4 — a SQL/query concern (clock-
  join, matview-safe), gated on the ADR 0024 clock beat. Mapped to Phase 7
  for context only; not built here.
- **Desk replay (P4 temporal source).** Deferred with P4 (ADR 0026 §3).
- **Config-file system for zoom levels.** v1 ships the level list as a
  hard-coded default data structure (ADR 0026 §6). File-based configurability
  plugs in later without touching consumers.
- **Local state beyond booleans.** Phase 6 ships `bool` only; strings/numbers
  deferred until a concrete need appears.

---

## Sizing, parallelism, and ADR 0026 mapping

| Phase | Size | Parallelizes with | ADR 0026 slice |
|-------|------|-------------------|----------------|
| P1 surface/zone | L | P2, P6, P8 (independent) | 1, 4 |
| P2 tokens | M | P1, P6, P8 (independent) | 1 (also: boards/dashboards) |
| P3 layering | M | after P1 (needs surface); P2 parallel | 1, 4 |
| P4 spatial DnD | M | after P1 + P3 (needs surface + ghost) | 4 |
| P5 animation | M | after P1 (needs rects to tween) | 4, 5 |
| P6 local state | S | all (independent) | cross-cutting |
| P7 zoom/chrome | M | after P1 (needs surface as a level); P2 parallel | 5 |
| P8 garnish | S–M | P2 (needs tokens); items parallel each other | 1 |

**Critical path (desk v1):** P2 (tokens) → P1 (surface) → P3 (layering) → P4
(spatial DnD). P6, P7, P8 fold in alongside. P5 (animation) is post-v1 polish
but its event-time discipline must be decided in P1 (rects must be capturable).

**Desk-independent wins to ship first (parallel track):**
- **P2 tokens** — every UI benefits today.
- **P6 local state** — kills hand-wired Rust toggles in gallery/boards.
- **P8 icon + typography** — general polish.

These three can land before any desk-specific phase and de-risk the desk
phases by maturing the token/value-fn seams they depend on.

**Sequencing note (honest cost):** the desk's *data model* (ADR 0026 §1, §3,
§5) ships independently of this plan — ordinary blocks, no new storage. This
plan is the **frontend capability** track. The two tracks meet at slice 4
(desk UI v1): the data model provides zone/placement blocks; this plan's P1–P4
provides the spatial UI that renders them. Editable cards (slice 7) remain
gated on ADR 0016 + P2 transclusion, not on anything here.
