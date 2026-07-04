# ADR 0026: Attention Environment — desk, zoom, and shell architecture

**Status:** Proposed (2026-07-13). Drafted with Martin in-session from the
ideation capture (vault: `Projects/Holon/Attention Environment.org`) and the
three-OS feasibility research
(`docs/Vision/Ideas/OS_Integration_Research_2026-07-13.md`,
`OS_Integration_Crates_2026-07-*.md`).
**Deciders:** Martin
**Relates to:** ADR 0015 (canonical vs display placement — the desk extends its
P2 contract), ADR 0016 (occurrence-keyed focus authority — the desk's rendering
prerequisite), ADR 0024 (PN actions — time-as-data for arrivals/sweeps),
`docs/Architecture/Model.md` (five layers, invariants 1–12).

## Problem

The ideation ratified a product shape: one bounded, user-arranged 2D **desk**
(shore / center / wake, time axis), **zoom** as the mode mechanism
(block ↔ page ↔ desk ↔ screen-edge vignette), a **focus contract** ("during
focus nothing moves; time-critical fades in before due; everything else
waits"), **task↔resource** bundles observed while a task is centered, and
per-OS **shell** presence (launcher, window sets, switcher, vignette). The
invariants are non-negotiable: the system never moves your stuff; bounded means
bounded; nothing leaves the shore unwitnessed; chrome obeys the focus contract;
the zoom-level set is never hard-coded.

This ADR decides how that lands on the existing architecture with the smallest
honest engine seam: what the engine learns, where desk state lives, how zoom
levels are represented, and where the OS-specific code stops.

## Decision

### 1. The desk is a document; a desk placement is an ordinary PLACEMENT BLOCK

The desk is **one ordinary block document** (file over app: readable, diffable,
synced). Placing a block on the desk creates a **placement block**: an ordinary
child block of a zone block (§2) whose content is a reference (`[[id][label]]`,
links ruling) to the canonical target block, with placement metadata as normal
block properties:

- **zone** = the placement block's `parent_id` (it is a child of the zone block);
- **ordinal within the zone** = its consolidator-minted `sort_key` — reordering
  cards is `move_block` (Model.md invariant 3 holds by construction: intent
  carries `after_sibling`, never an order key);
- `arrived_at`, `pinned` = property-drawer properties (existing org round-trip);
- "which desks hold block X" = the existing `block_links` backlink junction
  (the reference in the placement block's content is indexed for free).

No new edge type, junction table, parser, or writer. (A senior review of this
ADR's first draft found that a `desk → block` *edge with structured payload* —
the draft's design — does not exist as machinery: `EdgeField`/`EdgeFieldUpdate`
(`crates/holon-api/src/edge_field.rs:105-116`) supports only flat target sets
mapped to two-column junctions. The placement block uses only shipped
primitives instead; the edge design is Rejected below.)

**Desk placement is CANONICAL, not display placement.** The desk document is
the placement block's home: the user put the card there deliberately; it must
sync, persist, round-trip through org, and survive restarts. This is deliberate
scoping against ADR 0015: the ADR-0015 P2 machinery (occurrence keys, origin
markers, the inertness contract) is needed only for **rendering the referenced
block's interior** inside a desk card — an editable transclusion is a second
editable occurrence of a real block, which is exactly P2. The split:

- **Desk structure** (which placement blocks sit in which zone, in what order)
  — ordinary blocks + references, buildable today, no new primitives, no gate.
- **Desk card rendering** (live, editable transcluded content) — rides ADR 0016
  + P2. Until that lands, cards render **non-editable summaries**
  (title/first-line), which the contract already permits.

The ADR 0015 §3 rules bind the *referenced* block: a placement block mints no
`sort_key` in the referent's canonical home, does not change its `parent_id`,
does not count as a stored child of the referent for rollups (the placement
block is a child of the desk's zone, not vice versa), and card edits (post-P2)
route to the canonical home. The consolidator's order monopoly (invariant 2) is
untouched: the referent's sibling order never depends on desk state; the
placement block's own order is minted by the consolidator like any block's.

**Row-key safety for non-editable cards (pre-P2):** GPUI keyed rows map to
`(EntityUri, Occurrence::Canonical)` (`reactive.rs:617-625`), so a desk card
rendering a ref-known id must never share a `ReactiveRowProvider` with the main
panel or the keys collide regardless of editability. The desk renders in its
own panel with its own provider instance; `inv-main-panel-rows-match-focus`
scopes to the main-panel subtree and `inv-viewmodel-entity-ids-subset-of-data`
subtracts ref-known ids, so both stay green. Note the cards' *summaries* are
plain text derived from the referent, not keyed rows of the referent — only the
placement blocks themselves are rows in the desk panel.

### 2. Zones are data, never layout code

The desk document declares its zones as **child blocks** (default template:
`center`, `shore`, `wake`, `parking`, plus fixtures like `calendar`). A zone
block carries `{ role, capacity, axis-binding }` properties. Consequences:

- Placement edges reference the zone by containment: a placement block is a
  child of its zone block (§1) — "which zone" is `parent_id`, not a property.
- Adding/removing/renaming a zone is a data edit; no code change, no migration.
- **Bounded means bounded** is enforced by `capacity` on the zone: placing into
  a full zone requires displacing something (UI-level forcing function; the
  engine only stores the result — it never auto-evicts, per the
  never-moves-your-stuff invariant).
- v2 freeform coordinates are additive properties (`x, y` on the placement
  block), ignored by the zones-first renderer — no schema break, no re-keying.
- `role` is a boundary-parsed closed enum (`Center | Arrival | Exit | Free |
  Fixture`) following the `TaskState` open-label + closed-role pattern; logic
  branches on the role, never on the zone's display name.

### 3. Desk events are journal blocks; event-time is query re-evaluation

"Committed: X", "arrived, not taken", "worked on X, touched Y" are **ordinary
blocks on the day's journal page** referencing the same canonical block the
desk edge points at. No event store, no second writer (invariant 4 intact); the
journal page for a day *is* what fell off the desk that day.

**Event-time falls out of the reactive pipeline for free.** The shore is a
query over arrival candidates; queries re-evaluate on CDC deltas, i.e. when
desk facts change — not on wall-clock ticks. Attention boundaries (return to
desk, finish something, sweep) are ops that write journal facts, which is what
advances the shore. A timer never mutates state: "time-critical item due soon"
is a query predicate over `due_at` against ADR 0024's clock-as-data. This makes
"nothing leaves the shore unwitnessed" **structural**: retirement is a sweep op
the user performs, writing "arrived, not taken" journal facts — there is no
code path that removes a shore item without an op.

Desk replay ("show me Tuesday's desk") = replaying journal facts; it inherits
the P4 temporal-source limits and stays deferred with P4 (own ADR).

### 4. Centering and the focus contract are desk-document state + a query gate

`centered: Option<block-id>` and `focus_active: bool` are properties on the
desk document, written only through ops (attributable, undoable, ADR
0024-compatible). The **focus contract's enforcement point is the shore-advance
query**: arrival candidates surface only when `focus_active = false`, except
items matching the time-critical predicate (`due_at − ramp < now`). Enforcement
lives in the query definition — one place, all frontends, MCP-observable —
never per-frontend suppression logic. Fail-safe direction is free: if state is
stale, nothing surfaces (calm), and event-time guarantees nothing is lost.

Query-authoring constraints (code-verified 2026-07-13):

- `now` is a **JOIN against the `clock` relation** (`holon-api/src/clock.rs`,
  `holon-turso/sql/schema/clock.sql`), never `date('now')` — Turso IVM rejects
  non-deterministic expressions when the query is matview-ized by
  `watch_query` (BugFunnel F4). One-shot queries would pass; the desk's watched
  source block would not.
- **Hard dependency disclosed:** the time-critical ramp fires when the clock
  relation *beats* (ADR 0024's clock effect writing time-as-data). Without the
  clock beat wired, the gate degrades — visibly, per fail-loud policy — to
  interaction-driven shore advance only: time-critical items surface at the
  next desk event rather than as the deadline approaches. Slice 5 lists this.
- Author the gate in `holon_sql`/PRQL, not GQL (the external gql-transform
  fails loud on unmapped expressions like this predicate), keep it within
  sqlparser-0.61-parseable SQL (`apply_sql_transforms` silently skips
  `_change_origin` injection on parse failure — CDC trace loss), and keep a
  `TABLES_WITH_CHANGE_ORIGIN` base table (e.g. `block`) in the FROM so the
  watch propagates origins (`sql_parser.rs:734-741`).

The contract stays **Holon-native**: OS do-not-disturb is unreadable via public
API on macOS and Windows (research verdict), so the OS is never the source of
truth for focus.

### 5. Task↔resource association is typed child blocks, not edges

A resource is an ordinary **child block of the task** (under a conventional
`Resources` section block) with properties
`{ kind: app | file | dir | url, locator, last_touched }`; `kind` is
boundary-parsed (parse-don't-validate). Same rationale as §1: block properties
round-trip through org today, `EdgeField` payloads do not. A resource shared by
several tasks is one canonical resource block referenced (`[[id][label]]`) from
the others — backlinks via `block_links` again free. Association-by-observation:
while a task is centered, the shell reports touched resources; the engine
writes them through the normal op path (attributable; later retractable via ADR
0024 effects). The engine's whole knowledge of the OS is: **"task has resource
blocks" + "task is centered."** Nothing else crosses the seam.

### 6. Zoom levels are an ordered config list; mode is derived, not stored

```
zoom_levels: [
  { name: "vignette", chrome: none,    focus_contract: inherit, shore: hidden  },
  { name: "desk",     chrome: triage,  focus_contract: off,     shore: active  },
  { name: "page",     chrome: sidebar, focus_contract: off,     shore: hidden  },
  { name: "interior", chrome: palette, focus_contract: on,      shore: hidden  },
]
```

The engine/frontend state machine holds only `current_level: index` plus
transitions (zoom in/out = ±1, jump = set). **Nothing matches on level names**;
behavior reads the level's flags. Honesty about "config": Holon today has no
config-file system (env vars, constants, DI wiring only) — v1 ships the level
list as a hard-coded **default data structure**. The commitment "data, not
code" means an open list consumed by flag-reading logic rather than an enum
matched by name — cheap to extend, and file-based configurability plugs in
later without touching consumers. Adding the debated intermediate
"peripheral-awareness" level (`attention-q-intermediate-zoom`) = inserting a
list entry — the architecture keeps the product question open cheaply, as
required. Chrome retraction is *derived* from the current level's `chrome`
flag: sidebar-as-library at triage levels, sidebar-as-transient-palette at
focus levels — one organ (sidebar = launcher = palette), one state machine, no
separate mode feature. "Mode is the zoom position" is thereby literal: triage
vs focus is not stored anywhere; it is a projection of `current_level`.

The zoom axis extends the **existing** block ↔ page zoom gesture: the desk is
one more zoom-out past the document root; at page level Holon remains the
LogSeq-class outliner unchanged (adoption de-risk).

### 7. The shell is a capability-trait frontend; per-OS code never crosses it

The shell follows the existing multi-frontend pattern (gpui / dioxus / tui /
worker): another thin consumer of engine queries + op dispatch. Its boundary is
a capability trait with **per-capability graceful degradation**:

```rust
trait Shell {
    fn capabilities(&self) -> ShellCaps;              // what this OS/DE build can do
    // launch (everywhere, no permissions)
    fn launch(&self, r: &Resource, ctx: LaunchCtx) -> Result<LaunchHandle>;
    fn register_scheme(&self, scheme: &str) -> Result<()>;
    // observation (feeds association-by-observation)
    fn observe(&self, tx: Sender<ShellEvent>) -> Result<ObserveHandle>; // focus/app changes
    fn enumerate_windows(&self) -> Result<Vec<WindowRef>>;
    // window sets — OUR layer, never OS workspaces
    fn stow(&self, set: &[WindowRef]) -> Result<()>;   // hide, never close
    fn restore(&self, set: &[WindowRef]) -> Result<()>;
    // presence
    fn vignette(&self, v: VignetteState) -> Result<()>;   // click-through overlay
    fn status_item(&self, s: StatusState) -> Result<()>;  // tray / menu-bar
    fn global_chord(&self, c: Chord, tx: Sender<()>) -> Result<ChordHandle>;
}
```

Missing capability = absent from `ShellCaps`, callers degrade **visibly**
(feature hidden, not faked) — never a silent no-op (fail-loud philosophy;
`Result` everywhere). Crate stack per the research docs: macOS
`objc2-app-kit`/`core-graphics` (+ `screencapturekit` garnish-tier); Windows
`windows`/`windows-sys`; Linux `x11rb` + `smithay-client-toolkit`/
`wayland-protocols-wlr` + `zbus` (KWin/Activities), GNOME via a mandatory Shell
extension companion; cross-cutting `global-hotkey` + `ashpd`
(GlobalShortcuts/ActivationToken), `open`, `tray-icon`, winit-style overlay
windows. Linux is **never shipped as Flatpak** (foreign-toplevel requires
unsandboxed). Priority: KWin ≥ wlroots > X11 >> GNOME. Gestures and switcher
takeover are progressive enhancement, never core UX. Presence is also a
**protocol**: "current focus + next arrival" over MCP, so external surfaces
(editor statusline, terminal prompt, agents) can render it without the shell.

### 8. Sequencing — desk-first, launcher later, gate work pulled forward

Verified state of the ADR 0015/0016 gate (2026-07-13, code-checked): Phase 1b
spike **landed on main** (`focused_occurrence`, mirror keying, both spike
tests); Phase 0 `RowOrigin` **landed**; `RowIdentity` **landed** (LiveCell
suffix rows already carry `OccurrenceId`); ADR 0016 **Proposed, not ratified**;
Phase 1a invariant **not started**. Three product clusters (advice, Later,
desk-card-editing) queue behind ADR 0016 — it is pulled forward.

| # | Slice | Gate dependency | Notes |
|---|-------|-----------------|-------|
| 1 | Desk visual prototype (design gallery) | none | zones/cards as data; validates the spatial design |
| 2 | Phase 1a inert-render invariant + no-write guard | none (it IS gate work) | per the display-placement plan |
| 3 | Desk data model: desk doc, zone blocks, placement + resource blocks | none | ordinary blocks/properties/refs (§1, §5); content-type + role parsing, queries, ops — no new storage machinery |
| 4 | Desk UI v1: zones-first, non-editable cards, sweep, journal drops | none (cards not editable; own panel/provider per §1) | the PM slice: WIP limits, freeform kanban, standup from journal drops |
| 5 | Zoom state machine + chrome retraction + focus-contract query gate | clock beat (ADR 0024) for the time-critical ramp; degrades visibly to interaction-driven advance without it | levels as open list (§6) |
| 6 | ADR 0016 ratification + Surface 2 + per-frontend rollout | — | unblocks editable cards + advice + Later |
| 7 | Editable desk cards (P2 transclusion) | ADR 0016 + P2 | |
| 8 | Shell: observation + resource association on macOS | none (edges from §3) | association-by-observation loop |
| 9 | Launcher (deliberately late: must beat Raycast to be worth shipping) | none | sidebar = launcher = palette externalized |
| 10 | Window sets, switcher-augment, vignette, thumbnails | none | research build-order |

Desk-before-launcher is a product decision (Martin, 2026-07-13): a launcher
competes with Raycast-class tools and must be excellent to displace them; the
desk is the differentiated PM surface (bounded space = WIP limit; desk =
freeform kanban with a time axis; journal drops = flow views for free).

## Consequences

**Positive.** The engine seam is five ordinary things (a document, placement
blocks, resource blocks, journal blocks, two state fields) — no new stores, no
new writers, no new junctions, no consolidator changes; Model.md invariants
hold by construction. The desk data model and PM-grade desk UI ship before the
focus-rekeying rollout. Zoom levels stay a data question. OS code is
quarantined behind one trait with visible degradation. The same primitives
(placement blocks with typed properties, reference-backed sharing) remain
generic: dashboards, kanban boards, and the advice/Later cluster reuse them.

**Negative / cost.** Editable desk cards wait on ADR 0016 + P2 (per-frontend
rollout + MCP protocol revision — the known cost center). Non-editable v1 cards
are a real UX compromise (tap-through to the page interior instead of in-place
editing). The GNOME companion extension and the per-OS shell matrix are a
standing maintenance tax. Desk replay stays blocked on P4. The keystone PBT
must eventually cover desk semantics (placement-edge round-trip, shore-advance
witnessing, zone capacity) — new invariant work.

## Alternatives rejected

- **Desk placement as display placement (P2).** Wrong category: display
  placement is computed and never stored; the desk is user-curated, must
  persist and sync. Using P2 would also gate the whole desk on ADR 0016 —
  needlessly, since only card *interiors* need occurrences.
- **Desk placement as a typed edge with structured payload (this ADR's first
  draft).** `EdgeField`/`EdgeFieldUpdate` supports only flat target sets
  mapped to two-column junctions (`edge_field.rs:105-116`); a payload-carrying
  edge means a new junction, update variant, org drawer format, and writer —
  all to reinvent what a child block with properties already provides. Rejected
  on senior review.
- **Coordinates/zones stored on the target block.** Pollutes the block, breaks
  on multi-context reuse; placement metadata belongs to the placement, not the
  referent.
- **A desk event store / event sourcing for desk history.** Second store +
  second writer (violates invariant 4); the journal already is the ledger.
- **Zoom levels as an enum.** Hard-codes the level set; the intermediate-level
  product question must stay open (hard requirement).
- **OS workspaces / virtual desktops as the window-set substrate.** Ruled out
  by research on every OS (macOS Spaces need SIP off; Windows VD COM breaks
  2–3×/year; portal-less on Linux). Own layer via hide/show; KDE Activities as
  optional native integration.
- **OS DND as the focus-contract source.** Unreadable via public API on macOS
  and Windows; the contract must be legible and Holon-owned anyway.
