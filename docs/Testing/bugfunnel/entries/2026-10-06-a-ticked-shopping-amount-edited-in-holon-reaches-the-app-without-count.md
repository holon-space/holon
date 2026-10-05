---
id: 2026-10-06-a-ticked-shopping-amount-edited-in-holon-reaches-the-app-without-count
date: 2026-10-06
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An amount edited in Holon on a ticked shopping item reached the app as
  `pickedCount` only; the app's own edit sends `count` too, and the picked row
  renders `count`.
---

## Bug
Found by a fresh-context verifier reading the app's client code
(`lane-logs/shopping-edits-verify.md` D2, 2026-10-06).

## Root cause
The ticked `update` command in `assets/integrations/shopping.yaml` sent
`good: {name, cat, pickedCount}`. The app's own edit of a ticked amount
(`index.html:1405` → `app.js:2849-2851` with `keep_count: true` →
`list.js:1521`) keeps `count` and sends both fields.

## Missing piece
ORACLE: the keystone compared only Holon's mirror with the model, never the
app's stored entries. ENVIRONMENT: the fake app was written from the same
inference as the mapping, so it confirmed the guess.

## Remedy
The mapping sends `count` and `pickedCount`, both the new amount. New
invariant `inv-remote-list-peer-matches-ref` compares the fake app's entries
with the reference model's app. Red:
`lane-logs/a4-RED-remote-list-a-ticked-amount-edited-in-holon-reaches-the-app.log`.
