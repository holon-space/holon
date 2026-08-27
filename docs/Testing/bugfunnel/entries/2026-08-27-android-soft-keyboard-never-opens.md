---
id: 2026-08-27-android-soft-keyboard-never-opens
date: 2026-08-27
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  On Android the soft keyboard never opens when a block is focused: Holon does
  call the IME hook, but the whole Android IME bridge runs on the `android_main`
  thread instead of the Java UI thread, and every JNI failure on that path is
  swallowed by `let _ = with_env(...)`.
---

## Bug

Dogfooding the v0.0.18 release APK (95.7MB, `frontends/gpui/android/build-release/holon-release.apk`)
on-device: focusing a block puts the editor into text-entry mode and renders a
caret, but the platform soft keyboard never appears, so text entry is impossible
on the device. Desktop keystrokes are unaffected.

This is the residual leg explicitly left open by
`2026-07-20-soft-keyboard-does-pop-ios-android`, whose remedy fixed only the
search-box path and recorded: "Editor-only tap→keyboard path unchanged and could
NOT be reproduced as a code defect here ... needs on-device confirmation ... to
distinguish not-called vs platform-noop." This entry resolves that fork: it is
**platform-noop**, not not-called.

## Root cause

Holon's call chain is complete and correct — the defect is entirely below it.

Holon calls the hook on block focus:

- `frontends/gpui/src/render/builders/editable_text.rs:116` — the render-path
  focus edge calls `note_focus_gained_mobile()` under `feature = "mobile"`.
- `frontends/gpui/src/views/editor_view.rs:783` → `crate::mobile::editor_focus_gained()`.
- `frontends/gpui/src/mobile.rs:507` → `platform_show_keyboard()` →
  `frontends/gpui/src/mobile.rs:548` `gpui_mobile::show_keyboard()` under
  `#[cfg(target_os = "android")]`.

The Java IME host class ships correctly. It is byte-identical to the fork origin
(only a vendoring header added) and is present in the released APK:

- `frontends/gpui/android/java/dev/gpui/mobile/GpuiTextInputView.java`
- `unzip -l holon-release.apk` lists `classes.dex` (8348 bytes); its strings
  contain `Ldev/gpui/mobile/GpuiTextInputView;` and `showKeyboard`.
- `nm -D lib/arm64-v8a/libholon_gpui.so` exports all four
  `Java_dev_gpui_mobile_GpuiTextInputView_native*` symbols.

**Defect 1 — the IME is driven from a non-UI thread (this is why no keyboard appears).**

`android-activity` runs `android_main` on a thread it spawns itself, separate
from the Java UI thread:

- `android-activity-0.6.1/src/native_activity/glue.rs:908` — `std::thread::spawn(...)`,
  attached to the JVM as `"android_main"` (`glue.rs:943`), which then calls
  `android_main(app)` (`glue.rs:985`).
- `android-activity-0.6.1/src/native_activity/mod.rs:141` and `:144` name the two
  loopers separately: "Looper associated with the Rust `android_main` thread"
  versus "Looper associated with the activity's Java main thread, sometimes
  called the UI thread."

GPUI's foreground executor is bound to the *former*:
`gpui-mobile@75410fc/src/android/dispatcher.rs:163-175` builds the dispatcher
from `ALooper_forThread()` and must be called on the thread running `android_main`.
So every GPUI render/frame callback — including the focus edge above — executes
on `android_main`, never on the UI thread.

`GpuiTextInputView.showKeyboard` then manipulates the view hierarchy directly
from that thread: `ensureView()` calls `content.addView(view, params)`, and
`showKeyboard` calls `view.requestFocus()` before
`imm.showSoftInput(view, SHOW_IMPLICIT)`. `addView` reaches
`ViewRootImpl.requestLayout()` → `checkThread()`, which throws
`CalledFromWrongThreadException`. Independently, `showSoftInput` only succeeds for
the view the ViewRootImpl currently *serves*, and a view focused off the UI thread
never becomes the served view — so even a surviving call is a silent no-op
returning `false`.

There is no UI-thread hop anywhere on this path: grepping
`gpui-mobile@75410fc/src/android/` and its `dev/gpui/mobile/*.java` for
`runOnUiThread|getMainLooper|Handler|post(` returns no hits on the IME path.

