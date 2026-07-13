# OS Integration Crates: Screen & Window Capture for Live Thumbnails

> Research date: 2026-07-13
> Purpose: Evaluate Rust crates for live window thumbnails in a task switcher (garnish-tier capability; must degrade gracefully).
> Context: See `docs/Vision/Ideas/OS_Integration_Research_2026-07-13.md` for the broader OS-integration landscape.

---

## Summary Matrix

| Crate | Version | All-Time DL | Recent DL | macOS | Win | Linux | Per-Window | Video Stream | License | Health |
|-------|---------|-------------|-----------|-------|-----|-------|------------|--------------|---------|--------|
| **screencapturekit** | 8.0.0 | 834k | 400k | yes | -- | -- | yes | yes | MIT/Apache-2.0 | Excellent |
| **windows-capture** | 2.0.0 | 487k | 289k | -- | yes | -- | yes | yes | MIT | Good |
| **scap** | 0.1.0-beta.1 | 29k | 5k | yes | yes | yes | yes | yes | MIT | Active (beta) |
| **xcap** | 0.9.6 | 1,141k | 524k | yes | yes | yes | yes | WIP (recording) | Apache-2.0 | Excellent |
| **ashpd** | 0.13.12 | 11.0M | 2.7M | -- | -- | yes | via portal | via pipewire | MIT | Excellent |
| **pipewire** | 0.10.0 | 1,068k | 507k | -- | -- | yes | via portal | yes | MIT | Good |
| **libwayshot** | 0.8.0 | 217k | 76k | -- | -- | wlroots | output only | no (stills) | BSD-2-Clause | Active |

---

## macOS

### screencapturekit (doom-fish/screencapturekit-rs)

