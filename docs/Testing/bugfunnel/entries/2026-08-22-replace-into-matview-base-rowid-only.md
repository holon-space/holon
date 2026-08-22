---
id: 2026-08-22-replace-into-matview-base-rowid-only
date: 2026-08-22
gap: COVERAGE
secondary: null
status: NOTED
summary: >-
  A Turso-fork IVM defect silently drops rows from any matview when an
  INSERT OR REPLACE rewrites an unchanged row of a rowid-alias base table; no
  holon production write has that shape, so the reported exposure is refuted
  and the risk is latent rather than live.
---

## Bug
The turso-1d peer reported that `INSERT OR REPLACE` / `REPLACE INTO`
overwriting an EXISTING row with an UNCHANGED value corrupts the incrementally
maintained matview over that table, in ANY view shape. A scout then listed four
holon production REPLACE sites over matview bases, making this look like live
corruption of `current_focus` and `backlinks` and of every sidecar cache table
on a poll timer.

Found by peer report plus code audit (lane `replace-fix`), not by a failing
test. This entry records the escape AND the refutation, because the reported
exposure did not survive measurement.

## Root cause
The engine defect is real. On the REPLACE path `Insn::Delete` captures the old
row for view maintenance and the following `Insn::Insert` (REQUIRE_SEEK branch)
captures it a SECOND time, so one replace emits two retractions against one
insertion and the row's weight falls to -1.

Measured in holon's own harness
(`crates/holon-turso/tests/replace_into_matview_base.rs`), the trigger is a
CONJUNCTION, narrower than reported:

1. the table is a **rowid-alias table** — a lone `INTEGER PRIMARY KEY`, in
   EITHER spelling SQLite accepts (`id INTEGER PRIMARY KEY`, or `id INTEGER,
   …, PRIMARY KEY (id)`); AND
2. the write is a REPLACE that resolves a conflict on that key.

**The key shape is the necessary condition; the unchanged value is not.** An
UNCHANGED value corrupts every view shape measured — projection, filter,
aggregate, INNER JOIN, LEFT JOIN. A CHANGED value additionally corrupts
AGGREGATE views as soon as it moves the row back into a group that existed
before (`a→b→a→b→a` is correct through the first change and diverges from the
second onward). Only the projection case is genuinely safe under changed
values.

An earlier revision of this entry claimed "both halves are load-bearing" and
that a rowid table replaced with a changed value is green. That was measured
only against a PROJECTION view and is false in general; adversarial
verification produced the aggregate counter-example, and tripwire #6 now pins
it so the other witnesses cannot go green while it survives.

The key shape still decides exposure: a TEXT-keyed or composite-TEXT-keyed
table is green in every cell measured, including the revisiting-group aggregate
sequence. So "ANY view shape" holds only within the rowid case — which is why
the peer's own repros, all written against `id INTEGER PRIMARY KEY`, all
reproduce.

**A second route to the same corruption, with no REPLACE in the write at all.**
SQLite lets a table declare its conflict action in the schema —
`CREATE TABLE t (id INTEGER PRIMARY KEY ON CONFLICT REPLACE, …)`, or
`UNIQUE(col) ON CONFLICT REPLACE`. This fork ACCEPTS that DDL, and every later
PLAIN `INSERT` then carries full REPLACE semantics and corrupts the matview
identically (measured: maintained `[]` vs recomputed `[["1","a"]]`). The
corrupting statement contains no "replace" text anywhere.

That is the reason the guard's statement-level fast path can never be its
boundary: this class is REPLACE semantics WITHOUT the word, so no amount of
statement inspection can see it. The hazard is declared in the DDL, so it is
refused at the schema seam instead — see the two schema hooks under Remedy.

**The observed signature differs from the reported one.** The peer reported a
loud read failure (`Invalid data in materialized view: expected a positive
weight, found -1`). Through `DbHandle::query` the read SUCCEEDS and returns
ZERO rows — silent data loss, no error. Anything hunting this by grepping for
the weight message will not see it through holon's query path.

Why no holon production write is exposed, site by site:

- `navigation_cursor` (`crates/holon/sql/navigation/upsert_cursor.sql:1`,
  `set_cursor_to_history.sql:6`) — `region TEXT PRIMARY KEY`
  (`crates/holon-turso/sql/schema/navigation.sql`). Measured green against the
  production `current_focus` SELECT using the production upsert statement.
