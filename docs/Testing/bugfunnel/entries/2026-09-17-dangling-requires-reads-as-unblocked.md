---
id: 2026-09-17-dangling-requires-reads-as-unblocked
date: 2026-09-17
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A G1 task whose `:REQUIRES:` target has no `block` row was served as eligible
  by the now-query and by `now_for_agent`, because the anti-join used an INNER
  JOIN that dropped the unresolvable requirement instead of treating it as
  unmet.
---

## Bug

Task-coordination bug found by code audit during the `fix-dangling-requires-blocks`
lane. The vault contract (Projects/Holon/Now.org) is that a task is eligible only
if EVERY `:REQUIRES:` slug resolves to a DONE block. The live vault had
`block_requires` rows whose `required_id` had no `block` row, and the dependent
tasks were returned by `now_for_agent`.

Count correction (adversarial verification, `lane-logs/fix-dangling-requires-verify.md`):
at verification time the live vault carried exactly ONE dangling edge —
`block:ac2-cross-device-sync` → `block:cross-device-pbt` (G1, `task_state=DOING`,
`assigned-to=null`). The first draft claimed three; the other candidate,
`block:handoff-md-migration` → `block:edge-field-descriptor`, resolves (its target
is DONE). `now_for_agent` served `ac2-cross-device-sync` as ROW 1 while the fixed
shape withheld it.

## Root cause

The now-query existed in three places, all sharing one wrong shape: the PBT
ground-truth AST + Rust evaluator (`crates/holon-integration-tests/src/pbt/query_ast.rs`),
the MCP `now_for_agent` tool (`frontends/mcp/src/tools.rs`), and the vault source
block `now-query::src::0` in `/Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/Now.org`
(**not** edited by this lane). A fourth "copy" named in the first draft
(`crates/holon/src/api/backend_engine.rs:2102`) is a `#[cfg(test)]`-local fixture
literal, not a production duplicate.

The subquery was `NOT EXISTS (SELECT 1 FROM block_requires br JOIN block bl ON
bl.id = br.required_id WHERE br.block_id = b.id AND ...)`. An unresolvable
`required_id` produces no join row, so the subquery is empty and `NOT EXISTS`
reads the requirement as SATISFIED. The Rust evaluator had the identical
semantics (`let Some(b) = ... else { return false }`), so the reference model
agreed with the broken SQL and no invariant could have flagged it.

## Missing piece

No transition or invariant ran the now-query: `query_ast.rs`'s own module doc
declares `@pbt gen ORPHANED relative to the keystone` — the AST is exercised
only by three hand-written unit tests over the one canonical query, so the
eligibility semantics were never differentially checked against the SQL, and
no generator could reach the dangling-target state.

## Remedy

FIXED. `Predicate::EdgeExists` could not express the contract (the universal
"every requirement resolves to DONE" with a dangling requirement blocking), so
`query_ast.rs` adds `Predicate::AllRequiresDone`, compiled as a grouped
aggregate rather than a correlated subquery. The fragment lives once, in
`crates/holon/src/api/block_domain.rs`:

    LEFT JOIN block_requires br ON br.block_id = b.id
    LEFT JOIN block bl ON bl.id = br.required_id
    ...
    GROUP BY b.id
    HAVING count(br.required_id) = sum(iif(COALESCE(json_extract(bl.properties,
           '$.task_state'), '') = 'DONE', 1, 0))

`REQUIRES_DONE_JOINS_SQL` + `REQUIRES_DONE_HAVING_SQL` are what the PBT compiler
(`query_ast.rs`) emits, so the compiler cannot drift from the documented shape.
A block with no requirements joins one all-NULL row, so both counts are 0 and it
stays eligible.

