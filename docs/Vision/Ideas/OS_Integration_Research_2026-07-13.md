# OS Integration Research — Attention Environment Shell (2026-07-13)

Research for the "zoom hierarchy / task bundles / presence" cluster in the
vault topic doc `Attention Environment.org` (holon-pkm vault). Three
per-OS agent reports (macOS / Windows / Linux), lightly edited, plus a
cross-OS synthesis. Capabilities assessed: window enumeration + focus
observation, task-scoped window-set hide/reveal, switcher (Cmd-Tab/Alt-Tab)
replacement, gesture interception, live thumbnails, click-through overlay
vignette, tray/menu-bar presence + OS do-not-disturb, launch-with-context,
and virtual desktops/workspaces.

## Cross-OS synthesis

**The universal pattern: every OS guards its own workspace, gesture, and
DND concepts, but freely permits window observation, window hide/show,
overlays, and launching.** The OS lets us build our own attention layer;
it does not let us repurpose theirs. Consequences:

1. **Context launcher: EASY everywhere.** No permissions on any OS. Build
   first.
2. **Overlay vignette: EASY everywhere except GNOME** (needs our GNOME
   Shell extension companion; Mutter refuses layer-shell by policy).
3. **Window enumeration + focus observation: available everywhere.**
   macOS: Accessibility permission. Windows: no permission at all. Linux:
   ext-foreign-toplevel-list now spans KWin/Sway/Hyprland/COSMIC/niri and
   (new, 49.2) Mutter — but only for *unsandboxed* clients → **do not ship
   the Linux shell as a Flatpak**.
4. **Window-set hide/reveal (task bundles): DOABLE everywhere**, always as
   OUR OWN layer (AeroSpace/VirtuaWin lesson): macOS via Accessibility
   hide, Windows via ShowWindow (+ crash-recovery design, elevated-app
   gap), Linux via foreign-toplevel-management / KWin / EWMH (GNOME:
   extension). Native Spaces (macOS, needs SIP off) and undocumented
   virtual-desktop COM (Windows, breaks 2-3×/year) are both OFF the table;
   KDE Activities is the one native task-context substrate worth
   integrating with.
5. **Switcher: replaceable on Windows (LL keyboard hook + signing tax) and
   macOS (CGEventTap + Input Monitoring); on Linux per-DE.** Smarter
   default everywhere: AUGMENT with our own chord, leave the native
   switcher alone.
6. **Gestures: do not fight the OS — on all three.** macOS: Mission
   Control swipe uninterceptable for notarized apps (BetterTouchTool
   bridge → `holon://zoom-out` URL). Windows: Settings-level remap of
   3/4-finger swipe to a chord we own (Win11 only). Linux: per-compositor
   gesture bindings. Gestures are progressive enhancement, never core UX.
7. **OS do-not-disturb is unreadable/unwritable via public API on macOS
   AND Windows** (Focus modes / Focus Assist); Linux is per-DE. The focus
   contract must be Holon-native everywhere — which was the design lean
   anyway.
8. **Thumbnails are garnish, not load-bearing**: macOS monthly TCC
   re-prompts (ScreenCaptureKit on 15+), Windows yellow consent border
   (WGC on Win10; DWM thumbnails as the border-free display-only
   alternative), Linux portal consent + PipeWire overhead.

**Build order (all platforms): launcher → observation → window sets →
switcher-augment → vignette → thumbnails.** Architecture: engine knows
only "task has resource edges" + "task is centered"; per-OS thin shell
(and on Linux, per-DE companions — GNOME Shell extension mandatory, KWin
script optional; the GSConnect/ActivityWatch-proven pattern).

Platform priority within Linux: KWin ≥ wlroots > X11 >> GNOME.

---

# macOS report

## Executive summary

A notarized GPUI-based app can implement most window-management and
interaction capabilities, with key constraints: **Spaces control requires
SIP disabling** (not viable), **Cmd-Tab interception works via CGEventTap**
(needs Input Monitoring permission), **Mission Control gesture override is
architecturally blocked**, and **ScreenCaptureKit works notarized but
triggers monthly TCC re-prompts on macOS 15+**. Best path: Accessibility
window enumeration/hide-show, Cmd-Tab replacement, overlay vignettes, live
thumbnails as optional; skip native Spaces integration and gesture
remapping.

