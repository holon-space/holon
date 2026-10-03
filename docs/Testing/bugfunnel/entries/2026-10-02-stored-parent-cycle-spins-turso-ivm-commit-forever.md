---
id: 2026-10-02-stored-parent-cycle-spins-turso-ivm-commit-forever
date: 2026-10-02
gap: COVERAGE
secondary: null
status: MITIGATED
summary: >-
  A transaction that stores a block parent cycle never commits: Turso's
  incremental recursive view maintenance iterates without a fixpoint inside
  COMMIT, the Turso actor spins at 100 % CPU with unbounded memory growth, and
  every later SQL statement waits behind it. A hang, not an error.
---

## Bug

Found by the DD tracer lane (Inc 1a) while taking the red of the keystone case
`place-under-own-descendant-refused-on-the-sql-authority`: the first run spun
90+ minutes at 100 % CPU and grew to 25 GB. A bounded re-run
(`timeout 300`, test profile) reproduced it: RSS 0.9 → 1.6 → 2.2 GB at
52 s / 108 s / 163 s, the keystone printed
`[inv-settle-budget] WEDGED: 'PlaceUnderOwnDescendant' made no projection visible within 120s`,
and the run ended only by the timeout (exit 124).

## Root cause

- The stuck thread (`sample`, 3789/3789 samples) is
  `TursoBackend::handle_transaction` → `op_auto_commit` → `Program::commit_txn`
  → `apply_view_deltas` → `IncrementalView::merge_delta` → `DbspCircuit::commit`
  → `RecursiveOperator` / `JoinOperator::process_join_state` →
  `HashableRow::compute_hash` / `hash_str`. Evidence:
  `lane-logs/inc1/hangsql-sample{1,2,3}.txt`, `hangsql-stack-demangled.txt`
  in the dd-tracer workspace.
- The transaction is `SqlOperationProvider::place_row`'s placement
  (`crates/holon/src/core/sql_operation_provider.rs`), which wrote
  `parent_id = descendant`.
- [inf] The view is `blocks_with_paths`
  (`crates/holon-turso/sql/schema/blocks_with_paths.sql`): a `UNION ALL`
  recursive CTE that appends to a `path` string per hop. On a cycle each
  iteration yields a new, longer path, so the incremental fixpoint never
  converges (a from-scratch evaluation would yield no rows for the detached
  cycle). It is the only production recursive matview without a visited or
  depth guard; the watch-context subtree view (`crates/holon/src/api/block_domain.rs:68`)
  carries `visited`.
- Turso has no iteration bound and no error for a non-terminating recursive
  view, so the failure is a hang inside the write actor.

## Missing piece

No test ever handed the store a parent cycle, so no test saw what the IVM does
with one. The keystone rung `PlaceUnderOwnDescendant` (Inc 1a) is the first.

## Remedy

MITIGATED: the SQL placement path now refuses a cyclic `place_row` with
`ApiError::CyclicMove` before the transaction runs, so the dispatched move can
no longer store a cycle (keystone cases
`place-under-own-descendant-refused-on-the-sql-authority` / `-on-loro`, green).
`parent_id` and `sort_key` are now private fields (Model.md invariant 16): a
dispatched `set_field` or `update` that names them, from the UI or from MCP
`execute_operation`, is refused at the intent boundary with
`BlockWriteFieldError::Private` naming `move_block`. That closes the dispatched
`update { parent_id }` path, which stored a cycle and hung the commit (keystone
cases `update-parent-under-own-descendant-refused-on-the-sql-authority` and
`mcp-execute-operation-update-parent-id-refused`, red by hang before, green
after). Suspected next path: a dispatched `create` of an existing id with the
same title re-parents it through the upsert with no cycle check (keystone case
`create-recreate-reparent-refused-on-the-sql-authority`, red).
Open: any other path that can store a cycle (org ingest of a hand-edited file,
a peer import into SQL, an internal `parent_id` write outside `place_row`) would still hang the actor. Close it at the view: a cycle guard
in the recursive view (a visited path check, or a depth bound that fails loud)
or an iteration bound with a typed error in the fork's `RecursiveOperator`.
