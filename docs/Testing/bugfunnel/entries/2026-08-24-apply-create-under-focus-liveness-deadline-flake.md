---
id: 2026-08-24-apply-create-under-focus-liveness-deadline-flake
date: 2026-08-24
gap: ENVIRONMENT
secondary: ORACLE
status: UNCLASSIFIED
summary: >-
  `apply_create_under_focus` tripped `commit_creation_slot`'s 3s driver
  liveness deadline — the dispatched `block.create`'s row never landed in
  time. Under the load hypothesis the deadline is a test-only construct with
  no prod analogue (ENVIRONMENT); under the real-defect hypothesis no
  invariant is designed to catch a dropped birth at all (ORACLE, secondary).
  A/B rerun (both arms green) adjudicated it drawn/pre-existing; which
  hypothesis is true is undiagnosed.
---

## Bug

`apply_create_under_focus` (`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:3698-3705`)
drives the production creation-slot gesture via `UserDriver::commit_creation_slot`.
That call bailed on 2026-08-24 with the liveness deadline in
`crates/holon-frontend/src/user_driver.rs:701-717`:

```
"creation-affordance birth of {born} under {parent} did not land within 3s — \
 the block.create dispatched by the focus edge never produced a row"
```

Correction of the original filing (this entry was previously named and framed
around an invented "3-second birth budget"): there is no interaction→visible
latency measurement on this path. What actually fires is a wait loop that
polls `row_mutable(&born)` every 20ms and bails loudly if the newborn block's
row still hasn't materialized after 3s. The transition budget system's real
ceilings are unrelated and far looser: `transition_budgets.rs:466`
single-query 2s, `:479` wall 30s, `:496` settle Warn 2500ms, and
`apply_create_under_focus`'s own post-apply settle barrier is
`Duration::from_secs(5)` (`components.rs:3705`). As with the other entries in
this batch, the raw log with the actual case/seed did not survive a session
restart; only the orchestrator's adjudication note (signature, A/B method,
dates 2026-08-24/25) is available. File and id renamed from
`apply-create-under-focus-birth-budget-flake` because that name asserted the
same invented construct the body now corrects.

## Classification

Two hypotheses are live (see Root cause); the gap is argued from each rather
than asserted from a latency rule that does not apply here.

**Under the load/timing hypothesis**, the fixed 3s deadline is a test-harness-only
construct with no production analogue — production has no equivalent
liveness check on a creation birth, only the far looser transition budgets
above — so scheduler contention pushing an otherwise-successful birth past
that rigid test-only ceiling is exactly the ENVIRONMENT litmus: the thing
that fails (a hard 3s poll-and-bail) exists only in the keystone's wiring, not
in prod's. **Under the real-defect hypothesis**, the row genuinely never
lands, which no keystone PBT invariant is designed to catch — this liveness
wait is driver/harness plumbing, not a registered correctness invariant — so
the same silent drop could happen in production today with nothing to flag
it, which is the ORACLE gap (no invariant exists for "a dispatched create's
row eventually arrives"), kept as secondary since the load hypothesis is at
least as well supported by what survived.

## Root cause

Not diagnosed. Adjudicated DRAWN by A/B: rerun at the landing lane's tip and
at the base arm both came back green, so the failure did not reproduce on
either side of the change under test. That clears the landing lane but says
nothing about mechanism. Two hypotheses are live and this entry deliberately
does not pick one:

1. **Real create-path defect.** "The block.create dispatched by the focus
   edge never produced a row" is a literal description of a birth that
   silently failed to land — this is at least as consistent with a genuine
   bug in the creation-affordance birth path (`ReactiveEngine::birth_creation_affordance`,
   named at `user_driver.rs:683`) as with load. The comment immediately above
   the deadline (`user_driver.rs:695-700`) documents a related, NAMED race —
   "A driver that writes the instant focus moves would race ahead of the
   create and see 'Block not found'" — for a driver that skips the wait
   entirely; it does not establish that a driver which DOES wait and still
   times out at 3s is experiencing the same race rather than a real drop.
2. **Load/timing.** Scheduler or resource contention at the moment of the one
   slow run pushed an otherwise-successful birth past the fixed 3s ceiling
   with no headroom-for-load allowance.

Nothing in what survived distinguishes these.

## Missing piece

No per-stage timing or intermediate state (was the `block.create` dispatched
at all? did any row exist under a different id? was the engine still
processing when the deadline fired?) was captured for the failing run — the
wait loop's bail message reports only the terminal state, not the path to it.
Making this attributable would need the loop to log its dispatch confirmation
and poll count on timeout, so a recurrence can show whether the create was
ever acknowledged before the deadline hit.

## Remedy

OPEN in the sense that the mechanism is undiagnosed and neither hypothesis
above is ruled out. UNCLASSIFIED rather than NOTED: the A/B clears the
landing lane of causing this occurrence, but "row never arrived" is a
strong enough signal of a possible real defect that filing it as a settled,
no-action flake would be dishonest. If `commit_creation_slot`'s 3s deadline
fires again, capture the dispatch/poll detail above before it rotates out.
