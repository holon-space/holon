---
id: 2026-07-20-android-emulator-macos-max-cannot-show
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Android EMULATOR (`Medium_Phone_API_36.0`, macOS M4 Max) with `-gpu host`
  cannot show Holon GPUI: the guest exposes host Vulkan via gfxstream,
  `gpui_wgpu` selects the "Apple M4 Max (Vulkan)" adapter (it even PASSES the
  configuration test), then the first render pipeline fails host-side in
  MoltenVK — `[mvk-error] VK_ERROR_INITIALIZATION_FAILED: Shader library
  compile failed (Error code 3): program_source:356:16: error: no matching
  function for call to '_350'` / "Vertex shader function could not be compiled
  into pipeline" — and wgpu reports `device lost: reason=Unknown,
  message=Unexpected error variant (driver implementation is at fault)`; the
  current APK (2026-07-20 release) then dies fail-loud at startup. Same with
  `-feature VulkanNativeSwapchain`. Fallback ladder is also closed: with
  `-feature -Vulkan` the only adapter is the GLES translator, which
  `gpui_wgpu` REJECTS at device creation (`Limit
  'max_compute_workgroups_per_dimension' value 65535 is better than allowed
  0`) → "No GPU adapter found that can configure the display surface". WORKING
  configuration: `-gpu swiftshader_indirect` (guest SwiftShader Vulkan ICD)
  renders the full UI correctly (Journals tree, automation card, today's
  page), software-slow but conformant. So the SPIR-V that gfxstream re-emits
  for GPUI's vertex shader is untranslatable by MoltenVK's SPIRV-Cross — an
  emulator/driver-stack incompatibility, not a Holon logic bug (same APK
  renders on real devices and SwiftShader).
source_line: 1030
---

## Bug

Android EMULATOR (`Medium_Phone_API_36.0`, macOS M4 Max) with `-gpu host`
cannot show Holon GPUI: the guest exposes host Vulkan via gfxstream,
`gpui_wgpu` selects the "Apple M4 Max (Vulkan)" adapter (it even PASSES the
configuration test), then the first render pipeline fails host-side in
MoltenVK — `[mvk-error] VK_ERROR_INITIALIZATION_FAILED: Shader library
compile failed (Error code 3): program_source:356:16: error: no matching
function for call to '_350'` / "Vertex shader function could not be compiled
into pipeline" — and wgpu reports `device lost: reason=Unknown,
message=Unexpected error variant (driver implementation is at fault)`; the
current APK (2026-07-20 release) then dies fail-loud at startup. Same with
`-feature VulkanNativeSwapchain`. Fallback ladder is also closed: with
`-feature -Vulkan` the only adapter is the GLES translator, which
`gpui_wgpu` REJECTS at device creation (`Limit
'max_compute_workgroups_per_dimension' value 65535 is better than allowed
0`) → "No GPU adapter found that can configure the display surface". WORKING
configuration: `-gpu swiftshader_indirect` (guest SwiftShader Vulkan ICD)
renders the full UI correctly (Journals tree, automation card, today's
page), software-slow but conformant. So the SPIR-V that gfxstream re-emits
for GPUI's vertex shader is untranslatable by MoltenVK's SPIRV-Cross — an
emulator/driver-stack incompatibility, not a Holon logic bug (same APK
renders on real devices and SwiftShader).

## Missing piece

No automated rung runs the GPUI renderer on the Android emulator at all
(keystone is headless; device CI is sideload-only), so any emulator-GPU-mode
incompatibility is invisible until an agent/human tries it. Secondary
hardening candidate in `gpui_wgpu`: when the preferred Vulkan adapter's
device is lost during pipeline warm-up, fall back to the next adapter
(and/or relax the compute-limit request) instead of dying with no renderer —
the GL translator adapter was present but unusable solely because of the
requested compute limits.

## Remedy

OPEN 2026-07-20 (agent exploration, this session). Practical workaround
documented here: use `-gpu swiftshader_indirect` for emulator dogfooding;
real-device sideload remains the performance path. Upstream suspects:
gfxstream shader re-emission vs MoltenVK SPIRV-Cross; retest after
emulator/MoltenVK upgrades.
