---
id: 2026-08-19-ivm-antijoin-matview-silently-empty
date: 2026-08-19
gap: ORACLE
secondary: COVERAGE
status: PARTIAL
summary: >-
  A live_query whose SQL uses a correlated NOT EXISTS anti-join (or
  OR(EXISTS, NOT EXISTS)) is served from an IVM matview the Turso fork cannot
  maintain, so the widget paints empty while a fresh recompute returns rows.
---

## Bug
Martin's `Now.org` planning query (`:ID: now-query`, source language
`holon_sql`) renders nothing in the live app. The query asks for the unblocked,
G1, agent-eligible TODOs:

```sql
SELECT b.* FROM block b
WHERE json_extract(b.properties,'$.task_state')='TODO'
  AND json_extract(b.properties,'$.gate')='G1'
  AND NOT EXISTS (SELECT 1 FROM block_requires br JOIN block bl ON bl.id=br.required_id
                  WHERE br.block_id=b.id AND COALESCE(json_extract(bl.properties,'$.task_state'),'')<>'DONE')
  AND (EXISTS   (SELECT 1 FROM block_tags bt WHERE bt.block_id=b.id AND bt.tag='agent')
    OR NOT EXISTS(SELECT 1 FROM block_tags bt WHERE bt.block_id=b.id AND bt.tag='human-only'))
```

Found by agent dogfooding of the live instance (lane `lane-ivm-antijoin`,
investigation `scratchpad/now-org-query-report.md`). Fresh execution of the
identical SQL returns 5 rows; the live render paints a single `empty` widget.

## Root cause
The live_query render path serves rows from an IVM-maintained matview
(`watch_view_<hash>`), not from a fresh execution. The Turso fork's DBSP IVM
cannot maintain a **subquery-valued predicate** (`EXISTS` / `NOT EXISTS` /
`IN (subquery)` — the `Exists` operator itself, negated or not). The DEFECT is
not that it refuses these — it is that the refusal was SILENT for the shape that
ships.

