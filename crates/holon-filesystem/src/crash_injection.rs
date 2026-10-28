//! Named points where a test makes the file sync die, as a crash or a power
//! loss would, fail, or wait, to prove what survives and what is disclosed.
//! Feature-gated (`crash-injection`, off by default): a release build has no
//! such point.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use tokio::sync::Notify;

static ARMED: Mutex<Option<&'static str>> = Mutex::new(None);

/// The next time the file sync reaches `point`, it dies or fails there. One
/// shot.
pub fn arm(point: &'static str) {
    *ARMED.lock().expect("crash-injection lock") = Some(point);
}

/// Forget an armed point the test never expected to be reached.
pub fn disarm() {
    *ARMED.lock().expect("crash-injection lock") = None;
}

/// The point armed and not reached yet.
pub fn armed() -> Option<&'static str> {
    *ARMED.lock().expect("crash-injection lock")
}

/// Whether `point` is armed; reaching it disarms it.
pub fn fires(point: &'static str) -> bool {
    let mut armed = ARMED.lock().expect("crash-injection lock");
    if *armed == Some(point) {
        *armed = None;
        return true;
    }
    false
}

pub(crate) fn reached(point: &'static str) {
    if fires(point) {
        panic!("[crash-injection] the file-sync controller dies at {point}");
    }
}

enum RenderHold {
    Off,
    Armed(PathBuf),
    Holding(Arc<Notify>),
}

static RENDER_HOLD: Mutex<RenderHold> = Mutex::new(RenderHold::Off);

/// The next block-change render of the file at `path` waits at its start
/// until [`release_render`]. One shot. Keyed by path because the tests of one
/// binary share this process, so another session's render must pass.
pub fn hold_next_render(path: PathBuf) {
    *RENDER_HOLD.lock().expect("render-hold lock") = RenderHold::Armed(path);
}

/// Whether a render is waiting at the hold now.
pub fn render_held() -> bool {
    matches!(
        *RENDER_HOLD.lock().expect("render-hold lock"),
        RenderHold::Holding(_)
    )
}

/// Lets the held render go on.
pub fn release_render() {
    let hold = std::mem::replace(
        &mut *RENDER_HOLD.lock().expect("render-hold lock"),
        RenderHold::Off,
    );
    match hold {
        RenderHold::Holding(release) => release.notify_one(),
        RenderHold::Armed(path) => panic!(
            "[crash-injection] released a render hold no render of {} reached",
            path.display()
        ),
        RenderHold::Off => panic!("[crash-injection] released a render hold that was never armed"),
    }
}

pub(crate) async fn render_reached(path: &Path) {
    let release = {
        let mut hold = RENDER_HOLD.lock().expect("render-hold lock");
        if !matches!(&*hold, RenderHold::Armed(armed) if armed == path) {
            return;
        }
        let release = Arc::new(Notify::new());
        *hold = RenderHold::Holding(release.clone());
        release
    };
    release.notified().await;
}
