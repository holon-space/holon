---
id: 2026-09-29-task-state-category-ignores-the-documents-ring
date: 2026-09-29
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A task keyword written through set_field (UI, MCP, dense_patch) got its
  task_state_category from org's default done list, not from the block's own
  document ring, so `SHIPPED` in a `TODO | SHIPPED` file was stored active.
---

## Bug
Found by the Inc 6 round-8 adversarial verifier (`lane-logs/inc6r8v-verify.md`,
D14, probes `zzw1`, `zzw2`, `zzw6`). In a file with `#+TODO: TODO | SHIPPED`,
dense_patch set a row to `SHIPPED`: the store held category `active`, the file
and the dense view `SHIPPED` (done). The mirror case `CANCELLED` in
`TODO CANCELLED | DONE` was stored `done`. A row dense_patch created with a
keyword had no category at all.

## Root cause
Every storage boundary derived the category from the fixed list
`DEFAULT_DONE_KEYWORDS` (`TaskState::category_str_for_keyword`):
`sql_operation_provider.rs` (set_field arm), `loro_block_operations.rs`
(set_field arm), `block_cell_registry.rs` (write_field arm). The document ring
never reached them. Measured on the engine alone, the path Inc 6 does not
touch: `a_written_keyword_takes_its_documents_category` and
`a_created_task_carries_its_documents_category`
(`crates/holon/tests/cycle_task_state_vocabulary.rs`) are red with the base
engine (`lane-logs/inc6r9-red-d14.log`), so a UI or MCP `set_field` on main
stored the wrong category too. Inc 6 round 8 made dense_patch hit it after
group B's guard refused the explicit category write it used to send.

## Missing piece
- No check compared the stored category with the ring: every dense oracle
  and the keystone compare views whose category comes from the ring.
- The keystone's `TodoKeywordSet` draws only done keywords inside the
  default list (`DONE`, `CANCELLED`, `CLOSED`), where the fixed list agrees.

## Remedy
Inc 6 round 9. The engine derives the category from the block's own document
ring on every write that sets `task_state` (`classify_task_state` in
`crates/holon/src/api/operation_engine.rs`: set_field, create, update, the
task-keyword constituents and undo/redo replay). A set_field carries the pair
as `holon_api::TaskStateWrite`; the storage boundaries write it as given and
refuse a bare keyword, and a create or update that writes `task_state` without
its category is refused. `category_str_for_keyword` is deleted. dense_patch's
post-apply check and the engine harness's `judge` compare the stored category
with the ring and with the org file; the generated property uses a
`NEXT | SHIPPED` ring.
