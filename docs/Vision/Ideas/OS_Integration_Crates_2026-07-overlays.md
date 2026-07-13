# OS Integration Crates for Overlay Windows (2026-07)

Research conducted 2026-07-13 for the "vignette" concept: a click-through overlay appearing at screen edges to show gentle presence notifications. Covers macOS, Windows, and Linux (Wayland + X11).

---

## 1. Cross-Platform Windowing Foundations

### winit

| Field | Detail |
|-------|--------|
| **Latest version** | 0.30.13 (2026-03-02); pre-release track 0.31.0-beta.2 |
| **Downloads** | ~26.7M total, ~4.7M recent |
| **License** | Apache-2.0 |
| **crates.io** | https://crates.io/crates/winit |
| **GitHub** | https://github.com/rust-windowing/winit |

**Platform coverage**: macOS, Windows, X11, Wayland, Web, Android, iOS, Orbital.

**Overlay API fit**: Excellent as a cross-platform base. The stack for a click-through vignette:
- `WindowAttributes::with_transparent(true)` -- transparent background
- `WindowAttributes::with_decorations(false)` -- no title bar
- `WindowAttributes::with_window_level(WindowLevel::AlwaysOnTop)` -- float above other windows
- `Window::set_cursor_hittest(false)` -- click-through (macOS: `ignoresMouseEvents`, Windows: empty input region via `WS_EX_TRANSPARENT`, Wayland: empty input region, X11: XShape region)

