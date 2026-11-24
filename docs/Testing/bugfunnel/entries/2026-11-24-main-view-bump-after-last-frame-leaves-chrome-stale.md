---
id: 2026-11-24-main-view-bump-after-last-frame-leaves-chrome-stale
date: 2026-11-24
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A main-region navigation whose Main view bump lands after the last frame its own commit caused leaves the breadcrumb and the highlighted tab on the departed page until something else redraws the window.
---

## Bug
- Found by a fresh-context verifier of the change that moved the Main view bump from dispatch time to commit time, with a delay seam before the bump (no automated test was red).
- Seen: after Back (and by the same code shape every other main-region navigation), the breadcrumb shows the departed page and the tab strip highlights the departed tab. No later frame comes, so the stale state stays.

## Root cause
- The bump of `main_view_generation` runs in the dispatch task after the op commits (`crates/holon-frontend/src/reactive.rs`, `main_view_bump_on_commit`), off the GPUI thread.
- The breadcrumb (`frontends/gpui/src/lib.rs`, breadcrumb key) and the tab strip (tab strip key) read the generation only when the root view renders. When the commit's own frame is drawn before the dispatch task resumes, that frame reads the old generation, and nothing redraws after the bump.
- The only bump subscriber, the Main view pump, redrew only when an overlay drawer was open.

## Missing piece
- The windowed harness settles with forced frames, so a late bump was always followed by a frame in the tests; no seam held an op after its commit, so the interleaving "frame first, bump after" never ran in a test.

## Remedy
- `DispatchHold::hold_next_committed` parks a run after it committed and released its admission claim, before the caller sees the outcome.
- `every_main_navigation_moves_the_bar_and_the_tab_strip_by_its_bump_alone` (`frontends/gpui/tests/overlay_sidebar_dismisses_windowed.rs`) runs every `NavigationOp` with the hold, releases it after the commit's frames, and asserts both readers without a forced frame. It is red without the fix, naming the stale reader.
- The Main view pump (`spawn_main_view_pump`) now refreshes the window on every bump, so every render-time reader of the generation sees it.
