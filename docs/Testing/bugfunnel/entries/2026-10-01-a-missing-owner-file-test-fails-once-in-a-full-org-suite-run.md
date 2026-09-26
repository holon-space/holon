---
id: 2026-10-01-a-missing-owner-file-test-fails-once-in-a-full-org-suite-run
date: 2026-10-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `a_missing_owner_file_does_not_hand_the_block_away` failed once in a full org_suite run with no `block-in-two-files` condition raised; the cause is not found.
---

## Bug
Found by the D229 round-11 verifier (F1 in `lane-logs/d229r11v-verify.md`), lane d229-move. One failure in the first full org_suite run: no `block-in-two-files` condition was raised (left `[]`). 8/8 green alone (`lane-logs/d229r11v-missing-owner-loop.log`), green in two later full runs, and green in the round-12 full run (`lane-logs/d229r12b-orgsuite.log`, 97/97).

## Root cause
Not found. The test writes Overview.org and removes DayPage.org back to back, so the order in which the controller sees the two events may decide the outcome. Not confirmed; ENVIRONMENT (timing under load) is the working class until it is root-caused.

## Missing piece
No reproduction. A loop under load, or a test that fixes the event order, would show whether the product depends on that order.

## Remedy
Open. Not chased in round 12.