## Per capability

1. **Window enumeration & focus tracking — EASY→DOABLE-WITH-PERMISSION.**
   Accessibility API (AXUIElement) + AXObserver focus notifications; needs
   the Accessibility permission (notarizable, App-Store-eligible). Window
   IDs need undocumented bridging (fragility). Cannot see windows on other
   Spaces without private APIs. Prior art: Hammerspoon, yabai (AX part).
2. **Window-set hide/show — DOABLE (Accessibility) / BLOCKED
   (Spaces-aware).** AX hide/minimize actions + our own task→window
   registry. yabai's Spaces control needs SIP off (disqualifying);
   AeroSpace proves the own-virtual-workspace-layer alternative. Stale
   window references and user un-hides must be handled.
3. **Cmd-Tab replacement — DOABLE-WITH-PERMISSION.** CGEventTap +
   Input Monitoring permission (notarizable; NOT App Store). Prior art:
   AltTab, Contexts. Caveats: tap callback must be fast; secure-input
   fields block capture; startup races documented in AltTab issues.
4. **Mission Control gesture — BLOCKED** for notarized apps (handled below
   user-space; macOS hardcodes Mission Control/App Exposé/Launchpad/
   Notification Center gestures even for BetterTouchTool's system
   extension). Offer our own hotkey/menu-bar/hot-corner + document a
   BetterTouchTool bridge to `holon://zoom-out`.
5. **Live thumbnails — DOABLE-WITH-PERMISSION.** ScreenCaptureKit
   (macOS 13+), Screen Recording TCC; macOS 15 re-prompts MONTHLY →
   thumbnails must be optional garnish. CGWindowList images deprecated.
6. **Overlay vignette — EASY.** Borderless clear NSWindow, high window
   level, `ignoresMouseEvents`, one window per screen, listen for screen
   changes; test against fullscreen apps/Stage Manager per macOS version.
   No permissions.
7. **Menu bar — EASY** (NSStatusItem / MenuBarExtra). **Reading Focus
   modes — BLOCKED** (no public API; plist hacks fragile). Toggle via
   Shortcuts automation only.
8. **Launch-with-context — EASY.** NSWorkspace + URL schemes (`zed://`,
   `vscode://`, `obsidian://`); register `holon://` for inbound
   deep links. No API to query "which app handles this URL" — keep own
   registry.
9. **Spaces — BLOCKED notarized** (private APIs / SIP / Dock injection).
   Use our own window-set layer instead.

Ranking: launcher EASY · vignette EASY · window sets DOABLE ·
Cmd-Tab DOABLE-with-permission · gesture BLOCKED.

Key sources: github.com/lwouis/alt-tab-macos ·
github.com/nikitabobko/AeroSpace · github.com/koekeishiya/yabai
discussions #2274/#803 · developer.apple.com ScreenCaptureKit docs ·
folivora.ai BetterTouchTool docs · Apple notarization docs.

---

# Windows report

## Per capability

1. **Window enumeration + foreground tracking — EASY.** EnumWindows +
   GetWindowThreadProcessId/QueryFullProcessImageName;
   SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT) for
   push-based focus tracking, no injection, no privileges. Elevated
   processes partially opaque. Stable for decades.
2. **Window-set hide/reveal — DOABLE.** ShowWindow/SetWindowPos on foreign
   HWNDs (VirtuaWin technique; FancyZones repositions this way). Gotchas:
   UIPI (can't touch elevated apps' windows), SetForegroundWindow rules
   (fine when user invoked us), SW_HIDE crash-recovery obligation (persist
   hidden list + "show all" recovery; minimize-instead-of-hide as safer
   default). Groupy's re-parenting approach: avoid.
3. **Alt-Tab replacement — DOABLE with signing tax.** RegisterHotKey
   cannot claim Alt+Tab; the route is SetWindowsHookEx(WH_KEYBOARD_LL)
   swallowing Tab-while-Alt. Full robustness (over elevated windows) needs
   uiAccess="true" manifest → Authenticode-signed binary in Program Files.
   Win+Tab / Ctrl+Alt+Tab uninterceptable. Hook timeout: keep callback
   trivial. PowerToys stance ("add, don't replace") is the low-tax
   default: own chord via RegisterHotKey.
