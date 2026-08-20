---
id: 2026-08-20-nested-list-last-item-creation-slot-not-painted
date: 2026-08-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A creation slot appended as the LAST row of a nested virtualized `gpui::list`
  is present in the reactive data for every collection but is NOT painted (no
  bounds registered) for collections that have real rows — the documented
  dogfood-#3 "creation slot unreachable / list summary().height under-counts the
  final item(s)". Empty collections (slot is the sole/first row) paint it; a
  collection with content does not paint its trailing slot.
---

## Bug
Building the LogSeq journals feed, each day's content is a nested collection
(`live_query(from descendants)` → tree) whose creation slot is the trailing
row. Across settled windowed frames the slot painted for some days and not
others; empty days always painted it, non-empty days usually did not — even on a
fully-settled frame (`floored=3 materialized=4`, five stable settle iterations).

Found by agent exploration (this lane) + confirmed by the fresh-context
verifier (`journals-verify.md`), not by Martin.

## Root cause
Two-part, isolated by instrumentation:

1. STREAMING EMISSION (fixed in this lane): `AppendedRowsProvider` chained the
   slot onto the inner rows via `SignalVec::chain`, and futures-signals `chain`
   drops the trailing slot when a live inner emits `VecDiff::Replace`. Replaced
   with an atomic recompose (fold the slot into the inner's own signal). After
   that fix, a DATA-vs-PAINT probe in the windowed test showed the slot present
   in the reactive widget tree for EVERY day:

   ```
   DATA slots (snapshot): jday-000, jday-001, jday-002, jday-empty, <today>   (all)
   PAINT slots (elements): jday-002, jday-empty, <today>                      (subset)
   ```

2. PAINT / LAYOUT (this entry, NOT fixed): the slot is in the data for
   jday-000/001 but never painted — no entry in `rendered_elements`
   (BoundsRegistry). This is the dogfood-#3 issue the reactive_shell already
   documents inline (`frontends/gpui/src/views/reactive_shell.rs` ~L47-70):
   "creation slot is unreachable — the list's `summary().height` under-counts
   the final item(s)". The nested per-day virtualized `gpui::list` does not lay
   out its last row (the slot) at full height, so it registers no bounds. Empty
   days paint because the slot is the sole/first row, not a trailing one.

## Missing piece
ENVIRONMENT: the nested virtualized `gpui::list`'s `summary().height` under-count
means the final row is never laid out/painted; the `padding.bottom`
(`LIST_END_PADDING`) workaround addresses scroll reachability, not paint. The
"measure the last item at full height" true fix is flagged OPEN in
`reactive_shell.rs`.

## Remedy
- OPEN — NOT fixed here. Fix is out of this feature's scope (broad blast radius:
  every collection's trailing row).
- The journals feature does NOT depend on it: per Martin's JRN-2 ruling the
  creation bullet affords ONLY on EMPTY journal days (the slot is then the sole
  row and paints), so `resolve_creation_parent` resolves an explicit container
  to `None` for a non-empty rowset — a non-empty day emits no trailing slot at
  all. The batch PBT `gpui_journals_logseq_look.rs` hard-asserts BOTH (empty day
  paints; non-empty days paint no slot), so it would catch a regression of this
  bug backwards if a non-empty day ever started emitting a trailing slot.
- The true fix (last-item measurement) needs a windowed PBT that asserts a
  trailing row of a NON-empty nested collection paints — red on the current
  tree, green after the list measures its final item at full height.
