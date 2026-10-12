package dev.gpui.mobile;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;

/**
 * System clipboard access for the GPUI Android platform, called from Rust via JNI
 * (gpui-mobile {@code src/packages/clipboard/android.rs}).
 *
 * <p>A missing {@code ClipboardManager} throws; the Rust side logs the exception.
 * {@link #getText} returns {@code null} when Android withholds the clip, which it
 * does while the app is not the focused foreground app.</p>
 */
public final class GpuiClipboard {

    public static void setText(Activity activity, String text) {
        manager(activity).setPrimaryClip(ClipData.newPlainText("text", text));
    }

    public static String getText(Activity activity) {
        ClipData clip = manager(activity).getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) return null;
        CharSequence text = clip.getItemAt(0).getText();
        return text != null ? text.toString() : null;
    }

    public static boolean hasText(Activity activity) {
        return getText(activity) != null;
    }

    private static ClipboardManager manager(Activity activity) {
        ClipboardManager cm = activity.getSystemService(ClipboardManager.class);
        if (cm == null) throw new IllegalStateException("no ClipboardManager system service");
        return cm;
    }

    private GpuiClipboard() {}
}
