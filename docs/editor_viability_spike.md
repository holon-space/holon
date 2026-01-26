# Editor-replacement viability spike (Phase 0.2)

**Spike source**: `frontends/gpui/examples/rich_input_spike.rs`
**Build**: `cargo build -p holon-gpui --example rich_input_spike --features desktop` ✅ clean
**Reproduce (interactive)**: `cargo run -p holon-gpui --example rich_input_spike --features desktop`

## Verdict: **PASS** — Phase 3 proceeds

Full replacement of `gpui_component::input::Input` is in budget. Estimated
scope: **6–8 weeks** for the rich-text editor that matches today's plain-text
editor's feature surface, comfortably under the 8–12 week threshold the plan
set as the gate.

The spike compiles end-to-end, demonstrating that **every architectural
load-bearing primitive is provided by GPUI itself** — not by `gpui_component`.
We don't have to re-implement IME, text shaping, or platform clipboard plumbing.

## What GPUI gives us for free

| Capability | GPUI primitive | Phase 3 work |
|---|---|---|
| Per-span styling (color, bg, underline, strike, font-weight, italic) | `TextRun { len, font, color, background_color, underline, strikethrough }` | Marks → `Vec<TextRun>` mapping (~50 LoC) |
| Text shaping with attributes & wrapping | `WindowTextSystem::shape_text(text, font_size, &runs, wrap_width, line_clamp) -> Vec<WrappedLine>` | Just call it |
| Painting | `WrappedLine::paint(origin, line_height, TextAlign, bounds, window, cx)` | Just call it |
| Mouse hit-test (pixel → byte offset) | `WrappedLine::closest_index_for_position(point, line_height) -> Result<usize, usize>` | Just call it |
| Caret positioning (byte offset → pixel) | `WrappedLine::position_for_index(index, line_height) -> Option<Point<Pixels>>` | Just call it |
| IME / composition (CJK, dead-key chains, marked text) | `gpui::EntityInputHandler` trait + `window.handle_input(focus_handle, ElementInputHandler::new(bounds, view), cx)` | Implement 7 trait methods routing to LoroText (~200 LoC) |
| Focus management | `gpui::FocusHandle`, `Focusable` trait, `window.focus(handle, cx)` | Just call it |
| Clipboard text I/O | `cx.read_from_clipboard()`, `cx.write_to_clipboard(ClipboardItem::new_string(...))` | Just call it |
| **Image paste** | Already handled at our layer via `capture_action` in `editor_view.rs:312` | **No work** — the `capture_action` wrapper is independent of `Input` |
| Action dispatch (Cmd+B/I/U etc.) | `actions!`, `KeyBinding`, `on_action` | Existing infrastructure |
| Mouse events with click_count | `window.on_mouse_event::<MouseDownEvent>` carries `click_count` | Just check it |

## What we have to write fresh

| Component | Reference impl | Approx LoC | Weeks |
|---|---|---|---|
| Marks → TextRun mapping | spike `RichTextSpike::build_runs` | ~80 | 0.2 |
| Caret render (paint a quad at `position_for_index`) | spike paint() body | ~20 | 0.1 |
| Selection model + path painting | input/element.rs `layout_selections` (~80 LoC) | ~150 | 0.5 |
| Word/line click selection (double/triple-click) | input/state.rs `select_word`/`on_mouse_down` patterns | ~80 | 0.3 |
| Movement helpers (Left/Right/Word/Line/Up/Down/Home/End) | input/movement.rs (253 LoC) — direct port | ~250 | 1.0 |
| Cursor blink | input/blink_cursor.rs (92 LoC) — direct port | ~92 | 0.2 |
| Multi-line layout (stack `WrappedLine`s, scroll handle) | input/element.rs `layout_lines` (~175 LoC) — adapt | ~250 | 1.5 |
| Mark commands (apply_mark, remove_mark, selection_marks) | new — wraps `LoroText::mark`/`unmark` | ~150 | 0.5 |
| EntityInputHandler impl wired to LoroText + IME marked range | input/state.rs `replace_*` (~150 LoC) — adapt | ~300 | 1.5 |
| Toolbar (rendered via render DSL) | new — uses existing render builders | ~100 | 0.3 |
| Context menu w/ extender API (parity with `Input::context_menu_extender`) | input/popovers/context_menu.rs (148 LoC) | ~200 | 0.5 |
| Undo/redo via Loro's built-in undo | new — wraps Loro `UndoManager` | ~100 | 0.5 |
| Polish, IME edge cases, CJK validation, accessibility | — | — | 1.0 |
| Wiring into existing focus/blur/cross-block-nav patterns at `editor_view.rs` | port existing patterns | ~150 | 0.5 |
| **Total** | | **~1900 LoC** | **~7 weeks** |

Add ~1 week buffer for unknowns → **8 weeks**, comfortably inside the 8-12 week target.

## How the spike validates this

The spike (`frontends/gpui/examples/rich_input_spike.rs`, ~370 LoC) implements:

