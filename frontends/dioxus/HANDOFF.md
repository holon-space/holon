# Dioxus Desktop Frontend — Handoff for Remaining Work

## Current State (2026-03-05)

The Dioxus Desktop frontend compiles cleanly (`cargo check -p holon-dioxus` — zero errors, zero warnings). It has the full Holon architecture wired up:

- `FrontendSession` startup with `watch_ui()` CDC stream
- Reactive bridge: `tokio::sync::watch` channel → `use_future` → `Signal<WidgetSpec>`
- Render interpreter that maps `RenderExpr` → Dioxus `Element` (HTML/CSS via RSX)
- Screen layout with sidebar collapsing (left/right sidebars from `collapse_to: "drawer"`)
- Operations module with `dispatch_operation`, selectable action parsing
- CSS custom properties for dark theming (14 semantic tokens)
- Keyboard shortcuts: Cmd+Z undo, Cmd+Shift+Z redo
- 22 builder functions (7 stubs for advanced features, 5 stubs for low-priority builders)

### Implemented Builders

| Builder | Status |
|---------|--------|
| `text` | Full — size, bold, color |
| `row` | Full — horizontal flex |
| `col` | Full — alias for vertical flex (list) |
| `list` | Full — vertical flex with item template |
| `columns` | Full — horizontal layout + screen layout with sidebars |
| `section` | Full — title + children |
| `spacer` | Full — fixed height or flex grow |
| `icon` | Stub — renders name as text |
| `live_query` | Full — PRQL/GQL/SQL with context |
| `render_entity` | Full — query language dispatch + source display |
| `block_ref` | Full — recursive block rendering |
| `selectable` | Full — action dispatch on click |
| `editable_text` | Partial — renders `<input>`, dispatches `set_field` on `onchange` |
| `table` | Full — auto-table from columns + item template fallback |
| `tree` | Full — vertical list with item template |
| `checkbox` | Done — `[x]`/`[ ]` with semantic colors |
| `badge` | Done — pill-shaped label |
| `block` | Done — indented container |
| `outline` | Done — hierarchical tree with parent→child grouping + recursive indentation |
| `source_block` / `source_editor` | Done — language badge + `<pre>` code display |
| `state_toggle` | Done — cycle through states on click (TODO→DOING→DONE) |
| `focusable` | Passthrough — renders child, focus state not yet wired |
| `block_operations`, `pie_menu`, `drop_zone`, `query_result`, `draggable` | Stubs — render template children or placeholder |

### Files

| File | Purpose |
|------|---------|
| `src/main.rs` | App entry: tokio runtime, FrontendSession, watch channel bridge, desktop window config, CSS injection, keyboard shortcuts, `App` component |
| `src/state.rs` | `CdcState` — CDC-side widget spec accumulator, sends snapshots via `Send`-safe callback |
| `src/cdc.rs` | `ui_event_listener` handling `UiEvent::Structure` and `UiEvent::Data` |
| `src/operations.rs` | `dispatch_operation`, `find_set_field_op`, `get_entity_name`, `get_row_id` |
| `src/render/mod.rs` | `render_widget_spec()` entry point |
| `src/render/context.rs` | `RenderContext` with data rows, operations, session, depth tracking |
| `src/render/interpreter.rs` | Recursive `interpret(RenderExpr) -> Element`, arg resolution, binary ops |
| `src/render/builders.rs` | All builder functions in a single file |

## Reactivity Architecture

CDC updates flow through a cross-thread bridge:

1. **Tokio side**: `CdcState` receives `UiEvent`s, maintains local `WidgetSpec`, calls `notify` callback on every change
2. **Bridge**: `tokio::sync::watch::Sender` (Send-safe) transmits `WidgetSpec` snapshots
3. **UI side**: `use_future` polls `watch::Receiver`, updates `Signal<WidgetSpec>` on the UI thread
4. **Re-render**: Dioxus reactivity triggers `App` re-render when signal changes

**Why not `Signal.set()` directly?** Dioxus signals use `UnsyncStorage` (are `!Send`). The CDC task runs on a tokio thread. The watch channel bridges the two worlds.

## Theming

Colors use CSS custom properties defined in `BASE_CSS` (injected via `with_custom_head()`). All builders reference `var(--token)` instead of hardcoded hex values.

Tokens: `--bg`, `--bg-sidebar`, `--surface`, `--surface-elevated`, `--border`, `--text-primary`, `--text-secondary`, `--text-muted`, `--accent`, `--success`, `--warning`, `--info`, `--error`.

To add light mode: define a `@media (prefers-color-scheme: light)` block or toggle a `data-theme` attribute on `<html>`.

## Reference Implementations

- **Blinc** (`frontends/blinc/`) — same architecture, renders to blinc's `Div` type. Has the most complete builder set, each in a separate file under `src/render/builders/`. **Best reference for new builders.**
- **WaterUI** (`frontends/waterui/`) — same architecture, renders to waterui's native `AnyView`. Uses `BindingMailbox` for its cross-thread bridge.
- **Flutter** (`frontends/flutter/`) — most evolved frontend. Uses FFI bridge (`flutter_rust_bridge`) + Riverpod state management. Reference for expected UI behavior.

