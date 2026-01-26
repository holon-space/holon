
# WaterUI Frontend — Handoff for Remaining Work

## Current State (2026-03-05)

The waterui frontend compiles cleanly (`cargo check -p holon-waterui` — zero errors, zero warnings). It has the full Holon architecture wired up:

- `FrontendSession` startup with `watch_ui()` CDC stream
- Reactive `Binding<WidgetSpec>` + `watch()` for live re-renders on CDC updates
- Render interpreter that maps `RenderExpr` → waterui `AnyView`
- Screen layout with sidebar collapsing (left/right sidebars from `collapse_to: "drawer"`)
- Operations module with `dispatch_operation`, selectable action parsing
- 19 builder functions + stub passthrough for 8 more

### Files

| File | Purpose |
|------|---------|
| `src/lib.rs` | App entry: tokio runtime, FrontendSession, watch_ui, reactive binding, `watch()` root view |
| `src/state.rs` | `CdcState` — CDC-side widget spec accumulator, sends snapshots via notify callback |
| `src/cdc.rs` | `ui_event_listener` handling `UiEvent::Structure` and `UiEvent::Data` |
| `src/operations.rs` | `dispatch_operation`, `dispatch_undo/redo`, `find_set_field_op`, `get_entity_name`, `get_row_id` |
| `src/render/context.rs` | `RenderContext` with data rows, operations, session, depth tracking |
| `src/render/interpreter.rs` | Recursive `interpret(RenderExpr) -> AnyView`, arg resolution, binary ops |
| `src/render/builders.rs` | All builder functions in a single file |

## Reactivity Architecture

CDC updates flow through a cross-thread bridge:

1. **Tokio side**: `CdcState` receives `UiEvent`s, maintains local `WidgetSpec`, calls `notify` callback on every change
2. **Bridge**: `BindingMailbox::handle()` (sync `try_send` via `async_channel::Sender`) queues a job for waterui's local executor
3. **UI side**: waterui's `LocalExecutor` processes the job, updating `Binding<WidgetSpec>`
4. **Re-render**: `watch(binding, |ws| render_widget_spec(ws))` fires when binding changes

## Reference Implementation

**Blinc** (`frontends/blinc/`) is the reference implementation. It uses the same Holon architecture but renders to blinc's `Div` type. Blinc has each builder in a separate file under `src/render/builders/`. The waterui frontend puts them all in one file for now.

## Remaining Work — Ordered by Impact

### 1. ~~Editable Text Operation Dispatch~~ (DONE)

`build_editable_text` now dispatches `set_field` on every keystroke via `Binding::mapping` setter side-effect, with last-dispatched de-duplication. WaterUI TextField has no on_blur/on_submit — the mapping setter is the only hook point.

**Future improvement**: Add debounce/throttle using nami's `Throttle` to reduce dispatch frequency.

### 2. Icon Support (LOW — currently text placeholder)

Blinc uses a build script (`build.rs`) that embeds SVG icons as data URIs at compile time, then renders them with `img(data_uri)`.

**What to do**: Either port the build script and use waterui's image/SVG support, or use waterui's icon packs (see `~/.cargo/git/checkouts/waterui-*/*/icon-packs/`).

### 3. Theming (LOW — hardcoded colors)

All colors are hardcoded hex strings. Blinc uses `blinc_theme::ThemeState` with semantic `ColorToken`s.

**What to do**: Use waterui's `theme` module (`waterui::theme`, `ColorScheme`, `Theme`). Check if waterui has semantic color tokens. If not, define a small color palette struct and thread it through `RenderContext`.

### 4. MCP Server (LOW — not started)

Blinc embeds an MCP HTTP server so external tools can query the running instance.

**What blinc does** (`main.rs:48-69`): Spawns `holon_mcp::di::run_http_server()` on port 8520.

**What to do**: Add `holon-mcp = { path = "../mcp" }` to Cargo.toml, spawn the server in `app()` after session creation. Need `tokio-util` for `CancellationToken`.

### 5. Remaining Builder Stubs (LOW — 8 builders)

These builders currently pass through to a generic "render template or show placeholder" stub:
`badge`, `block_operations`, `pie_menu`, `state_toggle`, `focusable`, `drop_zone`, `query_result`, `draggable`

**Implemented**: `checkbox` (read-only display), `block` (depth-based indentation), `outline` (hierarchical tree with parent-child grouping), `source_block` (language badge + monospace source display + execute button), `source_editor` (delegates to source_block, read-only).

Priority order:
1. `focusable` — Focus tracking (needs reactive state)
2. The rest — drag/drop, pie menu, etc. are advanced interaction features

### 6. Sidebar Toggle (LOW — sidebars always open)