**The trigger is a computed conjunct beside the subquery, NOT chaining**
(turso-6f 8-shape bisect; corroborated by this lane's own probe matrix):

- Bare `NOT EXISTS`, and any `NOT EXISTS` with a **plain-column** conjunct
  (`b.id <> 'x' AND NOT EXISTS (…)`), is REFUSED LOUDLY at DDL over EVERY source
  (base table AND the chained `block` matview) —
  `Cannot convert LogicalExpr to AST Expr: Exists { … }`. (This lane's probe E,
  and `crates/holon-turso/tests/antijoin_pbt.rs::base_table_antijoin_ddl_rejected`.)
- When a **COMPUTED** conjunct sits beside the subquery — Now.org's leading
  `json_extract(properties,'$.task_state')='TODO' AND NOT EXISTS (…)` — the
  projection rewrite's catch-all pointed the `EXISTS` at the shared
  `__temp_filter_expr` temp column, so CREATE **silently SUCCEEDED** with an
  always-false compiled filter: 0 rows while a fresh recompute returns 5.
  (This lane's probe D; `now_org_antijoin_regression`; observed live:
  `SELECT count(*) FROM watch_view_9ef01e09eaebe900` → `0` vs inline → `5`.)

Chaining is a red herring: the earlier "chained-matview bypass" framing was
wrong — a chained source with only plain conjuncts still refuses loudly; a base
table with a computed conjunct beside the subquery would silently succeed the
same way. The variable is the computed conjunct, not the source.

The render emitting a silent `empty` widget instead of surfacing the divergence
is the fail-loud violation (CLAUDE.md: never silently degrade to look "fine").

**Not a swallowed error (hypothesis tested and refuted for the prod shape).**
One might expect the CREATE to fail loudly and holon to be swallowing it.
Evidence says otherwise for the shape that ships:
- Live app log `/private/tmp/holon-cold.log`: `stage="matview_ddl"
  view="watch_view_9ef01e09eaebe900" ms=27` immediately followed by
  `subscribe_cdc(...)` — the CREATE **succeeded** (27 ms); there is NO
  "Cannot convert LogicalExpr" / "Exists" / "could not be created" line
  anywhere in the log (225 `matview_ddl` lines, all INFO success timings).
- holon's watch-view path does not swallow or mint a stub: `execute_ddl` →
  actor → `finish_view_creation` calls `waiter.fail(msg)` on error (propagates,
  no stub `CREATE VIEW`), and `query_and_watch` propagates it with `?`.
So the silence was the ENGINE's — the computed-conjunct temp-column rewrite made
CREATE succeed with an always-false filter, so there was no error for holon to
swallow. The loud path IS reachable (base table / plain conjunct):
`StorageError::DatabaseError("Failed to execute DDL: Parse error: Cannot convert
LogicalExpr to AST Expr: Exists { … }")` through holon-turso.

The engine fix — validate every substituted sub-expression through the
conversion authority (no allowlist) so unsupported shapes refuse LOUDLY in ALL
combinations — is owned by the turso-side agent (`turso-6f`), landing
`3c76af40→90f25523` atop `c6cfab7d`. After that re-pin: the prod query refuses
loudly at CREATE (holon's proactive `sql_ivm_maintainable` predicate already
routes it eager BEFORE the CREATE, so the render is unaffected), and the DDL
error becomes an additional eager-routing signal. This lane owns only the
holon-side render unblock and the differential gate.

## Missing piece
Two, both structural:

- **ORACLE (primary):** no differential property asserted "matview-served rows
  ≡ fresh recompute" over IVM-un-maintainable query shapes. The keystone's
  `inv-matview-consistent-with-recompute` compares each matview against its own
  recompute, but the keystone never STANDS UP a live_query whose SQL is a
  correlated `NOT EXISTS` anti-join and then mutates the correlated tables
  (`block_requires` / `block_tags`) and the outer `task_state`/`gate`
  properties — so the invariant that would fire has nothing to judge.
- **COVERAGE (secondary):** the keystone generator has no transition that
  authors an anti-join / `OR(EXISTS,NOT EXISTS)` live_query, so the divergent
  state is unreachable.
- **ORACLE, wider than Now.org:** the composed keystone's query AST already
  emits subquery-predicate SQL — `Predicate::Membership` renders `EXISTS (…)`
  and `Predicate::Not(Membership)` renders `NOT (EXISTS (…))`
  (`crates/holon-integration-tests/src/pbt/query_ast.rs`). Every matview DRAW of
  such a shape is silently-empty by the same mechanism, yet the keystone never
  compares matview-served rows against a fresh recompute for those draws — so
  the whole `EXISTS`/`IN`-subquery family escaped, not just the dogfooded
  `NOT EXISTS`. `inv-matview-consistent-with-recompute` should be exercised over
  a keystone draw that stands up one of these live_queries.

## Remedy
- **Red-first differential PBT** — `crates/holon-turso/tests/antijoin_pbt.rs`:
  a shape-parametrised generator (proptest) asserting matview-served ≡ fresh
  recompute after each mutation, with an isolated correlated-`NOT EXISTS` arm
  plus the deterministic Now.org regression case (full `OR(EXISTS,NOT EXISTS)`
  shape). The pure-matview anti-join property is RED for the right reason
  (`matview 0 != fresh 5`) and is `#[ignore]`d pending the engine fix, citing
  this entry. Red log: `/tmp/ivm-aj-red.log`.
- **Routing predicate (engine-truth, AST-based):**
  `holon_turso::matview_manager::sql_ivm_maintainable` PARSES the SQL and routes
  to a matview ONLY when the parse tree contains no `Exists`/`InSubquery` node —
  so `EXISTS`, `NOT EXISTS`, `NOT (EXISTS …)` (the keystone generator's
  spelling) and `IN`/`NOT IN (subquery)` all route eager, and a `'… exists'`
  string literal does not. A parse failure routes eager (conservative). Pinned
  by `ivm_maintainable_flags_every_subquery_predicate_spelling`.
- **Render unblock (fail-loud, this lane):** `query_and_watch` serves an
  un-maintainable shape by **eager re-execution on the row-change bus** instead
  of a stale matview (correct 5 rows; reacts to mutations), keyed on a stable
  per-row identity so it does not churn. TWO classifiers guard it,
  defense-in-depth: (1) the AST predicate routes subquery shapes eager UP FRONT;
  (2) a BACKSTOP catches a shape the predicate thought maintainable but the
  engine refuses PERMANENTLY at CREATE (`Cannot convert LogicalExpr` — e.g.
  `CASE`), distinguished from a TRANSIENT `no such table` (which keeps the
  watcher's retry). The degraded disclosure travels WITH the stream
  (`BatchMetadata::degraded`), single-sourced from the backend; the reactive
  watcher lifts it onto `ReactiveRenderedRows::degraded`, and `annotate_degraded`
  stamps it as a `degraded_disclosure` prop on the rendered view model so it is
  present in the `describe_ui`/render tree. Green tests:
  `antijoin_live_query_served_eagerly_yields_rows_and_reacts` (5 rows + retract +
  disclosure on the stream), `permanent_matview_refusal_falls_back_to_eager_with_disclosure`
  (backstop serves `CASE` eager with the engine's refusal text),
  `permanent_vs_transient_matview_error_classifier`, and
  `degraded_disclosure_coexists_with_rows_and_is_observable` (prop reaches the
  view model). NOTE: painting a styled banner from that prop lives in the
  external `holon-gpui` crate (not in this repo) and is a thin follow-up.
- **Post-fix acceptance (dual-form):** the fork does NOT add anti-join
  maintenance — it makes the shape refuse LOUDLY. `now_org_antijoin_regression`
  is the CURRENT witness ("matview 0 vs fresh 5", succeeds-empty);
  `now_org_antijoin_create_refuses_loudly_after_fix` is the post-fix gate
  (`reconcile_named_view(...).is_err()`), RED now (returns `Ok`) and GREEN on the
  bypass-fix re-pin (~`90f25523`), at which point it un-ignores and the witness
  retires.
- **`LEFT JOIN … IS NULL` populate fix — measured + pinned.** This anti-join
  spelling has NO subquery node, so `sql_ivm_maintainable` keeps it on the
  matview path. On the pre-fix pin `54f3cc5e` the matview OVER-served (measured
  `direct=4 served=5`, undisclosed — a silent-wrong-rows bug of its own). The
  landed turso populate fix `c6cfab7d` (branch `matview-antijoin-populate-fix`)
  makes it CORRECT — re-measured through the real `block` matview via
  `query_and_watch`: `direct=4 served=4`. Guarded end-to-end by
  `holon::api::backend_engine::tests::left_join_isnull_matview_matches_fresh_after_populate_fix`
  (guards the fork against regressing). Note: the simplified `antijoin_pbt.rs`
  harness did NOT reproduce the over-serve — only the prod matview does — so the
  guard lives at the `query_and_watch` tier, not in the PBT.
- **Now.org guidance:** rewriting the readiness clause to `LEFT JOIN … IS NULL`
  is a VALID optimization on `c6cfab7d` (verified above), but eager +
  disclosure stays the honest default: the `NOT EXISTS` form is served correctly
  eager today regardless of engine pin, and the rewrite only pays off once the
  lane is on `c6cfab7d`+.
- **Pin durability caveat:** `c6cfab7d` matches holon main's turso pin, but it
  is an UNMERGED branch tip on the fork (`matview-antijoin-populate-fix`), not
  the fork's main — it could be rebased/force-pushed away. Durability of this rev
  is owned by the fork-line reconciliation stream (turso-6f is protecting the
  branch). If the rev disappears, `cargo` will fail to fetch it and the pin must
  be advanced to wherever the populate fix lands on the fork's main.
- **Related transform bugs found while triaging the backstop's reach** (both
  OPEN, pre-existing, NOT engine bugs): a live_query using `EXCEPT`/`INTERSECT`
  wedges because `JsonAggregationSqlTransformer` emits invalid `EXCEPT ALL`
  (`…-except-transform-emits-except-all`); a derived-table FROM wedges because
  the `_change_origin` transform leaks the inner alias
  (`…-change-origin-transform-leaks-derived-alias`). Of the three shapes the
  backstop is meant to cover, ONE (correlated scalar subquery in SELECT) is
  served eager end-to-end today (measured); the other two are blocked upstream by
  these transform bugs, which mangle the SQL for BOTH the matview and eager paths
  — so the backstop cannot rescue them and the fix belongs in the transforms.
- **Deferred (follow-up engine increment):** the turso-fork fix (owned by the
  `turso-6f` agent). Until it re-pins, the witness stays the recorded known-red.
