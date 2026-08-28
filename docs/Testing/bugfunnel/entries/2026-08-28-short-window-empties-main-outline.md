---
id: 2026-08-28-short-window-empties-main-outline
date: 2026-08-28
gap: ENVIRONMENT
secondary: COVERAGE
status: PARTIAL
summary: >-
  The main panel paints none of its outline rows once its box gets short, so
  raising the Android soft keyboard blanks the whole page — and because gpui
  only arms an input handler while the focused element is painted, typing dies
  with the rows.
---

## Bug

Martin, on-device dogfood (OnePlus DN2103, `space.holon.kbdtest`,
2026-08-28), deterministic. Reported as "as soon as the keyboard appears,
Holon goes to a different page/view/layout where no block is shown".

Before the tap the phone shows the journal day page: a large `2026-08-28`
heading and three bullet rows, no tab chip, no breadcrumb. Tapping a block row
raises the keyboard, and within a second a teal `2026-08-28` tab chip and a
`Journals › 2026-08-28` breadcrumb appear while the heading and **every** block
row stop painting. Only the `Linked references` accordion and the
`view_mode_switcher`'s ☰/• overlay bar survive. Nothing typed has anywhere to
go. Screenshots and logcat: this session's scratchpad `morning/`
(`before-tap.png`, `after-tap-1s.png`, `round3.txt`, `morning.txt`).

Lane `empty-block-rows`.

## Root cause

Not yet fully localized. What the evidence settles:

**The view does not navigate.** The round-3 logcat has NO
`execute_operation: entity=navigation` at the tap (21:43:34); it has only a
breadcrumb `block_with_path` lookup and a tab-strip `focus_roots` read. The
chip and breadcrumb appear because `HolonApp::render` resolves them ONLY on a
change of `UiState::focused_block` (`frontends/gpui/src/lib.rs:880` and
`:917`), and both latches start `None` alongside a `None` focus
(`lib.rs:2384`, `:2386`) — so after a cold start with an open tab in
`navigation_history` the chrome is never resolved until the first focus. The
first tap resolves it, and two bars appear at once. That stale-chrome-on-boot
defect is real on its own and is the trigger here, but it is not the damage.

**The damage is the short box.** Mounting the two bars (~50 logical px) on top
of the soft keyboard's safe-area inset (`safe_area_bottom`, applied as `.pb()`
on the page container at `lib.rs:1524`; ~290 logical px on this device) leaves
the main panel a fraction of its former height — and a short main panel paints
no rows at all. Reproduced end-to-end through the production inset path
(`raising_the_keyboard_must_not_hide_the_block_rows`,
`lane-logs/RED-keyboard-inset-hides-rows.log`):

```
[keyboard-inset] keyboard DOWN: panel_h=732.0 painted={"block:c1","block:c2","block:parent"}
[keyboard-inset] keyboard UP (340px): panel_h=392.0 painted={} lost=["block:c1","block:c2","block:parent"]
```

The keyboard-down frame is the control, so the loss is attributable to raising
the keyboard alone — and the panel still has 392px to draw three 36px rows in.

**The same end state without any tap or keyboard**, purely by opening a short
window (`a_short_window_still_paints_the_outline`, `lane-logs/sweep-boxes.log`):

| window  | main-panel box | outline region | outline rows painted |
|---------|----------------|----------------|----------------------|
| 393x852 | 790.0          | 528.5          | 3                    |
| 393x600 | 538.0          | 359.5          | 1 — the LAST row, at the region's top |
| 393x500 | 438.0          | 292.5          | **0**                |

The outline region is always exactly 67% of the panel (the `Linked references`
accordion claims its `max_height_fraction: 0.33` unconditionally, even though
its `live_query` is empty at `h=0.0`), so the rows are not being squeezed out
by the accordion growing. At 393x500 the region is 292.5px — room for eight
36px rows — and the only thing left inside it is one `tree_item` crushed to
10.5px carrying the virtual page-title row at 4.5px; `block:parent`,
`block:c1` and `block:c2` are not drawn at all.

