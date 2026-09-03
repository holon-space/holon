---
id: 2026-09-03-closing-quick-open-never-returns-keyboard-focus
date: 2026-09-03
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  After Escape closes the quick-open overlay the window holds no editor focus,
  so every following keystroke is dropped while the engine still reports a
  focused block.
---

## Bug

Found by the `dogfood-search` lane driving the live GPUI app as the dogfood
gate for the quick-open search fix.

Sequence: click a block (it takes focus and a caret), press cmd-K, press
Escape. The overlay closes and the page looks exactly as before — the engine
still reports that block as focused — but typing now does nothing at all. The
user has to click a block again to get a caret back.

The driver makes the state explicit. `send_key_chord` refuses:

    "block:compass-index-confirm-edges-health"'s editable_text never took
    window focus within 5s. Engine focused_block=
    Some("block:compass-index-confirm-edges-health"); editors reporting
    window focus: []

and `type_text` reports `dropped all 3 keystroke(s): no focused editor (or
bound action) consumed them`.

## Root cause

`SearchUiState::open` (`frontends/gpui/src/search_ui.rs:122-140`) moves window
focus to the modal's own text input (`input.focus(window, cx)`), which blurs
whatever editor held it. `SearchUiState::close` (`search_ui.rs:142-149`) only
flips `open = false` and dismisses the soft keyboard — it never hands window
focus back. Nothing else restores it, so the window is left with focus on a
widget that is no longer rendered while the engine's `focused_block` is
untouched. The two focus notions disagree, and the visible one is the one that
routes keystrokes.

The mismatch is exactly why the driver's window-focus barrier fires: engine
focus says "this block", the window says "nobody".

## Missing piece

The keystone's `Search` transition calls `quick_open_search` on the query
engine directly — it never opens or closes the GPUI overlay, so the
open/close focus handoff is not part of any generated sequence. The windowed
invariants that DO watch window focus therefore never see this state.

There is also no step vocabulary for "open search" / "close search", so the
flow cannot be recorded as a `.feature` either.

## Remedy

OPEN. `close()` should restore window focus to the editor that held it when
`open()` stole it — capture the focus handle in `open`, restore it in `close`
(and on the Enter-navigates path, hand focus to the navigated target instead).

Red-first before the fix: add an overlay-level open/close transition to the
GPUI windowed PBT and assert that after close, some editor reports window
focus whenever `focused_block` is `Some`.

## Dogfood re-run 2026-09-08

Still reproduces on the `search-fix` lane (port 8710, seeded throwaway vault).
A/B in one session: clicking `block:ss-ascii` then typing gives
`{"keystrokes_sent":1,"keystrokes_handled":1,"dropped":0}`; cmd-K then Escape
then typing gives `dropped all 1 keystroke(s): no focused editor (or bound
action) consumed them` with the content unchanged. The keyboard is dead after
Escape — cmd-K itself cannot be pressed again — until a block is clicked.
`send_key_chord` still reports `Engine focused_block=Some("block:ss-ascii");
editors reporting window focus: []`.

The invariant that names this defect, `inv-window-focus-matches-engine-focus`,
reports `"outcome":"skipped","reason":"no live source for SUT capability
SutDriver, SutLayout — the live snapshot hosts only SutBackend"` against the
live app (1 passed, 0 failed, 33 skipped of 34). The oracle exists; the one
environment where the bug lives is the one it cannot observe.

Screenshot: `lane-logs/dogfood-r2/21-after-escape.png`.
