# Turso Bug Fix: cross-junction LEFT JOIN insert duplicates aggregated matview row

## Bug Description

A materialized view over `base LEFT JOIN j1 LEFT JOIN j2` + `json_group_array(...) FILTER` + `GROUP BY <base cols>` emits a **duplicate identical output row** when a row is inserted into the *later* join's table (`j2`) after the *earlier* join (`j1`) already matched that group. Silent wrong result — no error.

Found by holon's composed keystone PBT (`inv-blocks-match-ref/matview`, forced-weight run 2026-07-07, post pin-bump c31f8f4d30). Minimal keystone trigger: `SetEdgeField Tags` then `SetEdgeField Requires` on the same block.

## Reproduction

### Holon-level reproducer (confirmed failing)

`crates/holon-integration-tests/tests/matview_duplicate_row_repro.rs` (worktree
`.claude/worktrees/advice-spikes`), 4 tests. Drives production writers
(`LoroBackend::set_block_tags` / `set_block_requires`) → project → SQL.

Narrowing matrix:

| Scenario | Result |
|---|---|
| tags {proj} → requires [b] (junction of join-1 first, then join-2) | **DUPLICATE — 2 identical rows** |
| requires → tags (reverse) | OK |
| tags → tags | OK |
| requires → requires | OK |

`advice_suppressed` (a third LEFT JOIN) is NOT required — the two original junctions reproduce with it empty.

### Matview DDL shape (holon's `block` matview, `crates/holon-turso/sql/schema/block_matview.sql`)

```sql
SELECT
    b.id, b.parent_id, /* ...16 block_raw cols... */
    COALESCE(json_group_array(bt.tag)         FILTER (WHERE bt.tag         IS NOT NULL), '[]') AS tags,
    COALESCE(json_group_array(br.required_id) FILTER (WHERE br.required_id IS NOT NULL), '[]') AS requires,
    COALESCE(json_group_array(asup.lesson_id) FILTER (WHERE asup.lesson_id IS NOT NULL), '[]') AS advice_suppressed
FROM block_raw b
LEFT OUTER JOIN block_tags        bt   ON bt.block_id    = b.id
LEFT OUTER JOIN block_requires    br   ON br.block_id    = b.id
LEFT OUTER JOIN advice_suppressed asup ON asup.anchor_id = b.id
GROUP BY b.id, b.parent_id, /* ... all 16 base cols */
```

### Pure-SQL sketch (port to a turso-side test)

```sql
CREATE TABLE base (id TEXT PRIMARY KEY, v TEXT);
CREATE TABLE j1 (base_id TEXT, a TEXT, PRIMARY KEY (base_id, a));
CREATE TABLE j2 (base_id TEXT, b TEXT, PRIMARY KEY (base_id, b));
CREATE MATERIALIZED VIEW m AS
SELECT base.id,
       COALESCE(json_group_array(j1.a) FILTER (WHERE j1.a IS NOT NULL), '[]') AS a_s,
       COALESCE(json_group_array(j2.b) FILTER (WHERE j2.b IS NOT NULL), '[]') AS b_s
FROM base
LEFT OUTER JOIN j1 ON j1.base_id = base.id
LEFT OUTER JOIN j2 ON j2.base_id = base.id
GROUP BY base.id;
INSERT INTO base VALUES ('x', 'v');
INSERT INTO j1 VALUES ('x', 'proj');   -- group transitions no-match → match on join 1
INSERT INTO j2 VALUES ('x', 'req');    -- THEN join 2 matches: emits spurious duplicate
SELECT COUNT(*) FROM m;                -- expected 1, actual 2 (identical rows)
```

(If the raw sketch doesn't reproduce standalone, mirror holon's exact write pattern: the junction writes are delete-then-reinsert re-projections inside the CDC/change-callback flow — see the holon test for exact statements. Holon uses `set_change_callback`.)

## Analysis

### Root cause hypothesis

Incremental maintenance of the chained LEFT JOIN: when j1's insert converts the group's NULL-padded row to a matched row, downstream operator state for j2's subsequent no-match→match transition appears to retract the wrong (stale) row shape, yielding insert(new) + leftover(old==new after aggregation) → duplicate delta through the aggregate. Only the "later join matches after earlier join already matched" order corrupts.

### Relevant code

DBSP/IVM join + aggregate operators (`turso-core` incremental subsystem; same area as compiler.rs:3550 NOT EXISTS refusal). Prior findings pinned by holon (`crates/holon-advice/tests/matview_build.rs::probe_ivm_shape_findings`): in-matview LEFT-JOIN anti-join silently ignored (shape-dependent); `block_raw` col in GROUP BY corrupts aggregates. Possibly the same family.

## Acceptance Criteria
- [ ] Turso-side failing test (integration, `test_ivm_*` style) demonstrating the duplicate
- [ ] Fix; existing turso tests pass
- [ ] Holon reproducer `matview_duplicate_row_repro.rs` goes green against the patched turso
- [ ] Changes minimal and focused

## Turso Repo
`~/Workspaces/bigdata/turso/` (branch: `holon`; fork is ours — extending IVM is sanctioned). Holon pin: c31f8f4d30.
