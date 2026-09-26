---
id: 2026-09-30-an-undone-deletion-record-outside-the-vault-wedges-every-ingest
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  One undone-deletion record whose path is outside the vault makes every later ingest fail, and the failure blames the file being ingested.
---

## Bug
Found by the D229 round-7 verifier (lane d229-move), probe VP-C in `lane-logs/d229r7v-probes3.log`. Report: `lane-logs/d229r7v-verify.md`, Residual 2.

## Root cause
`persist_undone_deletions` failed on the bad path, and `on_file_changed` returned that error for every ingest.

## Missing piece
No transition writes a malformed vault state record.

## Remedy
Fixed in round 8: each record is parsed at load and a bad one is refused alone and disclosed (R12.4); `an_undone_deletion_record_outside_the_vault_is_refused_alone`.
