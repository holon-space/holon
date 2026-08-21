---
id: 2026-08-21-sorted-streaming-list-duplicates-a-row-after-reorder
date: 2026-08-21
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Any SORTED streaming collection corrupts itself after its first sort-induced
  reorder: the next indexed update writes its row over an unrelated entry, so
  one row renders TWICE and another silently disappears. Seen in the Journals
  feed as the same day (`2026-08-20`) painted as two adjacent day sections;
  healed on restart.
---

## Bug
Martin, dogfooding the LogSeq-look Journals feed on 2026-08-20, saw the day
`2026-08-20` rendered as TWO adjacent day sections (screenshot: two identical
`2026-08-20` h2 headings, each with its own section body). A restart healed it.

Data corruption was ruled out first: the live DB's `journal_day_pages` had 30
rows and zero `HAVING count(*) > 1` duplicates, and the vault held exactly one
`Journals/<date>.org` file per date. So the duplicate existed ONLY in the live
render — a transient, session-local defect.

## Root cause
`crates/holon-frontend/src/reactive_view.rs` `create_flat_driver` keeps ONE
`entries: Arc<Mutex<Vec<(RowKey, Row)>>>` serving two incompatible roles.

Every `VecDiff` arm addresses `entries` by the UPSTREAM index, so it must stay
an exact positional mirror of `data_source.keyed_rows_signal_vec()`:

- `entries.lock().unwrap()[index] = (key, row)` — `UpdateAt`, :2041
- `entries.lock().unwrap().insert(index, ...)` — `InsertAt`, :2064
- `entries.lock().unwrap().remove(index)` — `RemoveAt`, :2075

But `full_rebuild` sorted that SAME vec IN PLACE by the collection's
`sort_key` (`lock.sort_by(...)`, :1995 pre-fix). The first sort that actually
reorders permutes the mirror away from upstream; the next `UpdateAt { index }`
then writes the updated row over an unrelated entry — that entry's row vanishes
and the updated one appears twice.

Prod sequence for the journals feed (`sortkey: "-content"`, date DESC): the
`daily_journal` rule mints the new day at the midnight rollover, which arrives
as a trailing `Push`; the rebuild's sort moves it to the FRONT and breaks
alignment. The org write-back of `Journals/<day>.org` then updates that same
block — an `UpdateAt` at its upstream trailing index, which clobbers the OLDEST
day. Restart heals it because boot is a single `Replace`, which re-establishes
the mirror.

Not journals-specific: it corrupts ANY collection with a `sort_key` whose
display order differs from arrival order.

Red-for-the-right-reason, with the in-place sort restored
(`crates/holon-frontend/src/reactive_view.rs`
`tests::flat_driver_sorted_feed_survives_update_after_reorder`):

```
assertion `left == right` failed: a sorted streaming `list` must stay a faithful
mirror of its upstream row set across an update that follows a sort-induced
reorder — no day duplicated, none lost;
  left:  ["2026-08-20", "2026-08-20", "2026-08-19", "2026-08-18"]
  right: ["2026-08-20", "2026-08-19", "2026-08-18", "2026-08-17"]
```

The duplicated day AND the silently dropped oldest day both reproduce exactly
as Martin saw them.

## Missing piece
COVERAGE (primary): no test ever mutated a SORTED streaming collection after it
had rendered. The windowed journals PBT
(`frontends/gpui/tests/gpui_journals_logseq_look.rs`) grafts every day BEFORE
the feed view exists, so the feed's first and only emission is a single
`Replace` — no indexed delta is ever produced, and the desynchronised-mirror
state is unreachable. The flat-driver unit tests likewise only covered a sorted
boot snapshot, never an update following a reorder.

ORACLE (secondary): had the state been reached, nothing would have gone red.
The windowed assertions count materialised days (`>= 2`), floored sections
(`>= 2`) and painted creation slots — none of them assert that the painted day
set is a duplicate-free, loss-free mirror of the feed's rows.

## Remedy
FIXED. `full_rebuild` now sorts a COPY:
`let mut ordered = entries.lock().unwrap().clone();` — the mirror stays
positionally aligned with upstream, and the display sort is a pure
presentation-time reordering.

Gap closed on both sides:

