---
id: 2026-09-30-turn-into-page-refused-mid-compound-leaves-an-orphan-page
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  "Turn into page" on a block whose child carries a keyword the new page's
  ring lacks was refused after the page was minted, leaving an orphan page
  that undo could not reach and that wedged every retry.
---

## Bug
Found by the Inc 6 round-11 verifier (probe `zz11v_p5`,
`lane-logs/inc6r11v-probe5.log`): page `#+TODO: TODO WAITING |`, a block under
it with a `WAITING` child, one `convert_block_to_page` call. The `create`
constituent minted and tagged the page, then the child's `move_block` was
refused by the ring check. No undo entry described the page, and a retry died
on the id-collision arm. `merge_blocks` had the same shape: its parked-body
`create` landed before a refused child move.

## Root cause
The ring check (`rehomed_root`) ran per constituent inside
`dispatch_constituent_op` (`crates/holon/src/api/operation_engine.rs`), so a
compound was judged one write at a time, after earlier constituents had
already written.

## Missing piece
No keystone transition drives `convert_block_to_page` or `merge_blocks`, and
the engine tests drove the move-family ring check only as single ops.

## Remedy
Inc 6 round 12. Both compounds judge every constituent that re-homes a block
before their first write (`refuse_move_into`, `refuse_merge_rehomes`).
Red/green: `a_convert_whose_child_keyword_the_new_page_lacks_writes_nothing`
and `a_merge_whose_moved_keyword_the_canonical_document_lacks_writes_nothing`
in `crates/holon/tests/cycle_task_state_vocabulary.rs`
(`lane-logs/inc6r12-red.log`, `lane-logs/inc6r12-green-b.log`). Round 13:
the minted page declares its source document's ring, so the convert no longer
refuses; `a_converted_page_declares_its_source_documents_ring` replaces the
convert test. The keystone coverage gap stays open.