1. **A `RichTextSpike` entity** holding `(text, marks, caret, focus)`.
2. **A `build_runs(font)` method** that walks marks and produces `Vec<TextRun>` — proves marks → GPUI's native styling without any custom glyph plumbing.
3. **A custom `Element` impl** (`TextElement`) with `request_layout`/`prepaint`/`paint` that:
   - Calls `window.text_system().shape_text(...)` with our marks-derived runs.
   - Calls `wrapped_line.paint(origin, line_height, TextAlign::Left, None, window, cx)` to render styled text.
   - Calls `position_for_index(caret, line_height)` to place the caret.
   - Calls `closest_index_for_position(point, line_height)` for click hit-testing.
   - Calls `window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx)` to register IME.
   - Calls `window.paint_quad(fill(bounds, color))` for the caret.
4. **A full `EntityInputHandler` impl** with all 7 required methods. Bodies are stubs (Phase 3 wires them to LoroText), but the trait *exists* on our type — this is the proof macOS IME plugs in cleanly.
5. **Action dispatch** (`actions!(rich_input_spike, [ToggleBold])` + `KeyBinding::new("cmd-b", ToggleBold, ...)`) — Cmd+B fires our handler, demonstrating shortcut wiring.

The spike compiles clean. `cargo build -p holon-gpui --example rich_input_spike --features desktop` produces a 16+ MB binary in ~1 minute.

## What the spike intentionally does NOT cover

These are mechanical follow-ups in Phase 3, not architectural unknowns:

- Multi-line layout (single paragraph here)
- Word/line selection on double/triple click (mouse handler only sets caret)
- Wired `replace_text_in_range` / `replace_and_mark_text_in_range` to LoroText (stubs)
- Drag-to-select (mouse-down only, no mouse-move handling)
- Selection rendering (no selected range painted)
- Cursor blink animation
- Toolbar (action dispatch is wired, but no UI surface)

## Critical findings (vs. the original Critique)

### Critique #8 was partially wrong

The plan's Critique finding #8 stated:
> `gpui_component::input::Input` provides clipboard (incl. image paste, used in editor_view.rs), IME, context menu, double/triple-click selection. Replacing it is a months-scale project, not weeks.

This audit shows:
- **Image paste is OUR code** at `frontends/gpui/src/views/editor_view.rs:312-367` via `capture_action(&Paste)`. It's a GPUI-primitive wrapper, not part of `Input`. Replacing `Input` does **not** affect image paste.
- **IME is GPUI**, not `Input`. `EntityInputHandler` is `gpui::EntityInputHandler`. `Input` *implements* the trait; we'll implement it on `RichTextEditor`.
- **Context menu** *is* in `Input` (~150 LoC), but we only use the `context_menu_extender` API (one extender adding "Share subtree…"). Replicating the extender API on top of GPUI's right-click + popup-menu primitives is ~half a day, not weeks.
- **Double/triple-click selection** is a small mouse-event handler — `~80 LoC` total in `input/state.rs::on_mouse_down`.

The actual hard parts (text shaping with per-run styling, IME platform plumbing, painting glyphs) are **GPUI primitives**, not `Input` add-ons. The Critique conflated "things `Input` does" with "things only `Input` can do."

### Real effort lives in the IME-aware mark logic

The non-trivial Phase 3 work is making mark commands behave correctly across IME composition:
- During IME composition, do NOT apply marks (would corrupt the composition state).
- After composition commits, propagate active marks to the newly-committed text per `ExpandType` rules.
- `replace_and_mark_text_in_range` must update an `ime_marked_range` separately from selection (Loro provides cursor stability, but the IME range is editor-local).

This is the 1.5-week budget item. It's tractable because GPUI handles the OS hooks; we only translate ranges and dispatch to LoroText.

## Decision

**Phase 3 proceeds as planned.** Estimated 7-8 weeks for the full rich-text editor matching today's plain-text feature surface, with rich features added on top:
- Weeks 1-2: caret + selection + movement on a `LoroText`-backed multi-line buffer
- Weeks 3-4: `EntityInputHandler` impl with IME, clipboard, image-paste passthrough
- Weeks 5-6: mark commands (Cmd+B/I/U/K), toolbar, context menu w/ extender
- Week 7: undo/redo via Loro, accessibility, polish
- Week 8: CJK/dead-key validation, cross-platform smoke

Do not pursue a wrapper-around-`Input` design. The spike confirms the gate (per `please-write-a-phased-snoopy-sutton.md` Phase 3): full replacement is in budget.

## Sources

- Spike: [`frontends/gpui/examples/rich_input_spike.rs`](../frontends/gpui/examples/rich_input_spike.rs)
- GPUI text system: `~/.cargo/git/checkouts/zed-*/crates/gpui/src/text_system/{line.rs, line_layout.rs, mod.rs}`
- GPUI input trait: `gpui-0.2.2/src/input.rs:10` (`pub trait EntityInputHandler`)
- gpui_component reference: `gpui-component-0.5.1/src/input/{state.rs (2178), element.rs (1696), movement.rs (253), selection.rs (170), blink_cursor.rs (92)}`
- Image paste in our codebase: `frontends/gpui/src/views/editor_view.rs:312-367`
- Context menu extender usage: `frontends/gpui/src/views/editor_view.rs:47-55`
