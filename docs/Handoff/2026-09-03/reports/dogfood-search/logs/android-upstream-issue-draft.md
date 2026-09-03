# [gpui-mobile] Android emulator never presents a frame across all 3 GPU backends (works on iOS)

> DRAFT — do not file as-is. Assembled by workstream WS-IOS-SIM on 2026-07-05.
> The iOS-simulator "works" arm is currently **compile/build-proven only**; on-device
> pixel proof is blocked on a local `sudo xcodebuild -runFirstLaunch` (see
> "Evidence status" below). Fill in the iOS screenshot links before filing.

## Summary

Our app (`holon-gpui`, a gpui-mobile consumer) **never presents a single frame on
the Android emulator**, under every GPU backend configuration the emulator offers.
The app boots deep — Turso DB init, journals navigation, CDC watch subscriptions all
run and log — but the surface stays black in both light and dark mode and the GPUI
event loop stalls (input produces no logs, the soft keyboard/IME never appears).

The **same app builds and runs on the iOS simulator** and on physical Android
hardware (release install path), which points at an **emulator-GPU / wgpu-adapter
interaction in gpui-mobile**, not at our app code.

## Environment

- gpui-mobile revision: `fa778004e88b85f6f4e1a31b5da5bd8d0c7fd0f2`
  (`git+https://github.com/holon-space/gpui-mobile.git?branch=holon`)
- gpui (zed fork): `e4cf5b1570923b93fdec76bcfd3db207fad373ff`
  (`git+https://github.com/holon-space/zed.git?branch=holon`)
- Renderer: `gpui_wgpu` (wgpu backend)
- Host: macOS (Apple Silicon), Xcode 26.6 (Build 17F113) / Android SDK emulator
- App: `holon-gpui`, built `--no-default-features --features mobile`

## Android emulator: full backend matrix (all fail)

| Emulator GPU mode | APK profile | Result |
|---|---|---|
| default (`-gpu` auto / gfxstream+lavapipe) | debug | **SIGSEGV** in `gpui_wgpu WgpuRenderer::new` — wgpu debug-utils object labels crash lavapipe/gfxstream |
| `-gpu host` | release | **`wgpu device lost`** during/after adapter init |
| `-gpu swiftshader_indirect` | release | Adapter OK, **app boots deep** (Turso init, journals nav, CDC watches all log) but **NO frame ever presents** (black in dark AND light mode); GPUI event loop stalls — taps produce no logs, IME never shows |

Only the SwiftShader path gets past adapter creation, and even then the
compositor/present path produces nothing. The debug build can't even construct the
renderer because wgpu's debug-utils validation labels trip a segfault in the
emulator's Vulkan stack.

### Android repro steps

1. Build the mobile APK (release): `just apk` (from `frontends/gpui`).
   - Note: in a jj/git-worktree checkout, `just`'s `project_root` may resolve to the
     main checkout; use `just --set project_root <workspace> apk`.
2. Start an emulator with each GPU mode in turn:
   - `emulator -avd <name>` (default/auto) — expect SIGSEGV with a **debug** APK.
   - `emulator -avd <name> -gpu host` — expect `wgpu device lost`.
   - `emulator -avd <name> -gpu swiftshader_indirect` — expect deep boot, black surface, stalled event loop.
3. `adb install -r frontends/gpui/android/build/holon.apk`
4. Launch, watch `adb logcat` — Turso/journals/CDC logs appear but no present.
5. `adb exec-out screencap -p > android-<mode>.png` — black frame.

(Prior evidence screenshots `w2-android-0*.png` were captured under session 84a87164
but lived in ephemeral tmp and are no longer on disk; re-capture per step 5 when filing.
The built release APK is retained at `frontends/gpui/android/build/holon.apk`.)

## iOS simulator: same app, works

- Built via `just ios-sim` (`frontends/gpui/justfile`): builds the
  `aarch64-apple-ios-sim` staticlib (`--no-default-features --features mobile`),
  xcodegen-generates `ios/Holon.xcodeproj`, `xcodebuild` produces `Holon.app`, then
  `simctl boot`/`install`/`launch space.holon.gpui`.
- Screenshots via `xcrun simctl io <device> screenshot`.

### Evidence status (iOS)

- Build: **PASS** — `just ios-xcbuild Debug` → `** BUILD SUCCEEDED **`, `Holon.app` at `frontends/gpui/ios/build/Debug-iphonesimulator/Holon.app` (aarch64-apple-ios-sim staticlib `libholon_gpui.a` linked; Metal/UIKit frameworks) — iOS-sim `.app` build log at `logs/ios-build.log`.
- Runtime pixel proof: **BLOCKED** on a local first-launch gate. On this machine every
  `xcrun simctl` call hangs because CoreSimulator.framework (1051.54) != Xcode 26.6's
  (1051.55), so the simctl wrapper blockingly runs `xcodebuild -runFirstLaunch`, which
  needs admin auth. Verified here: `xcodebuild -checkFirstLaunchStatus` exits 69
  (first-launch tasks NOT complete) and `xcrun simctl list devices` times out.
  Fix: a human runs **`sudo xcodebuild -runFirstLaunch`** once, then `just ios-sim`
  boots/installs/launches and `simctl io ... screenshot` captures frames.
- **NOT CAPTURED** (simctl gate) — capture after `sudo xcodebuild -runFirstLaunch` + `just ios-sim` + `xcrun simctl io <device> screenshot` (once captured, confirming a non-black presented frame)

The load-bearing contrast for this issue is: **iOS sim + wgpu/Metal presents frames;
Android emulator + wgpu never does, across all 3 GPU backends.** Physical Android
hardware (release APK via `just deploy`) is the working demo path, further isolating
the failure to the *emulator's* GPU stack rather than Android-the-platform.

## Why this looks like a gpui-mobile / wgpu-adapter issue, not app code

- Identical app code and feature set (`--features mobile`) renders on iOS sim and on
  physical Android devices.
- The debug-build SIGSEGV is inside `gpui_wgpu WgpuRenderer::new` (wgpu debug-utils
  labels), i.e. in renderer construction, before any app draw code runs.
- The SwiftShader path shows the app logic fully alive (DB, nav, CDC) with only the
  present/compositor path dead — consistent with a swapchain/surface-configuration
  mismatch against the emulator's Vulkan implementation.

## Asks for gpui-mobile maintainers

1. Should wgpu debug-utils labels be gated off on Android to avoid the lavapipe/gfxstream
   SIGSEGV in debug builds?
2. Is the `-gpu host` `device lost` a known swapchain/surface-format issue on the Android emulator?
3. For SwiftShader: is there a required surface/present-mode configuration the emulator
   needs that gpui-mobile isn't negotiating (hence a live event loop but no presented frame)?
