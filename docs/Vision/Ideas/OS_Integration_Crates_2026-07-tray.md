# OS Integration Crates -- Tray Icon / Menu-Bar Presence (2026-07)

Research for persistent "next obligation" glance in the OS status area.

---

## Dynamic Text Support: Platform Reality Check

Before the crate survey, the fundamental platform constraint:

| Platform | Menubar/Status-area text possible? | Mechanism |
|----------|-----------------------------------|-----------|
| **macOS** | YES -- first-class | `NSStatusBarButton.attributedTitle` (styled) or `NSButton.setTitle` (plain). Any text, any color, variable width. |
| **Linux** | SOMETIMES | StatusNotifierItem spec has a `Title` field, but most desktop environments (GNOME Shell, KDE) render it as tooltip/accessibility text, not visible in the panel. KDE may show it. |
| **Windows** | NO | `Shell_NotifyIcon` has `szTip` (hover tooltip, max 128 chars) and `szInfoTitle` (balloon notification title only). No mechanism for persistent text display in the notification area. |

**Bottom line**: A text-only "next obligation" glance is a **macOS-first** feature. On Linux it is desktop-environment-dependent and unreliable. On Windows it does not exist through the notification-area API -- the closest alternative is a taskbar toolbar (a separate window band) or a small always-on-top overlay window.

---

## Cross-Platform Crates

### 1. tray-icon (tauri-apps)

| Field | Value |
|-------|-------|
| **Version** | 0.24.1 (released 2026-06-10) |
| **Downloads** | 19.2M total / 8.2M recent |
| **License** | MIT OR Apache-2.0 |
| **Repository** | https://github.com/tauri-apps/tray-icon |
| **Stars/Forks** | 388 / 90 |
| **Open issues** | 29 |

**Platform coverage**:
- macOS: NSStatusItem (requires main-thread event loop)
- Windows: Shell_NotifyIcon + win32 event loop
- Linux: libappindicator3 or libayatana-appindicator3 + GTK event loop

**API fit for dynamic text**:
- `set_title(Some("text"))` -- **supported on macOS and Linux, UNSUPPORTED on Windows**
- On macOS: text renders in the menu bar, variable width, no icon required
- On Linux: title not shown unless there is also an icon; suited for "numerical and frequently updated information"
- `set_tooltip()` -- macOS and Windows (unsupported on Linux)
- Full menu support via `muda` crate
- Click handling via `TrayIconEvent::receiver()` channel or winit integration
- `set_icon_as_template(true)` for macOS template images

**Verdict**: The clear front-runner for cross-platform. Actively maintained by the Tauri team with releases every few weeks. The Windows `set_title` gap is a platform limitation (Windows has no menubar-text concept), not a crate bug. For a text-first "obligation glance," this works on macOS today; on Linux it's DE-dependent; on Windows you will need a fallback (tooltip or a separate widget).

---

### 2. tray (nobane/tray-rs)

| Field | Value |
|-------|-------|
| **Version** | 0.1.2 |
| **Downloads** | 760 total / 368 recent |
| **License** | MIT |
| **Repository** | https://github.com/nobane/tray-rs |
| **Stars/Forks** | 8 / 0 |
| **Commits** | 2 total |

**Platform coverage**:
- macOS: NSStatusItem
- Windows: Shell_NotifyIconW
- Linux: X11 system tray protocol (NO appindicator dependency -- uses raw X11)

**API fit for dynamic text**:
- Icon-only. Builder accepts RGBA pixel data for the icon; no `set_title` equivalent found in API.
- Tooltip via `.with_tooltip("My App")`
- Cross-window popup menus (GTK/Qt on Linux, TrackPopupMenu on Windows, NSMenu on macOS)
- egui and iced integration
- Thread-safe Send+Sync TrayIcon
- Full event support: click, double-click, enter, leave, move

**Verdict**: Architecturally interesting (native X11 on Linux avoids the appindicator C dependency), but far too early for production -- 2 commits, 760 downloads, zero releases. No text-title support. Check back in 6-12 months.

---

### 3. tray-item (olback)

| Field | Value |
|-------|-------|
| **Version** | 0.10.0 |
| **Downloads** | 157K total / 15K recent |
| **License** | MIT |
| **Repository** | https://github.com/olback/tray-item-rs |
| **Stars/Forks** | 328 / 51 |
| **Open issues** | 13 (plus 5 PRs) |
| **Commits** | 101 |
| **Tags** | 8 |

**Platform coverage**:
- macOS: Cocoa/NSStatusItem
- Windows: winapi/Shell_NotifyIcon
- Linux: ksni (D-Bus, default) or libappindicator (opt-in via `libappindicator` feature)

