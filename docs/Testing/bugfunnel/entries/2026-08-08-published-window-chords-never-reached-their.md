---
id: 2026-08-08-published-window-chords-never-reached-their
date: 2026-08-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  12 of the 15 published window chords never reached their window: cmd+K
  `open_search`, cmd+] / cmd+[ `cycle_tab_*` and cmd+1..9 `jump_to_tab_N` each
  failed with `"<action>: the window is gone: window not found"`
source_line: 756
---

## Bug

(task #28 lane, found by the dogfood-explorer pass P1 F2, driving the real
GPUI app over its embedded MCP) **12 of the 15 published window chords never
reached their window: cmd+K `open_search`, cmd+] / cmd+[ `cycle_tab_*` and
cmd+1..9 `jump_to_tab_N` each failed with `"<action>: the window is gone:
window not found"`** on a live, visible window, while `undo` and
`turn_into_page` succeeded on the SAME handle in the same session. Global
`cx.on_action` listeners are invoked from inside
`Window::dispatch_action_on_node` (compiled gpui = the Holon fork
`holon-space/zed@44506e18`, `window.rs:4729`, bubble-phase globals at
`:4802`), while `App::update_window_id` (`app.rs:1561-1602`) has the window
`take()`n out of `App::windows`; the three handler families that called
`cx.update_window(wh, …)` there re-entered an emptied slot. The 3 chords
that worked are the 3 that never re-enter — undo/redo hop through
`cx.spawn`, `turn_into_page` is an element `capture_action` handed a real
`&mut Window`.

## Root cause

task #28 lane, found by the dogfood-explorer pass (P1 F2) driving the real
GPUI app over its embedded MCP: **12 of the 15 published window chords —
cmd+K `open_search`, cmd+] / cmd+[ `cycle_tab_next`/`prev`, and cmd+1..9
`jump_to_tab_N` — never reached their window at all, every press failing
`"<action>: the window is gone: window not found"` on a live, visible,
screenshotted window** while `undo` and `turn_into_page` succeeded on the
SAME handle in the same session. ROOT CAUSE (one class, not one chord): GPUI
invokes global `cx.on_action` listeners from inside
`Window::dispatch_action_on_node`, i.e. while `App::update_window_id` has
the window `take()`n out of `App::windows` (the COMPILED gpui — the Holon
fork `git+https://github.com/holon-space/zed.git?branch=holon` at `44506e18`
— `app.rs:1561-1602`, take at `:1566` and the `"window not found"` context
at `:1602`; `window.rs:4729` `dispatch_action_on_node`, global listeners in
the bubble phase at `:4802`); a handler that then calls
`cx.update_window(wh, …)` re-enters the emptied slot and gets `Err("window
not found")`. The 3 chords that worked are exactly the 3 that never
re-enter: undo/redo hop through `cx.spawn` (the window update runs after
dispatch returns) and `turn_into_page` is an element-level `capture_action`
handed a real `&mut Window`. COVERAGE, **re-triaged from the dogfood pass's
proposed ENVIRONMENT**: the windowed TestPlatform rung reproduces it on the
first try with no harness change (`lane-logs/task28-red-1.log`), so nothing
about the environment hid it — no test had ever pressed these chords.
`tab_strip`'s six unit tests cover `cycle_target`/`jump_target` as pure
functions and stayed green throughout; the chord→handler→window wiring above
them had zero coverage, and `open_search` had none at any rung. FIXED
in-lane: the 11 tab chords call `apply_cycle`/`apply_jump` on the
dispatch-time `&mut App` (the `window` binding in the removed hop was
already `_window` — the hop was pure ceremony), and `open_search`, which
genuinely needs a `Window` to focus its input, wraps the hop in `cx.defer` —
GPUI's documented "run at the end of the current effect cycle, allowing
entities that are currently on the stack to be returned to the app". Covered
red-first by `frontends/gpui/tests/window_chord_reentrant_dispatch.rs`, one
assertion per handler family plus the modal-actually-open effect check, each
mutation-proven. Evidence:
`docs/Testing/fixture-logs-2026-08-08/task28-window-chord-reentrant-dispatch.txt`.
Residuals, disclosed: (i) real-keypress equivalence rests on the failure
being downstream of keystroke delivery (the rung drives GPUI's real dispatch
tree, not the MCP tool layer), NOT on an observed hardware press; (ii) the
cycle/jump presses run against a strip with <2 tabs, so they assert "the
handler reached its window", not the `navigation.activate` fan-out; (iii)
`cycle_tab_*` / `jump_to_tab_*` now settle `Succeeded` unconditionally and
have NO failure channel — `dispatch_intent` is fire-and-forget returning
`()`, so `Succeeded` attests that the handler ran, not that the tab changed,
and a downstream `navigation.activate` failure would still journal
`Succeeded`; (iv) if the window dies between dispatch and the effect flush,
`open_search` now stays `Pending` forever where it used to settle `Failed` —
a degrade to "unknown", never to false success (`key_chord_report`'s
`an_unsettled_outcome_is_reported_as_pending_never_as_success` covers the
reply side).)

## Missing piece

No test had ever pressed these chords at any rung. `tab_strip`'s six unit
tests exercise `cycle_target`/`jump_target` as pure functions and stayed
green; the chord→handler→window wiring above them, and `open_search`
entirely, had zero coverage. Missing piece = a windowed rung that presses
each window-registry chord and asserts the journal outcome SUCCEEDED (not
merely non-pending) plus the chord's visible effect. **The dogfood pass's
proposed ENVIRONMENT is refuted**: the TestPlatform rung reproduces the
exact string on the first try with no harness change.

## Remedy

**FIXED in-lane 2026-08-08 (task #28).** The 11 tab chords now run
`apply_cycle`/`apply_jump` on the dispatch-time `&mut App` — the removed
hop's window binding was already `_window`, so it was pure ceremony;
`open_search`, which needs a `Window` to focus its input, wraps the hop in
`cx.defer` (GPUI's documented escape: run at the end of the effect cycle,
once entities on the stack are back). Red-first in
`frontends/gpui/tests/window_chord_reentrant_dispatch.rs` with one assertion
per handler family, each mutation-proven, plus a modal-actually-open check
so a handler reporting success while doing nothing still reds. Evidence
`docs/Testing/fixture-logs-2026-08-08/task28-window-chord-reentrant-dispatch.txt`.
Residuals, disclosed: (i) real-keypress equivalence rests on the failure
being downstream of keystroke delivery, not on an observed hardware press;
(ii) the cycle/jump presses run against a strip with <2 tabs, so the
`navigation.activate` fan-out is unasserted; (iii) `cycle_tab_*` /
`jump_to_tab_*` settle `Succeeded` unconditionally with NO failure channel —
`dispatch_intent` is fire-and-forget returning `()`, so the entry attests
that the handler ran, not that the tab changed, and a downstream
`navigation.activate` failure would still read `Succeeded`; (iv) a window
that dies between dispatch and the effect flush leaves `open_search`
`Pending` forever where it used to settle `Failed` — degraded to "unknown",
never to false success (reply side covered by
`key_chord_report::an_unsettled_outcome_is_reported_as_pending_never_as_success`).