**`LIST_SCROLL_PAST_END_PX = 280.0`** (`frontends/gpui/src/views/reactive_shell.rs:72`,
applied as `.pb(...)` on the panel's `gpui::list` at `:992`). Proven by
controlled intervention: setting that one constant to `0.0` and changing
nothing else turns BOTH tests green — the keyboard-up frame paints all three
rows at a 392px panel, and 393x500 paints all three too. Restoring `280.0`
makes both red again.

The constant is itself a WORKAROUND, for a different Martin dogfood bug (#3:
gpui's list undercounts `summary().height` at the end, leaving the last block
~90% clipped at maximum scroll). It buys back scroll range by padding the
list's bottom — and `padding.bottom` counts toward the list's scrollable
content. That is harmless while the panel is tall (280px is a modest tail on a
528px region) and destructive once it is short: at a 292px region the padding
alone is 96% of the viewport, so ~192px of real rows plus 280px of padding make
a list that "scrolls" purely because of the padding, and its initial visible
window lands inside the padding instead of on the rows. Every observation
follows: nothing at 292px, only the LAST row at 359px (partly into the
padding), everything at 528px.

The rows are laid out the whole time — the list just is not showing them.
Probe (`HOLON_PROBE_SCROLL_UP=1`, log
`lane-logs/probe-scroll-up.log`): from the empty 393x500 frame, ten upward
wheel events over the panel and one re-settle produce all three rows at their
natural 36px in the SAME 292.5px region —

```
[block-focus-outline] window=393x500 panel_h=438.0 painted={}
[probe/scroll-up] painted after scrolling to the top: {"block:c1", "block:c2", "block:parent"}
[boxes/scrolled] rendered_text  block:parent  y=84.0  h=36.0
[boxes/scrolled] rendered_text  block:c1      y=126.0 h=36.0
[boxes/scrolled] rendered_text  block:c2      y=168.0 h=36.0
```

The laid-out content is only ~192px tall against a 292.5px viewport, so without
the padding the list could not be scrolled at all — and `settle_to_fixed_point`
had already driven the frame to a stable fixed point, so the empty frame is a
settled wrong state, not a transient.

The path from the layout to the list, for whoever fixes it:
`column::push_main_child` → `view_mode_switcher::render_virtualized`
(`frontends/gpui/src/render/builders/column.rs:262`,
`view_mode_switcher.rs:300`) → `ReactiveShell::render`'s `gpui::list`.

This is the fourth appearance of the same family in this file's history —
BugFunnel `2026-07-22` (outline vanished, divider + backlinks header left),
`2026-07-30` (sidebar unscrollable), `2026-08-18-left-sidebar-tail-unreachable-at-scroll-max`
(scroll viewport taller than its box). Each was fixed for its own context; the
panel still has no test that varies the height it is given.

## Why typing dies too, from the same cause

The instrumented IME build shows the gpui input handler taken ~350ms after
EVERY tap (both the visibly-swapping rounds and the visually-intact one), never
returning, with the fork then logging "N IME edit(s) cannot be applied: no
input handler is set" at frame rate; a re-tap does not restore it
(`scratchpad/morning/round5.txt`, `imediag2.txt`).

That is this defect, not a second one. `Window::handle_input`
(`gpui/src/window.rs:4059`) is a PAINT-phase call — `debug_assert_paint()`, then
`if focus_handle.is_focused(self)` push onto `next_frame.input_handlers` — and
the end of each draw re-arms the platform window from whatever that frame
pushed (`window.rs:2406`). A focused row that is not painted pushes nothing, so
the platform window is left with no input handler. It explains all three
observations: the loss is deterministic (the rows always go when the box
shrinks) while the *visible* swap is intermittent (the chrome only pops in on a
focus change the app has not yet resolved); the ~350ms matches the keyboard
inset settling; and a re-tap cannot recover it because the row is still not
painted, so there is nothing to hit-test and nothing to re-arm from.

The intermittency of the SWAP therefore does not need a separate mechanism, and
the bullet-vs-text tap-target hypothesis is not needed to explain any of it —
the reproduction here uses one unambiguous row click and no navigation op fires.
Worth a quick falsification pass on its own merits, but it is not this bug.

## Missing piece

No rung ever runs the app in a SHORT window.

- The keystone (`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`)
  is headless: there is no window, no chrome bar, no safe-area padding and no
  painted box, so the entire failing code path — flex allocation and
  virtualized row measurement — does not execute there. It cannot reproduce
  this and no change short of giving it a window would let it. **ENVIRONMENT.**
- The windowed rungs (`frontends/gpui/tests/*_windowed*.rs`) do paint, but
  every one of them takes the default desktop-sized window. Window height is
  not a generated or varied dimension anywhere in the suite, so the state
  "panel box under 500px" was unreachable. **COVERAGE (secondary).**

The prod/test parity gap that let the DEVICE case escape even a phone-sized
windowed test: nothing outside `HolonApp` could reach its `safe_area_bottom`.
The window's root view is a `gpui_component::Root` wrapping an `AnyView`, so
the handle cannot be downcast to `HolonApp`, and the platform re-read in
`render` is `#[cfg]`-gated to iOS/Android. No test could mount an IME inset at
all. **Closed** by carrying the root view out of the window-creation closure —
the same `OnceLock` slot pattern `RebindHandle` already uses for the search-UI
entity — and exposing `RebindHandle::set_safe_area_bottom`. A window resize is
NOT an adequate substitute: it also moves the viewport, which the real IME
never does, and gpui's test platform does not fire the resize callback anyway,
so the app never re-lays-out.

## Remedy

Open. Landed so far:

- `frontends/gpui/tests/block_focus_keeps_outline_windowed.rs` — two windowed
  tests, both RED for the right reason. Each asserts a control first so an
  empty frame can never be blamed on the fixture: the keyboard-down frame in
  one, a 438px panel box in the other. Red logs:
  `lane-logs/RED-keyboard-inset-hides-rows.log`,
  `lane-logs/RED-short-window-empties-outline.log`. Sweeps:
  `lane-logs/sweep-boxes.log`, `lane-logs/sweep-window-heights.log`. Probe:
  `lane-logs/probe-scroll-up.log`.
- `RebindHandle::set_safe_area_bottom` (`frontends/gpui/src/lib.rs`) — the
  parity seam described under **Missing piece**.

- `LIST_SCROLL_PAST_END_PX` and its `.pb(...)` are **DELETED**
  (`frontends/gpui/src/views/reactive_shell.rs`). Ruled by the team lead
  2026-08-28: fix the cause upstream (make gpui's list measure its last item at
  full height, in the fork — the constant's own comment already named this as
  the true fix) and remove the workaround with it, rather than re-tuning the
  workaround. The two alternatives were rejected on evidence: capping by the
  list's viewport is right in shape but the height is a layout RESULT,
  unavailable where the padding is set, and percentage padding does not help
  because Taffy follows CSS and resolves it against the containing block's
  WIDTH; capping by the WINDOW height is available but wrong for the device,
  since Android does not resize the window when the IME opens, so the cap would
  never bite on the frame that needs it.

With the padding deleted, on the CURRENT (un-patched) gpui:

| test | result |
|---|---|
| `raising_the_keyboard_must_not_hide_the_block_rows` | GREEN — 392px panel, all three rows |
| `a_short_window_still_paints_the_outline` | GREEN — 438px panel, all three rows |
| `sidebar_scroll_reaches_bottom` (dogfood #3 guard) | PASS |
| the other 12 scroll/accordion/virtualization binaries | PASS (41 tests) |

⚠ **`sidebar_scroll_reaches_bottom` does not discriminate.** It passes with the
padding at 280 AND with it deleted, so it is not a guard for dogfood #3 — which
is exactly what the deleted comment predicted ("a headless sub-viewport fixture
can't observe the padded scroll room; the effect only manifests at real prod
geometry. A regression test therefore belongs with the real fix in the gpui
fork"). No in-repo test detects the workaround's removal. **Landing the
deletion before the fork fix would fix this bug and silently re-open dogfood
#3.** The dogfood-#3 regression test has to come with the fork fix.

Still to do:

1. Re-run the triple against the patched fork rev (path/git override of
   gpui-mobile) once the fork lane lands the list-measurement fix.
2. Device gate, per Martin: after tap → keyboard raise, the rows stay, the
   focused block keeps its input handler, and the fork's held IME queue drains
   ("Hias" lands in the vault file).
3. The chrome pop-in is tracked separately and is FIXED —
   [2026-08-28-tab-strip-never-resolves-at-boot](2026-08-28-tab-strip-never-resolves-at-boot.md).
