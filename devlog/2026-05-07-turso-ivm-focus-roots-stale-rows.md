# Turso IVM: redundant UPDATE on a base table drops the row from a hydrating LEFT-JOIN matview (2026-05-07 — FIXED in nightscape@holon `05c332675`)

**Status:** **FIXED upstream** in `nightscape@holon` commit
`05c332675 fix: IVM LEFT JOIN drops null-padded row on redundant UPDATE`.
Holon's `Cargo.lock` bumped via `cargo update turso` (2026-05-07 evening);
both the 7-stmt minimal repro and the full 358-stmt PBT trace now pass
under the new revision (`turso-sql-replay replay <trace> --check-after-each
--no-break-on-inconsistency` reports `Issues found: 0`).
**Originally pinned at:** `7cf0a2e68a3a17d394ee03318e714027686daf2d` (unfixed).
**Holon trigger:** PBT panic at `crates/holon-integration-tests/src/pbt/sut.rs:4002` — `Region 'main' focus_roots mismatch after navigation. block:<id>: block_raw=✓ block=✗ focus_roots=false`
**Minimal repro:** `/tmp/trace_v5_minimal.sql` — 7 SQL statements, ~2.6 KB, self-checking via `-- ?ASSERT` directives.

## TL;DR

A second `UPDATE` of a `block_raw` row to a value the row already holds (a no-op write) **drops that row from the `block` materialized view** — even though `block_raw` still contains it. The `block` matview is the standard hydrating shape:

```
SELECT b.*, COALESCE(json_group_array(bt.tag) FILTER (...), '[]') AS tags,
            COALESCE(json_group_array(tb.blocker_id) FILTER (...), '[]') AS blocked_by
FROM block_raw b
LEFT OUTER JOIN block_tags    bt ON bt.block_id   = b.id
LEFT OUTER JOIN task_blockers tb ON tb.blocked_id = b.id
GROUP BY b.id, b.parent_id, ... /* every base column */;
```

The PBT presented this as a `focus_roots` mismatch, but the actual stale-row drop is one level deeper: `focus_roots` JOINs `block`, so when `block` loses the row, the JOIN inside `focus_roots` finds nothing and the PBT's per-row truth check fires.

## Minimal sequence (after the matview is created)

```sql
-- Schema (block_raw + block_tags + task_blockers + the `block` matview above)

-- stmt#5
INSERT OR IGNORE INTO block_raw
  (parent_id, sort_key, content, id, content_type, properties, created_at, updated_at)
VALUES
  ('block:ref-doc-0', '817F80', 'Dple6 lJaGjrHy3 4b', 'block:2u671h3', 'text',
   '{"ID":"2u671h3","sequence":3}', 1778174723926, 1778174723926);
-- after #5: block_raw=✓  block=✓

-- stmt#6
UPDATE block_raw SET content = 'D' WHERE id = 'block:2u671h3';
-- after #6: block_raw=✓  block=✓   (real value change; matview tracks it)

-- stmt#7
UPDATE block_raw SET content = 'D' WHERE id = 'block:2u671h3';
-- after #7: block_raw=✓  block=✗   <-- BUG: redundant UPDATE drops the row
```

Detected via EXCEPT-based row-set diff in `tools/src/turso_sql_replay.rs::check_matview_consistency`:

```
INCONSISTENCY in block: matview=0, fresh=1, extra=0 missing=1 rows
  block: MISSING rows (in fresh but not matview):
    block:2u671h3 | block:ref-doc-0 | 0 | 817F80 | D | text | NULL | NULL | ... | [] | []
```

## Suspected mechanism

The matview groups by *every* base column. A no-op UPDATE in SQLite still dispatches a row write through the WAL — and Turso's IVM appears to convert that into a delta even when the new and old projections are identical, then propagates a `(-1, +0)` (or similarly degenerate) Δ through the GROUP BY operator that ends up dropping the group entirely instead of keeping it stable.

This is consistent with — but more general than — the previously-fixed `tui_split_block_cdc_drop` (UPDATE through `WITH RECURSIVE` matview emits zero CDC). Here there's no `WITH RECURSIVE`, just a `LEFT OUTER JOIN` + `json_group_array(...) FILTER` + `GROUP BY`.

## Reproduction

```bash
# Build the replay tool (in holon checkout)
cargo build --release -p holon-tools --bin turso-sql-replay

# Run the minimal trace; exits non-zero when the bug reproduces
./target/release/turso-sql-replay replay /tmp/trace_v5_minimal.sql \
    --check-after-each --no-break-on-inconsistency
```

Expected output:

```
[assert] OK:    ROW-EXISTS block_raw 'block:2u671h3'
!!! [assert] FAIL: ROW-EXISTS block 'block:2u671h3'
!!! [assert] FAIL: ROW-COUNT block 1
!!! [stmt#7] INCONSISTENCY in block: matview=0, fresh=1, extra=0 missing=1 rows
  VERDICT: IVM BUG REPRODUCED!
```

## Why offline replay needed three iterations to surface this

Documented for future captures (the holon side now traces all of this):

- **First trace** captured only `actor_ddl` + `transaction_stmt` (writes inside transactions). Replay said "no inconsistencies."
- **Second trace** added `set_change_callback` so CDC events broadcast in the replay. Still clean.
- **Third trace** also captured `actor_query` (named-param queries) and `actor_exec` (positional-param writes). Without `actor_exec`, the second redundant UPDATE never replayed, and the offline matview state matched fresh re-evaluation by accident.

The fix on the holon side was to widen `trace_sql` to emit on stderr (workspace-hack enables `release_max_level_info`, which compiles `tracing::trace!` out of release builds) and to add the read-path tags to the actor command dispatch.

## Diagnostic-tool changes that landed alongside this report

In `tools/src/turso_sql_replay.rs`:

1. **EXCEPT-based row-set diff** in `check_matview_consistency` — replaces a count-only comparison; catches row-content drift and row-presence drift even when counts match.
2. **`--track-id <[TABLE=]ID>` flag** — repeatable, defaults to `block_raw,block`. Prints presence-vector transitions per DML so you can pinpoint the exact statement that drops a row.
3. **`-- ?ASSERT ROW-EXISTS / ROW-ABSENT / ROW-COUNT` directives** — self-checking repro files. Failure exits non-zero, suitable for CI gating.
4. **`--no-break-on-inconsistency`** — keep going past first inconsistency to find every divergence point.
5. **`print_query`** — switched from `row.get::<String>` (panics on Integer/Null via `unreachable!`) to `row.get_value` + per-`Value`-variant formatting. Necessary so the EXCEPT-diff row dump doesn't itself panic on multi-typed columns.
6. **Minimizer subprocess invocation** — `crashes_with_subprocess` and `detect_crash_pattern` now spawn `replay --check-after-each --no-break-on-inconsistency` so mid-replay matview drift surfaces in stdout/stderr; the minimizer can finally lock onto `INCONSISTENCY in <table>` patterns.
7. **`detect_crash_pattern`** — prefers `INCONSISTENCY in <table>` markers over generic "panicked at" lines, so unattended minimization picks the right signal automatically.

## Files

- Captured trace log: `/tmp/trace_v5_keep.log` (180 KB, 1602 lines) — full PBT run that produced the panic.
- Extracted SQL: `/tmp/trace_v5.sql` (359 stmts) — reproduces the bug at stmt#340.
- **Minimized self-checking SQL:** `/tmp/trace_v5_minimal.sql` (7 stmts, ~2.6 KB) — also committed in this repo as `devlog/2026-05-07-turso-ivm-focus-roots-minimal.sql`. The artifact to ship upstream.
