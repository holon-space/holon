---
id: 2026-11-21-fork-wrapper-panics-count-as-novel-beside-a-known-primary
date: 2026-11-21
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The known-red classifier read the two fork-mode wrapper panics of a proptest test as novel even when the primary panic of the same test was a known red.
---

## Bug
A proptest test in fork mode prints three panics for one defect: the primary, `Test failed normally in child process.` (child) and `failed in other process` (parent).
Verifiers of the loro-backend and replay-timeout lanes saw the gate FAIL with two novel signatures that were only these wrappers beside a registered primary.
Two stop-gap rows (`iroh-proptest-fork-child-wrapper`, `iroh-proptest-fork-parent-wrapper`) demoted them for one test, but the child row matched the wrapper of every forked test, so it also covered a wrapper next to a novel primary.

## Root cause
`scripts/keystone-known-reds.sh` matched each signature on its own against the rows.
The wrappers have no message of their own, so no row could describe them without covering every forked test.
The classifier already knew the test instance of each panic (collateral pairs), but wrappers did not use it.

## Missing piece
No rule tied a wrapper to the verdict of the primary panic of its own test instance.
Layer: the oracle that decides whether a red is known.

## Remedy
`fork_wrapper` in `scripts/keystone-known-reds.sh`: a wrapper is collateral only in an instance whose other signatures are all known and at least one matched a row; otherwise it is novel.
`scripts/keystone-known-reds-fixture.sh` cases `fork-wrapper/*` cover the evidence log, a loro-shaped log, a novel primary, a lone wrapper, a wrapper beside another test's primary and a wrapper beside another novel panic.
The two stop-gap rows are deleted.
