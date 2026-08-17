---
id: 2026-08-09-shrinking-keystone-divergence-aborts-entire-shrink
date: 2026-08-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Shrinking a keystone divergence aborts the entire shrink with `capability
  SutAppLifecycle was selected but is absent from the CapMap
  (selection/composition bug)` (`holon-pbt-core/src/composition.rs:221`)
source_line: 753
---

## Bug

(task #46 lane, found by a verifier reading a keystone lane log, not by any
test) **Shrinking a keystone divergence aborts the entire shrink with
`capability SutAppLifecycle was selected but is absent from the CapMap
(selection/composition bug)` (`holon-pbt-core/src/composition.rs:221`)**,
destroying the divergence report behind it (the #30 outdent-abort class).
`init_state` DRAWS the wiring, so the CapMap is a function of it;
`proptest-state-machine`'s `InitialState` shrink phase narrows that wiring
and re-validates the already-drawn transitions through `preconditions`
ALONE, which carried no cap/wiring gate — and transition value trees cannot
be regenerated from the shrunk state. A `{Loro, Turso}` draw carrying
`CreateDocument` (ref precondition `app_started`, seeded true for every
wiring) shrinks to the valid `{Loro}` manifest, which composes no frontend
component and hence no `SutAppLifecycle`. Not cap-specific:
`SutViewControl`/`SwitchView` survives the same way.

## Root cause

task #46 lane, found by a verifier reading a keystone lane log
(`verify-42-smoke.log`), not by any test: **SHRINKING a keystone divergence
aborts the whole shrink with `capability SutAppLifecycle was selected but is
absent from the CapMap (selection/composition bug)`
(`crates/holon-pbt-core/src/composition.rs:221`)** — the same
unregistered-hard-abort class as the #30 outdent abort: the abort replaces
the divergence report, so the run tells you nothing about the bug it
actually found. ROOT CAUSE is a selection-vs-insertion disagreement that
only shrinking can create. The keystone's `WideE2EMachine::init_state` DRAWS
the wiring (`any_valid_wiring().prop_map(wide_e2e_ref_for)`), so the CapMap
is a function of the drawn wiring, and `aggregate_transitions` admits a
variant only if `required_wiring().satisfied_by(wiring) &&
caps_available(required_caps())` (`transition_dispatch.rs`).
`proptest-state-machine`'s `SequentialValueTree` has an `InitialState`
shrink phase (`strategy.rs:450`) that narrows that wiring, and the ONLY
filter it re-applies to the already-drawn transitions is
`ReferenceStateMachine::preconditions` (`check_acceptable`,
`strategy.rs:508`) — which carried the ref-side checks ALONE, no cap or
wiring gate. Transition value trees are built from the ORIGINAL state and
can never be regenerated, so a `{Loro, Turso}` draw carrying
`CreateDocument` shrinks to the valid `{Loro}` manifest — whose
`ComponentSet` has no frontend component and therefore no `SutAppLifecycle`
— while `CreateDocument` (ref precondition: `app_started`, which the
composed oracle seeds true for EVERY wiring) survives untouched, and the
re-booted SUT dies in `CapMap::expect`. A Turso-ONLY draw cannot trigger it
(dropping Turso leaves zero storage adapters and `Wiring::validate` rejects
the shrink), which is why it needed a mixed draw and read as intermittent.
NOT SutAppLifecycle-specific: the same walk convicted `SutViewControl`
(`SwitchView`). ENVIRONMENT: the escape is pure harness machinery — no
product code is involved, the interaction is generated constantly, and every
invariant was adequate; what was missing is a test environment in which the
SHRINK phase is exercised at all. FIXED in-lane by making shrink acceptance
mirror generation: `WideE2EMachine::preconditions` now calls
`stepper::transition_applicable` — the SAME value-level `required_wiring` +
`required_caps` mirror the replay engine already uses — before the ref-side
precondition, so a composition that cannot host its sequence is
shrink-INVALID and proptest keeps the wider wiring. Deliberately NOT a
skip-if-absent arm in `apply_to_sut` (that would silently weaken every
capability assertion) and NOT CapMap regeneration (the CapMap already
regenerates correctly per initial state; it is the transition alphabet
proptest cannot regenerate). Red-first, three locks: two unit reds naming
the mechanism, plus
`the_shrinker_never_yields_a_sequence_its_capmap_cannot_host`, which drives
the REAL shrink loop with a real failing property and reproduces the abort
verbatim at base (`the shrinker kept CreateDocument under wiring {Loro},
whose CapMap cannot host it`) after walking `{Loro,Turso}` → `{Loro}`.
EVIDENCE SCOPE, stated precisely because the obvious reading overclaims: the
64-case keystone run shrank a genuine `org-blocks-ref-diverge` known red
through 2 shrink re-panics and reported it normally
(`keystone-known-reds.sh` PASS-WITH-NOTE, 0 novel), but its failing draw was
`storage={Turso}` ONLY — a shape that CANNOT trigger this bug, since
dropping Turso leaves zero storage adapters and the shrink is rejected as
invalid. That run is therefore NO-REGRESSION evidence, not an exercise of
the fixed path; the triggering `{Loro, Turso}` shape is covered ONLY by the
third lock, which works at reference level and boots no SUT. Its reported
case is also proptest's "minimal", not a proven minimum — it still holds 36
of ~39 transitions, budget-capped at `max_shrink_iters` 200. TRADEOFF of the
shrink-invalid choice, disclosed: a shrunk initial state is rejected while
ANY surviving transition is gated out, so wiring-axis shrink power is
materially reduced until `DeleteTransition` has cleared those transitions —
narrower minimal wirings than before are reached later, or not at all, in
exchange for never aborting. Header COUNTERS were re-summed against main
1dcf7175 (this lane's base 8347e18d predates the #44/#40 landings), so
`bugfunnel-check.sh` MISMATCHES in the lane worktree by construction and
reconciles post-weave. Evidence: `lane-logs/task46-red-shrinker.log` (shrink
trajectory + abort), `lane-logs/task46-green-final.log`,
`lane-logs/task46-keystone-64cases.log`.)

## Missing piece

Nothing exercised the SHRINK phase: every test drove generation and
application, so the one place where selection and insertion can disagree had
no coverage. Product code and invariants are not involved.

## Remedy

**FIXED in-lane 2026-08-09 (task #46).** `WideE2EMachine::preconditions` now
applies `stepper::transition_applicable` (the same value-level
`required_wiring` + `required_caps` mirror the replay engine uses) before
the ref-side precondition, so a composition that cannot host its sequence is
shrink-INVALID and proptest keeps the wider wiring — chosen over a
skip-if-absent arm (silently weakens every capability assertion) and over
CapMap regeneration (the CapMap already regenerates per initial state; the
transition alphabet is what proptest cannot regenerate). Red-first: 2
mechanism units plus
`the_shrinker_never_yields_a_sequence_its_capmap_cannot_host`, which drives
the real shrink loop and reproduces the abort verbatim at base after walking
`{Loro,Turso}` → `{Loro}`. Acceptance, scoped honestly: the 64-case keystone
run shrinks a real `org-blocks-ref-diverge` known red through 2 re-panics to
proptest's "minimal" case (still 36 of ~39 transitions,
`max_shrink_iters`-capped at 200) and classifies PASS-WITH-NOTE, 0 novel —
but its failing draw is `{Turso}`-only, a shape that cannot trigger this
bug, so that run is NO-REGRESSION evidence; the triggering `{Loro, Turso}`
shape is covered only by the third lock (reference-level, no SUT boot,
nondeterministic seed). Disclosed tradeoff: a shrunk initial state is
rejected while ANY surviving transition is gated out, so wiring-axis shrink
power drops until `DeleteTransition` clears them. Header counters re-summed
against main 1dcf7175.
