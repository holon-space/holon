// Vendored from the gpui-mobile fork, origin rev c0685fd:
//   example/android/gradle/app/src/main/java/dev/gpui/mobile/GpuiTextInputView.java
// Provides the platform-IME host View (dev.gpui.mobile.GpuiTextInputView) that the
// GPUI Android runtime looks up via find_app_class + JNI to raise the soft keyboard.
// Self-contained: imports only android.* (no androidx, no other Gpui* classes).
// Identical to the fork's copy on branch holon-ime-fixes apart from this header;
// re-sync from the new rev when bumping the pin.

package dev.gpui.mobile;

import android.app.Activity;
import android.content.Context;
import android.graphics.Rect;
import android.text.Editable;
import android.text.InputType;
import android.text.Selection;
import android.text.SpannableStringBuilder;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

/**
 * Hidden Android text editor used only to host the platform IME.
 *
 * The visible text is rendered by GPUI. This view mirrors enough text,
 * selection, and composing state for Android IMEs to behave like they do with
 * EditText, then forwards exact edit commands to Rust/GPUI.
 */
public final class GpuiTextInputView extends View {
    // Written on the UI thread (ensureView runs inside the showKeyboard runnable)
    // and read on the android_main thread by updateEditingState, so both need a
    // happens-before edge; without one a stale null silently drops updates.
    private static volatile GpuiTextInputView sView;
    private static volatile int sKeyboardType;

    /** Frames to keep re-asking for the IME while the view is not yet served. */
    private static final int IME_REQUEST_ATTEMPTS = 12;

    private final SpannableStringBuilder editable = new SpannableStringBuilder();
    private boolean applyingNativeState;

    public GpuiTextInputView(Context context) {
        super(context);
        setFocusable(true);
        setFocusableInTouchMode(true);
        // ViewRootImpl never makes a non-VISIBLE view the served view, so an
        // INVISIBLE host can never take focus nor raise the IME. Stay VISIBLE
        // and invisible in practice: 1x1 (see ensureView), transparent, unpainted.
        setVisibility(VISIBLE);
        setBackgroundColor(android.graphics.Color.TRANSPARENT);
        setAlpha(0f);
        setWillNotDraw(true);
    }

    // JNI calls these from the android_main thread, but view and IMM APIs are
    // only legal on the Android UI thread — hence the hops below.
    public static void showKeyboard(Activity activity, int keyboardType) {
        activity.runOnUiThread(() -> {
            GpuiTextInputView view = ensureView(activity);
            sKeyboardType = keyboardType;
            view.requestFocus();

            InputMethodManager imm =
                    (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null) {
                imm.restartInput(view);
                requestIme(view, imm, IME_REQUEST_ATTEMPTS);
            }
        });
    }

