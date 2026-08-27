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

NOT ESTABLISHED — recorded as an observation with a lead, not a diagnosis.

The commit path is
`GpuiInputConnection.commitText` → `currentReplacementRange()` →
`nativeReplaceText(start, end, text)`
(`frontends/gpui/android/java/dev/gpui/mobile/GpuiTextInputView.java`).
`currentReplacementRange()` derives `start`/`end` from the view's own
`SpannableStringBuilder` selection, which is only ever advanced by
`updateEditingState(...)` being called back from Rust. Text landing at offset 0
every time is consistent with that selection never advancing.

**Candidate cause to check FIRST, because this lane introduced it:** the soft
keyboard fix made `updateEditingState` asynchronous — it now does
`view.post(() -> view.applyEditingState(...))` instead of applying inline,
because the JNI caller runs on the `android_main` thread and touching the view
off the UI thread is illegal. If any part of the commit path expects the
editable to be current synchronously, that hop would leave the selection stale.
This is a HYPOTHESIS, not a finding: the keystrokes here were seconds apart, so a
one-frame deferral should have settled long before the next commit, which argues
against it. The competing explanation is that Rust never calls
`updateEditingState` back after a commit at all. Whoever picks this up should
establish which before changing anything.

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

OPEN — deliberately not fixed in the lane that found it, to keep the
soft-keyboard fix landable on its own evidence.

The soft-keyboard work that surfaced this is complete and verified separately
(see `2026-08-27-android-soft-keyboard-never-opens`); this entry is the defect
sitting behind it. Practical impact: on-device text entry now *works* but
produces reversed text, so it is not yet usable.
