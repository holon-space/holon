---
id: 2026-08-28-matview-version-skew-false-cycle-boot-fail
date: 2026-08-28
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Holon refuses to boot on Martin's production database, reporting a circular
  dependency between three materialized views that do not reference each other.
---

## Bug

`target/release/holon-gpui` (built from `main` 835d9d8d) cannot open
`~/.config/holon/holon.db`:

```
boot failed: component=turso stage=engine-resolve
Failed to open Turso database: Internal error: Cannot resolve materialized view
dependencies (possible circular dependency): block_requirement_edges,
block_with_path, watch_view_896c82d172bdae55
```

Found by Martin dogfooding. An Aug 25 binary fails identically, so it is not a
same-day regression — the database, not the build, carries the trigger. The
failure is total: the app has no usable state, and no `--repair` path exists.

The three named views have no dependency on one another. Extracted from the
database, each depends only on `block`:

- `block_requirement_edges` — `... FROM block_requires br JOIN block b ON b.id = br.required_id`
- `block_with_path` — `WITH RECURSIVE paths AS (... FROM block ...)`
- `watch_view_896c82d172bdae55` — `SELECT * FROM block`

That is a fan-out, never a cycle.

## Root cause

Two defects compose. Only the second is fatal.

**The stored `block` definition is stale.** The live `sqlite_master` (WAL
applied) holds exactly one `block` row, rowid 374 on page 7205, and it is a
pre-2026-08-06 definition that still selects `b.depth` — a column the current
`block_raw` does not have. The engine says so during boot:

```
WARN turso_core::schema: Materialized view 'block' is unusable:
Parse error: Column 'depth' with table Some("b") not found in schema
```

The WAL shows this old schema page reappearing across the last several commits
(frames 469/477/483/498 on page 8168, then frame 505 on page 7205), replacing
the current definition that page 119 still carries. This is version skew:
whatever binary last wrote the database left an older `block` behind.

By itself this is survivable. `turso_core::schema` already has the
`incompatible_views` channel for a view that no longer compiles, and Holon's
`reconcile_named_view` (crates/holon-turso/src/matview_manager.rs:59) DROP+
CREATEs any view whose stored SQL differs from its schema module's canonical
SQL. The database would repair itself on the next boot.

**The engine turns one degraded view into a fatal false cycle.** After the
multi-pass resolver stalls, `populate_materialized_views` splits the remaining
views into "permanently broken" and "circular"
(`core/schema.rs:2218-2249` at pin d2480114) using a raw substring test:

```rust
let references_pending_view = pending_names
    .iter()
    .any(|other| *other != view.name && view.sql.contains(other.as_str()));
```

`block` is a pending name and a substring of every view that selects from it,
so all three dependents are classified circular, and circular is a hard `Err`
out of `Database::open_file_with_flags`. `block` itself is correctly classified
stale — it is the only view here that is actually broken.

Evidence, all in the pinned workspace
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/matview-cycle`:

- Discriminator: the same database **without** its WAL opens fine; with the WAL
  it fails. The stale definition is only in the WAL-applied state.
- No holon-layer escape hatch exists: opening with `DatabaseOpts::with_views(false)`
  produces the identical fatal error, so nothing in `holon-turso` can run before
  the failure.
- Regression test `crates/holon-turso/tests/matview_version_skew_boot.rs`
  reproduces the exact error string and view list from synthetic DDL.

## Missing piece

No test ever opens a database whose persisted matview definitions were written
by an **earlier binary version**. Every existing test builds its schema and
reads it back within one process, where stored and canonical SQL agree by
construction — so the entire "stored definition no longer compiles" branch, and
with it the stale/circular classifier, is unreachable in the suite. `boot/DDL
ordering` is named in the ENVIRONMENT gap definition; this is that gap at the
persistence boundary.

Secondary: the keystone PBT cannot reproduce this. It never restarts the engine
against a database seeded by a different schema version.

## Remedy

Tests in `crates/holon-turso/tests/matview_version_skew_boot.rs`, all green
against the patched fork:

- `boot_survives_matview_definition_written_by_older_binary` — was RED with the
  production error string verbatim.
- `same_fanout_without_skew_opens_cleanly` — GREEN, proving the three views are
  not inherently circular.
- `a_view_the_engine_marked_incompatible_is_repaired_by_reconcile` — the second
  half of the contract: booting only helps if the database then repairs itself.

The fix is **engine-side** — `holon-turso` cannot reach the failure. In
`core/schema.rs`, `populate_materialized_views` now builds its dependency edges
from the parsed SELECT (`matview_source_names`, a schema-free walk of the FROM,
JOIN and WHERE-subquery sources with CTE names removed) and calls a view
circular only when it is reachable from itself. Everything else — including
every dependent of a view that failed permanently — joins `incompatible_views`
with the same WARN any other unusable view gets, and the open returns.

It needed a second part the diagnosis did not predict.

`DROP VIEW IF EXISTS block` did **not** succeed against a view the engine had
placed in `incompatible_views`. `translate_drop_view` tested existence against
`broken_views` — rows whose SQL failed to PARSE — and against
`materialized_view_names`, which an incompatible view never joins. A stale
`block` matched neither, so `IF EXISTS` made the DROP a silent no-op and the
following `CREATE ... IF NOT EXISTS` appended a SECOND `block` row to
`sqlite_master`. Reconciliation made the database worse, not better.

`broken_views` and `incompatible_views` are now read through one
`Schema::is_unusable_view` at all five sites that ask "is this name taken", and
`op_drop_view` clears both. With that, `reconcile_named_view` repairs the view
in place.

Verified on a COPY of the production database (`holon.db` + `holon.db-wal`): it
opens, the startup schema modules replace the stale `block` with the current
definition, `block_raw` keeps all 2181 rows, the rebuilt matview projects 2180
(the `sentinel:no_parent` row is excluded by its WHERE clause), and the next
open is clean. Martin's database self-repairs on the next launch; no manual
step. Dry-run harness: `crates/holon/tests/prod_db_skew_dryrun.rs` (ignored,
driven by `HOLON_DRYRUN_DB`).