- **Crate:** [screencapturekit](https://crates.io/crates/screencapturekit) v8.0.0
- **Repo:** [github.com/doom-fish/screencapturekit-rs](https://github.com/doom-fish/screencapturekit-rs) -- 205 stars, 43 forks
- **Maintainer:** Per Johansson (solo), very responsive; 45 releases, 23 runnable examples
- **Platform:** macOS 12.3+ (feature flags per macOS version: `macos_13_0` through `macos_26_0`, cumulative)
- **Downloads:** 834k total, ~400k recent (last 90 days)
- **API fit:** Excellent. Builder-pattern API covers screens, windows, and individual apps. Zero-copy frame delivery via IOSurface/Metal. Async support (waker-based, executor-agnostic). Also supports system audio + microphone capture (macOS 13+/15+), direct-to-file recording (macOS 14+/15+). Per-window capture is first-class. You get `SCStream` with per-frame callbacks delivering BGRA/pixel buffers -- exactly what a live thumbnail grid needs.
- **Dependencies:** Minimal -- thin `apple-cf` / `apple-metal` binding crates only. No heavyweight frameworks.
- **macOS 15+ TCC caveat:** Starting with Sequoia (macOS 15), ScreenCaptureKit triggers a **monthly re-prompt** dialog ("[App] is requesting to bypass the system private window picker..."). This is a user-facing nag that cannot be suppressed without MDM. Apple's preferred path is `SCContentSharingPicker` (the FaceTime-style picker), though that does not fit a background task-switcher use case. For a garnish-tier feature, graceful degradation means: if permission is denied or expired, fall back to static window titles/icons with no thumbnail. The `Persistent Content Capture` entitlement exists but is undocumented and not generally available.
- **License:** MIT OR Apache-2.0
- **Verdict:** The clear winner for macOS. Actively maintained (v8.0.0 released 2026-06-19), production-grade, minimal dependency footprint. The TCC monthly re-prompt is a UX annoyance but unavoidable for any ScreenCaptureKit user. Degrade gracefully when permission is absent.

### screen-capture-kit (rust-media/apple-media-rs)

- **Crate:** [screen-capture-kit](https://crates.io/crates/screen-capture-kit) v0.6.1
- **Maintainer:** Zhou Wei, part of `rust-media/apple-media-rs`
- **Verdict:** Alternative SCK bindings. Much less mature than `screencapturekit` (fewer releases, smaller API surface, lower download count). Not recommended -- use `screencapturekit` instead.

---

## Windows

### windows-capture (NiiightmareXD/windows-capture)

- **Crate:** [windows-capture](https://crates.io/crates/windows-capture) v2.0.0
- **Repo:** [github.com/NiiightmareXD/windows-capture](https://github.com/NiiightmareXD/windows-capture)
- **Platform:** Windows 10+ (uses Windows.Graphics.Capture API)
- **Downloads:** 487k total, ~289k recent
- **API fit:** Good for pixel-access capture. Captures windows or monitors via `GraphicsCaptureItem`. Configurable FPS, cursor visibility, crop rects. Provides `Frame` structs with raw pixel data. The 2.0.0 release (2026-04-14) was a major rework. Per-window capture is supported. Also has Python bindings.
- **Known issues:** `BorderConfigUnsupported` panic on Windows 10 without border rendering support. Several open bug reports as of 2026 (issues #189, #187, #186). The crate is maintained by a solo developer, and the issue tracker has some stale reports.
- **License:** MIT
- **Verdict:** The best Rust-native option for pixel-access window capture on Windows. Solid API, active enough (v2.0.0 landed April 2026). However, for a task-switcher thumbnail grid where you only need _display_ (not pixel access for processing), DWM thumbnails may be a better fit.

### DWM Thumbnails via the `windows` crate (Microsoft)

- **Crate:** [windows](https://crates.io/crates/windows) v0.62+ (Microsoft's official Rust projection)
- **API:** `DwmRegisterThumbnail`, `DwmUpdateThumbnailProperties`, `DwmUnregisterThumbnail` in `windows::Win32::Graphics::Dwm`
- **Docs:** [DwmRegisterThumbnail](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Dwm/fn.DwmRegisterThumbnail.html)
- **Fit for task switcher:** This is purpose-built for exactly this use case. DWM thumbnails are:
  - **Display-only** -- the DWM composites a live thumbnail of the target window into your HWND. No pixel buffers, no frame processing. You just specify source HWND, destination HWND, and a destination rect.
  - **No yellow border** -- unlike Windows.Graphics.Capture, DWM thumbnails do NOT draw the yellow capture border around the source window.
  - **Live** -- the thumbnail updates automatically as the source window changes. No polling, no frame callbacks.
  - **No permission prompt** -- DWM thumbnails do not trigger the screen capture permission dialog.
  - **Limitation** -- only works for non-minimized windows. Minimized windows show a static thumbnail (last frame before minimize).
- **Verdict:** If the task switcher is rendering into its own window (GPUI, egui, etc.), DWM thumbnails can be composited into a child region. This is exactly what Windows' own Alt+Tab and Task View use. Requires a native window handle (HWND) for the destination, which means the GPUI window must expose its HWND. For a garnish-tier switcher, this is the lowest-friction, lowest-overhead path on Windows.

### win-screenshot

- **Crate:** [win-screenshot](https://crates.io/crates/win-screenshot)
- **Verdict:** Simple screenshot crate (stills only, no streaming). Not suitable for live thumbnails.

---

## Cross-Platform

### xcap (nashaofu/xcap)

- **Crate:** [xcap](https://crates.io/crates/xcap) v0.9.6
- **Repo:** [github.com/nashaofu/xcap](https://github.com/nashaofu/xcap) -- 994 stars, 139 forks, 176 commits
- **Platform:** Linux (X11, Wayland), macOS, Windows, HarmonyOS/OHOS
- **Downloads:** 1,141k total, ~524k recent -- the most-downloaded screen-capture crate by far
- **API fit:** Screenshot-focused with WIP video recording. `Monitor::all()` and `Window::all()` enumerate displays and windows. `capture_image()` returns a static image -- good for periodic thumbnail refresh, but not designed for continuous 30fps streaming. Video recording is marked WIP on all platforms. The successor to the older `screenshots` crate.
- **macOS:** Uses Core Graphics (CGWindowListCreateImage) -- NOT ScreenCaptureKit. This means no TCC monthly re-prompt, but also no hardware-accelerated capture and potentially lower performance.
- **Windows:** Uses GDI/DXGI for screenshots.
- **Linux:** X11 via XGetImage; Wayland support listed as "not fully supported in some special scenarios" (likely uses wlr-screencopy or portal depending on compositor).
- **Dependencies:** Moderate -- `image`, `objc2-*` on macOS, X11/Wayland libs on Linux.
- **License:** Apache-2.0
- **Verdict:** Excellent for static screenshots. Good download numbers and active maintenance (v0.9.6 released 2026-05-24). For live thumbnails, the still-image-only API means you would poll `capture_image()` on a timer -- workable at 2-5 FPS for a switcher grid but not a real video stream. The avoidance of SCK on macOS (no TCC nag) is a significant UX advantage for a garnish feature.

### scap (CapSoftware/scap)

- **Crate:** [scap](https://crates.io/crates/scap) v0.1.0-beta.1
- **Repo:** [github.com/CapSoftware/scap](https://github.com/CapSoftware/scap) -- 623 stars, 133 forks, 453 commits
- **Platform:** macOS (ScreenCaptureKit), Windows (Windows.Graphics.Capture), Linux (PipeWire)
- **Downloads:** 29k total, ~5k recent
- **API fit:** The most ambitious cross-platform capture library. `Capturer` with configurable `Options` (FPS, cursor visibility, output resolution, frame type like BGRAFrame, source rect). Per-window and per-monitor capture. `get_all_targets()` enumerates capturable sources. Proper streaming architecture with frame callbacks -- designed for video, not stills.
- **Backed by:** CapSoftware, the team behind [Cap](https://github.com/CapSoftware/Cap) (open-source Loom alternative, 20k stars). The library is extracted from a production screen-recording app.
- **macOS caveat:** Uses ScreenCaptureKit, so subject to the same TCC monthly re-prompt as `screencapturekit`.
- **Maturity:** Still beta (0.1.0-beta.1 released 2025-08-04). The `zed-scap` fork (used by the Zed editor) has more downloads (117k) suggesting real-world usage. Recent CI activity (June 2026) shows ongoing maintenance.
- **License:** MIT
- **Verdict:** The most architecturally sound cross-platform option for continuous capture. If it matures past beta, it could be the single dependency covering all three platforms. Currently beta-quality -- expect API churn and platform-specific edge cases. Worth watching but not yet safe to depend on for production. The Zed editor's adoption via `zed-scap` fork is a positive signal.

---

## Linux

### ashpd + pipewire (XDG Desktop Portal + PipeWire)

- **Crates:** [ashpd](https://crates.io/crates/ashpd) v0.13.12 + [pipewire](https://crates.io/crates/pipewire) v0.10.0
- **Repo ashpd:** [github.com/bilelmoussaoui/ashpd](https://github.com/bilelmoussaoui/ashpd) -- 359 stars, 69 forks
- **Repo pipewire:** [gitlab.freedesktop.org/pipewire/pipewire-rs](https://gitlab.freedesktop.org/pipewire/pipewire-rs)
- **Maintainer ashpd:** Bilal Elmoussaoui (Red Hat), very active (64 releases, latest 2026-06-25)
- **Platform:** Linux (any desktop with XDG Portal support -- GNOME, KDE, wlroots compositors with xdg-desktop-portal-wlr)
- **Downloads ashpd:** 11.0M total (used by many non-capture applications for other portals)
- **Downloads pipewire:** 1,068k total
- **API fit:** The ScreenCast portal is the standard Linux path for screen/window capture:
  1. `ashpd::desktop::screencast::Screencast` -- create session, select sources (monitor or window, multiple allowed), start streaming.
  2. Each stream yields a `pipe_wire_node_id` + size/position.
  3. Hand the node ID to `pipewire` crate for consuming the video stream (DMA-BUF or MemFd buffers).
  4. ashpd repo includes example code for both raw PipeWire consumption (`screen_cast_pw.rs`) and GStreamer pipeline (`screen_cast_gstreamer.rs`).
  - Supports per-window capture via `SourceType::Window`.
  - `PersistMode::DoNot` means no persistent permission -- user must approve each session. `PersistMode::Transient` or `PersistMode::Persistent` can reduce prompts.
- **Compositor support:** Works on GNOME (Mutter), KDE (KWin), and any wlroots compositor with `xdg-desktop-portal-wlr` installed. The portal abstracts away compositor differences.
- **Dependencies:** ashpd depends on `zbus` (D-Bus), optional `pipewire`, optional `tokio`/`async-io`. pipewire crate depends on `pipewire-sys` (FFI bindings to libpipewire).
- **Caveat:** The pipewire crate is single-threaded (objects do not implement `Send`/`Sync`) -- you spawn a `MainLoop` in a dedicated thread and communicate via channels.
- **License:** Both MIT
- **Verdict:** The correct Linux path. The portal + PipeWire combination is the cross-compositor standard (replaces the old wlr-screencopy protocol, which is now deprecated). ashpd is very actively maintained by a Red Hat engineer. Complexity is moderate -- you need to bridge the D-Bus portal session to PipeWire stream consumption. For a garnish-tier feature, you could also consider falling back to xcap's `Monitor::capture_image()` for periodic still grabs if PipeWire integration is too heavy.

### libwayshot (Waycrate/wayshot)

- **Crate:** [libwayshot](https://crates.io/crates/libwayshot) v0.8.0
- **Repo:** [github.com/waycrate/wayshot](https://github.com/waycrate/wayshot)
- **Platform:** wlroots compositors only (Sway, Hyprland, River, etc.) via `zwlr_screencopy_v1`
- **Downloads:** 217k total, ~76k recent
- **API fit:** Still-image screenshots of outputs (not individual windows). Simple API: `WayshotConnection::new()` + `screenshot_all()`. No video streaming, no per-window capture.
- **Verdict:** Not suitable for a task switcher -- output-only (no per-window), stills-only, wlroots-specific. The protocol it uses (`zwlr_screencopy_v1`) is deprecated in favor of `ext-image-copy-capture-v1`.

### wlr-utils / ext-image-copy-capture-v1

- **Repo:** [github.com/sjourdois/wlr-utils](https://github.com/sjourdois/wlr-utils)
- **Protocol:** `ext-image-copy-capture-v1` (merged into wayland-protocols, replaces deprecated `zwlr_screencopy_v1`)
- **Features:** Zero-copy DMA-BUF capture, CPU SHM fallback, supports occluded and off-workspace windows. Includes a library crate (`wlr-capture`).
- **Caveat:** Not published on crates.io as a standalone library crate. Primarily a set of CLI tools (`wlr-shot`, `wlr-cap`). The capture engine is embedded in the tools.
- **Verdict:** Interesting for the per-window capability (including occluded windows), but not packaged as a reusable library crate. The protocol itself is the future of wlroots capture, but for a Rust library you would likely use ashpd + pipewire which abstracts over the portal (which in turn uses ext-image-copy-capture-v1 on wlroots compositors).

---

## Recommended Approach Per Platform

### macOS

| Priority | Crate | Rationale |
|----------|-------|-----------|
| **Primary** | `screencapturekit` v8.0.0 | Best API, per-window streaming, hardware-accelerated. Accept the TCC monthly re-prompt; degrade to no thumbnail when permission absent. |
| **Fallback** | `xcap` v0.9.6 | Uses CGWindowListCreateImage (no TCC nag), but stills only. Poll at 2-5 FPS. |

### Windows

| Priority | Crate | Rationale |
|----------|-------|-----------|
| **Primary** | DWM thumbnails via `windows` crate | Purpose-built for task switchers. No permission prompts, no yellow border, live compositing. Requires HWND access from the GPUI window. |
| **Alternative** | `windows-capture` v2.0.0 | If you need pixel access (e.g., for custom compositing or cross-platform uniform rendering). Has permission prompts and yellow border. |

### Linux

| Priority | Crate | Rationale |
|----------|-------|-----------|
| **Primary** | `ashpd` v0.13.12 + `pipewire` v0.10.0 | Cross-compositor standard via XDG ScreenCast portal. Per-window capture, hardware-accelerated DMA-BUF. |
| **Fallback** | `xcap` v0.9.6 | Stills-only, simpler integration. Works on X11; Wayland support is partial. |

### Cross-Platform One-Crate Option

`scap` v0.1.0-beta.1 is the most architecturally sound cross-platform option (covers all three platforms with native backends), but is still beta. Watch for a stable release. Until then, per-platform crates are safer.

---

## Key Design Considerations for a Garnish-Tier Feature

1. **Degrade, never block.** If capture permission is denied, unavailable, or fails at runtime, fall back to static window titles and app icons. The feature must never prevent the task switcher from working.

2. **TCC monthly re-prompt (macOS).** Cannot be avoided with ScreenCaptureKit. Either accept it (and degrade when permission lapses) or use the `xcap`/CGWindowListCreateImage fallback which avoids SCK entirely at the cost of stills-only capture.

3. **DWM thumbnails are display-only (Windows).** You get a live composited thumbnail region but cannot read pixels back. This is fine for a switcher grid (you just want to show what the window looks like) but means you cannot do custom processing (rounded corners, overlays, uniform scaling). Those would need `windows-capture` instead.

4. **Linux portal session persistence.** `PersistMode::DoNot` means the user sees a portal dialog every time. `PersistMode::Transient` or `PersistMode::Persistent` (if the compositor supports it) can make this a one-time grant. Test on target compositors (GNOME, KDE, Hyprland).

5. **Thumbnail refresh rate.** For a switcher grid, 2-5 FPS per thumbnail is sufficient (the user glances at thumbnails to identify windows, not to read content). This opens the door to polling-based approaches (xcap screenshots) rather than requiring continuous video streams.

6. **No single crate covers all three platforms well today.** `scap` aims to but is beta. `xcap` covers all three but is stills-only. The pragmatic path is per-platform crates with a common abstraction trait.
