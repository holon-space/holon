---
id: 2026-09-18-toggle-landing-loop-assumes-an-idempotent-click-verb
date: 2026-09-18
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  The shared `ToggleState` landing loop re-clicks while the projection lags on
  the stated premise that stale re-clicks are no-ops, which is only true for a
  click verb that computes its target from the view — not for the TUI's
  authority-relative `cycle_task_state` chord, whose every re-click is a real
  extra cycle.
---

## Bug

While fixing the TUI's missing state-toggle gesture (`2026-09-18-tui-state-toggle-
drove-nav-enter-not-the-cycle-chord`), the first fix routed the gesture through
the TUI's `cycle_task_state` chord (Ctrl+T). The verifier then found 10
state-toggle-family failures in 52 biased post-fix runs, including a silent
oracle divergence with no panic at all:

```
[inv-task-state-storage-coherence] 1 block(s) have task_state diverging from the
reference across block_raw SQL / Loro projection.
  block:c1: ref=Some("DONE") sql=Some("") loro=Some("")
[inv-viewmodel-state-toggle-correct] StateToggle current '' != reference 'DONE'
  — lane-logs/verify-v-post-toggle-r1.log, and 5 more in the same batch
```

Instrumented (temporary probes in `components.rs::toggle_state_via` and
`app_main.rs::dispatch_block_op_on_focused`, reverted with sha256 proof):
over 112 probed runs / 278 traced toggles `reclicks=0` in 110, and **the only
2 runs with a re-click are exactly the 2 runs that diverged** (2/2 vs 0/110) (`lane-logs/probeP-r3j5.log`, `-r3j6.log`).
The trace of `r3j5` is the mechanism in one line:

```
[probe-toggle] id=block:parent iter=0 RE-CLICK #1 block_raw_still="DONE"
[probe-toggle] id=block:parent iter=0 LANDED block_raw_state=""
...
[probe-toggle] id=block:parent DONE target="DOING" final_block_raw="DOING" reclicks=1
```

## Root cause

`HeadlessFrontendComponent::toggle_state_via`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:3765-3802`)
polls `block_task_state` for up to 500 ms after each click and RE-CLICKS if the
projection has not moved. Its comment states the premise: "Stale re-clicks are
idempotent (same value), and the first click after the view refreshes advances
exactly one step".

That premise holds only for the verb the other drivers use — a `set_field` whose
target is computed from a resolved view, so a re-click against a stale view
writes the keyword already stored. It does not hold for `cycle_task_state`,
which reads the prior keyword off the AUTHORITY and advances one step in the
ring (`crates/holon/src/api/operation_engine.rs:1917-1929`, ring `["", "TODO",
"DOING", "DONE"]`). Every re-click is therefore a real extra cycle the
reference never applied, and the SUT lands `#re-clicks` steps past it — which is
why the symptom is load-correlated (only a >500 ms projection lag triggers a
re-click) and why the loop can report success while the store is already ahead.

The loop is also not self-checking. Everything it reads — `block_task_state`,
and the view the click's target is computed from — comes from the SUT, and it
compares each reading only against the SUT's OWN previous reading, never against
the reference. It therefore breaks as soon as `block_raw` moves and can never
observe an over-advance: in the `r3j5` trace the loop reports `final_block_raw=
"DOING"` (= target, success) while the store is already one cycle past it. The
reference model has no way to express "the SUT advanced more than asked" either,
so only a later cross-layer reconcile can catch this — which is how it was
caught.

Gap class: COVERAGE. ORACLE does not fit, and the earlier label was wrong — the
invariants DID go red (`inv-task-state-storage-coherence`,
`inv-task-state-matches-ref`, `inv-viewmodel-state-toggle-correct`, and all four
`inv-blocks-match-ref` arms), so nothing was missing on the oracle side. What
the generator cannot do is REQUEST the trigger: the re-click path opens only
when the projection lags more than the loop's hardcoded 500 ms window, and no
transition, precondition or weight can ask for that.

## Missing piece

Any declaration of what a driver's toggle verb IS. `UserDriver::cycle_state_toggle`
documents the glyph-click contract and the geometry precondition, but nothing
states whether the verb is view-computed (idempotent under a stale view) or
authority-relative (one real step per call) — so a driver cannot be judged for
choosing a verb the shared loop cannot safely re-issue, and the loop cannot
adapt to one that does.

## Remedy

PARTIAL. The symptom is closed for the TUI by giving it the view-computed verb
the loop assumes (`TuiUserDriver::cycle_state_toggle` now dispatches the
resolved-tree `state_toggle` cycle intent, mirroring `ReactiveEngineDriver`) —
68 biased runs post-change, 0 toggle-family failures, at load up to 111. The
unsound assumption itself remains in the loop.

The structural fix is a trait-level one and is NOT made here — it is a design
fork for Martin: either `cycle_state_toggle` gains a declaration (e.g. whether
the verb is view-computed) that `toggle_state_via` branches on, or the loop
stops re-clicking entirely and asserts the final state instead. Until then, the
TUI's Ctrl+T path — a real production verb — is not exercised by this rung, and
any future driver that picks an authority-relative verb re-opens the same
divergence. No registry row: the signature does not reproduce against the tree
as it now stands.
