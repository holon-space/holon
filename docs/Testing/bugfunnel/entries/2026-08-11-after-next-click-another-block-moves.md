---
id: 2026-08-11-after-next-click-another-block-moves
date: 2026-08-11
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  After a `split_block`, the next click on another block moves ENGINE focus
  but no editor takes WINDOW focus for at least 5s — the caret is nowhere and
  the next keystroke would be dropped.
source_line: 736
---

## Bug

(task #92 Cucumber-dogfood rehearsal, found by DOGFOODING at main
`644a399d`; no automated test produced it) **After a `split_block`, the next
click on another block moves ENGINE focus but no editor takes WINDOW focus
for at least 5s — the caret is nowhere and the next keystroke would be
dropped.** The driver refuses loudly, which is the only reason it is
visible: `GpuiUserDriver::click_entity: "<id>"'s editable_text never took
window focus within 5s. Engine focused_block=Some("<id>"); editors reporting
window focus: []. The gesture reached the element but did not seat a caret,
so any following keystroke would land nowhere (or in another block).`
Reproduced 3/3 after a split, once on an idle machine (so not merely load).
An identical second click always recovered and typing then worked. A human
sees no refusal — the app has no driver to refuse — only "I clicked, I
typed, nothing happened".

## Root cause

task #92 Cucumber-dogfood rehearsal, found by DOGFOODING at main `644a399d`:
**after a `split_block`, the next click on another block moves ENGINE focus
but no editor ever takes WINDOW focus, so for at least 5 seconds the caret
is nowhere and the user's next keystroke would land nowhere.** The driver
refuses loudly (this is good behaviour, and it is the only reason the
condition is visible at all): `GpuiUserDriver::click_entity: "<id>"'s
editable_text never took window focus within 5s. Engine
focused_block=Some("<id>"); editors reporting window focus: []. The gesture
reached the element but did not seat a caret, so any following keystroke
would land nowhere (or in another block).` Reproduced 3/3 after a split,
including once on an otherwise IDLE machine (the first two coincided with a
concurrent cargo build, the third did not — so it is not simply load). A
second identical click always recovered, and typing then worked, so the
window is transient; a human would experience it as "I clicked, I typed,
nothing happened" and would not see the refusal, because in the real app
there is no driver to refuse — the keystroke is simply dropped. Primary
ENVIRONMENT: window focus vs engine focus is a GPUI-only distinction that
the headless rung has no representation for at all (`editors reporting
window focus` has no headless analogue), and the invariant that would catch
it — `inv-window-focus-matches-engine-focus` — is one of the 28 checks
`run_self_checks` reports as `skipped` against a live app (`no live source
for SUT capability SutDriver, SutLayout`), so the app's own live self-check
surface is blind to exactly this. Missing piece: a windowed rung that splits
and then clicks a DIFFERENT block, asserting an editor holds window focus
within a bounded budget. OPEN.)

## Missing piece

ENVIRONMENT: window focus vs engine focus is a GPUI-only distinction with no
headless representation, and the invariant that would catch it
(`inv-window-focus-matches-engine-focus`) is one of the 28 checks
`run_self_checks` reports `skipped` against a live app (`no live source for
SUT capability SutDriver, SutLayout`) — so the live self-check surface is
blind to exactly this. Missing piece: a windowed rung that splits, then
clicks a DIFFERENT block, asserting an editor holds window focus within a
bounded budget.

## Remedy

OPEN — reported, not fixed.
