# WaterUI Frontend — Handoff for Remaining Work

## Current State (2026-07-02)

Parked experimental frontend. It is **excluded from the root workspace**
(root `Cargo.toml` `exclude` list) because naga 27 (via wgpu) +
codespan-reporting 0.12 breaks workspace feature unification
(https://github.com/gfx-rs/wgpu/issues/7915; fix pending backport in
https://github.com/gfx-rs/wgpu/issues/8366). It therefore declares its own
`[workspace]` table (like `frontends/holon-worker`) so it builds standalone:

```sh
cd frontends/waterui && cargo check   # NOT `cargo check -p holon-waterui` from repo root
```

`waterui`/`waterui-ffi` are pinned to rev `3baf8b39` — upstream `dev` HEAD
points its `backends/android` submodule at a force-pushed-away commit and is
unfetchable. Re-pin to a newer rev once upstream fixes the submodule pointer.

**Known build blocker on Xcode 26+ SDKs**: `waterkit-screen` (mandatory dep
of `waterui-internal`) compiles Swift using `CGWindowListCreateImage`, which
the macOS 26 SDK removed ("use ScreenCaptureKit instead") — the same issue
for which `frontends/ply` is workspace-excluded. Everything else (744 crates
including all holon crates) checks cleanly; the Swift build script is the
sole failure. Needs an older SDK/toolchain or an upstream waterkit fix.
`Cargo.lock` here is seeded from the root workspace lock — a fresh resolve
picks stable `ed25519 3.0.0` which breaks `ed25519-dalek 3.0.0-pre.1` (root
pins the `-rc` line); keep the seeded pins.

Architecture wired up:

- `FrontendSession` startup with `watch_ui()` CDC stream
- Reactive `Binding<WidgetSpec>` + `watch()` for live re-renders on CDC updates
- The **shared** render interpreter from `holon_frontend::render_interpreter`
  (`RenderInterpreter<AnyView>`), wired via `holon_macros::builder_registry!`
  in `src/render/builders/mod.rs::create_interpreter()`
- Screen layout with sidebar collapsing (left/right sidebars from `collapse_to: "drawer"`)
- Operation dispatch via `holon_frontend` (the local operations module was removed)

### Files

| File | Purpose |
|------|---------|
| `src/lib.rs` | App entry: tokio runtime, FrontendSession, watch_ui, reactive binding, `watch()` root view |
| `src/render/builders/` | One file per builder + `mod.rs` with `create_interpreter()` (builder_registry!) |

CDC handling and app state live in `holon_frontend`'s `ReactiveEngine`; there
is no local `state.rs`/`cdc.rs`/`operations.rs`/`render/interpreter.rs`
anymore. The legacy single-file `render/builders.rs` pipeline was deleted —
do not resurrect it; extend `render/builders/` instead.

## Reactivity Architecture

CDC updates flow through a cross-thread bridge:

1. **Tokio side**: `ReactiveEngine` receives `UiEvent`s and maintains the `WidgetSpec`
2. **Bridge**: `BindingMailbox::handle()` (sync `try_send` via `async_channel::Sender`) queues a job for waterui's local executor
3. **UI side**: waterui's `LocalExecutor` processes the job, updating `Binding<WidgetSpec>`
4. **Re-render**: `watch(binding, |ws| render_widget_spec(ws))` fires when binding changes

## Reference Implementation

**Blinc** (`frontends/blinc/`) is the reference implementation. It uses the same Holon architecture but renders to blinc's `Div` type, with each builder in a separate file under `src/render/builders/` — the same layout this crate now uses.

## Remaining Work — Ordered by Impact

### 1. Icon Support (LOW — currently text placeholder)

Blinc uses a build script (`build.rs`) that embeds SVG icons as data URIs at compile time, then renders them with `img(data_uri)`.

**What to do**: Either port the build script and use waterui's image/SVG support, or use waterui's icon packs (see `~/.cargo/git/checkouts/waterui-*/*/icon-packs/`).

### 2. Theming (LOW — hardcoded colors)

All colors are hardcoded hex strings. Blinc uses `blinc_theme::ThemeState` with semantic `ColorToken`s.

**What to do**: Use waterui's `theme` module (`waterui::theme`, `ColorScheme`, `Theme`). Check if waterui has semantic color tokens. If not, define a small color palette struct and thread it through `RenderContext`.

### 3. MCP Server (LOW — not started)

Blinc embeds an MCP HTTP server so external tools can query the running instance (`main.rs:48-69`: spawns `holon_mcp::di::run_http_server()` on port 8520).

**What to do**: Add `holon-mcp = { path = "../../crates/holon-mcp" }`, spawn the server in `app()` after session creation. Need `tokio-util` for `CancellationToken`.

### 4. Sidebar Toggle (LOW — sidebars always open)

Screen layout renders sidebars at fixed 280px width. Blinc uses `State<bool>` per sidebar for open/close toggle. Need to add `Binding<bool>` for each sidebar and wire toggle buttons.

## WaterUI API Gotchas

1. **`waterui::prelude::*` shadows `Vec::get()`**: The prelude re-exports something that conflicts. Use `<[T]>::get(&vec, index)` for positional access on `Vec`.

2. **View modifiers return new types**: `text("x").bold()` returns `Bold<Text>`, not `Text`. You can't reassign `let mut t = text(...); t = t.bold()`. Either wrap each branch in `AnyView::new()`, or compose the full modifier chain in one expression.

3. **`AnyView::new()` requires `'static`**: Any `&str` borrowed from `RenderContext` or resolved args must be `.to_string()`'d before passing to `AnyView::new(text(...))`.

4. **`vstack`/`hstack` accept tuples or `Vec<AnyView>`**: For dynamic lists use `Vec<AnyView>`. For fixed layouts use tuples: `vstack((view1, view2))`.

5. **`waterui_ffi::export!()`** is required at crate root for the FFI bridge. Don't remove it.

6. **`Color` is not `Copy`**: Use `Color::srgb_hex(...)` inline each time instead of binding to a variable and reusing.

7. **`Str::from(&str)` requires `'static`**: Use `Str::from(String)` for dynamic strings (takes ownership).

8. **`Binding` is `!Send`**: Use `BindingMailbox` (via `binding.mailbox()`) for cross-thread updates. `BindingMailbox::handle()` is sync and `Send`.

9. **`binding()` function**: Not in the prelude — import from `waterui::reactive::binding`.

## Build & Run

### Version Alignment (CRITICAL)

The `Cargo.toml` pins `waterui` and `waterui-ffi` to a dev-branch rev of the waterui git repo. The `water` CLI (release v0.1.3) scaffolds the Xcode project with the **release** `apple-backend 0.2.0` Swift package. These two are **incompatible** — the release apple-backend's C header declares FFI symbols (`waterui_color_id`, `waterui_force_as_photo`, etc.) that don't exist in the dev-branch Rust crate.

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
cd frontends/waterui && cargo check   # Rust compilation check (own workspace)

# Scaffold (only needed once, or after deleting .water/):
water run --platform macos
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
HOLON_VAULT_ROOT=/path/to/orgfiles/ \
.water/apple/.water/DerivedData/Build/Products/Debug/WaterUIApp.app/Contents/MacOS/WaterUIApp
```

**Note**: `open WaterUIApp.app` won't pass env vars to the process. Use the direct binary path above.

### When this gets fixed

Once the `water` CLI dev branch compiles (currently has 2 compile errors in `cli/src/toolchain/doctor.rs`), installing it from source (`cargo install --git ... --branch dev waterui-cli`) will make `water run` work directly — the dev CLI supports `waterui_path` in `Water.toml` and scaffolds with the matching apple-backend.