This also explains the **iOS/Android asymmetry** — iOS raises the keyboard fine
(`2026-07-09-ios-soft-keyboard-raises-editor-focus`) because on iOS GPUI's main
thread *is* the UIKit main thread, and
`gpui-mobile@75410fc/src/ios/window.rs:1115` defers to the next run-loop
iteration on that same thread. The Android backend copies the shape of that
design onto a thread model where it does not hold.

**Why nothing is logged:** `gpui-mobile@75410fc/src/android/text_input.rs:141-155`
wraps `show_keyboard`/`hide_keyboard` in `let _ = jni_helpers::with_env(...)`.
The `Result<T, String>` is discarded with no log. `find_app_class`
(`src/android/jni.rs:175`) logs on *its* failure, but it succeeds here, so the
failure downstream is completely silent — a direct violation of the repo's
"NEVER swallow errors" rule, and the reason this cost on-device archaeology.

**Defect 2 — native methods are unresolvable under plain `NativeActivity`
(blocks typing even once Defect 1 is fixed).**

`frontends/gpui/android/AndroidManifest.xml` launched
`android.app.NativeActivity`. `NativeActivity` loads the `.so` via `dlopen`
(`loadNativeCode`), which does **not** register the library with the
classloader's native-library list, so JNI cannot resolve a Java class's `native`
methods from it. The fork's own activity documents and works around exactly this
— `GpuiActivity.java:42-64` calls `System.loadLibrary(libName)` explicitly with
the comment "NativeActivity loads the .so via dlopen (loadNativeCode), which does
NOT register JNI symbols with the classloader."

Holon calls `System.loadLibrary` nowhere (grep over `frontends/gpui/android/`
returns nothing), and the fork provides no fallback: the `.so` contains **no**
`JNI_OnLoad` (`nm -D | grep -c JNI_OnLoad` → 0) and `src/android/` contains no
`RegisterNatives` call. So the first IME `commitText` would hit
`UnsatisfiedLinkError` on
`Java_dev_gpui_mobile_GpuiTextInputView_nativeReplaceText`.

**Correction to the in-repo escalation note.** The escalation comment that stood
in `AndroidManifest.xml` (since replaced by this fix) proposed, as the next step,
switching to `dev.gpui.mobile.GpuiActivity` — which costs vendoring androidx
(splashscreen, media) plus an androidx-capable dex/resource build. Reading `GpuiActivity.java` shows it adds a splash screen,
deep-link handling, volume-key routing, and the `System.loadLibrary` call — and
**nothing IME- or thread-related**. It therefore cannot fix Defect 1. It happens
to fix Defect 2, but a one-line `System.loadLibrary` in a minimal
`NativeActivity` subclass achieves that at a fraction of the cost.

## Missing piece

No automated layer observes Android platform-IME behavior at all:

- The keystone
  (`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`) is
  headless. It has no JVM, no Android UI thread, no `InputMethodManager` — the
  entire failing code path is `#[cfg(target_os = "android")]` and does not
  compile into it. It cannot reproduce this at any generator or oracle strength.
- The windowed GPUI PBTs run on macOS, where `platform_show_keyboard` takes the
  desktop `#[cfg(not(any(ios, android)))]` branch (`mobile.rs:551`).
- No gate builds, installs, and drives the APK on a device or emulator, so
  neither the wrong-thread exception nor the missing `System.loadLibrary` is
  observable in CI.

Secondary contributor: the swallowed `let _ = with_env(...)` at
`text_input.rs:141-155` means even a human with logcat sees nothing, so the
defect is invisible to the `dogfood-explorer` gate too.

## Remedy

PARTIAL — all six defects fixed and verified on an emulator: the keyboard
raises, stays up, and typing on it reaches the vault. Not yet confirmed on the
DN2103, and typed text arrives reversed
(`2026-08-28-android-typed-text-reversed`), so on-device text entry is working
but not yet usable.

Applied (Option A, ruled by Martin — in-tree only, no androidx, no fork pin bump):

- Defect 1: `frontends/gpui/android/java/dev/gpui/mobile/GpuiTextInputView.java`
  now hops to the UI thread — `showKeyboard`/`hideKeyboard` wrap their bodies in
  `activity.runOnUiThread(...)`, `updateEditingState` posts through
  `sView.post(...)`. `ensureView` is reached only from inside the `showKeyboard`
  runnable, so its `addView` is on the UI thread too.