    public static void hideKeyboard(Activity activity) {
        activity.runOnUiThread(() -> {
            if (sView == null) {
                return;
            }
            InputMethodManager imm =
                    (InputMethodManager) activity.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm != null) {
                imm.hideSoftInputFromWindow(sView.getWindowToken(), 0);
            }
            sView.clearFocus();
        });
    }

    public static void updateEditingState(
            String text,
            int selectionStart,
            int selectionEnd,
            int composingStart,
            int composingEnd,
            boolean selectionReversed) {
        GpuiTextInputView view = sView;
        if (view == null || text == null) {
            return;
        }
        view.post(() -> view.applyEditingState(
                text, selectionStart, selectionEnd, composingStart, composingEnd, selectionReversed));
    }

    /**
     * Ask for the IME, retrying on the next frame while the request is refused.
     *
     * requestFocus() only queues the focus change; ViewRootImpl makes this view
     * the served view during a later traversal, and showSoftInput returns false
     * until then.
     */
    private static void requestIme(GpuiTextInputView view, InputMethodManager imm, int attemptsLeft) {
        if (!imm.showSoftInput(view, InputMethodManager.SHOW_IMPLICIT) && attemptsLeft > 0) {
            view.post(() -> requestIme(view, imm, attemptsLeft - 1));
        }
    }

    private static GpuiTextInputView ensureView(Activity activity) {
        if (sView != null) {
            return sView;
        }

        GpuiTextInputView view = new GpuiTextInputView(activity);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(1, 1);
        ViewGroup content = activity.findViewById(android.R.id.content);
        content.addView(view, params);
        sView = view;
        return view;
    }

    private void applyEditingState(
            String text,
            int selectionStart,
            int selectionEnd,
            int composingStart,
            int composingEnd,
            boolean selectionReversed) {
        applyingNativeState = true;
        editable.replace(0, editable.length(), text);
        int textLength = editable.length();
        if (selectionStart < 0 || selectionEnd < 0) {
            // No selection upstream. Clamping to 0 would assert the caret sits
            // before the first character, and every commitText would then compute
            // a replacement range of (0,0) and prepend instead of insert.
            Selection.removeSelection(editable);
        } else {
            int selStart = clamp(selectionStart, 0, textLength);
            int selEnd = clamp(selectionEnd, 0, textLength);
            Selection.setSelection(
                    editable,
                    selectionReversed ? selEnd : selStart,
                    selectionReversed ? selStart : selEnd);
        }
        BaseInputConnection.removeComposingSpans(editable);
        if (composingStart >= 0 && composingEnd > composingStart) {
            new BaseInputConnection(this, true) {
                @Override
                public Editable getEditable() {
                    return editable;
                }
            }.setComposingRegion(
                    clamp(composingStart, 0, textLength), clamp(composingEnd, 0, textLength));
        }
        applyingNativeState = false;
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
        outAttrs.inputType = androidInputType(sKeyboardType);
        // An explicit action other than IME_ACTION_NONE replaces the IME's
        // Enter key with that action's button, which leaves a multi-line editor
        // with no way to enter a line break.
        boolean multiLine = (outAttrs.inputType & InputType.TYPE_TEXT_FLAG_MULTI_LINE) != 0;
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN
                | (multiLine ? EditorInfo.IME_ACTION_NONE : EditorInfo.IME_ACTION_DONE);
        outAttrs.initialSelStart = Selection.getSelectionStart(editable);
        outAttrs.initialSelEnd = Selection.getSelectionEnd(editable);
        return new GpuiInputConnection(this, outAttrs);
    }

    private static int androidInputType(int keyboardType) {
        switch (keyboardType) {
            case 1:
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS;
            case 2:
                return InputType.TYPE_CLASS_PHONE;
            case 3:
                return InputType.TYPE_CLASS_NUMBER;
            case 4:
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI;
            case 5:
                return InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL;
            case 6:
                return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD;
            case 0:
            default:
                return InputType.TYPE_CLASS_TEXT
                        | InputType.TYPE_TEXT_FLAG_MULTI_LINE
                        | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT
                        | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES;
        }
    }

    private static int clamp(int value, int min, int max) {
        return Math.max(min, Math.min(max, value));
    }

    private static final class GpuiInputConnection extends BaseInputConnection {
        private final GpuiTextInputView view;
        private final EditorInfo editorInfo;

        GpuiInputConnection(GpuiTextInputView view, EditorInfo editorInfo) {
            super(view, true);
            this.view = view;
            this.editorInfo = editorInfo;
        }

        @Override
        public Editable getEditable() {
            return view.editable;
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            if (isLineBreak(text)) {
                // Not written into the mirror: GPUI answers Return with a block
                // split, so the buffer this mirrors never holds a newline.
                nativeKeyEnter();
                return true;
            }
            Range range = currentReplacementRange();
            boolean result = super.commitText(text, newCursorPosition);
            if (result && !view.applyingNativeState) {
                nativeReplaceText(range.start, range.end, text.toString());
                notifySelectionIfNeeded(range.start + text.length());
            }
            return result;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            Range range = currentReplacementRange();
            boolean result = super.setComposingText(text, newCursorPosition);
            if (result && !view.applyingNativeState) {
                int composingStart = BaseInputConnection.getComposingSpanStart(view.editable);
                int composingEnd = BaseInputConnection.getComposingSpanEnd(view.editable);
                int selStart = Selection.getSelectionStart(view.editable);
                int selEnd = Selection.getSelectionEnd(view.editable);
                int base = composingStart >= 0 ? composingStart : range.start;
                nativeSetMarkedText(
                        range.start,
                        range.end,
                        text.toString(),
                        Math.max(0, selStart - base),
                        Math.max(0, selEnd - base));
                if (composingStart < 0 || composingEnd <= composingStart) {
                    nativeUnmarkText();
                }
            }
            return result;
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            boolean result = super.setComposingRegion(start, end);
            if (result && !view.applyingNativeState) {
                int safeStart = clamp(Math.min(start, end), 0, view.editable.length());
                int safeEnd = clamp(Math.max(start, end), 0, view.editable.length());
                nativeSetMarkedText(
                        safeStart,
                        safeEnd,
                        view.editable.subSequence(safeStart, safeEnd).toString(),
                        0,
                        safeEnd - safeStart);
            }
            return result;
        }

        @Override
        public boolean finishComposingText() {
            boolean result = super.finishComposingText();
            if (result && !view.applyingNativeState) {
                nativeUnmarkText();
            }
            return result;
        }

        @Override
        public boolean setSelection(int start, int end) {
            boolean result = super.setSelection(start, end);
            if (result && !view.applyingNativeState) {
                nativeSetSelection(start, end);
            }
            return result;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            Range range = surroundingRange(beforeLength, afterLength, false);
            boolean result = super.deleteSurroundingText(beforeLength, afterLength);
            if (result && !view.applyingNativeState) {
                nativeReplaceText(range.start, range.end, "");
            }
            return result;
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
            Range range = surroundingRange(beforeLength, afterLength, true);
            boolean result = super.deleteSurroundingTextInCodePoints(beforeLength, afterLength);
            if (result && !view.applyingNativeState) {
                nativeReplaceText(range.start, range.end, "");
            }
            return result;
        }

        /**
         * IMEs that answer Return with a key event instead of `commitText`
         * reach Return through here. Consumed rather than forwarded so both
         * routes produce exactly one split.
         */
        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            boolean enter = event.getKeyCode() == KeyEvent.KEYCODE_ENTER
                    || event.getKeyCode() == KeyEvent.KEYCODE_NUMPAD_ENTER;
            if (enter) {
                if (event.getAction() == KeyEvent.ACTION_DOWN) {
                    nativeKeyEnter();
                }
                return true;
            }
            return super.sendKeyEvent(event);
        }

        private static boolean isLineBreak(CharSequence text) {
            String value = text.toString();
            return value.equals("\n") || value.equals("\r") || value.equals("\r\n");
        }

        @Override
        public boolean performEditorAction(int actionCode) {
            if ((editorInfo.inputType & InputType.TYPE_TEXT_FLAG_MULTI_LINE) == 0
                    || actionCode == EditorInfo.IME_ACTION_DONE) {
                hideKeyboard((Activity) view.getContext());
                return true;
            }
            return super.performEditorAction(actionCode);
        }

        private Range currentReplacementRange() {
            int composingStart = BaseInputConnection.getComposingSpanStart(view.editable);
            int composingEnd = BaseInputConnection.getComposingSpanEnd(view.editable);
            if (composingStart >= 0 && composingEnd >= composingStart) {
                return new Range(composingStart, composingEnd);
            }
            int selStart = Selection.getSelectionStart(view.editable);
            int selEnd = Selection.getSelectionEnd(view.editable);
            if (selStart < 0 || selEnd < 0) {
                return new Range(view.editable.length(), view.editable.length());
            }
            return new Range(Math.min(selStart, selEnd), Math.max(selStart, selEnd));
        }

        private Range surroundingRange(int beforeLength, int afterLength, boolean codePoints) {
            int selStart = Selection.getSelectionStart(view.editable);
            int selEnd = Selection.getSelectionEnd(view.editable);
            if (selStart < 0 || selEnd < 0) {
                return new Range(0, 0);
            }
            int start = Math.min(selStart, selEnd);
            int end = Math.max(selStart, selEnd);
            if (start == end) {
                if (codePoints) {
                    start = offsetByCodePoints(start, -beforeLength);
                    end = offsetByCodePoints(end, afterLength);
                } else {
                    start = clamp(start - beforeLength, 0, view.editable.length());
                    end = clamp(end + afterLength, 0, view.editable.length());
                }
            }
            return new Range(start, end);
        }

        private int offsetByCodePoints(int offset, int delta) {
            try {
                return Character.offsetByCodePoints(view.editable, offset, delta);
            } catch (IndexOutOfBoundsException ignored) {
                return delta < 0 ? 0 : view.editable.length();
            }
        }

        private void notifySelectionIfNeeded(int expectedCursor) {
            int selStart = Selection.getSelectionStart(view.editable);
            int selEnd = Selection.getSelectionEnd(view.editable);
            if (selStart != expectedCursor || selEnd != expectedCursor) {
                nativeSetSelection(selStart, selEnd);
            }
        }
    }

    private static final class Range {
        final int start;
        final int end;

        Range(int start, int end) {
            this.start = Math.min(start, end);
            this.end = Math.max(start, end);
        }
    }

    private static native void nativeReplaceText(int start, int end, String text);

    private static native void nativeSetMarkedText(
            int start, int end, String text, int selectionStart, int selectionEnd);

    private static native void nativeUnmarkText();

    /** Soft Return, delivered as an `enter` keystroke rather than as text. */
    private static native void nativeKeyEnter();

    private static native void nativeSetSelection(int start, int end);
}
