---
id: 2026-10-10-android-clipboard-in-memory-fake
date: 2026-10-10
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  On Android every GPUI clipboard write and read went to an in-process string,
  so Holon's Copy actions copied nothing other apps could paste, and Paste never
  saw text copied elsewhere — with no error anywhere.
---

## Bug

Found by code audit while adding a "Copy" button to the crash-history view: the
copy would be a silent fake on Android. Research report (orchestrator scratchpad,
`research/gpui-mobile-android-clipboard.md`) confirmed it at the pinned
gpui-mobile fork rev `49f07d28`.

## Root cause

`src/android/platform.rs:65-88` of the gpui-mobile fork implemented GPUI's
`Platform::{read,write}_to_clipboard` with `AndroidClipboard { contents:
Option<String> }`. The fork already carried a JNI `ClipboardManager` package
(`src/packages/clipboard/android.rs` + `GpuiClipboard.java`), but nothing called
it, `dev.gpui.mobile.GpuiClipboard` was not in `CACHED_APP_CLASSES`
(`src/android/jni.rs:206-209`), and Holon's APK/AAB packagers did not compile
`GpuiClipboard.java`. A non-text item (an image) was written as `""`.

## Missing piece

No automated layer runs on Android: the keystone PBT is headless and the
windowed GPUI PBTs take the macOS platform. The clipboard round-trip inside one
process cannot tell a fake from the real clipboard — only a second app reading
and writing the system clipboard can.

## Remedy

The platform clipboard calls `ClipboardManager` through `GpuiClipboard`; a JNI
failure or a non-text item is logged at error level and the read yields nothing
/ the write is dropped, with no in-process copy. `GpuiClipboard.java` is
vendored in `frontends/gpui/android/java/dev/gpui/mobile/` and packaged by all
three Android packagers. On-device proof: a second app
(`space.holon.clipprobe`) writes/reads the system clipboard around a GPUI copy
and paste on an emulator. OPEN until that device run is green.