- Defect 3: the same file's constructor now keeps the host view `VISIBLE` at 1×1,
  transparent and unpainted, and `showKeyboard` re-asks for the IME on later
  frames while the request is refused (defect 4). The vendored file and the
  fork's copy on branch `holon-ime-fixes` are byte-identical apart from the
  vendoring header.
- Defect 2: new `frontends/gpui/android/java/dev/gpui/mobile/GpuiNativeActivity.java`
  — a `NativeActivity` subclass whose static initializer calls
  `System.loadLibrary("holon_gpui")`, so the class's four `native` methods
  resolve. `AndroidManifest.xml` launches it instead of `android.app.NativeActivity`
  and keeps the `android.app.lib_name` meta-data. The launch component was updated
  in `build-apk.sh`, `frontends/gpui/justfile`, and the `dogfood-explorer` skill;
  all three packaging scripts now compile both Java sources.

Evidence (host-side, no device attached): `javac -source 8 -target 8` and
`d8 --min-api 33` accept both sources; `build-apk.sh release`,
`build-release-apk.sh`, and `build-release-aab.sh` all complete; the shipped
`holon-release.apk` (85.9MB, under the 157MB cap) carries a `classes.dex` whose
strings contain `GpuiNativeActivity`, `loadLibrary`, `holon_gpui`, `runOnUiThread`
and all four `native*` method names, and `aapt2 dump badging` reports
`launchable-activity: name='dev.gpui.mobile.GpuiNativeActivity'`.

### Device evidence for defects 1 and 2 (both PROVEN fixed, 2026-08-28)

Verified on Martin's DN2103 by packaging the fixed build under a throwaway
package id (`aapt2 --rename-manifest-package space.holon.kbdtest`) so his real
install was never touched.

- Defect 1 — the UI-thread hop is observable in logcat as a thread change within
  a single call: the JNI entry logs on the `android_main` thread and the
  resulting `InputMethodManager` call logs on the process's main thread.
  `show_keyboard_android` on TID 25209, `InputMethodManager` on TID 18305 = PID.
  `CalledFromWrongThreadException` count is 0 across every capture, and the host
  view is constructed and added instead of throwing.
- Defect 2 — the activity launches and takes focus, which only happens if the
  `System.loadLibrary` static initializer succeeded:
  `InputDispatcher: setFocusedApplication displayId=0 ActivityRecord{21d79f u0 space.holon.kbdtest/dev.gpui.mobile.GpuiNativeActivity}`.
  `UnsatisfiedLinkError` count is 0 across every capture.

### What fixing them uncovered — four further defects

Defects 1 and 2 were real and necessary, but they were not the whole chain. Each
one below was only reachable once the ones above it were fixed.

- **Defect 3 — the fork focuses a view it made unfocusable (ROOT CAUSE of the
  original symptom, fix PROVEN, not yet landed).** The constructor calls
  `setVisibility(INVISIBLE)` and `showKeyboard` then calls `requestFocus()` on
  that same view. `View.requestFocusNoSearch()` refuses any view whose visibility
  is not `VISIBLE`, so the view never becomes the served view and `IMM` logs
  `Ignoring showSoftInput() as view=...GpuiTextInputView{... IFED..... ......I. 0,0-1,1} is not served`
  (flag position 0 = `I` = INVISIBLE). Proven by A/B on device: with the
  INVISIBLE view, `GoogleInputMethodService.onWindowShown()` never appears in any
  control capture and the IME window is never relaid out to visible; with the
  view made `VISIBLE` + transparent + 1×1, the IME window is added, drawn and
  shown, and the IME receives a real `EditorInfo`
  (`inputType=2c001, Normal[MultiLine|CapSentences|AutoCorrect]`) instead of the
  previous `inputType=0, inputTypeString=NULL`.
- **Defect 4 — the IME is requested before the served-view handoff completes.**
  `showKeyboard` calls `requestFocus()` and `showSoftInput()` in one runnable, but
  `ViewRootImpl` only makes the view the served view on a later traversal:
  measured `showSoftInput` at `.923` versus `onStartInput` at `.950`. Deferring by
  one frame still lost the race; the keyboard raised anyway via the later
  `startInput` handshake, so this is a refinement rather than a prerequisite.
