# ADR 0016: Occurrence-keyed focus authority

**Status:** Proposed (2026-07-06). Rewritten after a code-grounded senior review
REJECTED the first draft (its additive dual-signal, its phantom occurrence
identity, and its "Surface 2 is a sibling" punt). This ADR is the gate
[ADR 0015](0015-computed-placement-and-curated-state-primitives.md) made **P2
conditional on**, and it is **mutually sequenced with P2**: P2 defines *what an
occurrence is*; this ADR defines *how focus carries one*. It decides the **shape
and constraints**, not the identity.
**Deciders:** Martin
**Relates to:** ADR 0010 (focus authority — the graduation it reserved, on the
*occurrence* axis), ADR 0015 (P2 / display placement — the caller and the
identity-owner), the Phase 1b spike (`docs/Proposals/display-placement-implementation-plan.md`).

## Problem

**Focus is a render-slot property, keyed today by bare entity id.** It lives in a
single global `UiState.focused_block: Mutable<Option<EntityUri>>`
(`reactive.rs:953`); every frontend mounts an editor / swaps the `text` variant
for `editable_text` **iff `focused_block == its row`** (`render_entity_view.rs:124`,
GPUI `editor_view.rs:477`; the same predicate in dioxus-web, tui, worker).

This is a concrete instance of the **entity-identity vs element-identity**
conflation named in ADR 0015 §1a: focus is an *element* (render-slot) concern that
has been keyed on *entity* identity only because, pre-P2, every block rendered
exactly once so the two coincided. Display placement (ADR 0015 P2) renders the
**same real block id** in more than one editable position. Under bare-id keying all
occurrences satisfy `focused_block == L` at once → **every occurrence mounts a
cursor** (the "multiple cursors" hazard ADR 0010 guards against). The node-level
`RowOrigin` marker cannot fix this: focus/caret/undo are keyed on the **id**, not
the node.

**So focus must key on element identity — but element identity must stay
entity-projectable, not opaque.** The reason is a census of who sets or reads
focus: of the seven writers/consumers, **six are entity-first by nature** and only
one knows a render slot:

| Writer / consumer | Names an | Source |
|---|---|---|
| click-to-focus (`shadow_builders/prelude.rs:38-51`) | **element** | the clicked row's `el_id` is in scope (discarded today) |
| `apply_structural_focus` (`reactive.rs:2036/2142`) | entity | backend op response (`split_block` returns a new **block id** — a backend cannot name a slot) |
| `maybe_mirror_navigation_focus` (`reactive.rs:2464`) | entity | `block_id` from intent params |
| delete-clear (`reactive.rs:2523`) | entity | entity event |
| MCP `InputAction::Focus{block_id, placement}` (`mcp/tools.rs:1850/1933`) | entity | agents address entities |
| `focus_chain` (`value_fns/focus_chain.rs:114`) | entity | joins query rows **from** the focused entity |
| worker `WatchEnvelope` (`holon-worker/lib.rs:420-426`) | entity | serializes the focused entity to the web page |

An **opaque** element id would force an element→entity lookup at the six
entity-needing sites and an entity→element resolution *policy* at the four+ writers
— and there is no stable opaque id to be had: the render tree is rebuilt every
interpretation, so any durable element id must itself be *derived from*
`(entity, occurrence)` to survive re-render (the positional-index instability §2
already rejects). Therefore element identity is the **structured tuple**
`(EntityUri, Occurrence)`: its entity projection is `.0` (free, no lookup), so every
entity-first writer sets `(entity, policy-resolved-occurrence)` and every
entity-needing consumer extracts without indirection. The tuple is **not a hybrid
compromise — it is element identity in normalized, entity-projectable form.**

ADR 0010 reserved this graduation but anticipated the **region** axis
(`MutableBTreeMap<Region, Option<EntityUri>>`). Display placement needs the
**occurrence** axis; the full element key is `(region, block, occurrence)` (§6).
The PBT ref model already region-keys focus (`focused_entity_id: HashMap<Region,
EntityUri>`, `ui_actor_state.rs:42-46`), so the test-side cost is largely paid.

## Decision

### 1. The focus key gains an occurrence coordinate — as a WIDENED TUPLE, not a parallel signal

```
focused_block: Mutable<Option<(EntityUri, Occurrence)>>
```

