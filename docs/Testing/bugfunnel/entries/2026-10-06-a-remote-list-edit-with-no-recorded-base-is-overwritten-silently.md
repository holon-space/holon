---
id: 2026-10-06-a-remote-list-edit-with-no-recorded-base-is-overwritten-silently
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Where a mirror row has no recorded merge base (every row of an existing
  database until its first pull), a Holon value that differs from the peer's
  was overwritten with no disclosure.
---

## Bug
Found by a fresh-context verifier reading the reconciler
(`lane-logs/shopping-edits-verify.md` D5, 2026-10-06).

## Root cause
`crates/holon-connections/src/reconcile.rs`: a NULL base fell to a silent
`_ => {}` arm and the peer's value was written.

## Missing piece
COVERAGE: no property drew the merge three-way, unknown base included.

## Remedy
An unknown base with differing values keeps the peer's value and discloses
Holon's, like a conflict. New property `a_merge_column_settles_three_way` in
`crates/holon-connections/tests/remote_list_reconcile_pbt.rs`. Red:
`lane-logs/a4-RED-d5-reconcile-pbt.log`.
