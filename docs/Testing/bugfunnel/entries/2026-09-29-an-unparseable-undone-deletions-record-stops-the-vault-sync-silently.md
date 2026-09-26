---
id: 2026-09-29-an-unparseable-undone-deletions-record-stops-the-vault-sync-silently
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An unparseable `.loro/undone-deletions.json` makes the org sync fail at every boot with no condition on any screen.
---

## Bug
Found by the D229 round-6 verifier (lane d229-move), probes vr3/vr3b in `<scratchpad>/d229r6v/g3-probes.log`: the record is overwritten with invalid JSON while Holon is closed; at boot `start_app` succeeds, no condition is raised, and no file syncs. Report: `lane-logs/d229r6v-verify.md`, Defect A.

## Root cause
`initialize()` returned the parse error, and the controller task logged it and ended; nothing told the user.

## Missing piece
No transition corrupts vault state files, and no invariant asserts that a stopped sync is disclosed.

## Remedy
Fixed in round 7: the file is moved aside and `VaultStateUnreadable` names it (R11.1); an `initialize()` failure raises `VaultSyncNotStarted` (R11.2). Pinned by `an_unparseable_undone_deletions_record_is_set_aside_and_disclosed` and `a_file_sync_controller_that_cannot_start_is_disclosed`.
