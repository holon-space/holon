---
id: 2026-10-05-a-refused-org-file-restored-without-an-event-stays-refused-and-shut
date: 2026-10-05
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An org file refused by its adapter and then restored to its last good bytes,
  with the change event of the restore lost, stayed refused on the bus and
  quarantined from write-back for the session, and every write-back skip of it
  logged an ERROR that claimed write-back would destroy on-disk lines.
---

## Bug
Found by the fresh verifier of the ingest-refusal-group lane (D71.b round 5),
by reading the code: `lane-logs/ingest-refusal-verify-r5.md`, defect D3.
Steps: a write-tier (org) file ingests; the user breaks it so the org adapter
refuses it; the user restores the exact prior bytes (an editor undo); the
change event of the restore is lost (fs events starve on a loaded machine).

## Root cause
A refusal quarantined the file under the same cause as a partial ingest
(`QuarantineCause::Ingest`, `crates/holon-filesystem/src/file_sync_controller.rs`
`on_file_changed_unpersisted`). Only a read of the file clears it, and every
read leg read only when disk != `last_projection`: `poll_tracked_files`,
`re_render_all_tracked` and the per-document write-back. Restored bytes equal
`last_projection`, so no leg read the file again. Each write-back then hit
`note_quarantine_skip`, which logged ERROR with text that is false for a
refusal: the DB holds the file's previous good content, not a truncated prefix.
Red: `lane-logs/r6-red.log` (`page_id_with_a_scheme_is_refused`, both new
tests: the last disclosure for the file stays "refused").

## Missing piece
No test reached the refusal path of a write-tier file: the keystone refusal
transitions and `cook_vault_ingest` use `.cook` files, which are read-only tier,
so `home_is_writable` returns before the quarantine check. No test restored a
refused file to its last good bytes without a change event.

## Remedy
- `QuarantineCause::Refused`, set for an `AdapterRefusal`. `read_was_refused`
  makes all three read legs read such a file even when its bytes equal the last
  projection; the echo short-circuit then clears the quarantine and the refusal.
- `note_quarantine_skip` is cause-aware: a refusal-caused skip logs WARN with
  true text; ERROR stays for a partial ingest and a write-back veto.
- Tests: `crates/holon-orgmode/tests/page_id_with_a_scheme_is_refused.rs`
  `a_refused_page_file_restored_without_an_event_is_lifted_by_the_poll` and
  `..._by_the_re_render`. Each read leg reverted alone turns its test red
  (`lane-logs/r6-red-poll-leg.log`, `lane-logs/r6-red-rerender-leg.log`).
- Open: the per-document write-back leg has no test of its own, and the
  keystone has no write-tier refusal transition.
