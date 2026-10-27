---
id: 2026-10-27-a-churning-file-stall-is-disclosed-twice-in-one-pass
date: 2026-10-27
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A write-back that stalls because its file keeps changing is disclosed twice
  in one re-render pass under host load, where the test expects exactly one
  disclosure.
---
## Bug

Integrate run w89 (cards #425 and #759, which change no holon-orgmode path)
failed `holon-orgmode::writeback_compare_and_rename
re_render_writes_the_other_files_while_one_file_keeps_rewriting_its_own_bytes`
in round 8 of 12:
`crates/holon-orgmode/tests/writeback_compare_and_rename.rs:1369:9`,
`the churning file's stalled write-back is not disclosed`, `left: 2`,
`right: 1`. The test passed in every earlier gate log. Found by the gate's
known-red classifier, which marked it novel. Evidence:
`docs/Testing/known-red-evidence/writeback-churning-file-stall-disclosed-twice/`.

The assertion message says "not disclosed", but the count is 2: the stall
was disclosed twice, not missed. A duplicate disclosure hides no data. The
risk is the other direction of the same mechanism: if the stall mark can be
written twice by a retry that fires inside the pass, it can also be absent
in the window between the retry-due tick and the re-fire (holon-work#779,
first task), and then a write-back that keeps failing is not disclosed.

## Root cause

Not proven. Hypothesis: under host load the write-back net's retry for the
churning file becomes due and re-fires during the same
`re_render_all_tracked` pass, and each failing attempt calls
`writeback_stalled` (`crates/holon-filesystem/src/file_sync_controller/writeback.rs:527`,
`crates/holon-filesystem/src/file_sync_controller.rs:8825`).

## Missing piece

The keystone does not run the write-back net at host-load timing: whether a
retry tick lands inside a pass depends on wall-clock time, which the test
does not control.

## Remedy

Open. Registered as known-red row
`writeback-churning-file-stall-disclosed-twice` in
`docs/Testing/KeystoneKnownReds.md`, owner holon-work#779. The fix decides
the disclosure rule for a retry inside a pass (one disclosure per stall, or
one per attempt) and pins it with a test that stops the clock at the
retry-due tick.
