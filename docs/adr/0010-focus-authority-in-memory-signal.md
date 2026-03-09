# ADR 0010: Editor focus is pure in-memory UI state (not persisted to Turso)

**Status:** Proposed (2026-06-09)
**Deciders:** Martin
**Context:** gpui click-to-focus steal-back bug (seed 1780986415, Step 16); a
dual-authority race between in-memory focus and the SQL `current_editor_focus`
matview.
**Relates to:** ADR 0001 (hybrid sync architecture), ADR 0003 (all-in-LoroTree)
**Implementation:** see the focus-authority plan (tasks T1–T8) for concrete sites,
sequencing, and the validation gate.

## Terminology — two distinct "focuses"

The word "focus" names two unrelated subsystems. This ADR decides only the second.

1. **Navigation focus (region target)** — *which page/block is displayed in a
   region*. Backed by `current_focus` = `navigation_cursor` JOIN
   `navigation_history`. **Has history** (back/forward are cursor moves). Out of
   scope here.
2. **Editor focus (active editor + caret)** — *which editor inside the displayed
   collection holds the caret, and at what offset*. Backed by
   `current_editor_focus`, a pass-through matview over `editor_cursor`
   (`region TEXT PRIMARY KEY → block_id, cursor_offset`). **No history.** Mirrored
   in memory as `UiState.focused_block: Mutable<Option<EntityUri>>`. **This is the
   subject of this ADR.**

## Problem

Clicking a non-focused block in the gpui app intermittently fails to move keyboard
focus: focus lands on the clicked block, then is *stolen back* to the previously
focused block a frame or two later (seed `1780986415`, Step 16). The behaviour is
non-deterministic — a cross-thread, cross-process race.

### Root cause

There are three representations of editor focus, and window keyboard focus is
driven by reading **back** from the slowest one:

- **Logical focus** — `UiState.focused_block` (in-memory `Mutable`), set
  *synchronously* by a click's dispatch mirror. Drives the render-variant switch
  (`is_focused` → `editable_text`), chord routing, and `inv-focus-matches-ref`.
- **Window focus** — gpui `window.focus(handle)`; which `InputState` gets keys.
- **SQL `current_editor_focus`** — persisted, `async`, IVM-derived.

Window focus is driven off the SQL matview's CDC stream (`watch_editor_cursor` →
per-editor cursor subscription → `window.focus`). The write is an `async`
`INSERT OR REPLACE INTO editor_cursor`, propagating through four lag-prone hops:
SQL write → Turso IVM matview → CDC stream → tokio bridge → editor subscription.

`editor_cursor` is keyed `region PRIMARY KEY`, so there is exactly one row per
region and no stale row to re-emit. The observed "stale" value (`ref-doc-0`,
cursor 38) is **Step 15's own legitimate focus write delivered late** — it arrives
*after* Step 16's click already moved focus synchronously in memory. The
previously focused editor sees its own id in that late emission and re-grabs
`window.focus` (Mechanism A). A symmetric clobber happens in memory when the
tokio-thread bridge adopts the late value (Mechanism B).

### Why this is unpatchable at the SQL layer

The late emission is **indistinguishable by content** from a fresh one — it is a
real, correct row, "stale" only relative to the in-memory timeline. CDC lag is
unbounded, so a previous-focus delivery can always arrive after a newer click.
Every discriminator attempt (`last_bridge_focus` identity match; `updated_at`
timestamp guard) tries to reconstruct an ordering **only the in-memory authority
knows**. Two point patches regressed this way. The approach is structurally
doomed, not merely hard.

## Decision

**Editor focus and caret position are pure in-memory UI state. The in-memory
reactive signal is the single authority; editor focus is *not persisted to Turso
at all*. There is no `editor_cursor` write, no `current_editor_focus` read-back,
and no boot-restore of focus.**

Rationale: a user does not expect the caret to reappear mid-block after a restart,
so persisting it buys nothing — and *every* version of the steal-back bug exists
only because focus round-trips through Turso. Removing the round-trip **dissolves
the bug class** instead of taming it.