INVERSION (Martin's ruling, same lane): the MCP tool must not hard-code the
now-query at all. `frontends/mcp/src/tools.rs`'s embedded SQL is DELETED;
`now_for_agent` executes its OWN vault block `now-for-agent::src::0` (separate
from the page's `now-query::src::0`, which binds no params) through the same path
`execute_source_block` uses, and binds four params the block must reference —
`$agent_id` (unclaimed OR assigned to me, plus claimed-first ordering),
`$state_todo` / `$state_doing` (the open-state set) and `$limit`. A block that
ignores any of them is REFUSED, because an ignored param is not an engine error
and the one that would go missing is the agent scope. The contract is in the
tool's doc comment.

Mechanism correction (verification): `count(br.required_id)` counts EVERY joined
requirement row — `required_id` is `NOT NULL`, so an unresolvable target inflates
the left count. The equality is nonetheless correct because a DONE-resolved
requirement is a resolved one: `count(all) = count(DONE)` holds exactly when
every requirement resolves AND is DONE. An earlier draft claimed `count()`
skipped unresolvable rows; that was wrong.

Two intermediate shapes were tried and rejected with evidence: a correlated
`LEFT JOIN` made the subquery match EVERY row (the unmatched row escaped the
correlation: 109 of 109 G1 TODOs "blocked"), and a double-nested `NOT EXISTS`
dropped tasks that have NO requirements at all (`block:b0` in the antijoin
fixture). The Rust evaluator was corrected to the same semantics.

Also FIXED (verifier residual 2): `Predicate::AllRequiresDone` renders no WHERE
fragment, so under `Or`/`Not` it produced unparseable SQL (`WHERE ( OR ...)`) —
`compile_to_sql` now returns `Err` naming the predicate and its position instead
of compiling it, and an all-aggregate filter emits no `WHERE` keyword rather than
an empty one.

Red logs: `lane-logs/21-defect1-FINAL-RED.log` (the query serves both tasks);
`lane-logs/delta-01-tool-RED.log` (the production tool served
`["block:dangling-g1-task", "block:free-g1-task"]`);
`lane-logs/delta-13-inversion-RED-mutation.log` (an SQL-owning tool ignores the
block: empty rows instead of a loud error, and serves the G1 task the block does
not select); `lane-logs/delta-16-refusal-RED-mutation.log` (the refusal removed:
`WHERE ( OR json_extract(b.properties, '$.gate') = 'G1')`);
`lane-logs/delta2-r1-red.log` (the refusal walker skipping `EdgeExists.inner`:
`WHERE br.block_id = b.id AND )`).
Green: `delta-02`, `delta-10`, `delta-14`, `delta-17`.
Tests: `pbt::query_ast::tests::dangling_requires_blocks_eligibility` (evaluator),
`...::all_requires_done_under_not_is_refused_with_its_position` (plus the `Or`,
`EdgeExists` and sole-filter cases), `api::backend_engine::tests::dangling_requires_blocks_the_task`
(real SQL) and `tools::now_for_agent_tests::*` (the tool's own path, real engine,
through a seeded vault block).

Live re-measurement with the tool's old SQL shape
(`lane-logs/delta-11-live-vault-evidence.log`): on `block:ac2-cross-device-sync`
the old shape serves 1 row, the aggregate shape 0; and the live vault carries
exactly one dangling edge.

The replacement block SQL is handed over at
`lane-logs/now-query-block.sql` (the same statement sits in the tool's doc
comment) and was executed against the live vault: it parses, returns the
eligible set, and withholds `block:ac2-cross-device-sync`.

CLOSED — both vault blocks have landed, so there is no fail-loud window. The page
block `now-query::src::0` in `Projects/Holon/Now.org` carries the aggregate shape
(`LEFT JOIN block_requires` + `GROUP BY`/`HAVING`), binds no params, and renders
Now.org. The tool's own block `now-for-agent::src::0` exists with the same
aggregate plus all four params (`$agent_id`, `$state_todo`, `$state_doing`,
`$limit`), and `now_for_agent` executes exactly that block
(`frontends/mcp/src/tools.rs:653`). The tool never reads the page block, so the
page block was never on its path: the fail-loud claim an earlier draft of this
paragraph made was wrong about the cause, not merely about the timing. The
refusal contract stands as designed — a later edit to the tool's block that drops
a bound param is refused loudly rather than silently dropping the agent scope.
