# Handoff: Wire GPUI PBT to launch a real window with screenshots

## Goal
The GPUI PBT test (`tests/gpui_ui_pbt.rs`) currently runs headless — it creates a `BoundsRegistry` but never opens a GPUI window. All UI mutations silently fall back to FFI. Wire it up to launch a real GPUI window so that:
1. The PBT engine drives changes through the real UI render loop
2. `BoundsRegistry` gets populated with element bounds during render
3. xcap-based screenshots capture the window on every PBT step

## What already exists

### Screenshot infrastructure (just completed)
- `ScreenshotBackend` trait in `crates/holon-integration-tests/src/ui_driver.rs`
- `XcapBackend` — finds window by title, captures via xcap (cross-platform, no host tools)
- `GeometryDriver.with_screenshots(backend, dir)` — captures on every step
- `save_screenshot()` annotates with red rectangle using element bounds from geometry
- Hooked into `run_pbt_with_driver_sync` — `driver.screenshot()` called on every post-startup step
- xcap confirmed working: it enumerates windows but finds no "Holon" window (because none is opened)

### GPUI app structure (`frontends/gpui/src/main.rs`)
- `HolonApp` struct: holds `FrontendSession`, `AppState`, `BoundsRegistry`
- `Render::render()` clears BoundsRegistry, interprets `WidgetSpec` via render builders
- But `BoundsRegistry.record()` is never called — no element bounds are recorded
- App launched via `gpui::Application::new().run()` which blocks the main thread

### Blinc PBT (reference: `tests/blinc_ui_pbt.rs`)
- Spawns the Blinc app on a separate thread (`WindowedApp::run()` blocks)
- Sends the `ElementRegistry` back via `mpsc::sync_channel`
- Creates `BlincGeometry` from the registry, feeds it to `GeometryDriver`
- Window title "Blinc" used by `XcapBackend`

## Implementation plan

### Step 1: Make BoundsRegistry populate during GPUI render

The core missing piece. GPUI's `.id()` method changes `Div` to `Stateful<Div>`, which is why this was deferred. Options:

**Option A: Post-layout bounds query (preferred)**
In `HolonApp::render()`, after the root element is built, use GPUI's `prepaint` or `after_layout` callback to query element bounds. GPUI elements with `.id("some-id")` can have their bounds queried via `cx.bounds()` or similar API. Check GPUI 0.2 docs for:
- `ElementId` / `GlobalElementId`
- `cx.with_element_state()` or `Element::prepaint()` to read bounds after layout

**Option B: TrackedElement wrapper**
Create a `TrackedElement<E: IntoElement>` wrapper that:
1. Wraps any element
2. Has a `BoundsRegistry` reference
3. In its `Element::paint()` impl, reads `self.bounds` and calls `registry.record(id, bounds)`

This is what the header comment suggests: "requires TrackedElement wrapper because GPUI's `.id()` changes Div to Stateful<Div>"

**Option C: Use `observe_bounds` (if available)**
Check if GPUI 0.2 has `observe_bounds` or similar layout-complete callback.

### Step 2: Launch GPUI window from the PBT test

Follow the Blinc pattern:

```rust
fn gpui_geometry_pbt() {
    let (registry_tx, registry_rx) = std::sync::mpsc::sync_channel(1);

    // Spawn GPUI app on a separate thread (Application::run blocks)
    std::thread::spawn(move || {
        // Need to create: FrontendSession with temp DB + orgmode dir
        // Then open GPUI window with HolonApp
        // Send BoundsRegistry back via channel
        let app = gpui::Application::new()
            .with_assets(gpui_component_assets::Assets);
        app.run(move |cx| {
            gpui_component::init(cx);
            let bounds_registry = BoundsRegistry::new();
            let _ = registry_tx.send(bounds_registry.clone());
            // ... open window with HolonApp using this bounds_registry
        });
    });

    let bounds_registry = registry_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Timed out waiting for GPUI BoundsRegistry");

    let backend = XcapBackend::new("Holon");
    let mut driver = GeometryDriver::new(Box::new(bounds_registry))
        .with_screenshots(Box::new(backend), screenshot_dir);

    run_pbt_with_driver_sync(15, &mut driver)
}
```

Key challenge: `HolonApp` needs a `FrontendSession`, which needs a `FrontendConfig` with a temp DB and orgmode root. The PBT's `E2ESut` already creates one — need to share it or create the session inside the GPUI thread.

### Step 3: Connect PBT engine to the GPUI window's FrontendSession

The PBT engine (`E2ESut`) creates its own `BackendEngine` + `FrontendSession`. The GPUI window needs to use the SAME session so that mutations applied by the PBT are reflected in the rendered UI:

- **Option A**: Pass the PBT's `FrontendSession` to the GPUI window (requires refactoring `HolonApp::new` to accept an external session)
- **Option B**: Create a shared `BackendEngine` and give both the PBT and the GPUI window access to it

Option A is simpler. Extract `HolonApp` construction to take an injected `FrontendSession`.

### Step 4: Re-render on PBT mutations

After each PBT step applies a mutation, the GPUI window needs to re-render:
- The `watch_ui` CDC stream already triggers re-renders in the production app
- If the PBT and GPUI share the same `FrontendSession`, CDC events will flow naturally
- May need to add a `driver.settle()` delay or explicit `cx.notify()` trigger

## Key files

| File | Role |
|------|------|
| `tests/gpui_ui_pbt.rs` | Test entry point — needs to spawn GPUI window |
| `frontends/gpui/src/main.rs` | `HolonApp` struct + render — needs to be extractable as lib |
| `frontends/gpui/src/lib.rs` | Currently just `pub mod geometry;` — needs to export `HolonApp` |
| `frontends/gpui/src/geometry.rs` | `BoundsRegistry` — needs `record()` calls during render |
| `frontends/gpui/src/render/builders/` | Render builders — need to tag elements with IDs |
| `crates/holon-integration-tests/src/ui_driver.rs` | `GeometryDriver` + `XcapBackend` (done) |
| `crates/holon-integration-tests/src/pbt/phased.rs` | `run_pbt_with_driver_sync` — screenshot hook (done) |

## Risks / open questions

1. **GPUI thread model**: `Application::run()` takes over the main thread on macOS. The test currently runs on the main thread. May need to run the PBT on a spawned thread instead of the GPUI app.
2. **Bounds population timing**: Elements may not have bounds until after the first paint. Need to wait for at least one render cycle before the BoundsRegistry has useful data.
3. **Screen Recording permission**: xcap requires macOS Screen Recording permission. The test should log a clear message if denied.
4. **PBT + real render latency**: Adding real rendering slows down each step. May need to increase settle time or add explicit "wait for render" synchronization.
5. **`widget_states` field**: The LSP shows `main.rs:29` has a missing `widget_states` field on some struct — may need fixing before this work starts.
