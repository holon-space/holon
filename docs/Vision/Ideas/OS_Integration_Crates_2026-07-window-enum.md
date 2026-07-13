# OS Integration Crates: Window Enumeration & Control (2026-07)

> Research for the "Shell" layer of a Rust desktop app that needs to observe and control OTHER apps' windows across macOS, Windows, and Linux.

**Key finding:** No single Rust crate covers window enumeration, focus observation, AND hide/show across all three platforms. The landscape splits into read-only observation (xcap) and platform-specific control APIs.

---

## Cross-Platform

### `xcap` v0.9.6

- **crates.io:** <https://crates.io/crates/xcap>
- **Repo:** <https://github.com/nashaofu/xcap> (994 stars, Apache-2.0)
- **Downloads:** 1.14M total, 524K recent | **Dependents:** 60
- **Platforms:** macOS (CGWindowList), Windows (EnumWindows + DwmGetWindowAttribute), Linux X11 (_NET_CLIENT_LIST_STACKING via XCB). No Wayland support.
- **API fit:**
  - Window enumeration: YES. `Window::all() -> Vec<Window>` sorted by z-order. Returns id, pid, app_name, title, x/y/z/width/height, is_minimized, is_maximized, is_focused.
  - Focus observation: YES (per-window `is_focused()` via platform-specific active-window check).
  - Hide/show: NO. Read-only.
- **macOS permission model:** Enumeration via CGWindowListCopyWindowInfo works **without any permissions**. Screenshot capture needs Screen Recording permission (TCC prompt).
- **Maintenance:** Active (last updated 2026-05-24). Holon uses the `nightscape/xcap` fork (`feat/macos-offscreen-windows` branch). The Wayland gap is the main weakness.
- **Verdict:** The best single crate for cross-platform read-only window observation. Already battle-tested in Holon's PBT infrastructure. For hide/show, combine with platform-specific crates. Contributing Wayland support (`ext-foreign-toplevel-list-v1`) upstream would close its biggest gap.

### `active-win-pos-rs` v0.11.0

