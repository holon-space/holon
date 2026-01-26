# WaterUI Frontend — Handoff Prompt

Copy everything below this line and paste it as a prompt in a new session.

---

## Context

You are continuing work on the **WaterUI frontend** for the Holon project. WaterUI is a Rust-native cross-platform UI framework (SwiftUI-like API). The frontend renders the same Holon data as the Flutter frontend but using native macOS views via WaterUI's FFI bridge.

**Working directory**: `frontends/waterui/` (within the holon monorepo at `/Users/martin/Workspaces/pkm/holon`)

Read these files first (in this order):
1. `frontends/waterui/HANDOFF.md` — architecture, file map, API gotchas, build instructions
2. `frontends/waterui/src/render/builders.rs` — all builder functions (725 lines, 14 implemented + 13 stubs)
3. `frontends/blinc/src/render/builders/` — reference implementation (one file per builder)

The app builds and runs successfully. The Holon backend initializes (DI, caches, org sync, CDC streams) and the UI renders real data: left sidebar with folder tree, main panel with content list, right sidebar with notes. All rendering is live via CDC — changes to org files update the UI automatically.

## Current state

- **Compiles cleanly**: `cargo check -p holon-waterui` — zero errors, zero warnings
- **Builds and runs**: xcodebuild succeeds after patching the Xcode project (see HANDOFF.md "Build & Run" section)
- **14 builders implemented**: text, row, list, columns, section, spacer, icon, live_query, render_block, block_ref, selectable, editable_text, table, tree
- **13 builders stubbed**: block, outline, checkbox, badge, block_operations, pie_menu, state_toggle, focusable, drop_zone, source_block, source_editor, query_result, draggable
- **Operations module**: `dispatch_operation` works for selectable actions but editable_text doesn't dispatch `set_field` on edit

## Tasks (ordered by impact)

### 1. Editable text save-on-blur (MEDIUM)
`build_editable_text` renders a `TextField` but doesn't persist changes. Blinc uses `on_blur` to dispatch a `set_field` operation. Investigate waterui's TextField lifecycle — does it have focus/blur callbacks? Look at the waterui source in `~/.cargo/git/checkouts/waterui-*/` for TextField API. The `dispatch_operation` helper in `src/operations.rs` already handles `set_field`.

### 2. Checkbox builder (MEDIUM)
Stub `checkbox` → real implementation. Blinc's `checkbox_builder.rs` toggles task state via `dispatch_operation` with a `set_field` op on the `task_state` field. WaterUI likely has a `Toggle` or `Checkbox` view.

### 3. Block / Outline builders (MEDIUM)
Stub `block`/`outline` → real implementation. These render individual blocks with indentation based on depth. Blinc's `block_builder.rs` wraps content in a container with left padding proportional to tree depth.

### 4. Source block display (LOW)
Stub `source_block`/`source_editor` → read-only code display. Blinc uses a monospace-styled text area. Full editing is out of scope — just render the source content with monospace font.

### 5. Theming (LOW)
All colors are hardcoded hex strings. Define a small `Theme` struct with semantic colors (background, text, accent, muted, border) and thread it through `RenderContext`. WaterUI has `waterui::theme` — check if it provides semantic tokens.

### 6. Icon support (LOW)
Currently renders icon name as text placeholder. Blinc embeds SVGs via build script. WaterUI has icon packs in `~/.cargo/git/checkouts/waterui-*/*/icon-packs/` — check what's available and use native icon support.

### 7. Sidebar toggle (LOW)
Sidebars render at fixed 280px with no collapse/expand. Add `Binding<bool>` per sidebar and wire toggle buttons. Blinc uses `State<bool>` for this.

## Key constraints

- **Reference implementation is Blinc** (`frontends/blinc/`). Every builder there has a waterui equivalent to implement. Port logic, not code — the view APIs are completely different.
- **`waterui::prelude::*` shadows `Vec::get()`** — use `<[T]>::get(&vec, index)` for positional access.
- **View modifiers return new types** — can't reassign. Wrap in `AnyView::new()` or chain in one expression.
- **`AnyView::new()` requires `'static`** — `.to_string()` all borrowed strings.
- **`Binding` is `!Send`** — use `BindingMailbox` for cross-thread updates.
- **Don't use `water run`** for building — it re-scaffolds and overwrites patches. Use `xcodebuild` directly (see HANDOFF.md).
- **waterui docs are sparse** — read the source in `~/.cargo/git/checkouts/waterui-*/` and use Context7 MCP to look up waterui API docs.
