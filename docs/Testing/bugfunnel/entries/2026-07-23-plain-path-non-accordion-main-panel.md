---
id: 2026-07-23-plain-path-non-accordion-main-panel
date: 2026-07-23
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  PLAIN-PATH (non-accordion) main panel outline frozen — Martin dogfooding his
  real vault (persisted user-owned LEGACY layout `column(collection_view(),
  divider(), row(icon,spacer,text), live_query(...))`, no accordion; 56-row
  outline genuinely overflowing). Wheel/trackpad no-op. ROOT CAUSE
  (coordinator live-instance H2, verified windowed): the per-block
  `ReactiveShell` wrapping every `block:default-*` panel (`live_block.rs:48`
  gate → `ReactiveShell::new_for_block`) renders its block-mode NON-EAGER
  final arm as `div().flex_col().size_full()` with no `overflow_y_scroll` and
  no `min_h_0` (`reactive_shell.rs:745`, pre-fix). Production chain:
  `columns.rs` overflow wrapper (viewport H, correct) → this `size_full` shell
  (viewport H, clips) → content-height column (overflows the shell). The shell
  exactly fills the outer scroll viewport, so the outer wrapper sees no
  overflow and can't scroll, and the shell itself has no scroll → the overflow
  is clipped, unreachable. The EAGER arm (`reactive_shell.rs:738`) already
  does `size_full().overflow_y_scroll()`; only this arm lacked it. Every prior
  windowed repro (`main_panel_scroll.rs`, my 4 `plain_path_scroll` rungs)
  mounted the column DIRECTLY under `columns` — no per-block shell — hence
  green, missing the bug.
source_line: 794
---

## Bug

PLAIN-PATH (non-accordion) main panel outline frozen — Martin dogfooding his
real vault (persisted user-owned LEGACY layout `column(collection_view(),
divider(), row(icon,spacer,text), live_query(...))`, no accordion; 56-row
outline genuinely overflowing). Wheel/trackpad no-op. ROOT CAUSE
(coordinator live-instance H2, verified windowed): the per-block
`ReactiveShell` wrapping every `block:default-*` panel (`live_block.rs:48`
gate → `ReactiveShell::new_for_block`) renders its block-mode NON-EAGER
final arm as `div().flex_col().size_full()` with no `overflow_y_scroll` and
no `min_h_0` (`reactive_shell.rs:745`, pre-fix). Production chain:
`columns.rs` overflow wrapper (viewport H, correct) → this `size_full` shell
(viewport H, clips) → content-height column (overflows the shell). The shell
exactly fills the outer scroll viewport, so the outer wrapper sees no
overflow and can't scroll, and the shell itself has no scroll → the overflow
is clipped, unreachable. The EAGER arm (`reactive_shell.rs:738`) already
does `size_full().overflow_y_scroll()`; only this arm lacked it. Every prior
windowed repro (`main_panel_scroll.rs`, my 4 `plain_path_scroll` rungs)
mounted the column DIRECTLY under `columns` — no per-block shell — hence
green, missing the bug.

## Root cause

user-owned LEGACY (non-accordion) main-panel outline frozen — the per-block
`ReactiveShell` block-mode non-eager arm (`reactive_shell.rs:748`) wrapped
the content-height column in a bare `size_full()` with NO
`overflow_y_scroll`, so an overflowing outline (Martin's 56 rows) was
clipped to the panel and the outer `columns.rs` scroll wrapper couldn't
scroll either (shell == viewport → no overflow); wheel no-op. FIXED by
mirroring the eager arm (id + `overflow_y_scroll`, keeping `size_full` for
the definite-height contract); red-first windowed rung
`plain_path_scroll.rs::shell_wrapped_main_panel_scrolls` routes a REAL
`live_block(block:default-main-panel)` shell — every prior windowed repro
mounted the column bare, hence couldn't see it. COVERAGE secondary — the
seed-following oracle moved `main_panel_scroll.rs` to the accordion shape,
silently losing the plain-path rung. `block:default-left-sidebar` shares the
arm — one fix, asserted by `shell_wrapped_sidebar_scrolls`.)

## Missing piece

Litmus (ENV): does the failing path run in the keystone/windowed wiring? NO
— every windowed reproduction omitted the production
`live_block(block:default-*)` shell that sits between the columns wrapper
and the content-height column; the harness composition never built it, so
the clipping arm never ran under test. Secondary COVERAGE: the
seed-following oracle moved `main_panel_scroll.rs` to the accordion shape
when the seed migrated, deleting the last windowed rung over the plain
non-accordion shape (a supported user-authored form forever).

## Remedy

FIXED 2026-07-23 — `reactive_shell.rs` block-mode non-eager arm now mirrors
the eager arm (`id` + `overflow_y_scroll`, keeping `size_full` so
relative-height descendants — `live_query` `relative(1.0)`, the accordion
cap, virtualized `gpui::list` sidebars — keep their definite-height parent;
NOT switched to content-height, which would re-trigger the percentage trap).
Red-first rung `plain_path_scroll.rs::shell_wrapped_main_panel_scrolls`
(real `live_block` shell) red→green (red-run-plainpath.log /
green-run-shell.log); 4 plain rungs + `shell_wrapped_sidebar_scrolls` (same
generic arm — one fix covers `block:default-left-sidebar`) green; accordion
suite + firewall green.