**API fit for dynamic text**:
- Supports text titles on macOS (via NSStatusBarButton)
- On Linux: no explicit text-title API surfaced; depends on ksni backend's `title()` trait method
- Simple builder API: `TrayItem::new("label", Icon::None)`
- On macOS: cannot run on non-main thread (Cocoa requirement)

**Verdict**: A solid, mature alternative with a simpler API than tray-icon. The `ksni` default on Linux means no C library dependencies there (pure Rust D-Bus). Good for straightforward use cases. Less actively maintained than tray-icon (8 tags vs 65 versions), but battle-tested with decent download count.

---

## Linux-Specific Crates

### 4. ksni

| Field | Value |
|-------|-------|
| **Version** | 0.3.5 |
| **Downloads** | 1.0M total / 294K recent |
| **License** | Unlicense (public domain) |
| **Repository** | https://github.com/iovxw/ksni |
| **Stars/Forks** | 144 / 20 |
| **Open issues** | 1 |
| **Commits** | 263 |
| **Tags** | 11 |

**Platform coverage**: Linux only (freedesktop StatusNotifierItem over D-Bus via zbus)

**API fit for dynamic text**:
- Trait-based: implement `ksni::Tray` with `id()`, `icon_name()`, `title()`, `menu()`
- `title()` returns a string -- but visibility in panel is **DE-dependent** (GNOME with AppIndicator extension shows only the icon; KDE may show the title)
- Menu support via trait method returning `Vec<MenuItem<T>>`
- Async-first (tokio or async-io via features)
- Optional `blocking` feature for synchronous use

**Verdict**: THE standard choice for Linux tray work. Pure Rust (no C deps), D-Bus native, used by 47+ other crates. The title-is-DE-dependent caveat is inherent to the freedesktop spec, not a ksni bug. Pairs well as the Linux backend inside a cross-platform crate (tray-item uses it this way).

---

### 5. appindicator3

| Field | Value |
|-------|-------|
| **Version** | 0.3.0 |
| **Downloads** | 11K total / 375 recent |
| **License** | MIT |
| **Repository** | https://github.com/rehar/appindicator3 |

**Platform coverage**: Linux only (libappindicator3 or libayatana-appindicator3)

**API fit**: Direct C bindings to AppIndicator/AyatanaAppIndicator. Provides `AppIndicator::new(id, icon, category)` with set_status/set_menu/set_title. The `set_title` method exists but, like ksni, actual visibility is DE-dependent.

**Verdict**: Niche. Only useful as a dependency for other crates (tray-icon uses the C library directly). Not a crate you would depend on directly for application-level work.

---

## macOS-Specific Crates

### 6. objc2-app-kit

| Field | Value |
|-------|-------|
| **Version** | 0.3.2 |
| **Downloads** | 39M total / 15M recent |
| **License** | Zlib OR Apache-2.0 OR MIT |
| **Repository** | https://github.com/madsmtm/objc2 |

**Platform coverage**: macOS only -- official Apple framework bindings

**API fit for dynamic text**:
- Full access to `NSStatusBar`, `NSStatusItem`, `NSStatusBarButton`
- `button.setTitle()` for plain text
- `button.setAttributedTitle()` with `NSAttributedString` for styled text (colors, fonts, etc.)
- Variable width via `NSStatusItem.variableLength`
- Complete control: click handling, drag, custom views, everything AppKit allows

**Verdict**: If you need rich styled text in the macOS menu bar (e.g., color-coded priority indicators, countdown timers with formatting), this is the only path. All higher-level crates eventually call these same AppKit methods, but may not expose `setAttributedTitle`. Use this directly if you are macOS-only, or as a complement to tray-icon if you need richer text than `set_title` provides.

---

### 7. system_status_bar_macos

| Field | Value |
|-------|-------|
| **Version** | 0.1.3 |
| **Downloads** | 6.8K total / 381 recent |
| **License** | MIT OR Apache-2.0 |
| **Repository** | https://github.com/amachang/system_status_bar_macos |

**Platform coverage**: macOS only. Thin wrapper around `[NSStatusBar systemStatusBar]`.

**API fit**: Minimal -- creates a status item with an icon. Not enough API surface documented to assess text title support.

**Verdict**: Too minimal. Prefer `tray-icon` for simplicity or `objc2-app-kit` for full control.

---

## Archived / Not Recommended

### 8. trayicon (Ciantic)

| Field | Value |
|-------|-------|
| **Version** | 0.4.1 |
| **Downloads** | 54K total / 1.9K recent |
| **License** | MIT |
| **Repository** | https://github.com/ciantic/trayicon-rs |

