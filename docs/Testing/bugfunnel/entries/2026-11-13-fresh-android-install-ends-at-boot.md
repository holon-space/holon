---
id: 2026-11-13-fresh-android-install-ends-at-boot
date: 2026-11-13
gap: ENVIRONMENT
secondary: PERCEPTION
status: FIXED
summary: >-
  A fresh Android install never created its vault folder, so the boot refused
  to start, and the refusal ended the process with a SIGSEGV in the GPU
  driver instead of showing the error.
---

## Bug
Found by Martin on Android v0.0.25 (fresh install, OnePlus DN2103, Mali GPU,
minSdk 33). The app closed at once on first launch. A manual
`adb shell mkdir .../files/holon-pkm` made the next launch boot.

Two defects:
- D1: `android_main` (`frontends/gpui/src/mobile.rs`) only joined the vault
  path (`android_storage_paths`) and never created it; iOS did
  (`ios_data_paths`, `create_dir_all` plus an `index.org` seed).
  `VaultLock::acquire` (`crates/holon-app/src/vault_lock.rs`) then refused:
  `refusing to start: vault /storage/emulated/0/Android/data/space.holon.gpui/files/holon-pkm does not exist or is not a directory`.
- D2: the refusal went to `boot_failed`, which called `std::process::exit(1)`.
  The process died with SIGSEGV in the Mali driver thread instead of showing
  the error. This broke the "boot always" directive: degrade, disclose, offer
  the fix.

## Root cause
D1: no platform entry other than iOS created its data dir; the vault check is
strict on purpose (a desktop user who names a missing vault made a mistake).

D2, the crash frames (logcat of the failed launch, pid 5367; main thread
8012 runs the app, 8048 is the Mali `mali-mem-purge` thread):

```
05.251  5367  8012 E holon-gpui: holon_gpui::mobile: BOOT FAILED
05.251  5367  8012 E holon-gpui: boot failed: component=session stage=session-resolve
05.364  5367  8012 W RefBase : RefBase: Explicit destruction, weak count = 0 (in 0x7a24bf5a58). Use sp<> to manage this object.
05.368  5367  8048 F libc    : Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x795f679f1c in tid 8048 (mali-mem-purge), pid 5367 (pace.holon.gpui)
05.386  5367  8012 D RefBase : #00 pc 00000000000ef8b8  /apex/com.android.runtime/lib64/bionic/libc.so (__cxa_finalize+280)
05.386  5367  8012 D RefBase : #01 pc 00000000000e1758  /apex/com.android.runtime/lib64/bionic/libc.so (exit+24)
05.386  5367  8012 D RefBase : #02 pc 0000000007f26e68  .../lib/arm64/libholon_gpui.so
...
05.386  5367  8012 D RefBase : #14 pc 00000000021254b8  .../lib/arm64/libholon_gpui.so (android_main+1168)
05.403   896   896 I Zygote  : Process 5367 exited cleanly (1)
```

Hypothesis H1 holds: `exit(1)` on the main thread runs the static destructors
(`exit` -> `__cxa_finalize`); one of them destroys a native object still in
use (the `RefBase` warning on thread 8012), and the Mali driver thread, still
running for the open surface, faults on the freed memory. Zygote records the
exit status 1 of `exit(1)`, not a signal death of the main thread.
Hypothesis H2 (a panic hook firing on an unwind) is refuted: the frames show
no panic and no unwind, only `exit` -> `__cxa_finalize`.

## Missing piece
- The keystone's `BootFault` axis had no "vault dir missing at boot" draw, and
  the keystone's vault was always a pre-made temp dir. The Android entry
  (`android_main`) runs on no host test.
- No mobile boot-failure path put the error on screen; only an `exit` existed.

## Remedy
- `holon_app::vault_lock::ensure_vault_dir` creates the vault and seeds an
  empty one with the bundled `index.org`; the shared mobile window entry calls
  it for both iOS and Android. `VaultLock::acquire` stays strict.
- Keystone: `BootFault::MissingVaultDir` starts the case with no vault dir
  (apart from the config and database dir, as on Android) and runs the same
  function before the boot. Red on base with the `refusing to start` message.
- Property test over vault states (absent, empty, with content, a file in the
  way): the dir exists afterwards, an existing vault keeps its bytes, a second
  call changes nothing, and the lock then succeeds.
- `boot_failed` on mobile opens a static error window with the attributed
  report, its cause chain and the vault path, and the process stays alive.
  `exit` remains only when that window cannot open, or before the platform
  can open any window. A property test over generated `BootError`s shows the
  text and that the exit is not reached.
- Not run: the device check on the OnePlus (fresh install, and a forced boot
  error). The fix of the SIGSEGV is proven only by removing the `exit` that
  the frames show.
