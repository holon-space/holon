---
id: 2026-08-18-arrow-keys-do-not-navigate-between-blocks
date: 2026-08-18
gap: COVERAGE
secondary: ORACLE
status: PARTIAL
summary: >-
  Bare Up/Down never moved the caret out of a main-panel block — every editor
  under a live_block routed input through a router that was never given a root
  tree (FIXED), and a first press from a mid-line caret is still spent snapping
  the caret to the line edge inside gpui-component (ESCALATED).
---

## Bug

Martin, dogfooding the GPUI desktop app (2026-08-18): "Navigating via arrow keys
does not work anymore."

Lane `bug-arrows`. Reproduced on `main` (not only on the D5.b stack) with a new
windowed rung, `frontends/gpui/tests/arrow_nav_windowed.rs`: two sibling rows
under the Main focus root, a real click on the second row, then a bare `up`.

```
assertion `left == right` failed: a bare `up` on the first line of the second
row must move focus to the previous sibling
  left: Some(EntityUri("block:undo-blur-sibling-arrownav"))
 right: Some(EntityUri("block:undo-edit-target-arrownav"))
```

The click precondition passes — both the engine's `focused_block` and the
painted `editable_text`'s window focus are on the second row — so the arrow
starts from a genuinely focused editor.

## Root cause — TWO defects, both on the same keystroke

Probed, not inferred: temporary `eprintln!`s at the capture handler, the bubble
handler, `handle_cross_block_nav`, and `InputRouter::bubble_input`.

### Defect A — the first arrow is eaten inside the row

`gpui-component`'s `InputState::up` (`crates/ui/src/input/movement.rs:156`,
pinned rev `8caad846`) only yields the action to the parent when the caret did
not move:

```rust
self.move_vertical(-1, window, cx);
// Zed pattern: if cursor didn't move, we're at the top boundary — propagate
if self.cursor() == cursor_before { cx.propagate(); }
```

`move_vertical` at row 0 clamps the target row to 0 and then sets
`display_point.column = 0`, so the caret moves to the START OF THE LINE. That
IS a cursor move, so nothing propagates and Holon's bubble-phase
`MoveUp`/`MoveDown` handlers (`frontends/gpui/src/views/editor_view.rs:1688`)
never run. Measured:

```
[PROBE] capture MoveUp reached cursor=10 text="beta three" pos=Position { line: 0, character: 10 }
[PROBE] capture MoveUp popup action=Propagate
                       (no bubble handler line — the action died in InputState)
```

Holon's boundary decision is therefore delegated to a "did the cursor move"
heuristic that is wrong for the outliner: `up` on the first line must leave the
block whatever the column, and it must preserve the column for the target row.

### Defect B — even from column 0 the router has no path

With the caret first driven to column 0, the action does propagate and
`handle_cross_block_nav` runs — and then finds nothing to navigate to:

```
[PROBE] capture MoveUp reached cursor=0 text="beta three" pos=Position { line: 0, character: 0 }
[PROBE] bubble MoveUp reached (cross-block nav)
[PROBE] handle_cross_block_nav row=block:undo-blur-sibling-arrownav dir=Up col=0
[PROBE] InputRouter: NO cached focus path for block:undo-blur-sibling-arrownav
[PROBE] bubble_input -> None
```

Dumping the router's own state at that point named the cause outright:

```
[PROBE] InputRouter: NO cached focus path for block:undo-blur-sibling-arrownav;
        has_resolver=false root=
InputRouter: no root set
```

**The editor is holding a router nobody ever wired.**
`live_block::get_or_create_live_block`
(`frontends/gpui/src/render/builders/live_block.rs`) built its `ReactiveShell`
with a freshly constructed `NavigationState::new()` instead of the ambient
`ctx.nav`. `NavigationState` is `Clone` over an `Arc<InputRouter>`, so every
other call site shares the one router the window populates — `set_root` in
`AppModel::rebuild` (`frontends/gpui/src/lib.rs:471`) and `set_block_resolver`
at `frontends/gpui/src/lib.rs:2155`. The live_block site opted out of that
sharing, so every editor beneath a live_block — which is EVERY main-panel block
— routes its input through a router with no root tree and no resolver, and
`bubble_input` can only ever answer `None`.

This is wider than the arrows: every input routed through `bubble_input` from a
main-panel row goes to the same dead router, so the `KeyChord` ops
(Tab / Shift+Tab / Alt+Up / Alt+Down) resolve to nothing by the same mechanism.

`ensure_focus_path` **fails quiet** — when the path cannot be built it simply
leaves the cache empty and returns `()`, so a routing failure is
indistinguishable from "nothing was bound". That silence is why the defect
reached a dogfood session instead of a test.

## Missing piece

The composed keystone HAS an `ArrowNavigate` transition
(`crates/holon-integration-tests/src/pbt/transitions/arrow_navigate.rs`) whose
reference model DOES move focus
(`crates/holon-integration-tests/src/pbt/ref_caps/nav.rs:317`), so the shape was
modelled. It did not catch this because:

- **COVERAGE**: the transition's precondition only requires
  `region_has_focus`; nothing forces the pre-arrow caret to a non-zero column,
  and the SUT driver's `apply_arrow_navigate` reports success on any keystroke
  GPUI marks `handled` — an `up` that merely snapped the caret to column 0 is
  "handled". So Defect A can be sampled and still look fine.
- **ORACLE**: no invariant asserts that a `Navigate` input which the reference
  resolved to a target must resolve to a target in prod too; a `bubble_input`
  returning `None` is silently a no-op on both the SUT side and in
  `ensure_focus_path`.

The rung that reproduces it, and now exists:
`frontends/gpui/tests/arrow_nav_windowed.rs`.

## Remedy

**Defect B is FIXED**: `get_or_create_live_block` passes `ctx.nav.clone()` — the
ambient router — into `ReactiveShell::new_for_block`. One line plus the comment
that says why a per-shell router cannot work.

**Defect A is NOT fixed and is NOT this lane's to fix.** It lives in the
`holon-space/gpui-component` fork (`crates/ui/src/input/movement.rs`), whose pin
`8caad846` has not moved in the last 25 revisions that touched `Cargo.lock` — so
it is long-standing, not the regression Martin hit. The correct fix is in that
fork: `up`/`down` must propagate when the DISPLAY ROW did not change, not when
the byte offset did not change, so a first `up` from a non-zero column on the
first line leaves the block (carrying its column as the `CursorHint`) instead of
being spent snapping to column 0. Working around it in Holon means guessing at
visual rows from logical `Position`s, which breaks for wrapped lines — escalated
rather than hacked.

Until then the shipped behavior is: the first arrow press from mid-line moves
the caret to the line edge, the second leaves the block. The rung's
`bare_arrow_keys_move_focus_between_sibling_blocks` arm pins the desired
behavior and stays RED against Defect A.
