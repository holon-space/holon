# Journal Feed as a Chained Matview — Plan (2026-07-18)

Ruling (Martin, 2026-07-18): **"journal-feed = chained matview; we need to be
able to rely on them on any platform."** The on-device reconcile bug that made
chained matviews risky was fixed today (commit `16fedd44`: `matview_manager`
no longer DROPs Turso-internal `__turso_internal` DBSP state).

## 1. Target UX

The **journal feed** is the content of the `block:journals` Page (reachable from
the left-sidebar "Journals" entry, seeded by `block_cell_registry`). Logseq-style:
a scrollable **reverse-chronological list of day pages, each inline-expanded**
with its own child blocks.

- Day page = a block tagged `Page` whose `parent_id = 'block:journals'`, its
  `content` being the date (`YYYY-MM-DD`, e.g. `2026-07-18`).
- Feed order = `content DESC` (newest day first).
- Each row renders via `render_entity()` with `expand_default = 1` so the day's
  children show inline (expansion is a *render* concern — the feed relation only
  needs the day-page rows).
- New-day auto-create is unchanged (the `daily_journal` `holon_rule` in
  `Journals.org`); the feed simply reflects whatever day pages exist and updates
  O(delta) as they are created/edited.

## 2. Chain design — TRUE chained matview (not logical)

### Hang-check evidence (the decisive design input)

The `turso-chained-matview-hang` skill (dated 2025-01-24) says matview-on-matview
hangs. **That is obsolete for the pinned Turso rev.** Evidence gathered on this
base (turso rev `3dd5d689`):

1. The fork ships `tests/integration/query_processing/test_ivm_chained_matview_reopen.rs`
   at that exact rev — `test_chained_matview_reopen_3_levels` creates a
   **3-level** chain (`current_focus` FROM tables → `focus_roots` FROM
   `current_focus` matview → `watch_view_eb3125` FROM `focus_roots` matview) and
   asserts all three populate. Chained matviews are supported and tested upstream.
2. **Holon already relies on chained matviews in production boot:** the `block`
   matview is chained on per-junction agg matviews; `block_with_path`,
   `block_requirement_edges`, and `focus_roots` are all matviews that select
   `FROM block` (the block matview). `query_engine` resolves block paths by
   reading the `block_with_path` matview directly. No hang.
3. The old app-level "skip queries that reference `blocks_with_paths`" workaround
   is **gone** from production code — reads through chained matviews are normal.

A runtime guard remains (`turso.rs::execute_ddl` bounds DDL with
`ddl_execution_timeout`) so that if a *future* rev regresses, the boot fails LOUD
with an attributed error rather than wedging the DB actor. Fail-loud, not silent.

**Conclusion: build a true chained matview.** This is the strongest possible
demonstration of the reliance the ruling asks for, and it matches every existing
matview in the tree.

### The chain

```
block_raw, block_tags   (base tables)
        │
        ▼
   block  (matview — hydrates tags/requires; already exists)
        │  JOIN block_tags (tag='Page'), WHERE parent_id='block:journals'
        ▼
 journal_day_pages  (matview — DAY-PAGE DETECTION)     ← new, stage 1
        │  + expand_default projection
        ▼
   journal_feed  (matview — FEED)                      ← new, stage 2
        │  SELECT * … ORDER BY content DESC
        ▼
  Journals.org holon_sql read → render list()
```

- **`journal_day_pages`** (matview): the day-page-detection layer. `block`
  matview JOIN `block_tags` (`tag='Page'`) WHERE `parent_id='block:journals'`.
  One row per day page (no fan-out: `block_tags` PK is `(block_id, tag)` and we
  filter a single tag). IVM-maintained O(delta) — exactly the `focus_roots`
  shape (block-matview JOIN a junction table). Reusable: a calendar view, a
  "jump to today", and journal backlinks can all read this relation.
- **`journal_feed`** (matview, chained on `journal_day_pages`): the feed
  projection. Adds `1 AS expand_default`. This stage is deliberately thin, and
  it stays thin: it is **not** a windowing seam — see the SUPERSEDED note below.

Ordering stays in the *read* query (`ORDER BY content DESC`), matching the
`automations_journal` convention (matviews are sets; the read orders).

### Why two stages and not one

The ruling + the "base → detection → feed" framing call for a chain, and the two
stages have distinct responsibilities (detection is reusable; feed-projection is
feed-specific). It also *exercises* matview-on-matview
for journal data, which is the reliability guarantee being asked for. See Open
Questions Q1 — a single matview is the alternative if Martin prefers minimalism.

## 3. Increments

- **Increment 1 (this lane):** `journal_day_pages` + `journal_feed` matviews,
  boot-owned via `SchemaModule` + DI providers; `Journals.org` reads
  `FROM journal_feed`. View-level tests (ordering, delta on new day) + seed-shape
  test + `keystone-smoke`.
- **Increment 2 — SUPERSEDED 2026-08-11, DO NOT BUILD.** It read: "feed
  windowing/pagination (LIMIT in `journal_feed` or a windowed read), 'load
  older' affordance."

  **A `LIMIT` in the feed — in the matview or in the read — is refused.**
  `crates/holon-turso/src/util.rs:43-48` states the contract in the source:
  "`LIMIT` / `OFFSET` are excluded: the matview holds the unbounded relation and
  its CDC stream delivers changes beyond any window, so re-applying a window to
  the snapshot alone would disagree with the stream." `strip_order_by`
  (`util.rs:36-41`) enforces it — a `LIMIT` written into the journals
  `holon_sql` is silently discarded (test `strip_order_by_also_strips_limit`,
  `util.rs:420-425`). That contract was written *after* this plan; where the two
  disagree, `util.rs` wins.

  **Ruling (Martin, 2026-08-11): windowing happens at the VIEW layer**, applied
  downstream of the unbounded CDC stream — the same discipline that already puts
  ordering in the render spec (`sortkey: "-content"`) rather than in SQL.
  `journal_feed` stays unbounded. The lazy term is not the row count (~365 thin
  rows/year, and `gpui::list()` already virtualizes painting) but the per-day
  content: `expand_default = 1` materialises a `live_query(from descendants)` —
  a `ReactiveShell`, a watched matview and a CDC subscription — for *every* day
  page. That is what viewport-driven expansion bounds.

  Adding cursor pagination (a window parameter through `QueryContext` /
  `execute_query` / `query_and_watch`, plus snapshot-vs-stream reconciliation
  for a windowed relation) is a new delivery protocol affecting every reactive
  surface, and needs its own ruling. See `plan-journals-view.md` §2.3 and §8.1.
- **Increment 3 (deferred):** per-day child-count / summary column on the feed
  (grouped aggregate on the detection layer, like `automations_journal`).

## 4. Out of scope

- Seed-vs-file authority for `block:journals` (BugFunnel row 25, `#[ignore]`d
  `journals_seed_file_collision.rs`) — a separate PRODUCT RULING. The feed
  matview reads whatever day pages exist; it neither causes nor fixes that bug.
- Changing journal auto-create (`daily_journal` rule) or the date/clock model.
- Calendar / date-picker UI.
