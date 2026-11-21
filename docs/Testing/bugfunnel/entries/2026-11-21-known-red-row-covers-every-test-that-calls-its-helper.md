---
id: 2026-11-21-known-red-row-covers-every-test-that-calls-its-helper
date: 2026-11-21
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A known-red row whose pattern and site sit in a shared test helper demoted the same assert in every test that calls the helper, so a new red there passed the gate.
---

## Bug
A known-red row matched a panic by message and site only.
When both live in a helper that many tests call, such as `crates/holon-integration-tests/src/pbt/op_write_cap.rs`, the row also demoted the same assert in every other caller.
Verifiers of the focus-roots-feed and hm4 lanes found this: a new red in a caller the row never saw passed as a known red.

## Root cause
`scripts/keystone-known-reds.sh` matched a row against the message and the location of a panic.
The classifier knew the test of each panic (nextest unit, owned by the text) but a row could not name one.

## Missing piece
No column or cell told the classifier which tests a row covers.
Layer: the oracle that decides whether a red is known.

## Remedy
A Companions cell may carry `; tests `<binary id> <test name>, ...``.
`scripts/check-known-red-evidence.py` parses it and hands the units to the classifier; the classifier lets the row match only inside an instance whose unit the row names.
`scripts/test-known-reds-test-scope.py` covers the named test, another test, a name that only starts or ends like the named one, a libtest section and a row without tests.
The windowed focus-bounds row is scoped to the three tests its evidence shows.
