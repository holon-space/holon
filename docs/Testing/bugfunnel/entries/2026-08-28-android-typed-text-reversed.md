---
id: 2026-08-28-android-typed-text-reversed
date: 2026-08-28
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  On Android every committed character is inserted at offset 0 instead of at the
  caret, so typing "H i a s" stores "saiH" — text accumulates backwards.
---

## Bug

Found by agent exploration on an API 36 emulator, immediately after the soft
keyboard was made to work (`2026-08-27-android-soft-keyboard-never-opens`).
With the keyboard up, tapping the IME's own keys — which drives the real
`InputConnection.commitText` path, unlike `adb shell input text` — put the
characters into the block in reverse.

Typed `H`, `i`, `a`, `s`. The vault file Holon itself wrote to disk:

```
/sdcard/Android/data/space.holon.kbdtest/files/holon-pkm/Journals/2026-08-28.org
* saiH
```

The on-disk file is the evidence, so this is not a rendering artifact: every
keystroke is committed at offset 0 and the stored text is the reverse of the
typed sequence. Reproduced across four keystrokes with ~2s between each, so it
is not a fast-typing race.

## Root cause

ESTABLISHED — the Java mirror is overwritten with sentinel state every frame.

The commit path is
`GpuiInputConnection.commitText` → `currentReplacementRange()` →
`nativeReplaceText(start, end, text)`
(`frontends/gpui/android/java/dev/gpui/mobile/GpuiTextInputView.java`).
`currentReplacementRange()` derives `start`/`end` from the view's own
`SpannableStringBuilder`, so that mirror is what every keystroke aims at.

The chain that corrupts it (fork `src/android/`):

1. `window.rs` `set_input_handler` called `sync_state_to_java` inline. gpui
   invokes that from inside `Window::draw`, with the `App` already borrowed.
2. `AsyncApp::update_window` therefore fails its `try_borrow_mut`, and gpui
   discards the failure with `.ok()`.
3. `sync_state_to_java` had `.unwrap_or_default()` on the text and
   `.unwrap_or((-1, -1, false))` on the selection, so instead of aborting it
   shipped `text=""` and `selection=(-1,-1)` to Java — every frame.
4. `applyEditingState` clamped that negative selection to 0, asserting the caret
   sits before the first character.
5. So every `commitText` computed `Range(0, 0)` and prepended. Typing `H i a s`
   stores `saiH`.

Each step is individually reasonable and the composition is silent: an error is
swallowed at (2), a placeholder substituted at (3), and a defensive clamp at (4)
converts the placeholder into a plausible-looking caret position. Nothing logs.

The asynchronous `view.post` hop introduced by the soft-keyboard fix was
considered and is NOT the cause — the mirror was being actively overwritten with
sentinels every frame, so staleness never needed to be timing-dependent.

## Missing piece

No automated layer drives a real `InputConnection`:

- The keystone PBT is headless — no JVM, no IME, and the whole path is
  `#[cfg(target_os = "android")]`.
- The windowed GPUI PBTs run on macOS and take the desktop branch.
- `adb shell input text` goes through instrumentation and injects key events
  directly; it does NOT reach `commitText`, so even an on-device gate built
  around it would miss this entirely.

The rung that catches it is a real typing test: an IME-key tap (`adb shell input
tap` on the soft keyboard's own keys) followed by asserting the *stored* text,
not just that the keyboard is visible. Note how narrowly the existing checks miss
it — `mInputShown` was true, the IME reported a correct `EditorInfo`, no
exception was logged, and the characters did appear on screen. Everything except
the content was green.

Secondary COVERAGE: even granting the platform gap, no test asserts
"text typed in order X is stored in order X" at any layer, so a shared
selection-handling defect would also go unnoticed.

## Remedy

Fix implemented on fork branch `holon-ime-fixes`, NOT yet verified on a device:

- move the mirror out of `set_input_handler` (which only stores the handler now)
  into the frame callback, in the same `input_handler.0.lock()` scope as
  `drain_into` and running every frame, not only when edits are pending;
- `sync_state_to_java` logs and returns instead of substituting placeholder text
  or selection, leaving the Java mirror untouched when the handler can't be read;
- `applyEditingState` treats a negative selection as "no selection"
  (`Selection.removeSelection`) rather than clamping it to 0;
- `sView`/`sKeyboardType` become `volatile`: the soft-keyboard fix moved
  `ensureView` onto the UI thread, so they are now written there and read on
  `android_main` with no happens-before edge — a regression introduced by that
  fix and fixed here.

Verification is BLOCKED: the emulator used for the original measurement died
mid-run and would not restart (see the parent entry's caveat about that rig).
The `saiH` measurement above stands; the expected post-fix result is `Hias`.