**Assessment**: Author targets Windows and KDE. macOS support added by a contributor in 0.3.0, but the author only verifies it compiles via `cargo check` -- never tests on real macOS. Changelog last entry is 2026-01-12. No text-title support. **Do not use for a macOS-first feature.**

### 9. systray (qdot)

| Field | Value |
|-------|-------|
| **Version** | 0.4.0 |
| **Downloads** | 37K total / 1.6K recent |
| **License** | BSD-3-Clause |

**Assessment**: Linux GTK and Win32 work, but the Cocoa (macOS) backend was never completed. Cross-platform in name only. **Do not use.**

### 10. sysbar

| Field | Value |
|-------|-------|
| **Version** | 0.3.0 |
| **Downloads** | 8.3K total |
| **License** | MIT/Apache-2.0 |

**Assessment**: macOS-only, fork of `rs-barfly`. 74 stars, 7 forks, minimal maintenance. **Do not use.**

### 11. tray-icon-win

| Field | Value |
|-------|-------|
| **Version** | 0.1.5 |
| **Downloads** | 5.1K total |
| **License** | MIT OR Apache-2.0 |

**Assessment**: Windows-only fork of tray-icon. Only useful if you want a Windows-specific tray crate without pulling in GTK/appindicator deps. Since `set_title` is unsupported on Windows anyway, this doesn't help for the text-glance goal.

---

## Recommendation

### For "next obligation" text glance (the primary use case)

**macOS (primary target)**: Use `tray-icon` with `set_title()` for plain text. If you need styled/colored text (priority color-coding, countdown formatting), drop down to `objc2-app-kit` for the macOS backend while keeping `tray-icon` for the other platforms.

**Linux**: Use `tray-icon`. Accept that text visibility is DE-dependent -- show the obligation title as a tooltip as well. The `ksni` backend (used transitively by `tray-item`) has the same limitation.

**Windows**: `Shell_NotifyIcon` has no persistent-text mechanism. Options:
1. **Tooltip-only** -- set a hover tooltip with the obligation (tray-icon's `set_tooltip` works on Windows)
2. **Taskbar toolbar** -- create a small always-visible toolbar band on the taskbar (separate Win32 API, no Rust crate found; would need custom win32 API calls)
3. **Small overlay window** -- an always-on-top borderless window near the system tray. Hacky but flexible.

### Crate stack suggestion

```
tray-icon        (cross-platform: icon + menu + click + tooltip + macOS/Linux text)
  └─ objc2-app-kit  (macOS only: rich attributed-string titles when needed)
  └─ ksni           (alternative Linux backend if you want pure Rust)
```

The `tray-icon` crate is clearly the best starting point. It is actively maintained, widely used (19M downloads), has the most complete platform support, and its `set_title` API maps directly to the text-glance use case on macOS.

---

## Sources

- [tray-icon on crates.io](https://crates.io/crates/tray-icon) -- v0.24.1, 19.2M downloads
- [tray-icon on GitHub](https://github.com/tauri-apps/tray-icon) -- 388 stars, releases through 2026-06
- [tray-icon docs (TrayIcon::set_title)](https://docs.rs/tray-icon/latest/tray_icon/struct.TrayIcon.html)
- [tray on crates.io](https://crates.io/crates/tray) -- v0.1.2, MIT
- [tray on GitHub](https://github.com/nobane/tray-rs)
- [tray-item on crates.io](https://crates.io/crates/tray-item) -- v0.10.0, 157K downloads
- [tray-item on GitHub](https://github.com/olback/tray-item-rs)
- [ksni on crates.io](https://crates.io/crates/ksni) -- v0.3.5, 1.0M downloads
- [ksni on GitHub](https://github.com/iovxw/ksni)
- [appindicator3 on crates.io](https://crates.io/crates/appindicator3)
- [objc2-app-kit on crates.io](https://crates.io/crates/objc2-app-kit) -- v0.3.2, 39M downloads
- [system_status_bar_macos on crates.io](https://crates.io/crates/system_status_bar_macos)
- [trayicon on crates.io](https://crates.io/crates/trayicon)
- [systray on crates.io](https://crates.io/crates/systray)
- [tray-icon-win on crates.io](https://crates.io/crates/tray-icon-win)
- [sysbar on GitHub](https://github.com/rust-sysbar/rust-sysbar)
- [Tauri tray API (set_title reference)](https://tauri.app/reference/javascript/api/namespacetray/)
- [Multi.app blog: Pushing the limits of NSStatusItem](https://multi.app/blog/pushing-the-limits-nsstatusitem)
- [NOTIFYICONDATA on Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw)
