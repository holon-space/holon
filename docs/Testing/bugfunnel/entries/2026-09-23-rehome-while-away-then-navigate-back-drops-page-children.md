---
id: 2026-09-23-rehome-while-away-then-navigate-back-drops-page-children
date: 2026-09-23
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  After Main jumps away from a page, one of the page's leaf children is
  re-homed to Holon storage, and Main navigates back to the page, the main
  panel renders the page with NO children: the remaining children exist in
  storage but never render again.
---

## Bug

`keystone-smoke` on the readauth-2 lane (adb627e9) went red with
`inv-main-panel-rows-match-focus` DROPPED ROW (verifier log
`scratchpad/v4g/smoke1.log`). The draw was Loro+Turso+MCPServer, and it shrank to
a Turso-only wiring with 3 steps: `JumpToSearchHit` (today's journal),
`RehomeEntity(block:c1)`, `NavigateFocus(Main, block:structural-page)`. The
panel renders `block:structural-page` and an `empty` widget. `block:c2` and
`block:parent` are missing. The violation persists for 5 s.

This bug is NOT caused by the lane. The hand-authored replay is red 3/3 on
main ffcb5394 and 3/3 on adb627e9 (Turso wiring), and 1/1 on each tree with the
original Loro+Turso+MCPServer wiring. If you remove either the jump or the
re-home, the case is green on main.

## Root cause

The watcher of a page does not re-render when the page becomes a focus root
again, although its render output depends on that fact.

`BlockDomain::render_entity` renders a block that has no query source as its
whole subtree when a region's cursor rests on it (`is_focus_root`). In all
other cases it renders the block as a leaf
(`crates/holon/src/api/block_domain.rs:167`). The UiWatcher re-runs that
decision only on its triggers: structural CDC of the block and its children,
a variant command, or a profile change
(`crates/holon/src/api/ui_watcher.rs`, `merge_triggers`). Focus-root
membership was not a trigger. The frontend keeps a watcher warm after its
panel leaves (`ReactiveEngine::ensure_watching`,
`crates/holon-frontend/src/reactive.rs:3037`), so the stale decision stays.

Evidence (`lane-logs/diag-uiwatcher.log`, with `holon::api::ui_watcher=info`):
1. `JumpToSearchHit`: Main leaves `structural-page`. The warm watcher stays in
   subtree mode because nothing triggers it.
2. `RehomeEntity(c1)`: c1 leaves the page. This is structural CDC, so the
   watcher re-renders. The page is not a focus root now, so it renders as a
   LEAF: `render_entity(...) OK: gen=2, render="render_entity"`. Its
   first-batch snapshot is `[block:structural-page]`, and `retain_keys`
   evicts `c2` and `parent`.
3. `NavigateFocus` back: Main's cursor rests on the page again. The watcher
   has no trigger for that, so it stays a leaf watch, and the panel renders
   the page with no children. The panel never recovers.

If you remove the jump, the re-render happens while the page is still the
focus root. If you remove the re-home, no re-render happens while Main is
away. In both cases the case is green.

This is a derived-state convergence defect: a derived render does not track
one of its inputs. It violates none of Model.md invariants 1–15, which cover
the write side. The matview, the CDC stream, and the `watch_context`
lifecycle are all correct here. The `CDC Deleted id=block:c2` in the
original smoke run comes from a longer shrink step in which `c2` was the
re-homed block.

## Missing piece

`RehomeEntity` is drawn only on frontend-bearing wirings, and it is rarely
drawn at the common weights (`rehome_entity.rs:84-88`). Thus no gate combined a
re-home of a page's child with a navigate-away-and-back until this smoke run.
No invariant localizes the snapshot or delta divergence below the viewmodel.
The failure shows as DROPPED ROW, and no layer reports it first.

## Remedy

FIXED. Focus-root membership is now a render trigger:
- `watch_ui` subscribes to `FOCUS_ROOT_MEMBERSHIP_SQL`, keyed by its block
  (`BackendEngine::subscribe_sql_keyed`).
- `is_focus_root` reads the same relation, so the trigger fires exactly when
  the decision can change.
- The three shared watch views are created at boot
  (`preload_keyed_watch_views`), so a re-render pays no DDL.
- `full_sync` drops every watch view, the membership view included. It now
  recreates the views a live subscriber listens to
  (`MatviewManager::drop_stale_views`), so the focus triggers of open watchers
  keep firing.

Replay cases: `rehome-then-navigate-drops-main-panel-rows` and `...-loro` go
red on main ffcb5394 and green with the fix;
`rehome-then-full-sync-then-navigate-back` goes red when the recreation is
removed. The pinned OpenTab budget charges both re-renders the fix adds (the
left block as a leaf, the reached block as a root when its watcher is warm)
from the reference; the `open-tab-*` cases pin them exactly.
