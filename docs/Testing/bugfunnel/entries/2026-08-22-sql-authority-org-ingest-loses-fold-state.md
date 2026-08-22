---
id: 2026-08-22-sql-authority-org-ingest-loses-fold-state
date: 2026-08-22
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `inv-blocks-match-ref` was VACUOUS on `collapsed` / `widget_only` — neither SUT
  snapshot SELECT carried the columns, so every parsed Block reported
  `collapsed = false` and the parity scenario red on a store that was already
  correct.
---

## Bug

Un-`@wip`ing the `logseq-parity` log:4 ingest scenario reds under the shipped
default wiring:

```
authority: block-CRUD=Sql(SqlOperationProvider); projection-sinks=Sql(block_raw,matview); org-writeback=on
inv-blocks-match-ref/matview: block:folded-parent  collapsed: sut=false ref=true
```

Found in lane `collapsed-bug` while un-`@wip`ing that scenario as the
corpus-level proof of the two Loro-path drops recorded in
[2026-08-22-org-ingest-drops-collapsed-into-property-bag](2026-08-22-org-ingest-drops-collapsed-into-property-bag.md)
and
[2026-08-22-loro-create-projection-drops-fold-state](2026-08-22-loro-create-projection-drops-fold-state.md).

**The store was never wrong.** This is a defect in the TEST HARNESS's readers,
not in production. Recorded because it is the reason the two real drops above
went undetected for so long, and because an invariant that silently cannot
observe a field is worse than no invariant at all.

## Root cause

`crates/holon-integration-tests/src/pbt/sut_row_parsing.rs` — neither snapshot
SELECT carried the column:

* `BLOCK_MATVIEW_SNAPSHOT_SQL` (backs `SutBackend::live_block_snapshot`, the
  `inv-blocks-match-ref/matview` reader)
* `BLOCK_RAW_SNAPSHOT_SQL` (backs `SutBackend::block_raw_snapshot`)

and `parse_block_row` never assigned `collapsed` / `widget_only`. So EVERY
`Block` those readers produce carried `collapsed = false` **by construction**,
for every block, on every wiring, regardless of the database. Comparing that
hardcoded `false` against a reference that correctly holds `true` reds on any
folded block — and, symmetrically, the invariant could never have caught a real
fold regression either.

MEASURED, in the composed harness under the failing scenario, each step a probe:

| where | value |
|---|---|
| `block_create_request` | `collapsed = true` |
| `flush_pending_creates` | `persisted = false`, `params.collapsed = Some(Boolean(true))` |
| batch op into the provider | `op = create`, `held = false`, `params.collapsed = Some(Boolean(true))` |
| emitted SQL | `INSERT INTO block_raw (… "collapsed" …) VALUES (…, 1, …)` |
| DB read immediately after commit | `collapsed = Some(Integer(1))`, rowcount 32 |
| any single-op writer (`execute_operation`) | never fired |
| the invariant's own reader, same tick | `collapsed = Some(false)`, rowcount 31 |

The rowcount moving 32 → 31 is what proved it was ONE database rather than two
stores, which excluded "a writer downstream" and pointed at the reader. The
after-commit read used an explicit `SELECT collapsed`; the invariant's read went
through the snapshot SQL. Same row, same DB, opposite answers — the difference
was the SELECT list.

SEVEN hypotheses died by measurement before this one, all recorded so nobody
re-runs them: the ingest seam; "block_raw green, matview stale"; the
docstring→render round trip; cold-boot vs live-watcher ingest; a post-ingest
clear; a document-UPDATE carrying a block-CREATE (probed with a matched control,
both arms 1/1); and an alternative materialisation route (refuted at source —
one `SutFixtureFs` impl, and `WriteOrgFile` has no branch).

A CLASS REVERSAL is recorded rather than quietly corrected: an earlier revision
of this entry said **PRODUCT**, on the strength of the composed harness writing a
real file whose bytes carried `:COLLAPSED: t` through the production path. That
observation was correct and the conclusion drawn from it was not — "the write is
correct AND the value reads wrong" has a third explanation beyond writer and
store, namely the reader, and it was not enumerated until the row COUNT
disagreed too. There is no import-time data loss.

## Missing piece

ORACLE, in the strict sense: the invariant exists, selects, runs, and is
**vacuous** on these fields. `inv-blocks-match-ref` advertises a field-by-field
`Block` comparison; two of those fields could not be observed by either store
arm. Both real drops (the sibling entries) were found by a test that issues its
own explicit `SELECT collapsed` — never by this invariant, which could not have
seen them.

Compounding it, the `/block_raw` arm compares only `{Content, Properties,
Marks}` (`compare_block_raw_subset`, `holon-turso-testing/src/correspondences.rs`),
so when the `/matview` arm fires on `collapsed` the silence of the `/block_raw`
arm says nothing — a trap that cost this investigation two wrong turns.

