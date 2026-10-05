---
id: 2026-10-06-deleting-a-shopping-key-the-app-holds-three-times-blocks-the-whole-list
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A Holon delete of a key the app holds three or more times as active entries
  sent one `del` per round commit, so the round failed after two commits and
  took every other item's change in that round down with it.
---

## Bug
Found by a fresh-context verifier (`lane-logs/shopping-edits-verify2.md`
DEFECT 1 and 2, 2026-10-06). The probe showed every round failing with "1
change(s) still un-pushed after 2 commits"
(`lane-logs/v2-probe2-probe-three-active-then-delete.log`).

## Root cause
The app's `cmd: del` removes one active entry per command (`www/js/list.js`
`applyCommands`). The mapping sent one `del` per Remove intent and a round
allows two commits, so the third entry kept the tombstone un-pushed.

## Missing piece
COVERAGE: the keystone can draw `AddDuplicate` twice on one key and then
`DeleteLocally`, but no replay case pinned it.

## Remedy
The pull mapping carries `active_entries` on each mirrored row; the commit
mapping emits that many `del` commands in the same commit, each under its own
id. Replay case `remote-list-a-key-held-three-times-is-deleted-in-one-round`.
Red: `lane-logs/a5-RED.log`. Teeth: `lane-logs/a5-TEETH.log`.
