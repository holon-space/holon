---
id: 2026-10-05-deleted-vault-org-file-is-recreated-by-projection-writer
date: 2026-10-05
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A vault org file the user deletes right after its first ingest is recreated
  by the projection writer, and its blocks stay in the database.
---

## Bug
Found while testing lane d45-followups (D66.a claim release). The test booted
a real session over a temp vault, wrote `profiles.org` (one block with a
profile source block), waited for the block to load, then deleted the file with
`std::fs::remove_file` and waited for the profile row to leave the database.
In 1 of 3 runs (and in every run when a second session booted at the same
time) the row never left: `profiles.org` existed again at the deadline and the
block was still in the database. With the file deleted again in a loop until
the block vanished, 5 of 5 runs passed.

A sibling observation: a user EDIT of the same file written shortly after the
first ingest was also lost in 1 of 8 parallel runs (the edit never reached the
profile load check: "the edit was never refused", log lane-logs/d66d-rep5.log).
It points at the same race, a write-back overwriting a fresh user write.

## Root cause
Not established. Observation only: the file reappears after the user's delete,
so a writer wrote it after the delete event. Candidates: the Loro/org
projection writer finishing its write-back of the freshly ingested document
after the delete; or the file watcher reading the delete as a stale event. No
trace was taken.

## Missing piece
No test deletes a vault document right after ingest. The keystone has no
"delete the file on disk" transition.

## Repro
1. Boot a session with `vault.root` set to an empty temp dir.
2. Write `profiles.org` with one headline holding a `holon_entity_profile_yaml`
   source block.
3. Wait until `SELECT id FROM block WHERE source_language =
   'holon_entity_profile_yaml'` returns a row.
4. Delete `profiles.org` once and poll the query for 30 s.
5. Expected: 0 rows and no file. Seen in 1 of 3 runs: the file exists again and
   the row remains. Logs: lane-logs/d66c-rep.log (the FAIL, "file exists
   again: true"), lane-logs/d66c-green.log (first failing runs).

## Remedy
Open. Not fixed in lane d45-followups. The D66 deletion test deletes the block
through `block.delete` instead, so it does not depend on this race.