Concretely:

- **Authority for *which block*** = `UiState.focused_block`. All focus-moving
  operations mutate this signal directly.
- **Authority for *caret offset*** = the editor's own gpui `InputState`. No new
  signal, no `CursorPosition` type, nothing in Turso.
- **Window focus is a pure function of the authority** — each editor grabs
  `window.focus` iff `focused_block == its row`, off the `focused_block` signal,
  never off a CDC stream.
- **No persistence, no restore.** On startup nothing is seeded; focus begins
  unset (or at a deliberate default such as the document title).
- **One path for all backends.** Focus already worked purely in memory with no
  query engine; this makes that the single path, removing the Turso/no-Turso split
  for focus.

This collapses the dual-authority race at the root: with no SQL focus state to
read back, a late CDC delivery has nothing to steal.

### Editor focus is single-region today

`editor_cursor` is keyed by region, but only `'main'` is ever written (the live
bridge hardcodes `WHERE region = 'main'`). `focused_block` is therefore a single
global `Mutable`, not a per-region map, and that is correct. If multi-region
editor focus ever becomes real, the signal graduates to
`MutableBTreeMap<Region, Option<EntityUri>>` — a separate ADR.

## Design constraints (decision-level)

These are the non-obvious obligations the decision creates; the plan turns each
into concrete edits.

1. **Window focus follows the signal.** Generalize the existing first-mount grab
   (which already reads `focused_block` synchronously) into a live subscription on
   the signal, and delete the cursor-subscription `window.focus` grab — that grab,
   firing on the late CDC emission, *is* Mechanism A. It must be removed, not
   authority-checked (an authority-checked version was the closest-to-failed prior
   patch).

2. **Backend follow-ups must reach the signal without a CDC round-trip (the
   crux).** The bridges are not gratuitous: the split/join follow-up `editor_focus`
   runs in the **backend** operation dispatcher, which has no handle to the
   frontend `UiState`. Mirroring `editor_cursor → set_focus` is how the bridge
   breaks the resulting deadlock (no editor exists yet for the new block → nothing
   grabs focus → `focused_block` never advances → editor never mounts). So the
   leak must be **closed at its source**: the follow-up's *focus* effect reaches
   the frontend mirror in-process (re-dispatched intent or an explicit focus-intent
   channel), never a second CDC-shaped read-back. Only then are the bridges dead
   weight and safe to delete. **Sequencing is load-bearing: mechanism first,
   bridge removal last** — removing the bridges first re-opens the deadlock.

3. **The initial caret offset needs an in-process carrier.** The offset write
   bought exactly one thing: the caret's *initial* position on mount — caret-at-end
   (`content.len()`) for a click, caret-at-start (`0`) for a split/join. With the
   matview gone, the follow-up/click path must carry **(block, offset)** to the
   mounting editor's `InputState`. Dropping this silently regresses the caret to
   its default position; it needs an explicit test.

4. **The `editor_focus` write is shared beyond gpui.** It is a cross-frontend
   operation: **gpui *and* TUI** dispatch it, and several test drivers / the PBT
   reference write it. There are also two divergent read-only-click handlers in
   gpui that disagree on offset and on whether they set the signal synchronously;
   they should be unified. Removing the write is a multi-frontend change, not a
   single edit — the plan enumerates every site, and TUI must migrate
   symmetrically or its focus silently relies on a removed write.

5. **Clear-on-delete becomes the signal owner's job.** IVM recomputation kept
   focus consistent implicitly; prod does not clear `focused_block` on delete today
   (only the PBT reference model does). A dangling in-memory focus is still wrong
   (chord routing / variant switch would target a deleted block), so clearing — or
   moving to a sibling — is net-new prod code with enumerable edge cases (last
   child, only block, focus in a non-active region).

6. **MCP/debug reads must read the signal.** Surfaces that read
   `current_editor_focus` (`describe_ui`, raw-SQL helpers) must read editor focus
   from the signal; per "fail loud, never fake", none may present a stale or empty
   removed matview as truth.

## Scope

