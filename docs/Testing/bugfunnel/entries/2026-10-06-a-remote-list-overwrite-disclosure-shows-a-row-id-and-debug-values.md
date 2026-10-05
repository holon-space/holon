---
id: 2026-10-06-a-remote-list-overwrite-disclosure-shows-a-row-id-and-debug-values
date: 2026-10-06
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The warning that Holon's edit was overwritten named the percent-encoded row
  id and printed Rust Debug values (`Some(String("3 L"))`).
---

## Bug
Found by a fresh-context verifier (`lane-logs/shopping-edits-verify.md`,
Minor, 2026-10-06).

## Root cause
`crates/holon-connections/src/reconcile.rs` formatted the disclosure with
`row.id` and `{:?}`.

## Missing piece
ORACLE: nothing read the `overwritten` list; the keystone SUT read only
`refused`.

## Remedy
The disclosure names the row's key (name and category for shopping) and
renders values as JSON text. The keystone reads `overwritten` and asserts it
names the item. Red:
`lane-logs/a4-RED-remote-list-an-amount-edited-on-both-sides-keeps-the-apps-and-discloses-holons.log`.