- `navigation_history` — has NO production REPLACE. The two cited statements
  (`crates/holon/src/api/backend_engine.rs`) are inside `#[cfg(test)] mod
  tests`, which opens at line 1270.
- `block_links` (`crates/holon/src/core/sql_operation_provider.rs:1762`) —
  composite `PRIMARY KEY (source_block_id, target, kind)`, all TEXT, and the
  writer `DELETE`s by source first so the replaces usually conflict with
  nothing. Measured green on the production write sequence.
- vtable cache tables (`crates/holon-mcp-client/src/mcp_vtable.rs:1012`) — the
  writeback IS timer-driven and rewrites unchanged rows by design (its own
  comment at `:994`: "the next refresh repairs (INSERT OR REPLACE is
  idempotent)"), and sidecar-declared views over these tables DO become real
  matviews (`crates/holon-mcp-client/src/mcp_integration.rs:961`). But every
  `write_through` entity's primary key is TEXT, so none is a rowid-alias table.

## Missing piece
No test exercised the engine's **write-form × key-shape** matrix, so holon had
no way to know whether its own REPLACE statements were safe — the question was
answerable only by measurement, and nothing measured it. That is a COVERAGE gap
at the engine seam, not at the keystone.

It is specifically NOT an oracle gap, and that was checked rather than assumed:
`inv-matview-consistent-with-recompute` enumerates every `CREATE MATERIALIZED
VIEW` from `sqlite_master`, reads each view AND re-executes its stored defining
SELECT, and compares sorted multisets
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2551`).
`current_focus` and `focus_roots` are covered automatically, and the corruption
presents exactly as that oracle's comparison — matview missing a row the
recompute returns.

**Caveat measured while verifying this, worth stating because it bounds the
argument:** that invariant's ENGAGEMENT is draw-dependent. Adversarial
verification recorded ZERO engagements of
`inv-matview-consistent-with-recompute` in its `just keystone-smoke` run, while
this lane's run of the same gate recorded `21/21` — same gate, different
proptest draws. So no single keystone-smoke run evidences the oracle's reach;
the claim rests on `just hand-authored`, whose deterministic replay engages it
in every case (2/2 through 23/23 across the recorded cases). The keystone stays green because the production writes are
not the corrupting shape, not because it cannot see the shape.

**Correction, found by adversarial verification:** the supporting check "no
`matview_*.sql` carries a `?`/`$` placeholder" was scoped to files matching
that name, and matviews are created from other files too. The skip predicate is
a naive substring test (`select_sql.contains('?') || select_sql.contains('$')`,
`components.rs:2595`), and
`crates/holon-turso/sql/schema/trust_proposals_matview.sql` is a fully STATIC
SELECT whose `json_extract(properties, '$._proposal.status')` paths contain a
`$` inside a string literal. The oracle therefore skips the `trust_proposals`
matview entirely — observed 198 times in one `just hand-authored` run. Its base
`block_raw` is `id TEXT PRIMARY KEY`, so no corruption follows here, but this is
a REAL pre-existing oracle hole of the same reader-vacuity family: a `$` in a
literal silently removes a view from the differential. Recorded as its own
finding; not fixed in this lane.

**The latent risk worth naming:** `focus_roots` is a projection over
`navigation_history`, which is `id INTEGER PRIMARY KEY AUTOINCREMENT` — holon's
one rowid-alias matview base. Before this lane it was protected by nothing but
the absence of a production REPLACE into it. One such statement would make
`focus_roots` silently drop rows and blank the main panel. The guard below now
enforces that absence rather than relying on it.

## Remedy
- **Engine-seam matrix** — `crates/holon-turso/tests/replace_into_matview_base.rs`:
  **22 green pins**, including the production `current_focus` and `block_links`
  write sequences (production DDL via `include_str!`, production statements read
  off disk so they follow the source), plus **8 `#[ignore]`d** red-on-this-pin
  witnesses. Five cover the peer's projection/filter and aggregate repros and
  the INNER JOIN and LEFT JOIN cells the peer had not verified — joins fail on
  the same trigger, so there is no second engine defect. Adversarial verification added the other three:
  `table_constraint_rowid_alias_corrupts_like_the_column_constraint_form` (the
  `id INTEGER, …, PRIMARY KEY (id)` spelling really does corrupt, so the guard's
  new PRAGMA-based detection is not rejecting something the engine never
  needed) and `replace_with_changed_values_revisiting_a_group_keeps_an_aggregate_correct`
  (tripwire #6 — the changed-value aggregate cell, which is what stops the others
  going green while that defect survives), and
  `a_plain_insert_into_an_on_conflict_replace_table_corrupts_the_matview` (the
  ON CONFLICT REPLACE route, where the corrupting write is a plain INSERT). All
  eight are the A/B witness for
  the engine fix (fork bookmark `ivm-replace-double-old-row-capture`, PR #8463):
  run with `--ignored`, expect green after the bump.
- **Guard, at ONE screening point** — `TursoBackend::screen_replace_statement`
  in `crates/holon-turso/src/turso.rs`, called from the actor's command
  dispatch so EVERY statement passes it whatever DbHandle method submitted it
  (`query`, `query_positional`, `execute`, `transaction`). Placement matters
  more than it looks: `query()` is write-capable and is the method all four
  production navigation REPLACEs use, so a guard installed only on
  `execute`/`transaction` — as the first revision of this lane had it — is
  inert against precisely the statements it exists to stop. Detection reads
  `PRAGMA table_info` rather than the DDL string (the string cannot see the
  table-constraint spelling of a rowid alias), blanks SQL comments AND
  single-quoted string literals before tokenizing (a production file opens with
  five comment lines above its REPLACE; and a literal carrying `INTO` ahead of
  the real target otherwise aims the guard at the wrong identifier, while one
  carrying the verb can turn it on for a statement that writes nothing), and
  takes the last segment of a schema-qualified target. The schema
  lookups run only once a statement is known to be a REPLACE. The guard
  immediately caught the `navigation_history` REPLACE in `backend_engine.rs`'s
  own test fixture, now a plain INSERT.
- **Sibling guard, same hole, moot at landing:** the older
  quoted-identifier guard (`quoted_write_target`) had the identical `query()`
  blind spot, but it is not fixed here because the re-pinned fork MAINTAINS
  quoted writes and the repin rev — already in the chain this one stacks onto —
  deletes that guard outright; the `query()`-leg maintained-direction test added
  at stack time is what pins the shape in the only direction that still exists.
- **Two schema hooks for the ON CONFLICT REPLACE class**, since statement
  screening structurally cannot reach it: (1) `screen_conflict_replace_ddl`
  refuses any DDL declaring the clause, through every DbHandle entry point;
  (2) `reject_on_conflict_replace_bases` refuses to register a materialized
  view over a base table whose stored DDL already carries it — covering tables
  created before this guard, or outside DbHandle entirely. Fail-closed by
  design, and free: a repo-wide grep finds ZERO production uses of
  `ON CONFLICT REPLACE`, so the rejection costs nothing today and keeps the
  trap out of the tree. Both are red-on-revert (M9, M10).
- **Not covered by the guard, by construction:** the vtable writeback
  (`mcp_vtable.rs:1012`) issues its `INSERT OR REPLACE` through a raw
  `Arc<CoreConnection>`, bypassing `DbHandle` entirely. That path's only
  defence is the sidecar YAML assertion below.
- **Conjunction guard for cache tables** —
  `crates/holon-mcp-client/tests/cache_tables_are_not_rowid_alias_tables.rs`:
  asserts no `vtable.write_through` entity declares a lone `INTEGER PRIMARY
  KEY`, and pins the set of rowid-alias cache tables that exist at all. That
  second test found `jsonplaceholder.yaml:jp_posts`, which a grep over the
  sidecars with `views:` had missed; it is safe because it is `sync:`-only and
  the sync path upserts with `INSERT ... ON CONFLICT DO UPDATE`
  (`crates/holon/src/core/queryable_cache.rs:243,789`), measured green on a
  rowid table with an unchanged value.
- **Fix candidate decided by measurement:** `INSERT ... ON CONFLICT DO UPDATE`
  is green in every cell measured, including rowid-alias + unchanged value. It
  is the safe upsert form on this fork.
- **Stale doc corrected** — `crates/holon-turso/src/schema_modules.rs:1147`
  claimed backlink queries need "no materialized view", 40 lines above the
  `reconcile_named_view(db_handle, "backlinks", ...)` call that creates one.
- **Deliberately NOT done:** the four production REPLACE statements were left
  alone. All are measurably correct on their TEXT/composite keys, and the guard
  is what prevents the bug from arriving rather than a rewrite of correct code.
- **Deferred:** the engine fix itself is owned by the turso-1d peer and the
  repin lane.