## Remedy

FIXED in lane `collapsed-bug`, in the harness only — no production file changed.

* `collapsed, widget_only` added to BOTH snapshot SELECTs.
* `parse_block_row` assigns both, via `required_sql_bool`, which PANICS on an
  absent column and names the two constants to fix. Deliberately fail-loud
  instead of defaulting: defaulting is exactly how this stayed invisible, and a
  silent `false` is the "silently degrades to look fine" outcome the repo's
  error ladder forbids outright.

Red → green, and the shape of the green IS the proof: the parity replay goes
from `2 replayed, 4 skipped` (scenario deselected) to `3 replayed, 3 skipped`
with the whole replay passing, **with zero production changes**. A production fix
could not have produced that; only the reader could.

Neutrality measured, not assumed: `holon-integration-tests --lib` reports
`377 passed; 9 failed` on the landed base AND with this change, with byte-identical
failing test names — the 9 are the documented region-literal known-red family
(`docs/Testing/KeystoneKnownReds.md:162-167,181`), untouched here.

## Full-field audit

Every typed field of `Block` against what the matview arm's comparator
(`compare_block_fields`) compares, what each table stores, and what each of the
THREE harness readers selects. Done because fixing one blind field is worth
little if its siblings are blind too.

| `Block` field | compared? | stored | matview reader | block_raw reader | `SutOrgRender` |
|---|---|---|---|---|---|
| `id` / `parent_id` / `content` / `content_type` / `source_language` | yes | yes | yes | yes | yes |
| `properties` / `marks` | yes | yes | yes | yes | yes |
| `source_name` | yes | yes | **ADDED** | **ADDED** | already |
| `collapsed` | yes | yes | **ADDED** | **ADDED** | already |
| `widget_only` | yes | yes | **ADDED** | **ADDED** | **ADDED** |
| `created_at` / `updated_at` | normalized away | yes | no — declared | no — declared | already |
| `tags` / `requires` / `advice_suppressed` / `contributes_to` | yes | junctions | yes | n/a | already |

Three readers, not two — the third was found by the verifier:
`SutOrgRender` (`frontend_slice/components.rs:2081-2089`) runs its OWN header
SELECT and parses with `Block::try_from`, whose `optional_bool`
(`holon-api/src/block.rs:817-826`) defaults an absent column to `false` BY
DESIGN. It listed `b.collapsed`, `b.completed`, `b.block_type` but not
`b.widget_only`, so it rendered every widget-only block as ordinary. Now added.
Measured before AND after that widening, because it touches the very field the
seed corpus differs on: `pbt::composed::live_mcp::tests::seed_wide_stays_aligned`
passes in BOTH states (`lane-logs/seedwide-BEFORE.log`, `seedwide-AFTER.log`).

SCOPE OF EACH ARM, so the widening is not over-read: the `/block_raw`
correspondence arm compares only `{Content, Properties, Marks}`
(`compare_block_raw_subset`, `holon-turso-testing/src/correspondences.rs:187-208`).
Fold state is therefore observed by the `/matview` arm ALONE; widening the
`block_raw` SELECT does not add a comparison, it only feeds `required_sql_bool`
so that a future edit dropping the column fails loud instead of silently.

DECLARED BLINDNESS, stated rather than silent:

* `created_at` / `updated_at` — `normalize_block` (`block_compare.rs:75-76`)
  zeroes BOTH sides before comparison, so selecting them is inert either way.
* The `/block_raw` arm cannot see the junction edge fields because `block_raw`
  does not store them — the documented subset, not a defect.

CORRECTION to an earlier revision of this entry, which listed `task_state`,
`priority`, `completed`, `block_type` and `sort_key` as further vacuities. That
list was wrong, and the error was mine — it came from grepping the parser rather
than reading the schema:

* `task_state` is `FieldStorage::Property` (`holon-pattern/src/schema.rs:210-214`)
  and `priority` is not in the schema at all. Both travel inside `properties`,
  which IS selected and IS compared (`normalize_block` keeps `task_state`,
  `block_compare.rs:113-126`). They are covered. `parse_block_row`'s
  `row.get("task_state")` / `row.get("priority")` branches are simply DEAD CODE
  against these SELECTs — worth deleting, but not a vacuity.
* `completed`, `block_type`, `sort_key`, `write_seq` are stored COLUMNS but not
  typed `Block` fields, so no `Block` comparison can involve them and they
  cannot be vacuous in `inv-blocks-match-ref`.

So `source_name` was the ONE genuine additional instance, and it is un-blinded
here alongside `collapsed` / `widget_only`.
