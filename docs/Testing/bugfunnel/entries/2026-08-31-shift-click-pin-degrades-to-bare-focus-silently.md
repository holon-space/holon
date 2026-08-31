---
id: 2026-08-31-shift-click-pin-degrades-to-bare-focus-silently
date: 2026-08-31
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The keystone's PinBlock drive path silently degraded a shift+click into a
  bare focus whenever the bullet had not streamed into the Main panel yet, so
  the pin never happened and the failure was mis-attributed to the focus_roots
  matview.
---

## Bug

Two `holon-integration-tests` lib tests were red on `main` (4ad0b03f) and stayed
red at every base measured back to `cc3439f0`, before the whole mobile wave:

- `pbt::frontend_slice::components::tests::headless_nav_history_ops_dispatch`
- `pbt::frontend_slice::components::tests::headless_pin_block_right_sidebar_probe`

Found when a land gate finally ran the `--lib` suite. No land gate had run it
for many landings, so the reds accumulated unseen. The pin probe's message
accused the matview — "the matview did not update without a window" — which is
not what was wrong.

The second test is also load-dependent: it passed when run alone and failed in
the full 411-test suite, so it doubles as a latent flake in the keystone's
`PinBlock` transition.

## Root cause

`SutNavHistoryDrive::pin_block` was rewritten from a synthetic
`dispatch_navigation("focus_pin", region, ...)` into the production gesture —
a shift+click on the block's bullet, whose `shift_action` is declared in
`assets/default/types/block_profile.yaml:158` and always targets
`right_sidebar`.

That gesture carries a precondition the synthetic dispatch did not: the bullet
must actually be rendered. The Main panel renders only the descendants of
`focus_roots(main)` (`block:default-main-panel::src::0`), streamed in through
nested `live_block` watches.

`ReactiveEngineDriver::click_entity_with_modifiers`
(`crates/holon-frontend/src/user_driver.rs:832`) polls the resolved tree for at
most 2s and then falls through to a bare `set_focus`
(`user_driver.rs:867-901`), emitting only a `tracing::warn!` — which has no
subscriber in the lib harness. That fallback is correct for production (GPUI
does the same for a click that binds nothing), but in the drive path it turns a
pin that never happened into a silent no-op.

Both tests tripped it, for two different reasons:

- `headless_nav_history_ops_dispatch` pinned with `Region::Main` and asserted
  `focus_roots(main)`, pinning the pre-rewrite semantics. Under the production
  gesture it hit the `assert_eq!(region, RightSidebar)` at
  `components.rs:3151`.
- `headless_pin_block_right_sidebar_probe` used the right region but never
  focused Main on the containing doc, so `block:ref-block-0` was never on
  screen and the click degraded. Under full-suite load the 2s poll also expired
  before the watch streamed, which is why it passed in isolation.

Evidence: `lane-logs/bisect-cc3439f0.log` (red before the mobile wave),
`lane-logs/red-baseline.log`, `lane-logs/red-baseline-pinprobe.log`,
`lane-logs/gate-lib-full-BASELINE.log` (32 failed) vs
`lane-logs/gate-lib-full.log` (30 failed, the two flipped, none newly red).

## Missing piece

No assertion fired at the point of degradation. The drive path let a click that
bound no intent look like a click that worked, and the only surviving signal
was a `tracing::warn!` into a harness with no subscriber — so the failure
surfaced several steps later as an empty `focus_roots` and blamed the matview.
`open_tab_via_modifier_click` already had exactly this barrier
(`await_sidebar_intent`) for the LeftSidebar; the Main-panel bullet had none.

Compounding it: the land gates did not run the `holon-integration-tests --lib`
suite, so these reds survived many landings without anyone seeing them.

## Remedy

Added `HeadlessFrontendComponent::await_main_click_intent`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1529`),
the Main-panel counterpart of `await_sidebar_intent`: it waits up to 5s for the
bullet to bind the modifier intent and panics loudly with
`click_intent_miss_reason` when it never does. `pin_block` now calls it before
the shift+click, so a degraded pin fails at the degradation instead of
downstream.

Both probes were updated to establish the production precondition — focus Main
on the containing doc — and to assert the effect where the shipped
`shift_action` actually puts it, `focus_roots(right_sidebar)`.

Land gates should run the `--lib` suite; 30 pre-existing reds remain in it and
are out of scope here.
