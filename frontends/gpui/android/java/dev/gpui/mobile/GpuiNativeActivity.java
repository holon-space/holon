package dev.gpui.mobile;

import android.app.NativeActivity;

/**
 * NativeActivity that loads the native library through the classloader.
 *
 * NativeActivity's own loadNativeCode() dlopens the library without registering
 * it with this class's classloader, so GpuiTextInputView's native methods would
 * not resolve. System.loadLibrary here registers it before any of them is called.
 */
public class GpuiNativeActivity extends NativeActivity {
    static {
        System.loadLibrary("holon_gpui");
    }
}
