---
id: 2026-08-30-android-ime-no-enter-key
date: 2026-08-30
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  On Android the soft keyboard offered a tick instead of an Enter key, so no new
  block could be created; Return also had no path to the editor's split action.
---

## Bug

Martin, dogfooding v0.0.19 on a DN2103 (2026-08-30): "The keyboard does not show
an enter key, but instead a tick box, so I can't create new blocks using enter."
Typing itself worked — this was the last missing input affordance on Android.

Two independent defects, either of which alone breaks new-block creation:

1. GBoard drew an action button (the tick) where its Enter key belongs.
2. Had Enter been drawn, pressing it would have inserted a literal newline into
   the block's text instead of splitting the block.

## Root cause

**The tick.** `GpuiTextInputView.onCreateInputConnection` set
`imeOptions = IME_FLAG_NO_FULLSCREEN | IME_ACTION_DONE` for every keyboard type.
An IME draws the button for whatever action `imeOptions` names, *replacing* its
Enter key. Holon asks for `KeyboardType::Default`
(`frontends/gpui/src/soft_keyboard.rs:122` → `gpui_mobile::show_keyboard`), which
maps to case 0 of `androidInputType` — already
`TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_MULTI_LINE`. So the input type was right all
along; the hard-coded action overrode it.

**The newline.** GBoard commits Return as `commitText("\n")`. That reached
`nativeReplaceText` → `TextInputCommand::ReplaceText` →
`PlatformInputHandler::replace_text_in_range`, i.e. into the buffer as text. It
never reaches gpui's keymap, so the editor's `Enter` capture — the arm that reads
the live caret and dispatches `split_block`
(`frontends/gpui/src/views/editor_view.rs:1429-1477`) — never runs. iOS already
translates this (`gpui-mobile` `src/ios/window.rs:1191-1215`, "Return was the
missing symmetric case"); Android was never given the same translation.

## Missing piece

No automated layer executes either site. `onCreateInputConnection` is Java that
only runs inside the APK against a real IME, and the JNI bridge below it is
`cfg(target_os = "android")` — neither exists in the keystone's wiring, and no
gate builds the Android target. The keystone's soft-keyboard rung
(`InteractionEvent::InsertText` → `dispatch_insert_text`,
`frontends/gpui/src/lib.rs:3485`) already translates `"\n"` into an `enter`
keystroke, so it modelled the *fixed* behaviour and could never have gone red on
the real one.

## Remedy

Fixed on both halves, mirroring the iOS translation:

- `frontends/gpui/android/java/dev/gpui/mobile/GpuiTextInputView.java` (and the
  fork's `example/` copy, kept identical): `imeOptions` names `IME_ACTION_NONE`
  when the input type carries `TYPE_TEXT_FLAG_MULTI_LINE`, keeping `IME_ACTION_DONE`
  for single-line types. `commitText` of a bare line break, and `sendKeyEvent` of
  `KEYCODE_ENTER`, both call the new `nativeKeyEnter` instead of forwarding text.
- gpui-mobile fork: `TextInputCommand::KeyEnter` and the `nativeKeyEnter` JNI
  export (`src/android/text_input.rs`); the frame callback drains up to a queued
  Return, releases the input-handler lock, and dispatches an `enter` keystroke
  through the same input callback the hardware key uses
  (`src/android/window.rs`).

Pinned by `frontends/gpui/tests/android_soft_return_contract.rs`. It strips the
Java's comments and binds each decision to the branch that makes it — the
multi-line ternary arm to IME_ACTION_NONE and the single-line arm to
IME_ACTION_DONE, `nativeKeyEnter` to the `isLineBreak` guard in `commitText` and
to the ACTION_DOWN guard in `sendKeyEvent` — so reinstating the bug turns it red.
Verified against three mutations: the ternary inverted (the reported bug
verbatim), the fix deleted with its comment kept plus every commit routed to
Return, and the whole pre-fix file.

These remain source predicates, not device assertions: they cannot catch an IME
that honours neither convention, and an emulator run of the packaged APK is still
the only end-to-end check. Ship-order is enforced separately —
`frontends/gpui/android/check-natives.sh`, run by all three packaging scripts,
fails the build when a `native` method the classes declare is not exported by the
`.so` being packaged. JNI resolves natives lazily, so without it a Java/library
mismatch surfaces as an `UnsatisfiedLinkError` on the IME binder thread at the
user's first Return.

Residual: a multi-character commit containing a newline (paste, some autocomplete
flows) still inserts a literal newline. Out of scope here — it is a different
interaction from pressing Return.
