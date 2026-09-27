---
id: 2026-09-27-refused-render-aborts-the-batch-pass
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  One file whose render was refused aborted the whole batch pass
  (`re_render_all_tracked`, `materialize_missing_page_files`), so every later
  file of the pass was skipped with no record of its own.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 4
(`lane-logs/groupA-r4-verify.md`, defect 3). Not landed: the render became
fallible in that round.

## Root cause
Both loops in `crates/holon-filesystem/src/file_sync_controller.rs`
propagated the render `Result` with `?`, while their other per-file failures
disclose and continue.

## Missing piece
The refusal test drove one file; no test had a second file in the same pass.

## Remedy
A refused render is disclosed once per document with its id and path
(`disclose_batch_render_failure`), the file is left untouched, and the pass
continues (`crates/holon-orgmode/tests/render_refusal_is_per_file.rs`, red
`lane-logs/groupA-r5-red.log`).

A file that does not read as a document (a refused page id on disk, a refused
pending ingest) aborted `re_render_all_tracked` the same way
(`lane-logs/groupA-r5-verify.md`, defect 2). It is disclosed once per file
(`disclose_batch_read_failure`) and the pass continues
(`the_tracked_re_render_writes_the_other_files_when_one_page_id_on_disk_is_refused`,
red `lane-logs/groupA-r6-red.log`).
