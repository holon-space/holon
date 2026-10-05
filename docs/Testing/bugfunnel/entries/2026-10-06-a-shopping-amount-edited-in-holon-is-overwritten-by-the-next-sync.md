---
id: 2026-10-06-a-shopping-amount-edited-in-holon-is-overwritten-by-the-next-sync
date: 2026-10-06
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  An amount changed in Holon on an item the That Shopping List app already
  carries is never pushed, and the next sync round writes the app's old amount
  back over it, silently.
---

## Bug
Found by a verifier (lane shopping-count, report `lane-logs/shopping-count-verify3.md`
defect V1) on 2026-10-06 after Martin's ruling D93.a (an edited amount reaches
the app and survives the next sync, for active and ticked items). Probe P1: the
peer lists Milk with `count` 2, a person sets `3kg` in Holon, one round later
the mirror reads `2` and nothing was pushed. Probe P2: a ticked item with no
`pickedCount`; the typed `2kg` was replaced by null.

## Root cause
The reconciler merged a merge column by "the peer wins" and could only emit a
push for a row the peer did not yet list (`reconcile.rs` Add arm). It kept no
base value, so a Holon edit and a peer edit looked the same. The shipped
`shopping.yaml` also had no mapping for an edit: the app's own mechanism is
`cmd: add` for the same name (active) and `picked: replace` (ticked)
(`docs/Plans/ThatShoppingList-API-2026-09-01.md`).

## Missing piece
COVERAGE: the keystone's remote-list fixture was a hand-written analogue of the
mapping and had no local-edit arm. The `AuthorLocally` arm always created a
fresh row, so no sequence edited an item the peer already held.

## Remedy
Three-way merge per merge column with a stored base (`list_sync.base_column`,
`remote_base`), `PushIntent::Update` with verb `update`, and update mappings in
`shopping.yaml`. The keystone fixture now runs the shipped mapping against a
fake app with the app's command semantics. Red logs before the fix:
`lane-logs/edits-red-an-active-amount-edited-in-holon-reaches-the-app.RED.log`,
`lane-logs/edits-red-a-ticked-amount-edited-in-holon-reaches-the-app.RED.log`.
Green: `lane-logs/e-green-summary.log`. A conflict (both sides changed) lets the
peer win and is disclosed by name (`overwritten`).
