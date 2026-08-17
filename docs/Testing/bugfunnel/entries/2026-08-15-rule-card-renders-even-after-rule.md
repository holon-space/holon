---
id: 2026-08-15-rule-card-renders-even-after-rule
date: 2026-08-15
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The rule card renders `last fired: -` even AFTER the rule has demonstrably
  fired and created its day page.
source_line: 699
---

## Bug

(task-#38/#42 web lane; found while verifying the rule-engine fix above)
**The rule card renders `last fired: -` even AFTER the rule has demonstrably
fired and created its day page.** In the green run the `daily_journal` rule
fired and `2026-08-15` appeared under Journals, yet the card's last-fired
field still showed `-`; it read `-` identically before the fix, so the field
appears never to be written or never read back.

## Root cause

task-#38/#42 web lane, found while verifying the rule-engine fix above:
**the rule card renders `last fired: -` even AFTER the rule has demonstrably
fired and created its day page.** In the green run the `daily_journal` rule
fired and `2026-08-15` appeared under Journals, yet the card's last-fired
field still showed `-`; it read `-` identically before the fix, so the field
appears never to be written or never read back. ORACLE, not ENVIRONMENT:
rules firing IS generatable in the keystone's wiring (native boot runs the
watchers and creates day pages — see the 2026-08-14 row's "the daily_journal
rule did not run" probe, which relies on that), so a case reaching this
state is reachable today; what is missing is any invariant asserting that a
successful rule firing updates its own bookkeeping. Remedy is a new
invariant in `pbt/composed/invariants/`: after a rule fires, its last-fired
stamp must be non-empty and non-decreasing. Rated worth its own row rather
than folded into the fix above because of its DIAGNOSTIC cost, which this
lane paid directly: `last fired: -` is the signal that correctly pointed at
the missing watchers, and it lies in exactly the same way on the success
path — a future debugger who trusts it after a real firing will be sent
hunting a watcher bug that no longer exists. NOT FIXED — filed as task #42,
out of scope for this lane.)

## Missing piece

Rules firing IS generatable in the keystone's wiring (native boot runs the
watchers and creates day pages — the 2026-08-14 "the daily_journal rule did
not run" probe relies on that), so a case reaching this state is reachable
today. What is missing is any invariant asserting that a successful rule
firing updates its own bookkeeping.

## Remedy

NOT FIXED — filed as task #42, out of scope for this lane. Remedy is a new
invariant in `pbt/composed/invariants/`: after a rule fires, its last-fired
stamp must be non-empty and non-decreasing. Rated its own row rather than
folded into the fix above because of its DIAGNOSTIC cost, which this lane
paid directly: `last fired: -` is the signal that correctly pointed at the
missing watchers, and it lies identically on the success path — a future
debugger who trusts it after a real firing will hunt a watcher bug that no
longer exists.
