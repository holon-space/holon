---
id: 2026-07-20-debug-builds-sigsegv-startup-android-emulator
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  DEBUG builds SIGSEGV at startup on the Android emulator EVEN in the working
  `-gpu swiftshader_indirect` mode (release runs fine, row 233): wild-pointer
  SEGV_MAPERR inside the guest Vulkan ICD `vulkan.ranchu` at
  `vk_common_SetDebugUtilsObjectNameEXT` (Mesa gfxstream common entry),
  reached via wgpu-hal `DeviceShared::set_object_name` ←
  `create_bind_group_layout` ← `wgpu_core indirect_validation Dispatch::new` ←
  `Device::new` ← `gpui_wgpu WgpuContext::create_device`. Debug-only because
  `wgpu::InstanceFlags::default()` includes DEBUG under `debug_assertions`,
  which makes wgpu-hal label every Vulkan object through VK_EXT_debug_utils —
  an entrypoint the emulator's guest ICD advertises but implements brokenly;
  release builds never call it. No env escape hatch existed (`instance()` used
  `InstanceFlags::default()` without `.with_env()`). Matches the SIGSEGV
  another session hit with a debug diagnostic build.
source_line: 1032
---

## Bug

DEBUG builds SIGSEGV at startup on the Android emulator EVEN in the working
`-gpu swiftshader_indirect` mode (release runs fine, row 233): wild-pointer
SEGV_MAPERR inside the guest Vulkan ICD `vulkan.ranchu` at
`vk_common_SetDebugUtilsObjectNameEXT` (Mesa gfxstream common entry),
reached via wgpu-hal `DeviceShared::set_object_name` ←
`create_bind_group_layout` ← `wgpu_core indirect_validation Dispatch::new` ←
`Device::new` ← `gpui_wgpu WgpuContext::create_device`. Debug-only because
`wgpu::InstanceFlags::default()` includes DEBUG under `debug_assertions`,
which makes wgpu-hal label every Vulkan object through VK_EXT_debug_utils —
an entrypoint the emulator's guest ICD advertises but implements brokenly;
release builds never call it. No env escape hatch existed (`instance()` used
`InstanceFlags::default()` without `.with_env()`). Matches the SIGSEGV
another session hit with a debug diagnostic build.

## Missing piece

Same emulator-shaped hole as row 233 (no automated rung runs the GPUI
renderer on the emulator), plus a latent hardening gap: gpui_wgpu passed
build-config instance flags to a driver stack that can't honor them, with no
per-driver quirk handling.

## Remedy

FIXED 2026-07-20 — LANDED: zed `holon` fast-forwarded ef2f1164→44506e1 and
Cargo.lock pin bumped in the same rev as this row (lock diff = only the 23
zed.git crates). Fix commit 44506e1: `gpui_wgpu::WgpuContext::instance()`
strips `InstanceFlags::DEBUG` when `getprop ro.boot.qemu` == 1, DISCLOSED
via a loud `log::warn!` naming the broken entrypoint; release builds and
real devices unaffected (flag absent / property 0). VERIFIED on-emulator:
patched debug APK boots, logs the warn, selects SwiftShader Vulkan, renders
the full UI, process alive at 60s (was: SIGSEGV in <1s, reproduced twice).
RE-VERIFIED post-pin-bump: debug APK built from the bumped lock boots on the
emulator, logs the warn, renders, alive at 45s.