- **Defect 5 — `find_app_class` fails on the TAP path, so `showKeyboard` is never
  invoked at all.** `find_app_class(dev.gpui.mobile.GpuiTextInputView): getClassLoader failed: Java exception was thrown`,
  in 4 of 4 tap-driven trials across two builds; the same call succeeds on the
  resume path. Every JNI call fails while an exception is pending, so one leaked
  exception poisons the next unrelated call on that thread. The lifecycle hooks
  probe `dev.gpui.mobile.GpuiPlatformView` — a class Holon does not ship — on
  every Resume and Pause, which is the suspected source; that link is
  **UNPROVEN**. Because tapping a block is the actual user gesture, this alone
  keeps the keyboard dead even with defects 1–4 fixed.
- **Defect 6 — Holon dismisses its own keyboard (~600 ms), and this one is
  OURS, not the fork's.** Identical signature in two independent runs: the
  keyboard appears, `safe_area_insets` grows from `bottom=48` to `bottom=792`
  (the IME height, via `android:windowSoftInputMode="adjustResize"`), and ~600 ms
  later Holon itself calls `hide_keyboard_android` and the IME window is hidden.
  So the inset resize triggers a relayout that drops editor focus, and the
  focus-lost path closes the keyboard. This is the last user-visible blocker and
  matches Martin's independent observation of "a keyboard pop up and disappear a
  few times" with "an automatic reaction that makes the page change".

Note on that observation: an earlier revision of this entry concluded the
keyboard never raised at all. That was wrong — `mInputShown` and the screencaps
were sampled seconds after the ~650 ms window had already closed. Martin's naked
eye was the correct instrument; the log timestamps confirm him.

### Emulator verdict, 2026-08-28 — all six green

Run on an API 36 emulator (`Medium_Phone_API_36.0`, `swiftshader_indirect`,
arm64-v8a) against the fork branch `holon-ime-fixes` plus the Holon-side fix at
`a7d4ec05`, packaged as the throwaway `space.holon.kbdtest`:

| # | Defect | Evidence |
|---|---|---|
| 1 | wrong thread | `CalledFromWrongThreadException` = 0 |
| 2 | natives unresolvable | `UnsatisfiedLinkError` = 0, and typing persists to disk (below) |
| 3 | INVISIBLE host view | `mInputShown=true`, `mImeWindowVis=3`, `mDecorViewVisible=true`, InputMethod window live in WindowManager, keyboard visible in the screenshot |
| 4 | served-view race | `is not served` = 0 |
| 5 | `find_app_class` | no CheckJNI abort, no SIGABRT, process alive throughout |
| 6 | self-dismiss | no `hide_keyboard_android`, no `onWindowHidden`; keyboard stayed up across minutes and a dozen taps |

Defect 2 is proven end-to-end rather than by absence of an error: tapping the
IME's own keys drove `commitText` → `nativeReplaceText` → Holon → the vault file
that Holon's own sync controller writes to disk. That is the test we had assumed
needed a human finger; `adb shell input tap` on the soft keyboard's keys does it.

The fix for defect 5 was NOT either of the two reference-lifetime corrections
tried first — both left the abort byte-identical. It was replacing the
hand-rolled `activity.getClassLoader().loadClass(name)` reflection with jni's own
`Env::load_class`, which consults the thread context class loader that
`android-activity` sets up and scopes the name string the way the runtime
requires.

Caveat: the emulator arms ART's CheckJNI, which is what made defect 5 findable at
all, but also makes it stricter than the DN2103. Real-hardware confirmation is
still outstanding.

Still open:

- **Confirmation on the DN2103** — the emulator is stricter than the phone;
  `MORNING-RUN-THIS.sh` plus the packaged APK reruns the same verdict there.
- **Reversed text** — `2026-08-28-android-typed-text-reversed`. Text entry works
  but stores characters backwards, so it is not yet usable.
- **JNI_OnLoad + RegisterNatives (fork)** — deferred. `GpuiNativeActivity` already
  resolves the natives for Holon, so this is belt-and-braces for embedders that
  ship no activity subclass.
- **The gate that would have caught all of it** — an emulator smoke gate
  asserting the keyboard raises AND is still up a second later AND that typed
  text arrives in order. Each of those catches a different one of these defects:
  a one-shot `mInputShown` check would have passed defect 6, and every check
  short of reading the stored text passes the reversed-text defect. No such gate
  exists, so this ships unpinned by any automated test — the `holon-feature` PBT
  exception recorded in the fix plan.