4. **Gestures — HACKY→BLOCKED globally.** No supported global
   interception of precision-touchpad 3/4-finger swipes. Pragmatic route:
   Win11 Settings → Touchpad → Advanced gestures lets the USER remap
   swipe to a keyboard chord we own via RegisterHotKey.
   TouchpadGesturesController (Win11 24H2) is foreground-only. Raw Input
   HID reading is observe-only (Task View still fires).
5. **Thumbnails — see sub-report:** Windows.Graphics.Capture (Win10 1803+,
   CreateForWindow from 1903) = GPU-texture per-window capture, works on
   occluded (not minimized) windows, full frames only (no dirty rects),
   MANDATORY yellow border on Win10 (removable Win11 via consent-gated
   `graphicsCaptureWithoutBorder`). DwmRegisterThumbnail = display-only
   live preview, no pixel access, no border — right tool for a switcher
   grid. DXGI Desktop Duplication = monitor-only with dirty rects.
6. **Overlay vignette — EASY/DOABLE.** WS_EX_LAYERED | WS_EX_TRANSPARENT |
   WS_EX_TOPMOST | WS_EX_NOACTIVATE; one overlay window PER MONITOR (never
   one spanning window — mixed DPI/hotplug); Per-Monitor V2 DPI awareness
   mandatory; re-assert topmost on foreground changes. Sub-report on
   fullscreen games: plain topmost works over everything DWM-composits
   (incl. modern auto-converted "fullscreen optimized" games); only true
   exclusive-fullscreen bypasses it, reaching those needs
   Present-hook DLL injection — never ship that. Detect via
   SHQueryUserNotificationState (QUNS_RUNNING_D3D_FULL_SCREEN) and treat a
   fullscreen game as an already-satisfied focus state.
7. **Tray — EASY** (Shell_NotifyIcon). **Focus Assist read — HACKY**:
   only the undocumented WNF state
   (WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED via NtQueryWnfStateData;
   Rafael Rivera gist; bitdisaster/windows-focus-assist). Setting: worse.
   Treat OS DND as best-effort read-only telemetry; own suppression
   in-shell.
