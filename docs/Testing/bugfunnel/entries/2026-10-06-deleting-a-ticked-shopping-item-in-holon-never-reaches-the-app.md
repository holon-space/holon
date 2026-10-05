---
id: 2026-10-06-deleting-a-ticked-shopping-item-in-holon-never-reaches-the-app
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A local delete of a ticked shopping item sends `cmd: del`, which removes only
  from the active list, so the picked entry stays and the sync round fails after
  two commits.
---

## Bug
Found by agent static analysis of the app's client code (`list.js`) while
building the edit lane (lane shopping-edits, 2026-10-06). The red replay
confirmed it: the round fails loud with "1 change(s) still un-pushed after 2
commits".

## Root cause
The remove mapping in `assets/integrations/shopping.yaml` always sent
`{cmd: del}`. A ticked item lives in the picked list, which the app removes with
`{picked: "del"}` (`list.js` removePickedItem). The re-pull still lists the
picked entry, so the delete is never confirmed.

## Missing piece
COVERAGE: the keystone had no arm that deleted a ticked item locally.

## Remedy
`PushIntent::Remove` carries the peer's record; the remove mapping sends
`picked: del` when `.remote.checked`. New keystone arm and replay case
`remote-list-a-ticked-item-deleted-in-holon-leaves-the-app`. Red:
`lane-logs/edits-red-a-ticked-item-deleted-in-holon-leaves-the-app.RED.log`.
