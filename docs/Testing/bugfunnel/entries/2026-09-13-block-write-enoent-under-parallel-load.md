---
id: 2026-09-13-block-write-enoent-under-parallel-load
date: 2026-09-13
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Under heavy parallel load a block WRITE — `set_field` or `create` — fails
  with a bare `No such file or directory (os error 2)`, across four tests on
  two different lanes' trees.
---

## Bug

**Renamed 2026-09-13** from `2026-09-13-set-field-enoent-under-parallel-load`:
the original title said `set_field`, and a `create` has since hit the same
error, so the name was narrower than the defect.

A block write dispatched through the production engine fails with

```
Operation 'set_field' on entity 'block' failed: No such file or directory (os error 2)
```

Seen first on the wave-13 weave gate for the cell-undo lane's Increment 1+2
(`w-cell-undo-nextest.47570.log` line 718, a 1259-test parallel leg), then
reproduced locally. Three DIFFERENT tests have hit it, none of which share
anything but the harness and the dispatch:

- `holon-app::cell_leg_delta_delivery::an_authoritative_write_reaches_the_cell_the_editor_reads`
- `holon-app::text_undo_manager::a_boundary_ingest_write_is_not_undoable`
- `holon-app::loro_write_origins::a_boundary_ingest_write_does_not_look_like_typing`
- `holon::editing_suite::metamorphic_property_ops_round_trip_through_undo` — on
  ANOTHER lane's tree (main + the undo-precondition change), failing 1 run in 4
  with `create the undo target block: Operation 'create' on entity 'block'
  failed: No such file or directory (os error 2)`; 3/3 green in isolation and
  3/3 on the full suite afterwards. **This is the observation that widened the
  entry**: the operation is `create`, not `set_field`, and the tree is not this
  lane's, so neither the operation nor this lane's code is the common factor.

Observed rates:

| Population | Result |
|---|---|
| one test file alone, ×10 | 0 failures |
| whole `-p holon-app` crate and multi-file batches, ~14 runs | ~4 occurrences |
| the same populations, 12 runs after the error enrichment below | 0 occurrences |

The last row is variance, NOT evidence of a fix: the enrichment changes only
what the message says, never whether the race happens.

**This is a product path, not a test defect.** It was checked: each test holds
its own `tempfile::TempDir`, and the harness declares that field LAST in
`Booted` (`crates/holon-app/tests/session_shutdown/harness.rs:95-111`) so it
outlives the session, the engine and the injector. There is no shared path and
no directory removed under a running session.

## Root cause

Not established. Ruled out by measurement:

| Candidate | Evidence against |
|---|---|
| A test's temp vault removed while the session runs | drop order puts `TempDir` last; each test has its own |
| The org write-back's atomic write | the error did not change when `write_atomic_blocking` began naming its path |

What is known: the io error is converted into the operation error inside
`DispatchingOperationEngine::execute_operation`
(`crates/holon/src/api/operation_engine.rs:2918`, backtrace frame 7), which can
add the operation and the entity but never knew the path.

**Hypothesis, testable at the next gate:** the write-back fold resolves the
document file for the block being written before boot ingest has created or
registered it, so under contention the read lands on a path that does not exist
yet. Widened by the `create` observation above: the trigger is ANY block write
whose document file the session has just ingested, not one operation. That
predicts the failure never appears on a write to a block whose document has
been on disk since before the session started, and never in isolation — both of
which hold across all four observations.

## Missing piece

The error named no path, so every occurrence looked like an unattributable
flake and would have been registered as one. Nothing in the filesystem port
added the path at the syscall, which is the only layer that has it.

Fixed as part of this entry: `RealFileSystem::read_to_string`, `read` and the
atomic write now name the path AND say whether the parent directory exists
(`crates/holon-filesystem/src/fs_port.rs`, helper `at_path`). The next
occurrence therefore distinguishes "the vault went away" from "this document's
folder was never created" without another hunt.

## Remedy

Open. The diagnosis is armed but the race is unfixed. Next step is to catch one
occurrence with the enriched message and read the path; if it names a
document-folder that ingest had not created yet, the fix is ordering (or a
`create_dir_all`) in the write-back fold rather than anything in the tests.
