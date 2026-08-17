---
id: 2026-07-18-android-app-boots-permanent-black-screen
date: 2026-07-18
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Android app boots to a permanent BLACK SCREEN — the first frame is never
  presented. Root cause (from adb/logcat investigation): gpui-mobile invokes
  `finish_launching` on the event-loop thread, so the frontend bootstrap and
  `cx.open_window` run synchronously ON the thread that must keep pumping to
  present frames. `launch_holon_window_impl` (frontends/gpui/src/lib.rs:1383)
  runs a root-layout pre-warm `fg_executor.block_on(async … signal stream …)`
  whenever `existing_engine` is `Some` — and the mobile path ALWAYS supplies
  one (mobile.rs:111 → `launch_holon_window_rebindable` → impl with
  `Some(engine)`). `block_on` on the foreground executor's OWN thread is a
  re-entrancy wedge: the event loop stops pumping right after "[GPUI] Opening
  window…", never presents, black screen. (Compounded observability: the
  on-device APK was a stale 2026-03-26 build.) Desktop unaffected: on desktop
  the foreground executor's `block_on` is not the platform present loop.
source_line: 1005
---

## Bug

Android app boots to a permanent BLACK SCREEN — the first frame is never
presented. Root cause (from adb/logcat investigation): gpui-mobile invokes
`finish_launching` on the event-loop thread, so the frontend bootstrap and
`cx.open_window` run synchronously ON the thread that must keep pumping to
present frames. `launch_holon_window_impl` (frontends/gpui/src/lib.rs:1383)
runs a root-layout pre-warm `fg_executor.block_on(async … signal stream …)`
whenever `existing_engine` is `Some` — and the mobile path ALWAYS supplies
one (mobile.rs:111 → `launch_holon_window_rebindable` → impl with
`Some(engine)`). `block_on` on the foreground executor's OWN thread is a
re-entrancy wedge: the event loop stops pumping right after "[GPUI] Opening
window…", never presents, black screen. (Compounded observability: the
on-device APK was a stale 2026-03-26 build.) Desktop unaffected: on desktop
the foreground executor's `block_on` is not the platform present loop.

## Missing piece

no device/emulator first-frame smoke gate exists; the keystone PBT runs
headless in-process and NEVER instantiates the Android platform stack
(gpui-mobile `finish_launching`/event-loop threading,
`block_on`-on-foreground re-entrancy), so a platform-thread present-wedge is
structurally unobservable

## Remedy

FIXED (frontends/gpui/src/lib.rs:1383): the pre-warm `if let Some(ref
engine) = existing_engine` block is now `#[cfg(not(target_os = "android"))]`
— Android skips the blocking pre-warm and opens with the loading state; the
tokio root-layout signal drives the first real repaint asynchronously once
the event loop is pumping. Desktop path byte-unchanged. ON-DEVICE CONFIRMED
(OnePlus DN2103, fresh-DB launch, logcat tag `holon-gpui`): after `window
opened` → `invoking finish_launching callback`, the async `watch_ui
block:root-layout` fires and `[UiWatcher] render_entity('block:root-layout')
OK: gen=1, render="if_space"` plus all three panels
(left-sidebar/main-panel/right-sidebar) render; the event loop keeps pumping
(`org.poll_tracked_files` every ~100ms, `ALooper_pollOnce` callbacks) out
past 80s — the exact opposite of the diagnosed "event-loop thread goes
permanently silent right after Opening window". Pixel screencap could NOT be
taken: the device is locked with a secure credential and Android blocks
`screencap` of protected content while `deviceLocked=1` (visual proof is the
render-log trail, not a bitmap). GAP REMEDY (not yet implemented): a
non-black-first-frame smoke gate on a real device/emulator (launch → cleared
logcat → screencap after ~5s/~15s asserting non-black UI + event-loop thread
keeps logging past the window open, or a `ScreenFgTime`/present-count
probe), analogous to the live-iOS MCP gate (`tests/*_live_mcp_gate.rs`) that
drives the real iOS platform stack over MCP.
