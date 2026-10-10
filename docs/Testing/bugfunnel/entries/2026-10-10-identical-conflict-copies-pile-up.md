---
id: 2026-10-10-identical-conflict-copies-pile-up
date: 2026-10-10
gap: ORACLE
secondary: COVERAGE
status: PARTIAL
summary: >-
  The same overruled text was saved again as a new conflict copy on every overrule, leaving byte-identical duplicates beside the file.
---

## Bug
Found by the round-4a verifier of the `merge-resurrect` lane. An overrule that
repeated with the same overruled text wrote a new conflict copy each time, so
the vault collected identical copies.

## Root cause
`FileSyncController::save_conflict_copy` always allocated a fresh numbered name
and never looked at the copies it had already written
(`crates/holon-filesystem/src/file_sync_controller.rs`).

## Missing piece
No test overruled the same text twice and counted the copies; no invariant
bounds the number of copies per distinct text.

## Remedy
Fixed in 2127edd3: the controller remembers the copies it wrote per file
(`conflict_copies`) and reuses one whose bytes still equal the text. Pinned by
`the_same_overruled_text_is_saved_once` (`crates/holon-app/tests`). Open: the
memory is in-process, so the same overrule after a restart writes a second
identical copy.
