---
id: 2026-11-21-timing-split-check-reads-local-ledger-and-date
date: 2026-11-21
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `timing-split.py check`, run by precommit and gate-arch, compared every cap with a p95 read from the local
  test-duration ledger and the current date, so one tree got different verdicts on different machines and days.
---

## Bug

`cmd_check` called `p95_low`, which reads `~/.holon-gate/test-duration-ledger.tsv` filtered by a 30-day window from
the current date. A cap that was inside its clamp on one machine or day was red on another. Found by the verify round of
the ledger card, not by a test.

## Root cause

The contract of the ledger card said what the ledger records and how a cap derives from it, but not that a check must
be a function of the tree. The ledger read was added to the check as a convenience.

## Missing piece

No test ran `check` against a planted ledger and several dates, so a verdict that moved with either went unseen.

## Remedy

`check` reads only the tree. `timing-split.py p95` (no argument: every capped test) prints the ledger's re-derivation
and always exits 0. `scripts/test-timing-split.py` plants a ledger in a temp dir that moves `p95_low` to 40 s and runs
`check` on three dates: the verdict is PASS and identical; `p95` shows the re-derived caps.