**One signal, atomic by construction.** The first draft's additive
`focused_occurrence` parallel `Mutable` is rejected (§Alternatives) — it buys
nothing (every reader's predicate changes anyway, §4), and two independent
signals desync: they have no joint lock, multiple async writers set focus
(`apply_structural_focus` `reactive.rs:2046/2142`, `maybe_mirror_navigation_focus`
`:2464`, delete-clear `:2523`), and synchronous readers snapshot bare
(`editor_view.rs:477`, `render_entity_view.rs:124`). Decisively:
`watch_snapshot_stream` refires the worker/dioxus snapshot off the
`focused_block` signal (`reactive.rs:1347`) — with a parallel signal an
occurrence-only focus move never refires, so the web page never re-renders the
variant. The tuple makes the signal change on every occurrence move, for free.

The `Occurrence` value is a small enum (`Canonical | Placed(…)`); readers that
only care about the block unwrap `.map(|(b, _)| b)`.

### 2. What an `Occurrence` IS is deferred to P2 — this ADR decides only the SHAPE

The first draft defined `Occurrence = Placed(DisplayPlacementEdgeId)`. That type
has **zero code hits** (`DisplayPlacement`, `RowOrigin::DisplayPlaced` do not
exist) and is minted by P2, which is gated on this ADR — circular. So:

- This ADR fixes only that focus carries a second coordinate, that edits ignore
  it (§5), and that caret/undo key on it.
- **P2 owns the identity**, and must answer the cases that make a positional
  index wrong AND an edge id insufficient: (a) one block placed twice under one
  anchor; (b) **one display-placement edge rendered in two panels/regions** →
  two visual occurrences, one edge id (so identity is at least
  `(region, edge, …)`, not `edge` alone — see §6); (c) *computed, edge-less*
  placements (backlinks, related-notes) that have no single durable edge. Until
  P2 answers these, `Occurrence`'s inner payload is `TBD-by-P2`; the spike used a
  throwaway `u32`.
- Do **not** name it `placement` — `InputAction::Focus { block_id, placement:
  CursorPlacement }` (`input.rs:55`) already uses that word for *caret* position.

### 3. Surface 2 is a PREREQUISITE of this ADR, not a sibling

In GPUI two occurrences of one block **cannot be represented or independently
edited** today, so this ADR's own validation is impossible until Surface 2 is
solved. Concretely, all keyed by bare id:

- collection-driver row diffing (`reactive_view.rs:945/1306/1646`) — duplicate
  `EntityUri` keys collide on move/remove;
- the editor entity cache `editable-text-{row_id}-{field}`
  (`render/builders/editable_text.rs:12`) — two occurrences resolve to **one**
  cached `EditorView`/`InputState`, so independent carets are structurally
  impossible;
- element id `render-entity-{id}` and its eviction sweep
  (`render_entity_view.rs:159/140-142`).

These must become occurrence-aware (row key, editor-cache key, element id) as
part of this work. The §Riskiest GPUI test cannot exist otherwise.

### 4. There is NO universal predicate — the rollout is per-frontend, structurally distinct

Each frontend decides focus differently; "swap one predicate N times" is false:

- **GPUI** — five mechanisms: the `is_focused` variant switch (editor mounts at
  all), the async signal binding `spawn_focus_binding` (`editor_view.rs:584`),
  the synchronous first-mount grab (`editor_view.rs:477`), the eviction latch
  (`render_entity_view.rs:125-157`), and the entity-cache identity (§3).
- **dioxus-web** — no signal: consumes a **serialized `WatchEnvelope`** and
  applies DOM focus (`worker_focus::apply`, `live_block.rs:102`), with a reverse
  DOM→worker `onfocusin` bridge (`editor.rs:95`).
- **worker** — reads focus after interpretation and refires whole snapshots on
  focus change (`lib.rs:420-426`), fed by `watch_snapshot_stream` (§1).
- **tui** — focus lives in `app_main.rs` (not `input_pump.rs`/`render/mod.rs`/
  `user_driver.rs`; the first draft mis-enumerated these).
- **shared holon-frontend readers** the rollout must also cover:
  `value_fns/focus_chain.rs:114`, `reactive_view.rs:814/1429`, `view_model.rs`,
  `editor_view_model.rs`, and `watch_snapshot_stream` (`reactive.rs:1347`).

Each site gets its own no-multiple-cursor test.

### 5. Edits resolve by canonical id; occurrence keys caret/undo ONLY

`editable_text(&block_uri)` is never keyed by occurrence — the write always lands
on the canonical block (ADR 0015 contract rule 3). Occurrence keys the caret (the
mirror's `(block_id, occurrence)` cursor map) and undo grouping only. This is the
one part genuinely spike-proven end-to-end (typing at a `Placed` occurrence
committed to canonical `block_raw.content`).

### 6. Occurrence and region interact — the tuple composes, the parallel signal squares the problem

ADR 0010 reserved `MutableBTreeMap<Region, Option<EntityUri>>` for the region
axis. The multi-panel case (§2b) shows occurrence is **not** cleanly orthogonal:
one edge in two panels is two occurrences distinguished by region. The real key
is `(region, block, occurrence)`. The widened tuple graduates cleanly to
`MutableBTreeMap<Region, Option<(EntityUri, Occurrence)>>`; a parallel occurrence
signal would require a *second* per-region map and desync squared. This is a
further reason for §1's tuple.

### 7. MCP + worker protocol change (a surface, not a footnote)

Occurrence must round-trip across versioned serialized boundaries with an
**external consumer (agents)**:

- the `WatchEnvelope` (worker `lib.rs:424-428`) serialized JSON→NAPI→JS, and its
  page-side consumer + the reverse `note_local_focus` bridge;
- `InputAction::Focus` (a new occurrence field, **distinct** from the existing
  `placement: CursorPlacement`) and its serialization in `send_navigation`
  (`mcp/tools.rs:1933`);
- the `describe_navigation` focus-path schema.

Treat as a protocol revision (agents observe/drive it), not an additive struct
field.

## Evidence (from the Phase 1b spike — stated honestly)

- **What is proven:** edits route to canonical while caret keys on occurrence
  (§5), end-to-end on Loro (`spike_display_occurrence_write_routes_to_canonical`);
  the mirror's `(block_id, occurrence)` cursor keying (unit test); no regression
  (16/16 existing focus tests — a *no-break* signal, not validation).
- **What the spike did NOT do, and where it MISLEADS:** it ran **headless only**
  (single-threaded, zero signal subscribers) — no evidence about GPUI's
  signal-driven multi-writer focus. It landed the **independent**
  `set_focus_occurrence(occ)` setter (`reactive.rs:996`, "no effect on
  focused_block") — i.e. the exact parallel-signal shape §1 now rejects; the
  unified tuple setter does **not** exist yet. No rendered second occurrence
  exists (occurrence was set via a test accessor), so it is end-to-*middle*.

## Consequences

Unblocks ADR 0015 P2 (jointly). Preserves the intent of ADR 0010's single focus
authority while refining its key. Cost is real and front-loaded: Surface 2 (§3)
is a prerequisite; the rollout is per-frontend and heterogeneous (§4); the
serialized protocol changes (§7). The spike's parallel-signal plumbing is
superseded and should be rewritten to the tuple, not extended.

## Riskiest assumption to validate first

**That two occurrences of one block can be represented and independently edited in
GPUI at all** — i.e. Surface 2 (§3) is solvable without a deep rework of the
collection driver + entity cache. Validate with a GPUI window-slice test that
renders a canonical and a placed occurrence of one block and moves focus between
them under real signal propagation. This test is **blocked on §3** — build the
occurrence-aware row/editor-cache keys first, or this ADR is unvalidatable in
GPUI. (The spike's headless green does not touch any of this.)

## Alternatives rejected

- **Additive parallel `focused_occurrence` signal (the first draft).** Buys no
  reader-churn saving (§4 touches them anyway), desyncs across independent writers
  and sync readers, and breaks the `watch_snapshot_stream` refire (§1) so
  dioxus/worker never re-render on an occurrence-only move. Squares the region
  composition (§6). The spike used this shape *because* it was single-threaded
  headless — not evidence for prod.
- **`Occurrence = Placed(DisplayPlacementEdgeId)` as a fixed identity here.**
  Phantom type (zero code hits), circular with P2, and under-determines the
  multi-panel case (§2). Identity is P2's to define.
- **`MutableBTreeMap<Region, Option<EntityUri>>` (ADR 0010's shape).** The region
  axis, not occurrence; the real key is `(region, block, occurrence)` (§6).
- **Occurrence as a positional/render-path index.** Not stable across re-render.
- **Fixing it at the `RowOrigin` node marker only.** Focus/caret/undo are id-keyed,
  not node-keyed — the reason this ADR exists.
- **An opaque, first-class element id (element identity *not* entity-projectable).**
  The intuitive "focus is on a UI element, so key it by an element id" — rejected on
  the writer census (§Problem): six of seven focus writers/consumers are entity-first,
  so an opaque id imposes an element→entity lookup at every consumer and an
  entity→element resolution policy at every writer, while buying nothing the tuple's
  `.0` projection doesn't. Worse, no *stable* opaque id exists (render tree rebuilt per
  interpretation), so it would have to be derived from `(entity, occurrence)` anyway —
  the tuple is that id, normalized. Element identity is correct; opaque encoding is not.
- **"Cursors in all instances" (keep entity-keyed focus; every occurrence shows a
  caret).** Not representable: GPUI window focus is singular (one focus handle owns
  keyboard input), and the blur-commit path (`editor_view.rs:595-640`) assumes exactly
  one live editing buffer — two live `InputState`s means IME double-composition and an
  ambiguous commit-on-blur (the converge problem squared). What this option actually
  wants — *the other occurrence updates live while you type* — is delivered by the
  shared entity `Cell` as **text** liveness (ADR 0015 §1a), not caret duplication.
  Single-element focus is non-negotiable.
