---
id: 2026-08-10-ignores-document-declared-vocabulary-walks-hardcoded
date: 2026-08-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `cycle_task_state` ignores the document's declared `#+TODO:` vocabulary and
  walks the hardcoded default ring, writing keywords the document does not
  admit — and a re-ingest then silently DEMOTES the block to body text, so the
  task disappears.
source_line: 1196
---

## Bug

(dogfood-explorer gate on task #68, live GPUI SqlOnly, `CustomVocab`
document declaring `#+TODO: NEXT WAITING \ | DONE`; finding F3)
**`cycle_task_state` ignores the document's declared `#+TODO:` vocabulary
and walks the hardcoded default ring, writing keywords the document does not
admit — and a re-ingest then silently DEMOTES the block to body text, so the
task disappears.** Observed: `execute_operation block/cycle_task_state` on
`cusvocab-b4` produced `TODO` -> `DOING` -> `DONE`, of which only `DONE` is
a keyword of that document. Left in the invalid state it round-trips
destructively: `task_state=TODO, content="delta external"` renders to `**
TODO delta external`, and a cold boot on a fresh DB reads back
`content="TODO delta external"` with NO task_state. Same #67 data-mutation
class. The two write seams disagree after #68: promotion became
vocabulary-aware (B1/B2), cycling did not — the feature made per-document
vocabulary first-class without bringing its sibling seam along. AUDIT (this
lane): THREE independent hardcoded rings exist —
`sql_operation_provider.rs:~3286` (literal `vec!["", "TODO", "DOING",
"DONE"]`), `loro_block_operations.rs:~1270-1293` + cycle at `~1309-1322`,
and `render_eval.rs:133-152` `resolve_states` (the WIDGET ring behind the
state_toggle click). The op path reaches only the first two, so fixing the
widget ring alone would not touch the reproduced defect.

## Missing piece

no keystone draw cycles task state INSIDE a document that declares `#+TODO:`
(the generator mints no such document — the gap task #68 explicitly left
open), so the disagreement is ungeneratable; secondary ORACLE, and
independently damning: the PBT REFERENCE at
`crates/holon-integration-tests/src/pbt/ref_caps/toggle.rs:~134-151` writes
the TARGET KEYWORD DIRECTLY and models no ring at all, so even a draw that
DID cycle in a custom-vocab document could not have disagreed with a wrong
ring

## Remedy

FIXED 2026-08-10 in this lane (task #79) — red-first; see the lane report
for the ring-construction decision, the red logs, and how far the ref-cap
oracle gap was closed. GAP NOT FULLY CLOSED: the keystone generator arm that
mints `#+TODO:` documents remains open (inherited from task #68).
