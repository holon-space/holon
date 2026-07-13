# OS Integration Crates: Global Shortcuts + Launcher Plumbing

Research date: 2026-07-13. All download counts are approximate and sourced from crates.io at time of writing.

---

## 1. Global Hotkeys

### `global-hotkey` (0.8.0)

- **Crate / Source:** [crates.io/crates/global-hotkey](https://crates.io/crates/global-hotkey) | [github.com/tauri-apps/global-hotkey](https://github.com/tauri-apps/global-hotkey)
- **Downloads:** ~3.6M total, ~1.6M recent
- **Dependents:** Used in 114 crates
- **License:** Apache-2.0 OR MIT
- **Platform coverage:**
  - **macOS:** Carbon/CGEvent-based. Requires event loop on the main thread. Accessibility permissions needed for some shortcuts.
  - **Windows:** `RegisterHotKey` via `windows-sys`. Needs a win32 event loop on the registering thread (not necessarily main).
  - **Linux:** X11 only (via `x11rb` + `xkeysym`). No Wayland support.
- **API fit:** Clean builder API — `GlobalHotKeyManager::new()`, `HotKey::new(modifiers, code)`, `manager.register(hotkey)`, and a `GlobalHotKeyEvent::receiver()` crossbeam channel. Supports string parsing (`"shift+alt+KeyQ"`), bitflag modifiers (ALT, CONTROL, META, SHIFT), and `id()` / `matches()` helpers. Threading model differs per platform (main-thread on macOS, any event-loop thread on Windows).
- **Health:** Active. Latest release 2026-05-01 (v0.8.0). Tauri team maintained. 256 stars, 46 forks. 34 open issues, 19 open PRs — moderate backlog. Automated Renovate dependency management.
- **Verdict:** The de facto standard for desktop Rust hotkeys. Tauri-ecosystem backing gives it longevity. The X11-only Linux limitation is a real gap for Wayland users, and the per-platform threading requirements add complexity for abstraction. If you can live with X11-only Linux (or accept degraded Wayland), this is the safest bet.

### `handy-keys` (0.3.0)

- **Crate / Source:** [crates.io/crates/handy-keys](https://crates.io/crates/handy-keys) | [github.com/handy-computer/handy-keys](https://github.com/handy-computer/handy-keys)
- **Downloads:** ~94K total, ~84K recent
- **Dependents:** Few (niche, newer crate)
- **License:** MIT
- **Platform coverage:**
  - **macOS:** Accessibility API (helper functions: `check_accessibility`, `open_accessibility_settings`).
  - **Windows:** Low-level keyboard hook.
  - **Linux:** Uses `rdev` under the hood. Wayland: hotkey blocking may not work due to compositor restrictions.
- **API fit:** Richer feature set than `global-hotkey`. Supports hotkey blocking (prevents registered shortcuts from reaching other apps), modifier-only hotkeys (Cmd+Shift without a key), string parsing, a hotkey recording mode for UI flows, and Serde derive on all types. ~4,500 lines of code — larger surface area.
- **Health:** Active. Latest release 2026-07-07 (v0.3.0). 16 stars. Small team (handy-computer org). 11 versions since 2026-01, rapid iteration.
- **Verdict:** The most feature-complete option if you need hotkey blocking or modifier-only shortcuts. Smaller community and fewer battle-tests than `global-hotkey`. The `rdev`-based implementation means Wayland support is limited to listening (no blocking). Good choice for an app that owns its shortcuts and wants blocking plus recording UX.

### `hotkey-listener` (0.3.2)

- **Crate / Source:** [crates.io/crates/hotkey-listener](https://crates.io/crates/hotkey-listener) | [github.com/martintrojer/hotkey-listener](https://github.com/martintrojer/hotkey-listener)
- **Downloads:** ~1,100 total
- **Dependents:** Essentially none
- **License:** MIT
- **Platform coverage:**
  - **Linux:** evdev (`/dev/input`) — works on **both X11 and Wayland**. This is the crate's key differentiator. User needs read permission on event devices.
  - **macOS:** Uses `rdev::listen()` — receives all keyboard events system-wide. Listener thread cannot be interrupted once started; terminates only at process exit.
  - **Windows:** Not explicitly covered (no Windows implementation mentioned in docs).
- **API fit:** Minimal — a push-to-talk style pressed/released callback. Automatic keyboard reconnection on USB plug/unplug. Unified API across supported platforms.
- **Health:** Very small. 5 versions since 2026-01-29. Single maintainer (Martin Trojer). No visible GitHub stars/forks in fetch data.
- **Verdict:** The only Rust crate with native Wayland hotkey support via evdev. This matters if you need Wayland coverage and don't want to go through the XDG GlobalShortcuts portal. However, it is essentially a one-person project with negligible adoption, no Windows support, and a macOS limitation (non-interruptible listener). Only consider this if Wayland without portals is a hard requirement and you're willing to contribute maintenance.

### `tauri-plugin-global-shortcut` (2.3.2)

- **Crate / Source:** [crates.io/crates/tauri-plugin-global-shortcut](https://crates.io/crates/tauri-plugin-global-shortcut) | [github.com/tauri-apps/plugins-workspace](https://github.com/tauri-apps/plugins-workspace)
- **Downloads:** ~2.47M total, ~1.19M recent
- **License:** Apache-2.0 OR MIT
- **Verdict:** Tauri plugin wrapping the `global-hotkey` crate. If you're building a Tauri app, use this. If you're not on Tauri, use `global-hotkey` directly. Same platform limitations (X11-only Linux).

---

## 2. URL-Scheme / Deep-Link Registration

### `tauri-plugin-deep-link` (2.4.9)

- **Crate / Source:** [crates.io/crates/tauri-plugin-deep-link](https://crates.io/crates/tauri-plugin-deep-link) | [github.com/tauri-apps/plugins-workspace](https://github.com/tauri-apps/plugins-workspace)
- **Downloads:** ~3.73M total, ~1.47M recent
- **License:** Apache-2.0 OR MIT
- **Platform coverage:**
  - **macOS / iOS / Android:** Full event-based handling — plugin emits events with URL payload when scheme is invoked.
  - **Windows / Linux:** OS spawns a new process instance with the URL as a CLI argument. Combine with Tauri's single-instance plugin for consistent behavior.
- **API fit:** Tauri-specific. Register schemes in a declarative manifest. On supported platforms, receive a structured event with the URL. On Windows/Linux, parse `std::env::args()`.
- **Verdict:** The most polished deep-link solution in the Rust ecosystem — but only if you're on Tauri. If not, the per-OS registration is thin enough that manual plumbing via build scripts and platform-specific files (Info.plist on macOS, registry writes on Windows, `.desktop` with `%u` on Linux) is practical and avoids the Tauri dependency.

### `robius-url-handler` (unpublished / 0.x)

- **Crate / Source:** [crates.io](https://crates.io) (may not be published; note: 404 on crates.io API) | [github.com/project-robius/robius-url-handler](https://github.com/project-robius/robius-url-handler)
- **Downloads:** Not available (unpublished or zero)
- **License:** No license file in repo
- **Platform coverage:** Claims Linux, macOS, Windows support. Used in Robrix (Makepad-based Matrix client) for `matrix:` scheme handling.
- **API fit:** Register as default handler for a URL scheme, then receive incoming URLs. Sibling crate `robius-open` (0.1.2, 20K downloads) handles the outbound direction (opening URIs across platforms).
- **Health:** Very low. 3 stars, 7 commits, no releases, no license file. The Robius project has archived several of its repos (as of mid-2025). This crate may be effectively dormant.
- **Verdict:** Conceptually right but practically too immature and likely abandoned. The idea of a cross-platform URL handler abstraction is sound, but this isn't a dependable dependency today.

### `system_uri` (ARCHIVED — maidsafe-archive)

- **Crate / Source:** [docs.rs/system_uri](https://docs.rs/system_uri) | [github.com/maidsafe-archive/system_uri](https://github.com/maidsafe-archive/system_uri)
- **Downloads:** Low (archived, legacy)
- **License:** Modified BSD OR MIT
- **Platform coverage:** macOS (Info.plist / LSSetDefaultHandler), Windows (HKCR), Linux (`.desktop` files / `xdg-mime`). Did the heavy lifting of per-OS plumbing.
- **API fit:** Simple: register a scheme and an app path. Key limitation: does NOT pass URL parameters to the app — only triggers launch. If the app is already running, handling the URL in-process requires additional platform work.
- **Health:** ARCHIVED. Lives under `maidsafe-archive` on GitHub. Not maintained.
- **Verdict:** Historically interesting but dead. Reference the source code for per-OS registration patterns, but do not depend on it.

### Manual URL-Scheme Registration (no-crate approach)

For a non-Tauri app, direct per-OS plumbing is the most reliable path:

| OS | Mechanism |
|---|---|
| **macOS** | `Info.plist` `CFBundleURLTypes` / `CFBundleURLSchemes` array. For runtime registration: `LSSetDefaultHandlerForURLScheme`. The `plist` crate (0.7, 6.4M downloads) can generate the plist from Rust build scripts. |
| **Windows** | Registry entries under `HKCU\Software\Classes\<scheme>` with shell/open/command. Can be done via `winreg` crate (0.54, 12M downloads). On install, write to registry; on uninstall, clean up. |
| **Linux** | `.desktop` file with `MimeType=x-scheme-handler/<scheme>;` and `Exec=<app> %u`. Register via `xdg-mime default <desktop-file> x-scheme-handler/<scheme>`. |

This is small enough (<200 lines per platform) that a native Rust crate wrapping all three would be valuable but doesn't meaningfully exist today outside Tauri.

---

## 3. Launching Apps / Files / URLs

### `open` (5.4.0)

- **Crate / Source:** [crates.io/crates/open](https://crates.io/crates/open) | [github.com/Byron/open-rs](https://github.com/Byron/open-rs)
- **Downloads:** ~40.8M total, ~11.75M recent
- **License:** MIT
- **Platform coverage:**
  - **macOS:** `/usr/bin/open`.
  - **Windows:** `ShellExecuteW` (default) or `cmd /c start` (via `shellexecute-on-windows` feature, or the `insecure` feature for legacy behavior).
  - **Linux:** `xdg-open`, falling back to `gio open`, `gnome-open`, `kde-open`.
  - **WSL:** PowerShell `Start-Process` with fallbacks.
- **API fit:** `open::that(url_or_path)` — single-call convenience. `open::commands(url)` — returns `Vec<Command>` for custom handling. `open::with_command(url, "app_name")` — open with a specific application. `open::that_in_background(url)` — detached process. Binary included (`cargo install open` gives a CLI `open` tool).
- **Health:** Excellent. Maintained by Sebastian Thiel (Byron), one of Rust's most prolific maintainers. 59 versions since 2015. Latest release 2026-07-12 (days before this report). Extremely widely depended upon.
- **Verdict:** The clear winner for "open this thing with the system default." 40M downloads and Byron's track record make it the safest dependency in this entire report. Use `open::with_command()` when you need to launch a specific app (e.g., `zed://` deep links). Note the caveat: on UNIX, MIME type resolution is inherently fragile — the crate inherits the platform's weaknesses.

### `opener` (0.8.5)

- **Crate / Source:** [crates.io/crates/opener](https://crates.io/crates/opener) | [github.com/Seeker14491/opener](https://github.com/Seeker14491/opener)
- **Downloads:** ~30.9M total, ~4.46M recent
- **License:** MIT OR Apache-2.0
- **Platform coverage:** Windows, macOS, Linux. Uses D-Bus/zbd on Linux. Feature-gated `reveal` option to open parent directory in file manager.
- **API fit:** `opener::open(path)` and `opener::open_browser(url)`. Simpler API than `open` — no `with_command` equivalent. The `reveal` feature is a differentiator: open the file manager at a specific path.
- **Health:** Healthy. 21 versions since 2018. Moderate activity.
- **Verdict:** A solid alternative to `open` with slightly less API surface. The `reveal` feature is useful if you need "show in folder." Otherwise, `open`'s broader API and higher maintenance velocity make it the better default choice.

### `webbrowser` (1.2.1)

- **Crate / Source:** [crates.io/crates/webbrowser](https://crates.io/crates/webbrowser) | [github.com/amodm/webbrowser-rs](https://github.com/amodm/webbrowser-rs)
- **Downloads:** ~25.1M total, ~6.7M recent
- **License:** MIT OR Apache-2.0
- **Platform coverage:** Linux, Windows, macOS, iOS, Android, WASM. CI-tested on all six.
- **API fit:** `webbrowser::open(url)` — single call. Guarantees a browser opens (not a text editor for `.html` files, which `open`/`opener` can't promise). Optional `hardened` feature disables non-http(s) URL handling. Non-blocking for GUI browsers, blocking for text-mode browsers (lynx).
- **Health:** Healthy. Maintained by Amod Malviya. 42 versions since 2015. Steady release cadence through 2026.
- **Verdict:** Use this specifically when you need a browser guarantee — e.g., opening an OAuth URL, documentation links, or web-based content. Pair it with `open` for generic file/URL launching. The `hardened` feature is important for security-conscious apps.

### `robius-open` (0.1.2)

- **Crate / Source:** [crates.io/crates/robius-open](https://crates.io/crates/robius-open) | [github.com/project-robius/robius-open](https://github.com/project-robius/robius-open)
- **Downloads:** ~20.6K total
- **License:** MIT
- **Platform coverage:** Cross-platform including Android. Wraps platform openers behind an abstraction layer.
- **Verdict:** Conceptually similar to `open` but part of the dormant Robius ecosystem. Not recommended over `open` unless you specifically need Android support in a non-Tauri context.

---

## 4. Wayland Integration (XDG Portals)

### `ashpd` (0.13.12)

- **Crate / Source:** [crates.io/crates/ashpd](https://crates.io/crates/ashpd) | [github.com/bilelmoussaoui/ashpd](https://github.com/bilelmoussaoui/ashpd)
- **Downloads:** ~11M total, ~2.65M recent
- **License:** MIT
- **Platform coverage:** Linux only. Wraps all XDG desktop portal D-Bus interfaces.
- **Relevant modules:**
  - **`ashpd::desktop::global_shortcuts`** (behind feature `global_shortcuts`): `bind_shortcuts()` with `NewShortcut` descriptors, `list_shortcuts()`, `configure_shortcuts()`. Signal-based: `Activated`, `Deactivated`, `ShortcutsChanged`. This is the Wayland-safe path for global hotkeys — the compositor mediates access rather than the app grabbing keys directly. Works in sandboxed environments (Flatpak).
  - **`ashpd::desktop::remote_desktop`** (behind feature `remote_desktop`): Input capture for broader keyboard control.
  - **`WindowIdentifier` / `ActivationToken`**: The xdg-activation plumbing. `WindowIdentifier::from_native()` takes a GTK4 native, `from_raw_handle()` accepts raw-window-handle. An `ActivationToken` is obtained via the portal and passed to the app being launched so it can request focus transfer — this is how Wayland enforces the "no stealing focus" rule. Without this token, a launched app on Wayland cannot take focus from the launcher.
- **API fit:** The `global_shortcuts` module gives you Wayland-global hotkey registration without requiring `/dev/input` permissions or root. The xdg-activation token path is essential for focus handoff when launching another app from a launcher. Both are gated behind feature flags, keeping compile times manageable.
- **Health:** Very active. Maintained by Bilal Elmoussaoui (GNOME/GTK contributor). 64 versions since 2020. Frequent releases (multiple per month). 100% documented.
- **Verdict:** Essential for serious Linux/Wayland support. If you're building a launcher: use `global_shortcuts` for the trigger hotkey, and pipe an `ActivationToken` through when launching apps so they can take focus. This is the only Rust crate that properly integrates with the XDG portal system for these capabilities.

---

## 5. Recommended Composition

For a cross-platform desktop app with **global shortcut trigger + launcher** capabilities, the recommended stack is:

| Capability | Crate | Why |
|---|---|---|
| **Global hotkey** (trigger) | `global-hotkey` (0.8.0) | De facto standard. Falls back to X11 on Linux; for Wayland, add `ashpd::global_shortcuts` behind a feature flag. |
| **Wayland global hotkey** | `ashpd` (0.13.12) | XDG GlobalShortcuts portal — the Wayland-safe path. Feature-gate behind `wayland-hotkey` so non-Linux builds don't pull D-Bus. |
| **Launch files/URLs** | `open` (5.4.0) | The universal opener. Use `open::with_command()` for deep links to specific apps (e.g., `zed://file/path`). |
| **Launch in browser** | `webbrowser` (1.2.1) | Use when the target is specifically a web URL and you need the browser guarantee. |
| **Wayland focus handoff** | `ashpd` (0.13.12) | `ActivationToken` plumbing for xdg-activation. Pass the token as `XDG_ACTIVATION_TOKEN` env var to the launched process. |
| **URL scheme registration** | Manual per-OS | No mature cross-platform crate exists outside Tauri. Per-OS plumbing is <200 lines each. Use `plist` for macOS, `winreg` for Windows, `.desktop` files for Linux. |

### Architecture Sketch

```
┌─────────────────────────────────────────────────┐
│  GlobalShortcutService                          │
│  ┌───────────────────────────────────────────┐  │
│  │ macOS: global-hotkey (CGEvent)            │  │
│  │ Windows: global-hotkey (RegisterHotKey)   │  │
│  │ Linux/X11: global-hotkey (XGrabKey)       │  │
│  │ Linux/Wayland: ashpd GlobalShortcuts      │  │
│  └───────────────────────────────────────────┘  │
│              │                                   │
│              ▼                                   │
│  LauncherService                                │
│  ┌───────────────────────────────────────────┐  │
│  │ open::with_command() for deep links       │  │
│  │ webbrowser::open() for web URLs           │  │
│  │ ashpd ActivationToken for Wayland focus   │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### Key Risks / Gaps

1. **Wayland hotkey fragmentation.** `global-hotkey` is X11-only; `ashpd::global_shortcuts` works on Wayland but only if a portal backend is running (standard on GNOME/KDE, may be absent on minimal WMs). `hotkey-listener` uses evdev but requires device permissions and has no Windows support. A composable approach — try portal first, fall back to X11, warn if neither works — is the pragmatic path.

2. **No cross-platform URL scheme registration crate.** The Tauri `deep-link` plugin is the only maintained solution, and it's Tauri-only. A standalone crate wrapping the three platforms (~200 lines each) would be a valuable addition but doesn't exist today. Until then, manual per-OS registration in build scripts is the recommended approach.

3. **macOS accessibility permissions.** Both `global-hotkey` and `handy-keys` require Accessibility permissions on macOS (System Settings > Privacy & Security > Accessibility). This is a user-facing friction point: the app must guide users to enable it. `handy-keys` provides helper functions for this; `global-hotkey` does not.

4. **xdg-activation on non-Wayland.** The `XDG_ACTIVATION_TOKEN` approach is specific to Wayland. On X11/macOS/Windows, focus stealing is controlled differently (EWMH `_NET_ACTIVE_WINDOW`, `NSApplication.activate()`, `SetForegroundWindow`). The launcher service needs per-platform focus-handoff logic.
