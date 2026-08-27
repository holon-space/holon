//! Soft-keyboard focus lifecycle.
//!
//! The platform keyboard must be up exactly while a text input owns focus.
//! gpui delivers Blur/Focus in no guaranteed order on a block→block focus
//! move (the zombie-editor blur can arrive AFTER the next editor's focus),
//! so a naive hide-on-blur dismisses the keyboard mid-editing. Guard with a
//! focus generation counter: every focus bumps it; a blur schedules a
//! deferred hide that only fires if no focus arrived in the meantime.
//!
//! Only [`platform_show_keyboard`] / [`platform_hide_keyboard`] are
//! platform-specific. The decision *policy* compiles on every target so a
//! windowed PBT on the desktop can drive it and read back
//! [`keyboard_hide_requests`].

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use gpui::App;
use gpui::FocusHandle;
use gpui::Window;

static KEYBOARD_FOCUS_GENERATION: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_SHOW_REQUESTS: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_HIDE_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// How long a scheduled hide waits for a successor focus before firing.
/// One frame is enough for the mount→grab pipeline; 150ms adds margin for
/// slow re-renders (variant switch re-mounts the editor) without a user-
/// perceivable keyboard flicker window.
pub const KEYBOARD_HIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

/// Number of raise requests the lifecycle has issued to the platform.
pub fn keyboard_show_requests() -> u64 {
    KEYBOARD_SHOW_REQUESTS.load(Ordering::SeqCst)
}

/// Number of dismiss requests the lifecycle has issued to the platform.
/// The soft-keyboard invariant is observable through this counter: it may
/// only advance while no text input holds window focus.
pub fn keyboard_hide_requests() -> u64 {
    KEYBOARD_HIDE_REQUESTS.load(Ordering::SeqCst)
}

/// A text input gained focus: claim the next generation (cancelling any
/// pending deferred hide) and raise the platform soft keyboard. Returns the
/// generation this focus claimed; the editor stores it and passes it back to
/// [`editor_focus_lost`] so a *stale* editor's later blur cannot hide the
/// keyboard out from under whoever currently holds focus.
pub fn editor_focus_gained() -> u64 {
    let generation = KEYBOARD_FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::debug!(generation, "soft keyboard: show (editor focus)");
    platform_show_keyboard();
    generation
}

/// A text input reported a focus-out EVENT (gpui `InputEvent::Blur`, or a
/// `cx.on_blur` listener). Such an event is NOT proof that focus moved:
/// gpui derives it from the rendered frame's focus PATH, which goes empty
/// when the focused element is absent from that frame's dispatch tree or
/// the window is inactive — `window.focus` keeps naming the input in both
/// cases (`Window::focus_path`, `App::release_dropped_focus_handles`). An
/// Android IME inset resize therefore reaches here as a blur although the
/// caret never left the editor.
///
/// Authoritative test: does the input still hold window focus? If it does,
/// the event was a geometry/activation artefact and the keyboard stays up.
pub fn editor_blur_event(window: &Window, focus: &FocusHandle, cx: &mut App, my_generation: u64) {
    if focus.is_focused(window) {
        tracing::debug!(
            my_generation,
            "soft keyboard: hide skipped (blur event while the input still holds window focus \
             — relayout or window deactivation, not a focus move)"
        );
        return;
    }
    editor_focus_lost(cx, my_generation);
}

/// A text input lost focus: schedule a deferred hide keyed to the generation
/// that focus *claimed* (`my_generation`, from the matching
/// [`editor_focus_gained`]).
///
/// The bare generation counter only guards blur-BEFORE-focus (a successor's
/// focus bumps the counter, so a hide scheduled by the predecessor's earlier
/// blur is skipped). It does NOT guard blur-AFTER-focus: gpui delivers
/// Focus/Blur unordered on a block→block move (and on the iOS render-path the
/// unmounting editor's `is_focused=false` edge can be evaluated *after* the
/// successor's `true` edge in the same frame), so the stale editor's blur
/// reads the already-advanced counter and schedules a hide that nothing
/// cancels — the keyboard drops ~150ms after focus though a block is focused.
///
/// Fix: only the editor still holding the current generation may schedule a
/// hide. A stale editor (`my_generation != current`) has already been
/// superseded by a later focus and its blur is ignored.
///
/// Callers that only have a focus-out *event* must go through
/// [`editor_blur_event`] instead — this entry point assumes the caller has
/// already established that focus really left.
pub fn editor_focus_lost(cx: &mut App, my_generation: u64) {
    if KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst) != my_generation {
        tracing::debug!(
            my_generation,
            "soft keyboard: hide skipped (stale editor blur; focus already moved on)"
        );
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(KEYBOARD_HIDE_GRACE).await;
        if KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst) == my_generation {
            tracing::debug!("soft keyboard: hide (editor blur, no refocus)");
            platform_hide_keyboard();
        } else {
            tracing::debug!("soft keyboard: hide skipped (focus moved to another input)");
        }
    })
    .detach();
}

fn platform_show_keyboard() {
    KEYBOARD_SHOW_REQUESTS.fetch_add(1, Ordering::SeqCst);
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::show_keyboard();
    #[cfg(all(feature = "mobile", not(any(target_os = "ios", target_os = "android"))))]
    tracing::warn!(
        "soft keyboard show requested but this platform has no soft-keyboard backend (mobile \
         feature enabled on a desktop OS) — input continues via hardware keyboard"
    );
}

fn platform_hide_keyboard() {
    KEYBOARD_HIDE_REQUESTS.fetch_add(1, Ordering::SeqCst);
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::hide_keyboard();
    #[cfg(all(feature = "mobile", not(any(target_os = "ios", target_os = "android"))))]
    tracing::warn!(
        "soft keyboard hide requested but this platform has no soft-keyboard backend (mobile \
         feature enabled on a desktop OS)"
    );
}