- COVERAGE, as a PROPERTY: `sorted_collection_converges_on_its_row_set_after_every_change`
  (`crates/holon-frontend/src/reactive_view.rs`) states the whole convergence
  contract rather than the one dogfood path — generated sequences of
  `holon_api::Change` (upsert/delete over a small id alphabet and an
  even smaller sort-value domain, so reorders and ties are frequent) against a
  generated ascending/descending `sort_key`, asserting after EVERY change that
  the rendered order equals an independently maintained model of the live row
  set. Generation is at the `Change` level, not synthesised `VecDiff`s, because
  `Change` is what production feeds the provider — the provider decides which
  diff arm it becomes, so the property covers the reachable arms rather than a
  hand-built input production never emits.

  With the in-place sort restored it goes red and SHRINKS to a 3-step, 2-row
  counterexample far smaller than the dogfood sequence:

  ```
  step 2 (Upsert { slot: 0, value: 0 }) under sort_key "sortval":
    the collection never converged on its row set.
      expected: ["row-0", "row-1"]
      observed: ["row-0", "row-0"]
      ops so far: [Upsert{slot:0,value:1}, Upsert{slot:1,value:0},
                   Upsert{slot:0,value:1→0}]
  ```

  Two rows, one reorder, one update: `row-0` duplicated and `row-1` dropped.
  Unsorted collections are deliberately out of scope — with no `sort_key` the
  driver never reorders, so the mirror has nothing to desynchronise from.
- COVERAGE, as the hand-authored pin:
  `flat_driver_sorted_feed_survives_update_after_reorder` keeps the exact prod
  sequence — sorted boot snapshot, rollover `Push` that sorts to the front,
  then an `UpdateAt` on that row. Red as quoted above; green after the fix
  (holon-frontend 551/551 including the property).
- COVERAGE + ORACLE, windowed: `gpui_journals_logseq_look.rs` now mutates the
  feed while it is LIVE — it grafts a day that sorts FIRST and then renames it,
  so the collection receives indexed `Push`/`UpdateAt` deltas instead of only
  the single `Replace` a pre-seeded feed emits. That was the missing parity.
  REQ5 asserts the EMPTY day's section survives a mutation aimed at ANOTHER
  day. Red with the in-place sort restored:

  ```
  REQ5 live-mutation faithfulness: the EMPTY day jday-empty lost its section
  when ANOTHER day was grafted and renamed in the live feed.
  ```
  (`[journals-logseq] empty_slot` goes true → false; green run has
  `empty_slot=true`.)

### Harness note: the windowed drive needed a convergence loop
Adding the live mutation made the frame's readiness marginal. `settle()` stops
at the first STABLE element count, but the streaming creation slot arrives
later — `AppendedRowsProvider` appends it once its inner stream resolves — so
one run in eight read a frame where the empty day's section had not landed and
failed REQ5/REQ3 with the fix correctly in place. The final snapshot now
re-settles and re-snapshots until that slot is present (capped), and every
assertion reads that one converged frame: 6/6 green after, versus 7/8 before.

The loop does NOT mask the defect — with the in-place sort restored it spends
its full budget and REQ5 still goes red, which is the check that the wait is
waiting for convergence rather than assuming it.

Measured and rejected along the way: moving the empty day to the TOP of the
feed also removes the flake, but it destroys REQ5's ability to go red at all.
The late day's position is load-bearing — pushing the feed down by one section
is exactly what makes the empty day the row the corrupted mirror destroys.

### What the windowed rung structurally CANNOT see

The DUPLICATE half is unobservable from the windowed harness, measured both
ways rather than assumed:

- `rendered_elements` comes from the bounds registry, which keys elements by
  `el_id` — a block painted twice registers once.
- `widget_tree_snapshot` is a sync RE-DERIVATION from the query, not a read of
  the corrupted `ReactiveView.items`. Instrumented under the reverted fix, it
  reported the feed's `list` container holding all five days exactly once, in
  correct date-DESC order, while the app was in the corrupted state.

So the windowed rung witnesses only the destroyed-day half. The duplicate is
pinned one rung down, at
`holon_frontend::reactive_view::tests::flat_driver_sorted_feed_survives_update_after_reorder`,
which asserts the rendered order equals the upstream row set and shows both
halves at once. Closing the windowed blind spot would mean giving the harness a
capability that reads the live collection's item list rather than re-deriving
it — worth doing if this class recurs, not done here.

Audited the only other `sort_by` in the file (:2294, board lane bucketing): it
sorts freshly-cloned per-bucket vecs, not the index-addressed mirror, so it is
unaffected.
