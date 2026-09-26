---
id: 2026-09-29-ending-a-copy-relation-silently-undoes-the-users-deletion
date: 2026-09-29
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  When the user deletes the whole copied heading from the copy file, a child the user had deleted from its own file stays in the store and on disk with no disclosure.
---

## Bug
Found by the D229 round-5 verifier (lane d229-move), probe VP3 in `lane-logs/d229r5v-probes2.log`: after the put-back of the child, the user deletes the copied heading from `Overview.org` as the `block-in-two-files` banner asks. The conditions become empty and the child the user deleted is back for good. Report: `lane-logs/d229r5v-verify.md`, Defect 2.

## Root cause
Ending the copy relation cleared the undone deletion with its disclosure, but did not let the deletion stand.

## Missing piece
The keystone model shared the bug: its copy map cleared the undone member when the copy relation ended, so `inv-conditions-match-ref` stayed green.

## Remedy
Fixed in round 6: an undone deletion resolves only by standing (R10.1); the model deletes the member when no copy holds it.
