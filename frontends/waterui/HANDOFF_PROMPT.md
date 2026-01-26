# WaterUI Frontend — Handoff Prompt

Copy everything below this line and paste it as a prompt in a new session.

---

## Context

You are continuing work on the **WaterUI frontend** for the Holon project. WaterUI is a Rust-native cross-platform UI framework (SwiftUI-like API). The frontend renders the same Holon data as the other frontends but using native macOS views via WaterUI's FFI bridge.

**Working directory**: `frontends/waterui/` (within the holon monorepo at `/Users/martin/Workspaces/pkm/holon`)

Read these files first (in this order):
1. `frontends/waterui/HANDOFF.md` — current state, architecture, API gotchas, build instructions
2. `frontends/waterui/src/render/builders/mod.rs` — `create_interpreter()` wiring via `holon_macros::builder_registry!`, one file per builder alongside it
3. `frontends/blinc/src/render/builders/` — reference implementation (same one-file-per-builder layout)

## Current state

- **Parked experimental frontend, excluded from the root workspace** (wgpu/naga codespan-reporting conflict, see HANDOFF.md). It has its own `[workspace]` table; build with `cd frontends/waterui && cargo check` — `cargo check -p holon-waterui` from the repo root does NOT work.
- `waterui`/`waterui-ffi` are pinned to a rev because upstream `dev` HEAD has an unfetchable submodule pointer.
- Rendering goes through the **shared** `holon_frontend::render_interpreter::RenderInterpreter`; the legacy local interpreter/context/builders single-file pipeline was deleted. Do not resurrect it.
- Operations and CDC handling live in `holon_frontend` (`ReactiveEngine`); there is no local `operations.rs`/`state.rs`/`cdc.rs`.

## Tasks (ordered by impact)

### 1. Theming (LOW)
All colors are hardcoded hex strings. Define a small `Theme` struct with semantic colors (background, text, accent, muted, border) or use `waterui::theme` if it provides semantic tokens.

### 2. Icon support (LOW)
Currently renders icon name as text placeholder. Blinc embeds SVGs via build script. WaterUI has icon packs in `~/.cargo/git/checkouts/waterui-*/*/icon-packs/` — check what's available and use native icon support.

### 3. Sidebar toggle (LOW)
Sidebars render at fixed 280px with no collapse/expand. Add `Binding<bool>` per sidebar and wire toggle buttons. Blinc uses `State<bool>` for this.

### 4. MCP server (LOW)
Blinc spawns `holon_mcp::di::run_http_server()` on port 8520; do the same in `app()` after session creation.

## Key constraints

- **Reference implementation is Blinc** (`frontends/blinc/`). Every builder there has a waterui equivalent to implement. Port logic, not code — the view APIs are completely different.
- **`waterui::prelude::*` shadows `Vec::get()`** — use `<[T]>::get(&vec, index)` for positional access.
- **View modifiers return new types** — can't reassign. Wrap in `AnyView::new()` or chain in one expression.
- **`AnyView::new()` requires `'static`** — `.to_string()` all borrowed strings.
- **`Binding` is `!Send`** — use `BindingMailbox` for cross-thread updates.
- **Don't use `water run`** for building — it re-scaffolds and overwrites patches. Use `xcodebuild` directly (see HANDOFF.md).
- **waterui docs are sparse** — read the source in `~/.cargo/git/checkouts/waterui-*/` and use Context7 MCP to look up waterui API docs.
