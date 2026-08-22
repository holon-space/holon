---
id: 2026-08-23-recompute-oracle-skips-static-matview
date: 2026-08-23
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `inv-matview-consistent-with-recompute` skipped `trust_proposals` on every
  run — a substring test read the `$` inside its JSON-path string literals as a
  bind placeholder — so that matview was never recomputed and any IVM defect on
  it was invisible to the keystone.
---

## Bug

The recompute oracle enumerates every materialized view from `sqlite_master`
and re-runs its defining SELECT, comparing the result against the matview's own
rows. It decided "this view is parameterised, skip with disclosure" with a
SUBSTRING test (`frontend_slice/components.rs`):

```rust
if select_sql.contains('?') || select_sql.contains('$') { … skip … }
```

`crates/holon-turso/sql/schema/trust_proposals_matview.sql` is a fully STATIC
SELECT over `block_raw` whose `json_extract(properties, '$._proposal.status')`
paths carry `$` inside SINGLE-QUOTED LITERALS. So it matched, and the view was
skipped every run — 198 `SKIP view trust_proposals` lines in one hand-authored
run. The invariant's promise ("every matview from sqlite_master is recomputed")
was false for it.

Found by lane `verify-replace` outside an automated test, so triaged here.

## Root cause

A substring test cannot tell a bind placeholder from a placeholder CHARACTER
inside a string literal or a comment. Measured red-for-the-right-reason on the
extracted predicate before any fix (`lane-logs/red-placeholder.log`), and the
red showed the predicate was wrong in BOTH directions — one more than the
report that prompted this:

* FALSE POSITIVE — `SELECT '$x' AS a` and the whole `trust_proposals` SELECT
  read as parameterised, and were skipped.
* FALSE NEGATIVE — `:name` and `@name` are real SQLite bind placeholders and
  contain neither `?` nor `$`, so such a view would have been RECOMPUTED with an
  unbound parameter rather than skipped. No such view exists today; the hole was
  open regardless.

## Missing piece

ORACLE, and specifically the drop-3 family: the READER decides what the
invariant can see. The invariant existed, selected, ran, and reported success
while never examining one of its subjects — the same shape as
`inv-blocks-match-ref` being vacuous on `collapsed`
([2026-08-22-sql-authority-org-ingest-loses-fold-state](2026-08-22-sql-authority-org-ingest-loses-fold-state.md)).
A skip is only honest if its predicate is.

## Remedy

FIXED in lane `collapsed-bug`, harness only — no production file changed.

`has_bind_placeholder` is now a literal-aware scanner: it walks the SELECT and
ignores single-quoted literals (`''` escapes), double-quoted identifiers, `--`
line comments and `/* */` block comments, reporting SQLite's four placeholder
forms (`?`, `?NNN`, `$name`, `:name`, `@name`) only OUTSIDE those. A hand
scanner, not a SQL-parser dependency. `placeholder_scan_tests` pins each case,
including the `trust_proposals` SELECT as a fixture, an escaped `''` quote, a
comment containing `?`, and a literal that CLOSES before a real `?`.

MEASURED after the fix: `trust_proposals` SKIP count 0, and NO view is skipped
at all — the disclosure line no longer fires for anything. `trust_proposals`
AGREES with its matview once recomputed (`just keystone-smoke` green, 4 passed),
so un-skipping it exposed no divergence — there is no second bug behind this one.

### Audit — every matview, recomputed or skipped-with-reason

Classification is by the scanner against each defining SELECT; the runtime
measurement is the SKIP-line count in a real run (now zero across the board).

| matview | sigils present | classified | why |
|---|---|---|---|
| `trust_proposals` | `$` ×9 | RECOMPUTED (was skipped) | every `$` is inside a `'…'` JSON-path literal |
| `journal_day_pages` | `:` | RECOMPUTED | `'block:journals'` — inside a literal; `YYYY-MM-DD` is in a comment |
| `matview_focus_roots` | `@` | RECOMPUTED | `nightscape@holon` — inside a `--` comment |
| `journal_feed` | none | RECOMPUTED | static |
| `automations_journal` | none | RECOMPUTED | static |
| `block_requirement_edges` | none | RECOMPUTED | static |
| `matview_current_focus` | none | RECOMPUTED | static |
| synthesized views (`block` matview, advice matviews, `reconcile_named_view` callers, sidecar views) | — | RECOMPUTED | present in `sqlite_master` and produced no SKIP line in the measured run |

Only `trust_proposals` changes classification. `journal_day_pages` and
`matview_focus_roots` were already recomputed under the old predicate (neither
carries `?` or `$`) and remain so — the scanner reaches the same verdict for the
right reason rather than by luck.

NOT closed by this entry: no view in the tree currently uses `:name`/`@name`
binds, so the false-negative half is fixed pre-emptively and is unexercised by
any live view. It is pinned by `placeholder_scan_tests` rather than by a corpus.