In: editor focus (#2). Out: the general "cache-ify all matview reads" migration;
navigation focus (#1); other persisted UI state (panel sizes, collapse, drawer).

## Alternatives considered

- **SQL-layer discriminator (timestamp / `last_bridge_focus`).** Structurally
  impossible (see "Why unpatchable"). Two patches regressed this way.
- **Authority-check the SQL-driven grab** (grab iff `focused_block == row`).
  Addresses Mechanism A only, leaves the bridges and their cross-thread race;
  closest to an already-failed patch.
- **Single-thread the `focused_block` writes** (bridge onto the gpui main thread).
  Removes the cross-thread race but not Mechanism A, and is blocked by the headless
  path, which has no gpui thread. Subsumed by deleting the bridges entirely.
- **Keep persistence, make Turso a write-through cache + boot-restore** (an
  earlier draft of this ADR). Rejected: it still has to manage cache lag, write
  ordering, and a stale-caret residual, and it persists state users don't expect
  to survive restart. Not persisting at all is strictly simpler.

## Consequences

**Positive:**
- The steal-back race is removed by construction — no SQL focus state to read back.
- The stale-caret residual disappears for the same reason.
- Two duplicate cross-thread bridges (~80 lines of clobber logic), the
  `editor_cursor` table, and the `current_editor_focus` matview are deleted.
- First-mount grab and live grab become one rule; focus is single-path across
  backends, aligning with the no-Turso direction.

**Negative / risks:**
- **The follow-up mechanism (constraint 2) is net-new and is where this can
  fail.** With no Turso fallback, it is the *only* path — no safety net. Sequencing
  is load-bearing.
- Clear-on-delete is net-new prod code with edge cases; must be tested.
- Removing the table/matview is a multi-frontend change (TUI included) and must be
  gated on an exhaustive reader/writer sweep, including any chained matview (cf.
  the project's chained-matview-hang history).
- Editor focus no longer survives restart — intentional, but a behaviour change
  from the persisted status quo.
- Establishes a precedent ("editor focus is pure in-memory UI state") and an
  explicit non-precedent ("this is *not* a general UI-state cache").

## Future work (recorded, not decided here)

### Navigation focus (#1)
If it later adopts an in-memory model, its history needs a reactive sequence.
`futures-signals 0.3` has no purpose-built log type, but `MutableVec<T>` /
`SignalVec` + a `Mutable<usize>` cursor models it cleanly (navigate = truncate +
push + cursor→last; back/forward = cursor ∓ 1). Deriving the current target from
`(SignalVec, cursor)` is awkward in pure combinators; since navigation is
low-frequency, maintain a derived `Mutable<Option<EntityUri>>` imperatively. No
bespoke type needed.

### Cursor presence (collaborative)
Peers seeing each other's caret is **presence/awareness**, not persistence. The
local caret stays local (`InputState` + `focused_block`), never shared CRDT state.
Loro contributes (a) cursor *anchoring* — already in `LoroTextCellBacking`
(`anchor_cursor`/`resolve_cursor`), nothing new — and (b) *ephemeral awareness*
for broadcasting. **Do not model this as `CellBacking<CursorPosition>`:**
`CellBacking<T>` is single-value and symmetric (you read what you write), whereas
presence is multi-peer and asymmetric (write only your own, read everyone else's).
The fitting shape is a per-peer map beside `CellBacking`, reusing `CursorAnchor`:

```rust
trait CursorPresence {                       // ephemeral, asymmetric, multi-peer
    fn set_local(&self, anchor: CursorAnchor);                 // broadcast MY caret
    fn peers(&self) -> /* SignalMap<PeerId, CursorAnchor> */;  // render THEIRS
}
// future impl: LoroEphemeralPresence over a LoroEphemeralStore
```

### Deliberate UI-state persistence
"Should this survive restart?" is a per-field decision — an argument against a
blanket UI-state cache. Caret/editor focus → no (this ADR). Panel sizes, collapse
state, last-viewed page → plausibly yes, designed deliberately per field.
Navigation focus is the more interesting first candidate.
