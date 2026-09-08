---
id: 2026-09-09-the-task-priority-view-reads-an-uppercase-key-the-org-parser-never-writes
date: 2026-09-09
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `TaskEntity::priority()` read the property key `PRIORITY` while the org parser
  stores the rank under the canonical lowercase `priority`, so the view answered
  "no priority" for every block that came from a file — and its unit test passed
  because the test wrote the same wrong key.
---

## Bug

Found by the verifier on the `org-priority` lane (rev 1).

`TaskEntity` is the task-shaped view over a block: completion, priority, due
date. Its `priority()` looked the value up under `PRIORITY`. The org parser and
`OrgBlockExt::set_priority` both write `priority`, lowercase — the canonical
storage key, and a real SQL column. So the view returned `None` for every
file-originated block in the vault, which is all of them.

Reproduced (`lane-logs/rev2-RED-a.log`):

```
FAIL holon-core traits::trait_unit_tests::task_priority_reads_the_canonical_lowercase_key
  assertion `left == right` failed: the key the org parser writes must be the
  key this view reads
    left: None
   right: Some(1)
```

## Root cause

Two spellings of one storage key, with no single place that owns it. The reader
(`crates/holon-core/src/traits.rs`, `TaskEntity::priority`) and the writer
(`crates/holon-org-format/src/models.rs`, `OrgBlockExt::set_priority`) each
picked one, and nothing made them agree.

## Missing piece

**ORACLE.** The state was perfectly reachable and a test did exercise it —
`task_entity_view_maps_state_priority_due_date` — but the test SET the property
with `set_property("PRIORITY", …)`, the same wrong spelling the reader used, so
it asserted the two halves of one mistake against each other. A test that
constructs its input the way production does could not have passed; this one
constructed it the way the code under test happened to read it.

## Remedy

FIXED in the `org-priority` lane (rev 2): the view reads the canonical lowercase
`priority`, and the existing test seeds that key. One key, one place.

Covering test (red above, green in `lane-logs/rev2-GREEN-final2.log`):
`holon-core::traits::trait_unit_tests::task_priority_reads_the_canonical_lowercase_key`
— asserts BOTH directions: the key the org parser writes is read, and an
uppercase spelling is not a second storage key.
