# Ply Frontend Handoff

## What was done

Added `frontends/ply/` — a new Holon frontend using [ply-engine](https://github.com/TheRedDeveloper/ply-engine), a cross-platform immediate-mode UI framework built on macroquad.

**Status**: Compiles, runs, and renders real data from org files. Dark themed window with title bar, text measurement working via Arial system font. Layout is functional but rough (columns overlap — needs layout tuning).

## Architecture

Ply is immediate-mode (no retained widget tree like GPUI's `Div`). The key adaptation:

- **`PlyWidget = Box<dyn Fn(&mut ply_engine::Ui<'_, ()>) + Send + Sync>`** — widgets are closures that build ply elements when invoked
- Builders return `PlyWidget` closures instead of framework-native widget values
- The interpreter in `render/interpreter.rs` walks `RenderExpr` and dispatches to builders (same as Blinc's manual match pattern, NOT using `RenderInterpreter<W>`)
- `main.rs` uses `#[macroquad::main("Holon")]` for the render loop + a separate tokio runtime on a background thread for holon's async backend
- Non-blocking init: ply engine starts first (window appears immediately with "Loading..."), holon backend initializes on background thread

## Key implementation details

- **Text measurement**: Uses `Ply::new(&DEFAULT_FONT).await` with system Arial font. Cross-platform needs an embedded font.
- **Frame presentation**: Requires `macroquad::prelude::next_frame().await` after `ply.show()` — ply-engine doesn't call this internally.
- **WatchHandle kept alive**: The `_watch_cmd_tx` sender is stored in `HolonState` to prevent the UiWatcher from shutting down.
- **MCP server**: Port 8521 (different from GPUI's 8520 to avoid conflicts).
- **live_query builder**: Fully wired — inlined from Blinc's pattern (compile query, extract context, `query_and_watch`, recurse via interpreter).

## Files

```
frontends/ply/
├── Cargo.toml                          # ply-engine (git), macroquad-ply, workspace crates
├── HANDOFF.md                          # This file
├── src/
│   ├── main.rs                         # Entry point: macroquad loop + tokio runtime + MCP server
│   ├── state.rs                        # AppState (identical to GPUI)
│   ├── cdc.rs                          # CDC event handler (identical to GPUI)
│   └── render/
│       ├── mod.rs                      # PlyWidget type alias + empty_widget()
│       ├── interpreter.rs              # RenderExpr → PlyWidget dispatch
│       └── builders/
│           ├── mod.rs                  # Manual name→builder dispatch (27 builders)
│           ├── prelude.rs              # Common imports + fixed() helper
│           ├── operation_helpers.rs    # Shared operation helpers
│           ├── live_query.rs           # WIRED — query execution + recursive render
│           └── (27 builder files total, matching GPUI's set)
```

## What still needs work

### 1. ~~Embed a cross-platform font~~ DONE
Embedded Inter-Regular.ttf (411KB) via `FontAsset::Bytes { data: include_bytes!("../../../assets/fonts/Inter-Regular.ttf") }`. Font lives in shared `assets/fonts/` for reuse across frontends.

### 2. ~~Layout tuning~~ DONE
Fixed 3-column overlap: sidebars get `fixed(SIDEBAR_WIDTH)` + `TopToBottom` layout, main panel children each get `grow!()` width wrapped in `TopToBottom` elements. Single main widget skips the extra LeftToRight wrapper.

### 3. ~~Click/interactivity~~ DONE
Wired `clickable`, `state_toggle`, and `block_operations` builders using ply's `ui.element().on_press()` callback API. Each clones `Arc<Session>` and `runtime_handle` into the `on_press` closure for `dispatch_operation()`. `pie_menu` still renders child-only (no radial menu in ply).

### 4. Text editing (LOW)
`editable_text` renders as plain text. Ply has `text_input()` on `ElementBuilder` for full editing with cursor/selection/undo. Wiring it requires maintaining text state across frames.

## Ply API quick reference

```rust
// Layout directions
ply_engine::layout::LayoutDirection::TopToBottom
ply_engine::layout::LayoutDirection::LeftToRight

// Sizing
ply_engine::grow!()                          // flexible grow
ply_engine::layout::Sizing::Fixed(200.0)     // fixed px

// Padding
ply_engine::layout::Padding::new(left, right, top, bottom)  // u16 values
padding(16u16)                                               // all sides

// Text
ui.text("hello", |t| t.font_size(14).color(0xCCCCCC));     // font_size takes u16, NOT f32

// Alignment
ply_engine::align::AlignX::Left / CenterX / Right
ply_engine::align::AlignY::CenterY

// Layout builder
.layout(|l| l.direction(...).gap(8).padding(16u16).align(AlignX::Left, AlignY::CenterY))

// gap() takes u16 (not gaps(), not f32)

// Interactivity (immediate-mode)
ui.element().id("my-button").on_press(|_, _| { ... });
ui.pressed("my-button")  // returns bool
ui.hovered()              // returns bool for current element

// Text input
ui.element().id("my-input").text_input(|t| t.placeholder("...").font_size(14));
ui.get_text_value("my-input")  // read current value

// Frame presentation — REQUIRED after ply.show()
macroquad::prelude::next_frame().await;
```
