# W2 mobile-GPUI usability (2026-07-05)

Workstream: make iOS/Android GPUI builds usable + native-services research.

## Changes

1. **`chain_ops` made reactive** (`crates/holon-frontend/src/value_fns/chain_ops.rs`).
   Was: a static `SyntheticRows` snapshot cached in `ProviderCache` — the
   mobile bottom action bar rendered empty (nothing focused at first eval)
   and never retargeted: `target_id` stayed on whatever block was focused
   when the provider was built, so taps dispatched ops against the wrong
   block. Now: `ChainOpsProvider` projects `focused_block_mutable()` through
   `ops_rows_for_uri` on every emission, mirroring `FocusChainProvider`.
   Needs `services.clone_arc()`; guarded behind `focused_block_mutable()`
   being `Some`, so stub/headless services (which panic on `clone_arc`)
   keep getting the empty static provider.

2. **Soft-keyboard lifecycle hardened** (`frontends/gpui/src/mobile.rs`,
   `frontends/gpui/src/views/editor_view.rs`). gpui does not guarantee
   Blur(old)→Focus(new) order on block→block focus moves (the zombie-editor
   blur can land after the next editor's focus), so hide-on-blur could
   dismiss the keyboard mid-editing. New `editor_focus_gained()` /
   `editor_focus_lost(cx)`: a focus-generation counter + 150 ms deferred
   hide that no-ops if any input regained focus. Unsupported-platform
   branches log a disclosed `tracing::warn!` instead of gpui-mobile's
   silent no-op.

3. **Bottom dock scrolls horizontally**
   (`frontends/gpui/src/render/builders/bottom_dock.rs`): one `op_button`
   per registered block op exceeds a phone width; `id` + `overflow_x_scroll`
   (same idiom as `board.rs` lanes) instead of clipping.

4. **Cargo.lock**: baseline was red — commit d996406f had run a wholesale
   `cargo update`, bumping the RustCrypto stack (ed25519 3.0.0 / pkcs8
   0.11.0 stable) past iroh's pinned `ed25519-dalek =3.0.0-pre.1` and
   moving gql-transform to a rev holon-turso wasn't adapted to. The
   supervisor restored the parent commit's lock (ed25519 3.0.0-rc.4,
   gql-transform 0c5a6dd); this workstream's earlier interim pins were
   superseded by that restore. No lock deltas belong to this workstream.

5. **Mobile-build bitrot fixed** (`frontends/gpui/src/mobile.rs`): the
   `[loro]` → `[crdt]` HolonConfig rename never reached the
   `#[cfg(feature = "mobile")]` entry points (nothing compiles them in CI);
   `holon_config.loro.enabled` → `holon_config.crdt.enabled`. Suggest a CI
   job for `--target aarch64-apple-ios-sim --no-default-features --features
   mobile` to stop this class of rot.

6. **Research doc**: `docs/Research/gpui-mobile-native-services.md` —
   location + notifications from GPUI on iOS/Android (objc2 vs JNI routes,
   lifecycle/permission story, `hasCode="false"` push constraint,
   recommended `holon-native-services` DI seam).

## Runtime verification session (2026-07-05 afternoon)

- **Android emulator, debug APK: native crash at startup.** SIGSEGV in
  `gpui_wgpu::WgpuRenderer::new` → root frame differs by GPU mode:
  - default/`-no-window` (lavapipe CPU Vulkan): SEGV inside renderer init.
  - `-gpu host` (gfxstream): SEGV in guest driver
    `vulkan.ranchu.so vk_common_SetDebugUtilsObjectNameEXT` — debug-profile
    wgpu enables debug-utils object labels which the emulator's Vulkan
    driver mishandles. Release profile (the justfile default — wgpu debug
    labels off) is the working configuration; re-verified with a release
    APK.
- **justfile portability bug for jj workspaces**: `project_root :=
  `git rev-parse --show-toplevel`` resolves to the MAIN checkout when run
  from a secondary jj workspace (no `.git` there), so `just apk` packages
  a `.so` from the wrong target dir. Workaround: `just --set project_root
  <workspace> apk`. Consider `justfile_directory()/../..` instead.
- **iOS simulator: BLOCKED environmentally.** Xcode was updated to 26.6 but
  `xcodebuild -runFirstLaunch` never ran: installed CoreSimulator.framework
  is 1051.54, wrapper expects 1051.55, so Apple's `simctl` wrapper script
  spawns `-runFirstLaunch` on every invocation, which blocks forever
  awaiting admin authorization (no sudo password available to agents).
  Every simctl call therefore hangs indefinitely. FIX (human, once):
  `sudo xcodebuild -runFirstLaunch` — then `just ios-sim` should work.
  (Also: root disk hit 732 MB free during the session; freed ~27 GB of
  build artifacts from this workspace's target/.)

### Android emulator matrix (release APK, API 36 arm64 image, emulator 36.6.11)

| GPU mode | Result |
|---|---|
| default/`-no-window` (lavapipe) + DEBUG build | SIGSEGV in `WgpuRenderer::new` (wgpu debug-utils) |
| `-gpu host` (gfxstream) | Vulkan adapter "Apple M4 Max" selected, then immediate `wgpu device lost: Unexpected error variant (driver implementation is at fault)` → black screen, UI loop stalled |
| `-gpu swiftshader_indirect` (SwiftShader Vulkan) | adapter selected OK, **no device-lost**, backend fully boots (schema, journals nav, matview watches, memory monitor) — but **no frame is ever presented** (black in dark AND light mode) and input taps produce no log activity → GPUI frame/event loop stalled after first present attempt |

All three failures are inside gpui-mobile/gpui_wgpu presentation on the
emulator — none touch this workstream's UI-level changes. `just deploy`
against a physical device is the currently credible Android runtime path.
Also fixed en route: stale-DB panic after a crashed run ("table `block`
already exists") is cleared by `just clear` (pm clear).
Release APK kept at `frontends/gpui/android/build/holon.apk` (568M, signed
debug-keystore) for a physical-device install.