**Caveats**:
- `set_cursor_hittest(false)` on X11 was broken in 0.29.x but fixed in later releases. Always-on-top is an OS hint, not a guarantee.
- macOS: setting `ignoresMouseEvents` is all-or-nothing -- you cannot have some interactive regions and some click-through regions on the same window without toggling the property dynamically.
- Wayland: winit does NOT support `wlr-layer-shell` (see issue [#2582](https://github.com/rust-windowing/winit/issues/2582), still open, low priority). It creates a regular top-level surface, not a layer surface. This means proper overlay anchoring (e.g. "stick to top edge, reserve no space") is unavailable on Wayland through winit alone.
- Transparency on Windows with winit historically required removing `DwmEnableBlurBehindWindow`; `WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)` suffices for uniform transparency but not per-pixel alpha.

**Verdict**: The standard cross-platform windowing crate. For a vignette overlay, it provides the right primitives (transparent, undecorated, always-on-top, click-through) on macOS and Windows. On Linux, the lack of layer-shell support means it creates a regular window rather than a compositor-integrated overlay -- functional but less polished (no screen-edge anchoring, compositor may stack it incorrectly). Actively maintained (119 versions, 0.30.13 in March 2026).

---

### tao

| Field | Detail |
|-------|--------|
| **Latest version** | 0.35.0 (published ~hours ago); 0.34.x line active |
| **Downloads** | Not directly listed (tao-macros has 4.26M; tao itself is a Tauri core dep) |
| **License** | Apache-2.0 |
| **crates.io** | https://crates.io/crates/tao |
| **GitHub** | https://github.com/tauri-apps/tao (~2,073 stars, 290 forks) |

**Platform coverage**: Windows, macOS, Linux, iOS, Android.

**Overlay API fit**: A fork of winit maintained by the Tauri project. The API is very similar:
- `WindowAttributes.transparent` field (bool)
- `WindowAttributes.always_on_top` field (bool)
- `WindowAttributes.decorations` field (bool)
- `WindowAttributes.focusable` field (bool)
- `WindowBuilderExtMacOS::with_has_shadow(false)` -- remove drop shadow on macOS
- `WindowBuilderExtMacOS::with_titlebar_transparent(true)` / `with_fullsize_content_view(true)` -- macOS-specific titlebar control

**Differences from winit**:
- On Linux, tao replaces winit's raw Wayland/X11 backend with GTK3, which changes behavior (e.g., transparency on Linux via GTK is different from raw X11/Wayland).
- Does NOT expose `set_cursor_hittest` as a public method. To achieve click-through on tao, you would need to access the raw window handle and call platform-specific APIs yourself (NSWindow, Win32, GTK).
- macOS transparency recently had a bug with incorrect border rendering (tao 0.34.5, macOS 15.6.1) -- transparent windows showed black/grey borders.

**Verdict**: Healthiest maintenance signal of any windowing crate (Tauri team, 120 versions, multiple releases per week in 2026, backports to old version lines). However, the missing `set_cursor_hittest` API and GTK-on-Linux approach make it less straightforward for a click-through vignette than winit. Best suited if you are already in the Tauri ecosystem.

---

## 2. macOS-Specific Considerations

Both winit and tao map their overlay window flags to the standard AppKit recipe:

```objc
// What winit does under the hood:
window.styleMask = .borderless
window.backgroundColor = .clear
window.isOpaque = false
window.hasShadow = false
window.level = .floating  // or .screenSaver for extreme top
window.ignoresMouseEvents = true  // set_cursor_hittest(false)
window.collectionBehavior = [.canJoinAllSpaces, .stationary]
```

**Key macOS nuance**: `NSWindow.ignoresMouseEvents` is all-or-nothing. Once you set it (even to `false`), the per-pixel transparency-based click-through behavior is permanently disabled for that window. If you ever need some interactive elements on the vignette, you must toggle `ignoresMouseEvents` dynamically based on cursor position. This is a known Electron bug pattern too.

**Recommended window level**: `NSFloatingWindowLevel` (above normal windows, below modal dialogs) is typically correct for a notification vignette. `NSScreenSaverWindowLevel` would place it above everything including fullscreen apps.

---

## 3. Windows-Specific Considerations

Both winit and tao map overlay flags to Win32:

```rust
// Effective style: WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE
// Per-monitor DPI awareness: SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
```

**Key Windows nuances**:
- `WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA, ..., alpha, ...)` gives uniform window transparency. For per-pixel alpha, you need `UpdateLayeredWindow` instead.
- `WS_EX_TRANSPARENT` makes the window click-through at the Win32 level (compositor forwards input to windows below).
- `WS_EX_TOOLWINDOW` prevents the window from appearing in the taskbar.
- `WS_EX_NOACTIVATE` prevents the window from taking focus when clicked (if click-through is temporarily disabled).
- Per-monitor DPI awareness v2 (`PMv2`) is essential for a vignette spanning multiple monitors or appearing on non-primary monitors at correct scale.

### windows (Microsoft official crate)

| Field | Detail |
|-------|--------|
| **Latest version** | 0.62.2 |
| **Downloads** | ~19.2M/month; 1,550 direct dependents |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/windows |
| **GitHub** | https://github.com/microsoft/windows-rs |

**Verdict**: If winit/tao need supplementation on Windows (e.g., to set `WS_EX_NOACTIVATE` or programmatic DPI awareness that the windowing crate doesn't expose), the `windows` crate provides direct, strongly-typed Win32 bindings. You can extract an `HWND` from winit/tao via `raw-window-handle` and call additional Win32 APIs. Actively maintained by Microsoft, 1.21B total downloads across the family.

---

## 4. Linux: Wayland (wlr-layer-shell)

### wayland-protocols-wlr

| Field | Detail |
|-------|--------|
| **Latest version** | 0.3.10 |
| **Downloads** | ~30.4M total |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/wayland-protocols-wlr |
| **GitHub** | https://github.com/Smithay/wayland-rs |

**Platform coverage**: Wayland only (client + server bindings to wlr-protocols).

**Overlay API fit**: Provides the raw protocol bindings for `zwlr_layer_shell_v1`. The `Layer` enum has four values: `Background (0)`, `Bottom (1)`, `Top (2)`, `Overlay (3)`. An overlay vignette would use `Layer::Overlay` -- the topmost layer, above all other layers including panels and lockscreens. The protocol supports:
- Anchoring to any screen edge/corner
- Keyboard interactivity modes: `None` (no keyboard), `Exclusive` (lock screen style), `OnDemand`
- Input region control for click-through

This crate is the protocol layer only -- you need a higher-level crate (smithay-client-toolkit) to actually drive it.

**Verdict**: Essential low-level dependency for any Wayland layer-shell approach. Very healthy (30M downloads, part of the well-maintained Smithay/wayland-rs family). Not something you'd use directly for a vignette; it's the foundation that SCTK or a custom integration builds on.

---

### smithay-client-toolkit (SCTK)

| Field | Detail |
|-------|--------|
| **Latest version** | 0.20.0 |
| **Downloads** | ~43.9M total |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/smithay-client-toolkit |
| **GitHub** | https://github.com/Smithay/client-toolkit |

**Platform coverage**: Wayland only (client-side toolkit on top of wayland-client).

**Overlay API fit**: The highest-level Wayland client library in Rust. Its `shell::wlr_layer` module provides:
- `LayerSurface` -- create with `Layer::Overlay` for vignette placement
- `Anchor` -- anchor to edges (e.g., `Anchor::TOP | Anchor::RIGHT` for top-right vignette)
- `KeyboardInteractivity::None` -- render-only, no input capture
- Configure events -- compositor tells you the assigned size/position
- `SlotPool` + `Shm` for software rendering, or EGL for GPU rendering

The official `simple_layer.rs` example demonstrates the full flow: bind `LayerShell` global, create `LayerSurface` with overlay layer + anchors, handle configure events, render via shared memory or EGL.

**Verdict**: The canonical Wayland client toolkit. For a vignette on Wayland that needs true layer-shell behavior (screen-edge anchoring, predictable z-ordering, "overlay" layer above lockscreens), SCTK is the right choice. It requires writing Wayland-specific rendering code (no automatic GL context like winit provides), so you'd use it alongside a rendering backend (femtovg, tiny-skia, wgpu via raw handles). Actively maintained, 44M downloads.

---

### layer-shika

| Field | Detail |
|-------|--------|
| **Latest version** | Early development (not 1.0) |
| **Downloads** | Low (early stage) |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/layer-shika |
| **GitHub** | https://github.com/waydeerwm/layer-shika |

**Platform coverage**: Wayland only.

**Overlay API fit**: A higher-level layer-shell wrapper built on SCTK + Slint UI + femtovg/EGL rendering. Provides a fluent builder API, declarative configuration, multi-surface support, multi-output support. Created specifically for shell components like status bars, panels, and notifications.

**Verdict**: Promising but not production-ready. If you want to prototype a Wayland vignette quickly with a declarative UI toolkit, this is the lowest-friction path. But early-stage API instability makes it unsuitable for shipping code. Worth watching.

---

### egui-layer-shell (kierandrewett)

| Field | Detail |
|-------|--------|
| **Latest version** | Fork, not published on crates.io |
| **GitHub** | https://github.com/kierandrewett/egui-layer-shell |

**Verdict**: A fork of egui targeting layer-shell. Unpublished, maintenance unclear. Interesting as a reference implementation for how to wire egui rendering into a layer-shell surface, but not a dependency you'd take.

---

## 5. Linux: X11

### winit (X11 backend)

winit's X11 backend supports `set_cursor_hittest(false)` via `xfixes_set_window_shape_region()`. This was broken in some 0.29.x releases but is fixed in 0.30.x. The window is created as a regular override-redirect or managed window (not an X11 overlay window), with ARGB visual for transparency.

### x11-overlay

| Field | Detail |
|-------|--------|
| **Latest version** | Early development |
| **Downloads** | Very low |
| **License** | Not specified on crates.io page |
| **crates.io** | https://crates.io/crates/x11-overlay |

**Platform coverage**: X11 only.

**Overlay API fit**: Purpose-built for click-through overlays on X11. Uses ARGB visuals for true transparency, XShape extension for click-through, Cairo for text rendering. Provides composite components (status panels, notifications) and a modular component-based UI architecture.

**Verdict**: X11-only and early-stage. For a vignette that also targets Wayland/macOS/Windows, this is too narrow. Only relevant if you're specifically building an X11-only overlay with Cairo rendering.

---

## 6. Purpose-Built Overlay Crates

### egui_overlay

| Field | Detail |
|-------|--------|
| **Latest version** | ~0.4.x area (depends on egui ^0.29, egui_window_glfw_passthrough ^0.9) |
| **Downloads** | ~23K total |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/egui_overlay |
| **GitHub** | https://github.com/coderedart/egui_overlay |

**Platform coverage**: Linux (X11 natively, Wayland via Xwayland), macOS (wgpu backend), Windows.

**Overlay API fit**: An egui integration that creates desktop overlays using GLFW passthrough windows:
- Linux: X11 supported natively; Wayland works via Xwayland (not true layer-shell)
- macOS: egui_render_wgpu backend (no OpenGL, which Apple deprecated)
- Windows + macOS: egui_render_three_d backend
- Input passthrough via GLFW's mouse passthrough extension
- Works with any egui-based UI

**Verdict**: The closest thing to a purpose-built "vignette in Rust" crate. If your notification UI is built in egui, this is a viable shortcut. Caveats: uses GLFW, not winit; Wayland support is Xwayland-only (no true layer-shell); ~23K downloads suggests a niche but real user base. The author recommends using `egui_window_glfw_passthrough` directly (~150 lines) for more control.

---

### egui_window_glfw_passthrough

| Field | Detail |
|-------|--------|
| **Latest version** | 0.9.0 (published ~12 months ago) |
| **Downloads** | Low (niche dependency) |
| **License** | MIT |
| **crates.io** | https://crates.io/crates/egui_window_glfw_passthrough |

**Platform coverage**: Same as GLFW -- macOS, Windows, Linux (X11 + Xwayland).

**Overlay API fit**: An egui windowing backend using GLFW with mouse passthrough. Lower-level than egui_overlay; you wire rendering and event loop yourself.

**Verdict**: Specialized GLFW-based passthrough window backend. The underlying GLFW mouse passthrough patch is not yet in a stable GLFW release (targeted for 3.4). Maintenance note: the author plans to eventually migrate to piston's glfw-rs once GLFW 3.4 ships with the passthrough patch. In the meantime, this uses a fork (`glfw-passthrough`).

---

## 7. The GNOME/Mutter Gap

A critical Linux caveat: **GNOME's Mutter compositor deliberately refuses to implement `wlr-layer-shell`** (tracked in [mutter#973](https://gitlab.gnome.org/GNOME/mutter/-/issues/973) and [gnome-shell#1141](https://gitlab.gnome.org/GNOME/gnome-shell/-/work_items/1141)). As of mid-2026, this policy has not changed.

**Impact on vignette**:
- On GNOME Wayland, any approach using wlr-layer-shell (SCTK, wayland-protocols-wlr, layer-shika) will NOT work.
- The only Wayland path on GNOME is a regular top-level window (winit/tao approach) -- no screen-edge anchoring, no overlay layer z-ordering.
- On GNOME, winit with `set_cursor_hittest(false)` on a transparent undecorated window is the best available option.
- GNOME Shell extensions can achieve similar effects but from JavaScript, not Rust, and require packaging as a GNOME extension.

**Compositors that DO support wlr-layer-shell**: Sway, Hyprland, Wayfire (wlroots family), KDE Plasma (KWin), COSMIC (Smithay). This covers roughly 60-70% of the Linux desktop Wayland user base.

---

## 8. Interoperability Crates

### raw-window-handle

| Field | Detail |
|-------|--------|
| **Latest version** | 0.6.2 |
| **Downloads** | ~78.4M total, ~19.7M recent |
| **License** | MIT / Apache-2.0 |
| **crates.io** | https://crates.io/crates/raw-window-handle |

**Verdict**: The standard interoperability layer between windowing crates and graphics crates. If you use winit to create windows but need to call platform-specific APIs (e.g., `SetWindowLongPtrW` on Windows, `NSWindow.setIgnoresMouseEvents` directly on macOS, GTK window manipulation on Linux), extract the raw handle here. Not updated in ~2 years because it's API-stable. Universal dependency (78M downloads).

---

## 9. Recommendation Matrix

For a click-through vignette overlay, the strategy depends on how much per-platform tuning you want to do:

### Option A: winit-only (simplest, least platform polish)

| Platform | Approach | Click-through | Edge anchoring | Z-order guarantee |
|----------|----------|---------------|----------------|-------------------|
| macOS | `with_transparent + set_cursor_hittest(false)` | Yes | Manual positioning | `AlwaysOnTop` (best effort) |
| Windows | Same | Yes | Manual positioning | `AlwaysOnTop` (best effort) |
| Linux Wayland | Same (regular window, not layer) | Yes (empty input region) | Manual positioning | `AlwaysOnTop` (best effort) |
| Linux X11 | Same | Yes (XShape) | Manual positioning | `AlwaysOnTop` (best effort) |

**Pros**: One code path, well-tested, 26M downloads of winit.
**Cons**: No true layer-shell anchoring on Wayland; no GNOME overlay; window stacking is best-effort.

### Option B: winit + SCTK on Wayland (recommended for Linux polish)

| Platform | Approach |
|----------|----------|
| macOS | winit (as in Option A) |
| Windows | winit (as in Option A) |
| Linux Wayland (non-GNOME) | SCTK wlr-layer-shell overlay layer |
| Linux Wayland (GNOME) | fall back to winit regular window |
| Linux X11 | winit (as in Option A) |

**Pros**: True layer-shell behavior on Wayland compositors that support it (Sway, KDE, Hyprland, etc.) -- proper z-ordering above lockscreens, screen-edge anchoring, no window decorations/management interference.
**Cons**: Two rendering paths on Linux (SCTK for Wayland, winit for X11/GNOME). SCTK requires Wayland-specific rendering setup (no winit convenience).

### Option C: egui_overlay (if UI is egui-based)

If your vignette is rendered with egui, this is the quickest path to a working overlay across all platforms. The GLFW passthrough approach handles click-through uniformly. However, it does NOT use layer-shell on Wayland (Xwayland only), so the same Linux caveats as Option A apply.

---

## 10. Key URLs

| Crate | crates.io |
|-------|-----------|
| winit | https://crates.io/crates/winit |
| tao | https://crates.io/crates/tao |
| windows (Microsoft) | https://crates.io/crates/windows |
| smithay-client-toolkit | https://crates.io/crates/smithay-client-toolkit |
| wayland-protocols-wlr | https://crates.io/crates/wayland-protocols-wlr |
| raw-window-handle | https://crates.io/crates/raw-window-handle |
| egui_overlay | https://crates.io/crates/egui_overlay |
| egui_window_glfw_passthrough | https://crates.io/crates/egui_window_glfw_passthrough |
| layer-shika | https://crates.io/crates/layer-shika |
| x11-overlay | https://crates.io/crates/x11-overlay |

---

## 11. Summary Verdict

For a production vignette in mid-2026:

1. **Use winit** as the cross-platform foundation. It has the right primitives for macOS and Windows, and a reasonable Wayland story via `set_cursor_hittest(false)`. Its maintenance is healthy (0.30.13 from March 2026, 26M downloads).

2. **Supplement with smithay-client-toolkit** on Linux Wayland for compositors that support wlr-layer-shell (Sway, KDE Plasma, Hyprland, COSMIC). This gives you true overlay-layer z-ordering and screen-edge anchoring. Fall back to winit's regular window path for GNOME/Mutter and X11.

3. **Avoid tao** unless you're already in the Tauri ecosystem. Its missing `set_cursor_hittest` API and GTK-on-Linux approach add friction for a click-through vignette.

4. **raw-window-handle** bridges winit to platform-specific APIs when you need to set extra flags (e.g., `WS_EX_NOACTIVATE` on Windows, `canJoinAllSpaces` on macOS).

5. **egui_overlay** is a legitimate shortcut if your vignette UI is egui-based and you accept GLFW as the windowing backend instead of winit.

The fundamental gap remains **GNOME's refusal to implement wlr-layer-shell**. On GNOME Wayland, the vignette can only be a regular transparent window -- no screen-edge anchoring, no overlay-layer guarantee. GNOME Shell extensions are the only way to get true shell integration on that platform, and those are JavaScript, not Rust.