- **crates.io:** <https://crates.io/crates/active-win-pos-rs>
- **Repo:** <https://github.com/dimusic/active-win-pos-rs> (151 stars, MIT OR Apache-2.0)
- **Downloads:** 160K total, 61K recent | **Dependents:** 15
- **Platforms:** macOS + Windows only. No Linux.
- **API fit:** Active (foreground) window position/size/title only. No full window enumeration.
- **macOS:** Uses Accessibility API (requires Accessibility permission -- harder than xcap's CGWindowList approach).
- **Verdict:** Too narrow. Single active window only, no Linux, macOS Accessibility permission requirement. Useful as reference code, not as a dependency.

### `winit` v0.30.13

- **crates.io:** <https://crates.io/crates/winit>
- **Repo:** <https://github.com/rust-windowing/winit>
- **Downloads:** 45.6M total, 8.8M recent | **Dependents:** High (iced, egui, GPUI all use it)
- **Platforms:** macOS, Windows, Linux (X11 + Wayland), Android, iOS, Web
- **API fit:** Creates and manages windows YOUR app owns. Does NOT enumerate or control foreign windows.
- **Verdict:** Not relevant. winit is a window creation library, not a window management library.

### `accesskit` v0.24.1

- **crates.io:** <https://crates.io/crates/accesskit>
- **Repo:** <https://github.com/AccessKit/accesskit> (1,484 stars, MIT OR Apache-2.0)
- **Downloads:** 21M total, 5M recent | **Dependents:** 118
- **Platforms:** macOS, Windows, Linux (AT-SPI via D-Bus)
- **API fit:** Built to expose YOUR app's accessibility tree to screen readers. Can theoretically consume other apps' trees but this is not the design goal. Platform backends (`accesskit_macos`, `accesskit_windows`, `accesskit_unix`) wrap the native APIs but expose them for accessibility tree purposes, not window management.
- **Verdict:** Wrong abstraction layer. Using accesskit for window enumeration is like using a screwdriver as a hammer -- it technically touches the right APIs (AXUIElement, UIA, AT-SPI) but through a layer designed for a completely different purpose.

---

## macOS

### `objc2` v0.6.4 + `objc2-app-kit` v0.3.2 + `objc2-foundation` v0.3.2

- **crates.io:** <https://crates.io/crates/objc2> | <https://crates.io/crates/objc2-app-kit> | <https://crates.io/crates/objc2-foundation>
- **Repo:** <https://github.com/madsmtm/objc2> (981 stars, MIT)
- **Downloads:** objc2: 79.7M | app-kit: 39.2M | foundation: 56.5M
- **Dependents:** objc2: 715 | app-kit: 282 | foundation: 582
- **Platforms:** macOS only
- **API fit:**
  - Window enumeration: `NSWorkspace::runningApplications` -> `NSRunningApplication` list (PID, bundle ID, localized name). For full window list, need AXUIElement (not in objc2-app-kit yet -- thin gap over objc2's `msg_send!`).
  - Focus observation: `NSWorkspace::sharedWorkspace` + `NSWorkspaceDidActivateApplicationNotification` via `NSNotificationCenter`. Push-based, no polling.
  - Hide/show: `NSRunningApplication::hide` / `unhide` / `activateWithOptions`.
- **Maintenance:** Extremely active (commits this week). madsmtm is responsive. Already in Holon's dependency tree via GPUI.
- **Gap:** AXUIElement not yet in objc2-app-kit. Needs ~20 lines of `msg_send!` wrapper or a manual `extern_class!` definition for full window enumeration.
- **Verdict:** The best macOS foundation. NSRunningApplication + NSWorkspace notifications cover focus observation and hide/show. Combine with `core-graphics` for window enumeration (CGWindowList). The AXUIElement gap is trivial to fill.

### `core-graphics` v0.25.0

- **crates.io:** <https://crates.io/crates/core-graphics>
- **Repo:** <https://github.com/servo/core-foundation-rs> (1,272 stars, MIT OR Apache-2.0)
- **Downloads:** 55.5M total, 15.8M recent | **Dependents:** 185
- **Platforms:** macOS only
- **API fit:** `CGWindowListCopyWindowInfo` -- enumerates ALL system windows with PID, bounds, title, layer, ownership. Read-only. No hide/show or focus manipulation.
- **Permission model:** Works without any permissions (unlike Accessibility API). This makes it the preferred enumeration path.
- **Maintenance:** Servo-maintained, stable/mature, low churn. Already in Holon's dependency tree.
- **Verdict:** The right tool for macOS window enumeration. No permissions needed, well-maintained, already in-tree. Pair with objc2-app-kit for hide/show/control.

### `core-foundation` v0.10.1

- **crates.io:** <https://crates.io/crates/core-foundation>
- **Repo:** <https://github.com/servo/core-foundation-rs>
- **Downloads:** 355.8M total, 96.1M recent | **Dependents:** 398
- **Platforms:** macOS only
- **API fit:** CFArray, CFDictionary, CFString, CFRunLoop -- foundational Apple types. Building blocks, not a direct window management API.
- **Verdict:** Essential foundation layer. Already heavily used in Holon's tree. Not sufficient alone for window work.

### `cocoa` v0.26.1

- **crates.io:** <https://crates.io/crates/cocoa>
- **Repo:** <https://github.com/servo/core-foundation-rs>
- **Downloads:** 27.2M total, 5M recent | **Dependents:** 180
- **Platforms:** macOS only
- **API fit:** Wraps NSRunningApplication, NSWorkspace, NSWindow. Functionally capable but uses old `objc` v0.1.x bindings -- verbose and less safe than objc2.
- **Maintenance:** Legacy. Largely superseded by objc2. Still in Holon's tree (GPUI uses it), but not receiving active feature development.
- **Verdict:** Superseded. New code should use objc2 + objc2-app-kit directly.

### `icrate` v0.1.2

- **crates.io:** <https://crates.io/crates/icrate>
- **Downloads:** 6.6M total, 1.3M recent | **Dependents:** 8
- **Maintenance:** Low activity. Another objc bindings approach with minimal adoption.
- **Verdict:** Not recommended. Tiny ecosystem. objc2 is the clear winner.

### `accesskit_macos` v0.26.2

- **crates.io:** <https://crates.io/crates/accesskit_macos>
- **Repo:** <https://github.com/AccessKit/accesskit>
- **Downloads:** 9.3M total, 2.4M recent | **Dependents:** 10
- **Platforms:** macOS only
- **API fit:** Wraps AXUIElement for accessibility tree consumption. Not designed for window enumeration/hide/show. The underlying AXUIElement bindings could be reused but the abstraction layer adds indirection.
- **Verdict:** Wrong tool. Building AXUIElement wrappers directly via objc2 is simpler and more targeted.

---

## Windows

### `windows` v0.62.2

- **crates.io:** <https://crates.io/crates/windows>
- **Repo:** <https://github.com/microsoft/windows-rs> (12,558 stars, MIT OR Apache-2.0)
- **Downloads:** 266.4M total, 56.2M recent | **Dependents:** 1,587
- **Platforms:** Windows only
- **API fit (feature `Win32_UI_WindowsAndMessaging`):**
  - Window enumeration: `EnumWindows` callback-based enumeration of all top-level HWNDs.
  - Focus observation: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)` -- push-based foreground tracking, no injection, no privileges.
  - Hide/show: `ShowWindow(hwnd, SW_HIDE/SW_SHOW)`, `SetWindowPos`, `SetForegroundWindow`.
  - Additional: `GetWindowThreadProcessId` (HWND -> PID), `GetWindowText` (title), `IsWindowVisible`, `IsIconic` (minimized).
- **Feature flags:** Enable `Win32_UI_WindowsAndMessaging` specifically; avoid pulling the full Win32 surface to reduce compile time.
- **Maintenance:** Microsoft-maintained, programmatically generated from Win32 metadata. Multiple releases per month. Commits today.
- **Verdict:** The canonical choice. The only downside is that the `windows` crate types (BSTR, HSTRING, COM wrappers) add compile-time cost. For a slimmer approach, use `windows-sys` (next entry) -- same APIs, raw FFI signatures.

### `windows-sys` v0.61.2

- **crates.io:** <https://crates.io/crates/windows-sys>
- **Repo:** Same <https://github.com/microsoft/windows-rs>
- **Platforms:** Windows only
- **API fit:** Same Win32 surface as `windows`, but via raw `extern "system"` FFI function pointers. `EnumWindows`, `ShowWindow`, `SetWinEventHook` etc. are direct unsafe FFI calls -- no wrapper types, zero-cost, lighter compile.
- **Already in Holon's tree:** Yes, at v0.61.2 via transitive deps (async-io, polling, tokio, nix). Adding a direct dep costs nothing.
- **Verdict:** Pragmatic choice for "just call EnumWindows." The five APIs needed (EnumWindows, GetWindowThreadProcessId, SetWinEventHook, ShowWindow, SetForegroundWindow) are simple `extern "system"` signatures. A thin safe wrapper module over these five FFI calls is ~50 lines. Prefer this over `windows` for compile-time hygiene.

### `winsafe` v0.0.28

- **crates.io:** <https://crates.io/crates/winsafe>
- **Repo:** <https://github.com/rodrigocfd/winsafe> (666 stars, MIT)
- **Downloads:** 40.2M total, 7.6M recent | **Dependents:** 12
- **Platforms:** Windows only
- **API fit:** Safe, idiomatic Rust wrappers over Win32 GUI APIs. Covers `EnumWindows`, `HWND`, `ShowWindow`, window procedures. Reduces unsafe blocks with RAII/safe types.
- **Maintenance:** Active (updated 2026-07-01, 4 open issues). Single maintainer (Rodrigo) -- bus-factor concern.
- **Verdict:** A well-designed safe wrapper if you prefer Rust idioms over raw FFI. The single-maintainer risk is real but the crate has been stable for years. Already in Holon's tree (v0.0.19 transitively via GPUI's Windows backend). Worth considering if you want `HWND` as a safe handle type rather than `isize`.

### `winapi` v0.3.9 (legacy)

- **crates.io:** <https://crates.io/crates/winapi>
- **Downloads:** Historical (declining as ecosystem migrates)
- **Verdict:** DEPRECATED. Archived since ~2021. Development moved to `windows-rs`. Missing modern Win32 APIs. DO NOT USE for new code. Listed for awareness only -- appears in Holon's lock file transitively from old crates.

---

## Linux

### `x11rb` v0.13.2 (X11 -- Pure Rust)

- **crates.io:** <https://crates.io/crates/x11rb>
- **Repo:** <https://github.com/psychon/x11rb> (423 stars, MIT OR Apache-2.0)
- **Downloads:** 48.6M total, 13M recent | **Dependents:** 156
- **Platforms:** Linux X11 only
- **API fit:**
  - Window enumeration: `x11rb::protocol::xproto::QueryTree` -- returns child windows of root. Combine with `GetProperty(_NET_CLIENT_LIST)` for EWMH-compliant window manager's window list.
  - Focus observation: `GetProperty(_NET_ACTIVE_WINDOW)` + `ChangeWindowAttributes` with event mask for `PropertyNotify` on the root window. Or poll `GetInputFocus`.
  - Hide/show: `UnmapWindow` / `MapWindow` for X11-level hide/show. For EWMH-compliant minimize, `SendEvent` with `_NET_WM_STATE` + `_NET_WM_STATE_HIDDEN`.
  - Properties: `GetProperty` / `ChangeProperty` for `_NET_WM_NAME` (title), `_NET_WM_PID` (PID), `WM_CLASS` (app class), `_NET_WM_WINDOW_TYPE`, `_NET_WM_STATE`.
- **Maintenance:** Active (commits 2 days ago: 2026-07-09). Pure Rust implementation (no Xlib C dependency), async-ready, well-tested. The `x11rb-protocol` crate provides the pure-Rust protocol implementation.
- **Verdict:** The best pure-Rust X11 client. Use with `x11rb::protocol::xproto` for raw X11 calls and `x11rb::protocol::ewmh` (via `x11rb::ewmh` module which wraps the EWMH helpers) for desktop-convention-level operations. Preferred over `x11-dl` for new code because it avoids the libX11 C dependency.

### `x11-dl` v2.21.0 (X11 -- Dynamic Loading)

- **crates.io:** <https://crates.io/crates/x11-dl>
- **Repo:** <https://github.com/AltF02/x11-rs> (204 stars, MIT)
- **Downloads:** 48.7M total, 12.9M recent | **Dependents:** 64
- **Platforms:** Linux X11 only
- **API fit:** Dynamic-loading wrapper around libX11, libXrandr, libXcursor, libXxf86vm, libXi. Calls into the system's installed libX11.so.
- **Maintenance:** Low. Last updated 2023-01-18. Still widely used (transitively by many crates) but not receiving feature updates.
- **Verdict:** Superseded by `x11rb` for new code. Use only if you specifically need libX11 rather than the X11 protocol (e.g., for XIM input method, XRender, or extensions x11rb doesn't cover).

### `xcb` v1.7.0 (X11 -- XCB Bindings)

- **crates.io:** <https://crates.io/crates/xcb>
- **Repo:** <https://github.com/rust-x-bindings/rust-xcb> (168 stars, MIT)
- **Downloads:** 5.9M total, 1.1M recent | **Dependents:** 73
- **Platforms:** Linux X11 only
- **API fit:** Safe Rust bindings for the XCB C library (libxcb). Similar scope to x11rb, but x11rb is pure Rust (no C dep), while xcb wraps the C library.
- **Maintenance:** Active (updated 2026-01-03, commits as recent as 2026-07-10).
- **Verdict:** Viable if you prefer the C XCB library over a pure-Rust implementation. x11rb is generally preferred for new pure-Rust projects because it eliminates the C build dependency.

### `wayland-client` v0.31.14 + `wayland-protocols` v0.32.13 + `wayland-protocols-wlr` v0.3.12

- **crates.io:** <https://crates.io/crates/wayland-client> | <https://crates.io/crates/wayland-protocols> | <https://crates.io/crates/wayland-protocols-wlr>
- **Repo:** <https://github.com/smithay/wayland-rs> (1,399 stars, MIT)
- **Downloads:** client: 53.5M | protocols: 58.7M | wlr: 38.4M (all very high)
- **Dependents:** client: 239 | protocols: 148 | wlr: 104
- **Platforms:** Linux Wayland only
- **API fit:**
  - Window enumeration: `wlr-foreign-toplevel-management-unstable-v1` (in wayland-protocols-wlr) gives a list of all top-level windows with title, app_id, state. Also `ext-foreign-toplevel-list-v1` (newer, in wayland-protocols) provides a simpler enumeration-only protocol.
  - Focus observation: The foreign-toplevel-management protocol emits events when the active toplevel changes. No polling needed.
  - Hide/show: `zwlr_foreign_toplevel_handle_v1::set_maximized` / `unset_maximized` / `set_minimized` / `unset_minimized` / `set_fullscreen`. Note: `set_minimized` exists but may not be honored by all compositors. `close()` is also available.
  - Additional: `set_rectangle` for repositioning/resizing surfaces via the compositor.
- **Limitations:** Wayland compositors control what foreign clients can do. Window hide/minimize may be restricted or ignored depending on compositor policy (e.g., GNOME may reject minimize requests from foreign clients). The `ext-foreign-toplevel-list-v1` protocol is simpler and more widely supported.
- **Maintenance:** Extremely active (commits today). The smithay project is the de facto standard for Wayland in Rust.
- **Verdict:** The canonical Wayland window management stack. Use `wayland-protocols-wlr` for `wlr-foreign-toplevel-management-unstable-v1` (most compositors support this) and `wayland-protocols` for `ext-foreign-toplevel-list-v1` (newer, simpler). Paired with `wayland-client` for the connection and `smithay-client-toolkit` for higher-level helpers.

### `smithay-client-toolkit` v0.20.0

- **crates.io:** <https://crates.io/crates/smithay-client-toolkit>
- **Repo:** <https://github.com/smithay/client-toolkit> (424 stars, MIT)
- **Downloads:** 49.6M total, 11.9M recent | **Dependents:** 79
- **Platforms:** Linux Wayland only
- **API fit:** Higher-level toolkit on top of `wayland-client`. Provides compositor connection management, seat/input handling, SHM buffer allocation. Does not directly wrap foreign-toplevel -- you use the raw wayland-protocols-wlr bindings alongside it.
- **Maintenance:** Active (last update 2025-08, ongoing development).
- **Verdict:** Useful for managing the Wayland connection lifecycle (seat, output, registry). Use alongside `wayland-protocols-wlr` (for foreign-toplevel) rather than instead of it.

### `zbus` v5.17.0 (D-Bus -- KWin/GNOME/Desktop Environments)

- **crates.io:** <https://crates.io/crates/zbus>
- **Repo:** <https://github.com/z-galaxy/zbus> (718 stars, MIT)
- **Downloads:** 63.1M total, 16M recent | **Dependents:** 398
- **Platforms:** Linux (any desktop with D-Bus)
- **API fit for KWin Activities and desktop-environment-specific operations:**
  - KWin: D-Bus interface `org.kde.KWin` provides `reconfigure`, scripting, virtual desktop management. KWin Activities (grouping apps into activities) are accessible via `org.kde.ActivityManager`.
  - GNOME Shell: D-Bus interface `org.gnome.Shell` for extensions and window management.
  - The `zbus` crate is the standard Rust D-Bus client. Pure Rust, async (tokio), strongly typed.
- **Maintenance:** Active (commits 3 days ago). v5.x is the current major version with a complete rewrite for improved ergonomics.
- **Verdict:** Essential for desktop-environment-specific window management beyond what X11/Wayland protocols provide. KWin scripting, GNOME Shell extensions, virtual desktop switching, and activity management all go through D-Bus. Not a replacement for x11rb or wayland-client, but a complement for desktop-environment-specific features.

### `penrose` v0.4.0 (X11 Window Manager Library)

- **crates.io:** <https://crates.io/crates/penrose>
- **Repo:** <https://github.com/sminez/penrose> (1,348 stars, MIT)
- **Downloads:** 57.5K total, 2K recent | **Dependents:** 3
- **Platforms:** Linux X11 only
- **API fit:** A tiling window manager library. Provides high-level abstractions for X11 window management: `XConn` trait for X server interaction, window layout algorithms, keybinding handling, status bar. Built on `x11rb`.
- **Maintenance:** Low (last pushed 2026-02-10). More of a "build your own WM" library than a window observation toolkit.
- **Verdict:** Excellent reference architecture for how to structure X11 window management in Rust. Its `XConn` trait and separation of concerns are worth studying. Not a direct dependency (too heavy for just enumeration/control), but its code patterns are valuable.

---

## Summary Table

| Capability | macOS | Windows | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| **Window enumeration** | `core-graphics` (CGWindowList) | `windows-sys` (EnumWindows) | `x11rb` (QueryTree + _NET_CLIENT_LIST) | `wayland-protocols-wlr` (foreign-toplevel-management) |
| **Focus observation** | `objc2-app-kit` (NSWorkspace notifications) | `windows-sys` (SetWinEventHook) | `x11rb` (_NET_ACTIVE_WINDOW + PropertyNotify) | `wayland-protocols-wlr` (foreign-toplevel events) |
| **Hide/show** | `objc2-app-kit` (NSRunningApplication hide/unhide) | `windows-sys` (ShowWindow) | `x11rb` (UnmapWindow / EWMH _NET_WM_STATE) | `wayland-protocols-wlr` (foreign-toplevel set_minimized) |
| **KWin Activities** | N/A | N/A | N/A | `zbus` (org.kde.ActivityManager) |

**Crates already in Holon's dependency tree:** `core-graphics`, `core-foundation`, `objc2`, `objc2-app-kit`, `objc2-foundation`, `windows-sys`, `winsafe` (transitive), `cocoa` (legacy), `x11-dl` (transitive), `xcap` (fork).

**Recommended architecture:**

1. Define a `WindowObserver` trait with `enumerate()` and `observe_focus() -> Stream`.
2. Define a `WindowController` trait with `hide(id)`, `show(id)`, `focus(id)`.
3. Implement per-platform using the "Recommended" crates from the summary table.
4. For Wayland, handle the compositor-policy limitation -- document that hide/minimize may not work on all compositors (GNOME in particular).
5. For cross-platform read-only observation, `xcap` provides a ready-made solution; extend with Wayland support upstream.
