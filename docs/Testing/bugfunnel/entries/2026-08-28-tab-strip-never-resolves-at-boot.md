---
id: 2026-08-28-tab-strip-never-resolves-at-boot
date: 2026-08-28
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The open-tabs strip and the breadcrumb re-resolve only on a change of focused
  block, and their latches start empty next to an empty focus — so a restart
  with tabs already open draws no chrome until the user's first click, which
  then reflows the main panel mid-gesture.
---

## Bug

Found while investigating
[2026-08-28-short-window-empties-main-outline](2026-08-28-short-window-empties-main-outline.md)
(Martin, on-device DN2103). It is what made that defect look like a page
switch, and it is a defect in its own right.

Before the tap the phone showed a bare journal day view — no tab chip, no
breadcrumb. Tapping a block made a teal `2026-08-28` chip and a
`Journals › 2026-08-28` breadcrumb appear within a second, which reads as the
app navigating somewhere. It had not navigated: the round-3 logcat has no
`execute_operation: entity=navigation` at the tap, only the breadcrumb's
`block_with_path` lookup and the strip's `focus_roots` read. The tab had been
open the whole time — since before the app restarted.

## Root cause

`HolonApp::render` re-resolves both chrome bars only when the focused block
CHANGES (`frontends/gpui/src/lib.rs`, the `last_breadcrumb_focus` and
`last_tab_strip_focus` blocks). Both latches were `Option<EntityUri>`
initialised to `None`, and `UiState::focused_block()` is also `None` on the
first frame because nothing has been clicked yet. The two compare EQUAL, so
neither bar ever resolves — the strip stays empty however many tabs
`navigation_history` holds — until something unrelated moves the focus.

The latch type conflated two different states: "resolved while nothing was
focused" and "never resolved at all". Only the first should compare equal to a
`None` focus.

The user-visible cost is not just a missing chip. Both bars sit ABOVE the
content in the page container, so resolving them late inserts ~50 logical px of
chrome and reflows the main panel — mid-gesture, on the very frame the soft
keyboard is also taking ~290px away. That combination is what empties the
outline in the sibling entry, and it is why the swap looked intermittent there:
the chrome only pops in on the first focus change after a cold start.

## Missing piece

No rung looked at the window chrome at all, and none could:

- The keystone is headless — `HolonApp::render` and its chrome bars do not
  execute there, so the state is unreachable. **ENVIRONMENT.**
- The windowed rungs paint, but every one of them observes the panels; none
  had ever asserted anything about the tab strip or the breadcrumb, and the
  strip registers no bounds so geometry-based rungs could not see it either.
  **COVERAGE (secondary).**

## Remedy

FIXED.

- `last_breadcrumb_focus` / `last_tab_strip_focus` are now
  `Option<Option<EntityUri>>`, so the outer `None` means "never resolved" and
  the first frame resolves once. The comparison became
  `self.last_x.as_ref() != Some(&focused)`.
- `RebindHandle::drawn_tab_count` exposes the strip's RESOLVED state (the tabs
  it is drawing), which is the thing that was wrong — distinct from the
  `navigation_history` rows behind it.
- `frontends/gpui/tests/tab_strip_resolves_at_boot_windowed.rs` pins it, with a
  vacuity guard reading the open tabs straight from `focus_roots` so an empty
  strip can never be excused as "no tabs were open". Red log:
  `lane-logs/RED-tab-strip-boot.log` (`tabs open in navigation state=1 drawn by
  the strip=0`); green: `lane-logs/GREEN-tab-strip-boot.log` (`=1` / `=1`).

The open product question — **what should the breadcrumb show when nothing is
focused?** — was ruled by Martin (D41.b, 2026-08-30): the bar means the CURRENT
VIEW's path, so with nothing focused it resolves from the Main view root, and a
focused block still wins. That removes the last ~31px pop-in.

- `last_breadcrumb_focus` became `last_breadcrumb_key`, a
  `(Option<EntityUri>, main_view_generation)` pair.
- `UiState::main_view_generation` is a SECOND counter, bumped by every op that
  moves the `main` cursor — `go_back`/`go_forward`/`activate`/`close` included,
  which move the view root without a page change. It is separate from
  `main_nav_generation` because that one drives the main-panel scroll reset, and
  a tab switch must keep the switched-to tab's scroll.
- A caret that did not move while the view did is treated as stale: the
  cursor-moving ops never set the focus, so it still names a row on the page
  just left. The view wins in that case — but ONLY if the resolved view root
  actually changed. `navigation.close` names a row rather than a cursor and
  carries no region, so it bumps the generation even when a BACKGROUND tab
  closes and the cursor does not move; comparing the resolved root is what keeps
  such a bump from stealing the bar off a live caret.
- `QueryEngine::region_view_root` reads the region's open root behind the
  capability, so the frontend stays free of SQL.
- `frontends/gpui/tests/breadcrumb_resolves_from_view_root_windowed.rs` pins the
  cold boot, the navigation move, the focused-block case, Back, a tab switch and
  the cleared-focus case, plus a background-tab close that must NOT move the
  bar. Red logs: `lane-logs/breadcrumb-view-root-RED.log`
  (`bar_block=None segments=Some(0)` against view root `block:journals`) and
  `lane-logs/breadcrumb-back-activate-RED.log` (after Back the view is on
  `block:wslice-graft-page` and the bar still reads `block:c1`) and
  `lane-logs/breadcrumb-bg-close-RED.log` (a background close moves neither the
  root nor the caret, and the bar leaves `block:c1` anyway).

## Residual

The bar resolves once per key change and never retries. A transient failure —
`session.query_engine()` absent, or a `region_view_root` error on the first
frame — latches an error/empty bar that persists until the focus or the view
generation next moves. The cold boot itself is safe (the outer `None` still buys
the FIRST resolution), so the exposure is failure-RETRY, not first-paint. Left
uncoded deliberately: retrying on error means re-querying every frame while the
error persists, which is a worse failure than the one it fixes.

The windowed legs drive each gesture as its own settled frame. A real user can
land a caret click and a Back within one frame, which the legs do not reproduce;
the precedence they pin is the resolver's, not the frame scheduler's.

The tab strip keeps the older, narrower trigger — it re-resolves on focus alone
and keeps its active-tab highlight in sync optimistically — so its tab SET can
still go stale on a cursor move that opens or closes a row. Not this entry's
defect; recorded here because the two bars now differ in how they stay fresh.