Screen layout renders sidebars at fixed 280px width. Blinc uses `State<bool>` per sidebar for open/close toggle. Need to add `Binding<bool>` for each sidebar and wire toggle buttons.

### 7. CLI Arguments (LOW — env vars only)

Blinc accepts `--orgmode-root`, `--loro`, `--help` CLI args. WaterUI only reads env vars since it's a library loaded via FFI (no `main()`). This may not be needed — env vars work fine for FFI libs.

## WaterUI API Gotchas

1. **`waterui::prelude::*` shadows `Vec::get()`**: The prelude re-exports something that conflicts. Use `<[T]>::get(&vec, index)` for positional access on `Vec`.

2. **View modifiers return new types**: `text("x").bold()` returns `Bold<Text>`, not `Text`. You can't reassign `let mut t = text(...); t = t.bold()`. Either wrap each branch in `AnyView::new()`, or compose the full modifier chain in one expression.

3. **`AnyView::new()` requires `'static`**: Any `&str` borrowed from `RenderContext` or `ResolvedArgs` must be `.to_string()`'d before passing to `AnyView::new(text(...))`.

4. **`vstack`/`hstack` accept tuples or `Vec<AnyView>`**: For dynamic lists use `Vec<AnyView>`. For fixed layouts use tuples: `vstack((view1, view2))`.

5. **`waterui_ffi::export!()`** is required at crate root for the FFI bridge. Don't remove it.

6. **`Color` is not `Copy`**: Use `Color::srgb_hex(...)` inline each time instead of binding to a variable and reusing.

7. **`Str::from(&str)` requires `'static`**: Use `Str::from(String)` for dynamic strings (takes ownership).

8. **`Binding` is `!Send`**: Use `BindingMailbox` (via `binding.mailbox()`) for cross-thread updates. `BindingMailbox::handle()` is sync and `Send`.

9. **`binding()` function**: Not in the prelude — import from `waterui::reactive::binding`.

## Build & Run

### Version Alignment (CRITICAL)

The `Cargo.toml` pulls `waterui` and `waterui-ffi` from the **`dev` branch** of the waterui git repo. The `water` CLI (release v0.1.3) scaffolds the Xcode project with the **release** `apple-backend 0.2.0` Swift package. These two are **incompatible** — the release apple-backend's C header declares FFI symbols (`waterui_color_id`, `waterui_force_as_photo`, etc.) that don't exist in the dev-branch Rust crate.

After the CLI scaffolds `.water/apple/`, you must patch the Xcode project before building:

**1. Switch apple-backend to dev branch** — in `.water/apple/WaterUIApp.xcodeproj/project.pbxproj`, change:
```
requirement = {
    kind = upToNextMajorVersion;
    minimumVersion = 0.2.0;
};
```
to:
```
requirement = {
    kind = branch;
    branch = dev;
};
```

**2. Add framework linker flags** — `hyper_util` (via `system_configuration` crate) needs macOS frameworks. Change both `OTHER_LDFLAGS` entries from:
```
OTHER_LDFLAGS = "-lwaterui_app -lc++";
```
to:
```
OTHER_LDFLAGS = "-lwaterui_app -lc++ -framework SystemConfiguration -framework Security -framework CoreFoundation";
```

**3. Delete stale Package.resolved** (if it exists):
```sh
rm -f .water/apple/WaterUIApp.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved
```

### Building

```sh
cargo check -p holon-waterui   # Rust compilation check

# Scaffold (only needed once, or after deleting .water/):
cd frontends/waterui && water run --platform macos
# This will fail on link — apply the patches above, then:

# Build directly:
cd .water/apple && xcodebuild -project WaterUIApp.xcodeproj -scheme WaterUIApp \
  -configuration Debug -sdk macosx \
  -derivedDataPath .water/DerivedData \
  ARCHS=arm64 ONLY_ACTIVE_ARCH=YES build \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY=-
```

### Running

The app is an FFI library loaded by the Swift host — env vars must be passed at launch time:
```sh
HOLON_DB_PATH=/tmp/holon-water.db \
HOLON_ORGMODE_ROOT=/path/to/orgfiles/ \
.water/apple/.water/DerivedData/Build/Products/Debug/WaterUIApp.app/Contents/MacOS/WaterUIApp
```

**Note**: `open WaterUIApp.app` won't pass env vars to the process. Use the direct binary path above.

### When this gets fixed

Once the `water` CLI dev branch compiles (currently has 2 compile errors in `cli/src/toolchain/doctor.rs`), installing it from source (`cargo install --git ... --branch dev waterui-cli`) will make `water run` work directly — the dev CLI supports `waterui_path` in `Water.toml` and scaffolds with the matching apple-backend.