8. **Launch-with-context — EASY.** ShellExecuteEx + URI schemes; own
   `holon://` scheme via HKCR. Placement: launch → PID → WaitForInputIdle
   → match window via EnumWindows/WinEventHook → SetWindowPos (the
   FancyZones dance); single-instance apps (vscode://) need
   foreground-change matching instead of PID.
9. **Virtual desktops — documented slice DOABLE (observation:
   GetWindowDesktopId for ANY window; MoveWindowToDesktop own windows
   only); full control HACKY** (IVirtualDesktopManagerInternal GUIDs break
   2-3×/year; MScholtes/VirtualDesktop ships five per-build variants).
   Use window-set hide/show as the stable substitute; VD for observation
   only.

Ranking: launcher EASY · window sets DOABLE · vignette DOABLE ·
Alt-Tab DOABLE-with-tax · gestures HACKY/BLOCKED. Everything runs
unelevated; isolate UIAccess signing and undocumented surfaces (WNF, VD
COM) behind capability flags with graceful degradation.

Key sources: learn.microsoft.com (SetWinEventHook, SetForegroundWindow,
IVirtualDesktopManager, flip-model, high-DPI, Precision Touchpad portal) ·
blog.misterfoo.com Alt-Tab replacement · github.com/MScholtes/
VirtualDesktop · gist Rafael Rivera focus-assist WNF ·
github.com/robmikh/Win32CaptureSample · fredemmott.com in-game overlays ·
GameTechDev/PresentMon.

---

# Linux report

## Headlines

- **No portal for window listing/control exists** (xdg-desktop-portal
  issue #304, open since 2019, rejected for sandboxed apps on privacy
  grounds; #980 "window position portal" closed not-planned). Everything
  window-shaped is per-environment, for TRUSTED UNSANDBOXED clients →
  **do not ship as Flatpak**.
- **ext-foreign-toplevel-list-v1** (enumeration + focus, read-only) now
  spans KWin 6.6, Sway 1.11, Mutter 49.2 (GNOME's historic first), COSMIC,
  Hyprland, niri, labwc, river, Weston 14. Observation converged; control
  did not.
- **GNOME is the outlier by policy** (no layer-shell — Mutter #973; no
  control protocol; locked-down Shell DBus). A **GNOME Shell extension
  companion is mandatory**; the pattern is proven (GSConnect,
  ActivityWatch, window-calls) and its per-release breakage cost is
  well understood.
- **KDE is the richest target**: every capability green, GlobalShortcuts
  portal since Plasma 5.27, KWin scripting + QML task-switcher plugins,
  WindowThumbnail QML, and **KDE Activities** — the one native
  task-scoped-context substrate on any OS, fully DBus-controllable
  (org.kde.ActivityManager: List/Current/SetCurrentActivity + windows
  assigned via KWin scripting). Closest living relative of Xerox Rooms.

## Capability × environment matrix

| Capability | X11 | KWin | wlroots | GNOME |
|---|---|---|---|---|
| Enumerate + focus | EASY (EWMH) | EASY | EASY (wlr/ext-foreign-toplevel) | DOABLE (49.2+) / extension before |
| Window sets hide/show | EASY | EASY (+Activities) | EASY (foreign-toplevel mgmt; scratch-workspace idiom) | HACKY (extension) |
| Switcher | DOABLE (XGrabKey) | EASY (portal + switcher plugins) | DOABLE Hyprland / HACKY Sway,niri (config-injection) | DOABLE (portal 48+ chord; AATWS-style extension for takeover) |
| Gestures | EASY (libinput-gestures, touchegg) | HACKY | DOABLE (bindgesture / Hyprland gestures → exec) | HACKY (extension Clutter hooks) |
| Thumbnails | EASY (XComposite) | EASY (portal window-cast; WindowThumbnail QML) | DOABLE (ext-image-copy-capture on Sway 1.10+/niri; portal-wlr is monitor-only; Hyprland portal has window cast) | DOABLE (portal only, consent + PipeWire) |
| Overlay vignette | EASY (override-redirect + empty input region) | EASY (layer-shell) | EASY (wlr-layer-shell OVERLAY; Smithay toolkit native) | BLOCKED native / DOABLE via extension actor |
| Tray + DND | EASY / per-DE | EASY / DOABLE (Notifications.Inhibit) | EASY / DOABLE (makoctl, swaync-client) | DOABLE (AppIndicator ext; GSettings show-banners) |
| Launch + activation | EASY (.desktop, xdg-open; _NET_MOVERESIZE placement racy) | EASY (xdg-activation; window rules for placement) | EASY (xdg-activation; for_window/windowrule placement) | DOABLE (placement extension-only) |
| Workspaces | EASY (EWMH) | EASY (DBus + Activities) | DOABLE (per-compositor IPC; ext-workspace-v1 converging) | HACKY (extension) |

Wayland absolute: **clients cannot position windows, period** — placement
is always compositor-side (rules/scripts/extension). Correlate launched
app ↔ new toplevel via app_id + xdg-activation token.

GlobalShortcuts portal adoption: KDE (Plasma 5.27) · GNOME 48 · Hyprland;
NOT xdg-desktop-portal-wlr (Sway — issue #240 open since 2022) nor COSMIC.
Sway/niri fallback: one-line user config binding → exec (culturally
accepted there).

## Architecture recommendation

Rust core with a Shell trait; backends: X11/EWMH (x11rb), wlroots
(smithay-client-toolkit + foreign-toplevel, layer-shell,
ext-image-copy-capture, xdg-activation), KWin (protocols + zbus for
Activities/VirtualDesktops/KGlobalAccel), GNOME (DBus to our own
extension). GNOME Shell extension companion mandatory; small optional KWin
script unlocks placement rules + Activities window-assignment. Gestures
and post-launch positioning are per-DE progressive enhancements, never
core UX. Environment priority: **KWin ≥ wlroots > X11 >> GNOME.**

Key sources: github.com/flatpak/xdg-desktop-portal issues #304/#980/#1064 ·
wayland.app protocol matrices (ext-foreign-toplevel-list,
wlr-foreign-toplevel-management, wlr-layer-shell, ext-workspace,
ext-image-copy-capture) · gitlab.gnome.org mutter#973 · KDE
kactivitymanagerd DBus · develop.kde.org KWin scripting ·
github.com/ickyicky/window-calls · GSConnect · ActivityWatch
aw-watcher-window · specifications.freedesktop.org EWMH ·
freedesktop.org StatusNotifierItem.
