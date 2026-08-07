//! Bridge threads: fresh OS threads that run one piece of their spawner's work
//! to escape a runtime context (`Handle::block_on` is illegal on a thread that
//! is already inside a runtime), then join back immediately.
//!
//! Such a thread is a CONTINUATION of its spawner, but the OS and every
//! thread-keyed facility see an anonymous new thread. Observability tooling
//! that attributes work per thread — the PBT harness charges SQL spans to the
//! test scope owning the emitting thread — therefore loses everything a bridge
//! does unless the relationship is made explicit. [`capture`] carries it
//! across.
//!
//! Without an installed [`BridgeThreadHook`] this is two moves of an `Option`.

use std::sync::OnceLock;

/// How an observability system identifies the context a bridge must inherit.
/// Opaque here on purpose: this module knows only that the value names the
/// spawner's context and that the installer can re-enter it.
#[derive(Clone, Copy, Debug)]
pub struct BridgeThreadHook {
    /// The calling thread's context, read on the SPAWNER before the bridge
    /// starts. `None` means the spawner has no context to pass on.
    pub current: fn() -> Option<u64>,
    /// Enter `context` on the calling (bridge) thread.
    pub enter: fn(u64),
    /// Leave the context — the bridge thread is about to exit, and thread ids
    /// are recycled, so a left-behind registration would misattribute whatever
    /// thread the OS hands the id to next.
    pub leave: fn(),
}

static HOOK: OnceLock<BridgeThreadHook> = OnceLock::new();

/// Install the process's bridge-thread hook. First call wins; later calls are
/// ignored, so a harness may install unconditionally at subscriber setup.
pub fn install_bridge_thread_hook(hook: BridgeThreadHook) {
    let _ = HOOK.set(hook);
}

/// Capture the spawner's context. Call on the SPAWNING thread, then
/// [`BridgeContext::run`] the bridge body inside the new thread.
pub(crate) fn capture() -> BridgeContext {
    BridgeContext(
        HOOK.get()
            .and_then(|hook| (hook.current)().map(|c| (*hook, c))),
    )
}

/// The spawner's context, ready to be entered on a bridge thread.
pub(crate) struct BridgeContext(Option<(BridgeThreadHook, u64)>);

impl BridgeContext {
    /// Run the bridge body with the spawner's context entered.
    pub(crate) fn run<T>(self, body: impl FnOnce() -> T) -> T {
        let Some((hook, context)) = self.0 else {
            return body();
        };
        (hook.enter)(context);
        // A panicking bridge body must still leave: the thread id it holds is
        // about to be recycled to an unrelated thread.
        let _leave = LeaveOnDrop(hook);
        body()
    }
}

struct LeaveOnDrop(BridgeThreadHook);

impl Drop for LeaveOnDrop {
    fn drop(&mut self) {
        (self.0.leave)();
    }
}
