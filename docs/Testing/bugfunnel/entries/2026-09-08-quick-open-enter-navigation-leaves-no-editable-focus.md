---
id: 2026-09-08-quick-open-enter-navigation-leaves-no-editable-focus
date: 2026-09-08
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Pressing Enter on a quick-open hit navigates to it and then nothing is
  editable, so every keystroke after the jump is dropped until the user clicks.
---

## Bug

Found by the verifier on the `quick-open-focus` lane while checking the claim
that the `close` hand-back covers the Enter path too, then reproduced with a
windowed probe on the same lane.

Sequence: focus a row, cmd-K, type a query, press Enter. The overlay closes and
the main region navigates to the hit — but no editor holds window focus and a
typed character is never consumed (`send_raw_keystroke_until_handled: keystroke
"v" never consumed within 5s`). The user must click a block to type again,
which is the same dead-keyboard symptom as
`2026-09-03-closing-quick-open-never-returns-keyboard-focus`, reached by a
different route.

## Root cause

Not the hand-back. Two mechanisms meet:

1. `navigation.focus` makes the hit the main region's focus root
   (`crates/holon-frontend/src/reactive.rs`, `maybe_mirror_navigation_focus`),
   and a focus root renders through the `page_title` variant —
   `assets/default/types/block_profile.yaml:71-74`, `priority: 2`,
   `render: text(col("content"), h1)`. That priority beats the `editing`
   variant (`priority: -1`, `condition: is_focused`), so the navigated block
   has NO `editable_text` no matter who focuses it.
2. The row that held the caret is the same block, so it unmounts as an editor
   when it becomes the title. The handle `close` restores points at a widget
   that is no longer rendered.

The windowed probe's frame after Enter contains
`text-block:chord-target-content type=text` and the creation slot
`selectable-block:__virtual:chord-target` — and nothing with
`focused=Some(true)`. `engine.focused_block()` is the navigated block, which is
correct for a navigation focus.

`inv-window-focus-matches-engine-focus` therefore SKIPS here, and legitimately:
its own contract says engine focus on a row with no mounted `editable_text`
(sidebar / navigation focus) is not a divergence.

## Missing piece

No transition or test ever navigated out of an open overlay, so nobody
observed that "the overlay closed" and "the user can type" are different
questions on this path. The oracle that would have caught a zombie cannot
speak about an empty window at all.

Underneath sits a product question this lane did not have the standing to
answer: should a quick-open jump seat a caret in the destination? Doing so
means focusing the destination's first editable row — for an empty destination
that is the creation slot (`block:__virtual:<id>`) — which makes
`focused_block` differ from the navigated root that the focus chain,
breadcrumb and `page_title` condition all read. That is a change to what
`focused_block` MEANS, not a local fix in `search_ui.rs`.

## Remedy

OPEN — the product rule is unruled.

Pinned meanwhile by `enter_navigating_out_of_quick_open_leaves_no_zombie_focus`
(`frontends/gpui/tests/quick_open_returns_focus_windowed.rs`): after Enter the
modal is closed, engine focus is the destination, NO editor holds window focus
(a zombie would fail the oracle), and the oracle's skip reason must still name
the missing destination editor. When the rule is decided that rung goes red and
must be tightened to `Ok`.