## Remaining Work — Ordered by Impact

### 1. CDC-Driven Sub-Widget Updates (MEDIUM — nested `block_ref` / `live_query` are static)

`build_block_ref_by_id` and `build_prql_query` currently call `render_entity()` / `query_and_watch()` synchronously (via `std::thread::scope`) and discard the returned CDC stream. This means nested queries show initial data but don't update live.

**What to do**: Convert these builders into proper Dioxus components (with their own `use_future` hooks) that subscribe to the returned CDC stream. The component would hold a `Signal<WidgetSpec>` that updates when data changes, just like the root `App` component.

### 2. Sidebar Toggle (LOW — sidebars always open)

Screen layout renders sidebars at fixed 280px width. Need `Signal<bool>` per sidebar for open/close toggle and toggle buttons in the UI. Could use CSS transitions for smooth open/close animation.

### 3. Icon Support (LOW — currently text placeholder)

`build_icon` renders the icon name as text. Since Dioxus Desktop runs in a webview, you can use SVG directly in RSX: `rsx! { svg { dangerous_inner_html: SVG_CONTENT } }`, or embed a web icon font (e.g. Lucide, Material Icons). The webview approach makes this trivial.

### 4. MCP Server (LOW — not started)

Blinc embeds an MCP HTTP server so external tools can query the running instance.

**What blinc does** (`main.rs:48-69`): Spawns `holon_mcp::di::run_http_server()` on port 8520.

**What to do**: Add `holon-mcp = { path = "../mcp" }` to Cargo.toml, spawn the server in `main()` after session creation on the tokio runtime. Need `tokio-util` for `CancellationToken`.

### 5. Remaining Builder Stubs (LOW — 5 builders)

These builders currently pass through to a generic "render template or show placeholder" stub:
`block_operations`, `pie_menu`, `drop_zone`, `query_result`, `draggable`

These are advanced interaction features. Consult `frontends/blinc/src/render/builders/` for reference implementations.

### 6. Light Mode (LOW — dark only)

CSS custom properties are in place. Add `@media (prefers-color-scheme: light)` with light values, or toggle via a `data-theme` attribute on the root element.

## Dioxus API Gotchas

1. **Signals are `!Send`**: Cannot capture `Signal` in closures sent to tokio tasks. Use channels (`tokio::sync::watch`, `mpsc`) to bridge between tokio and the UI thread.

2. **`rsx!` returns `Element` (= `Option<VNode>`)**: Unlike WaterUI's `AnyView::new()`, Dioxus elements don't need wrapping. `rsx! {}` for empty is fine.

3. **CSS via attributes, not methods**: Where WaterUI uses `.bold()`, `.size(14.0)`, Dioxus uses HTML/CSS attributes: `font_weight: "bold"`, `font_size: "14px"`.

4. **Dynamic children use iterators**: `{views.into_iter()}` inside `rsx!` to render a `Vec<Element>`.

5. **`use_hook` runs once**: For one-time initialization (spawning the CDC listener). `use_future` runs an async block and re-runs on dependency changes.

6. **Context via `use_context_provider` / `use_context`**: The session and runtime handle are provided as context in `main()` via `LaunchBuilder::with_context()` and consumed in components via `use_context::<T>()`.

7. **Dioxus Desktop = webview (wry)**: The render output is HTML/CSS. This means full CSS support, browser devtools (right-click → Inspect), and all standard web layout. When migrating to Blitz, the HTML/CSS RSX stays the same but the renderer changes from webview to Stylo (native).

8. **`onchange` vs `oninput`**: `onchange` fires when the input loses focus (like HTML), `oninput` fires on every keystroke. Use `onchange` for commit-on-blur behavior (matches blinc's `on_blur` pattern).

## Build & Run

```sh
cargo check -p holon-dioxus   # Rust compilation check

# Run:
HOLON_DB_PATH=/path/to/db \
HOLON_ORGMODE_ROOT=/path/to/orgfiles/ \
cargo run -p holon-dioxus

# Or with Loro CRDT:
HOLON_DB_PATH=/path/to/db \
HOLON_ORGMODE_ROOT=/path/to/orgfiles/ \
HOLON_LORO_ENABLED=1 \
cargo run -p holon-dioxus
```

## Future: Blitz Migration

When Blitz (native CSS renderer using Mozilla's Stylo) matures, the migration path is:

1. Change `Cargo.toml`: `dioxus = { ..., features = ["blitz"] }` (or equivalent)
2. Change `main.rs` launch: `dioxus_blitz::launch(App)` instead of `LaunchBuilder` with desktop config
3. **No changes** to: state, cdc, operations, context, interpreter, or builder logic
4. **Possible changes** to `builders.rs` if Blitz doesn't support certain CSS properties — but the RSX structure stays the same
5. The `BASE_CSS` custom properties and scrollbar styling may need adjustment for Stylo's CSS subset

The entire component layer (signals, hooks, RSX markup) is shared between Dioxus Desktop and Blitz by design.
