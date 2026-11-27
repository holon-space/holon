//! The one window settle of the GPUI tests: pump the window until a committed
//! frame reflects every change the engine emitted, and the window is at rest.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use gpui::App;
use gpui::AppContext as _;
use gpui::BackgroundExecutor;
use gpui::EntityId;
use gpui::HeadlessAppContext;
use gpui::TestAppContext;
use holon_api::Clock;
use holon_api::EntityUri;
use holon_api::SystemClock;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::render_epoch::EngineSeq;
use holon_frontend::render_epoch::RenderEpoch;
use holon_frontend::render_epoch::UiWatermark;
use holon_gpui::geometry::BoundsRegistry;

/// The longest settle a test may ask for: below the 2-minute nextest cap of a
/// windowed test, so a hang ends as a settle timeout that names its laggard.
pub const HANG_DEADLINE: Duration = Duration::from_secs(60);

/// The window was not at rest with a frame at the engine's target by the hang
/// deadline.
#[derive(Clone, Debug)]
pub struct SettleTimeout {
    pub deadline: Duration,
    pub epoch: RenderEpoch,
    pub target: EngineSeq,
    /// The seq through which the engine routed every change to its watchers:
    /// a frame can reflect no seq above it.
    pub routed: EngineSeq,
    /// The last pump committed a new frame, or a frame forced for the pending
    /// views changed the screen: something keeps the window dirty.
    pub dirty: bool,
    /// The views a notify during a draw marked dirty that no frame has drawn.
    pub pending_views: Vec<EntityId>,
    /// The UI subscriptions that hold the watermark, with why.
    pub owed: Vec<String>,
    /// The engine's admitted operations that have not finished.
    pub operations_in_flight: usize,
    /// The rows whose bridge waits on a shut test hold.
    pub held_bridges: Vec<EntityUri>,
    /// A pump ran [`PUMP_TICKS`] tasks without the executor parking: a task
    /// wakes itself on every poll.
    pub runnable: bool,
}

impl SettleTimeout {
    /// The report names what the settle still waits for.
    pub fn names_a_wait(&self) -> bool {
        !self.owed.is_empty() || self.runnable || self.routed < self.target
    }
}

#[derive(Debug)]
pub enum SettleError {
    Timeout(SettleTimeout),
    Loading(RenderEpoch),
    Engine(anyhow::Error),
    /// The window settled, but the change the caller waits for is not painted.
    Undelivered(RenderEpoch),
}

impl std::fmt::Display for SettleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettleError::Timeout(t) => write!(
                f,
                "window did not settle within {:?}: frame {:?} reflects {:?}, target {:?}, \
                 routed {:?}, dirty {}, pending views {:?}, owed {:?}, operations in flight {}, \
                 held bridges {:?}, task still runnable {}",
                t.deadline,
                t.epoch.frame,
                t.epoch.reflects,
                t.target,
                t.routed,
                t.dirty,
                t.pending_views,
                t.owed,
                t.operations_in_flight,
                t.held_bridges,
                t.runnable
            ),
            SettleError::Loading(epoch) => {
                write!(
                    f,
                    "window settled with a loading placeholder at frame {:?}",
                    epoch.frame
                )
            }
            SettleError::Engine(e) => write!(f, "the engine cannot report its emitted seq: {e:#}"),
            SettleError::Undelivered(epoch) => write!(
                f,
                "window settled at frame {:?}, but the change the caller waits for is not painted",
                epoch.frame
            ),
        }
    }
}

impl std::error::Error for SettleError {}

/// The app context a window test pumps.
pub trait Pump {
    /// The clock a settle's hang guard reads.
    fn clock(&self) -> &dyn Clock;
    /// Run window tasks until none is runnable (true), or until `go_on`, asked
    /// after each task, says stop (false).
    fn run_until_parked(&self, go_on: &mut dyn FnMut() -> bool) -> bool;
    /// Every view of every window that a notify marked dirty and that no frame
    /// has drawn since.
    fn pending_views(&self) -> Vec<EntityId>;
    /// Draw every window once more, dirty or not, then pump like
    /// [`Pump::run_until_parked`].
    fn force_frame(&self, go_on: &mut dyn FnMut() -> bool) -> bool;
    /// Block the calling thread for at most `timeout`, or until it is
    /// unparked: by a settle's change waker, or by a window task that became
    /// runnable.
    fn park(&self, timeout: Duration);
}

/// Unparks the thread that polled the change.
struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

/// Park until `change` resolves, a window task wakes, or `timeout` passes.
fn await_change(app: &impl Pump, change: Pin<&mut impl Future<Output = ()>>, timeout: Duration) {
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    if change
        .poll(&mut std::task::Context::from_waker(&waker))
        .is_pending()
    {
        app.park(timeout);
    }
}

/// The most tasks one pump runs before it stops. Counted in ticks, not wall
/// time: a pump the settle itself woke runs to its park however late it starts.
pub const PUMP_TICKS: usize = 100_000;

/// Run `run` with a budget of [`PUMP_TICKS`] tasks; true when it parked.
pub fn pump_parks(run: impl FnOnce(&mut dyn FnMut() -> bool) -> bool) -> bool {
    let mut left = PUMP_TICKS;
    run(&mut || {
        left -= 1;
        left > 0
    })
}

/// Tick `executor` until it parks or `go_on` says stop, with tokio's task
/// budget lifted. A harness step pumps the window inside one
/// `Runtime::block_on` poll; once that poll spends its budget, every tokio
/// channel a window task awaits answers Pending and wakes the task at once.
fn tick_until_parked(executor: &BackgroundExecutor, go_on: &mut dyn FnMut() -> bool) -> bool {
    let mut run = std::pin::pin!(tokio::task::unconstrained(async move {
        while executor.tick() {
            if !go_on() {
                return false;
            }
        }
        true
    }));
    match run
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        Poll::Ready(parked) => parked,
        Poll::Pending => unreachable!("the pump has no await, so one poll runs it"),
    }
}

impl Pump for HeadlessAppContext {
    fn clock(&self) -> &dyn Clock {
        &SystemClock
    }

    fn run_until_parked(&self, go_on: &mut dyn FnMut() -> bool) -> bool {
        tick_until_parked(self.background_executor(), go_on)
    }

    fn pending_views(&self) -> Vec<EntityId> {
        pending_views_of(&mut self.app.borrow_mut())
    }

    fn force_frame(&self, go_on: &mut dyn FnMut() -> bool) -> bool {
        refresh_windows(&mut self.app.borrow_mut());
        Pump::run_until_parked(self, go_on)
    }

    fn park(&self, timeout: Duration) {
        std::thread::park_timeout(timeout);
    }
}

impl Pump for TestAppContext {
    fn clock(&self) -> &dyn Clock {
        &SystemClock
    }

    fn run_until_parked(&self, go_on: &mut dyn FnMut() -> bool) -> bool {
        tick_until_parked(&self.executor(), go_on)
    }

    fn pending_views(&self) -> Vec<EntityId> {
        self.update(pending_views_of)
    }

    fn force_frame(&self, go_on: &mut dyn FnMut() -> bool) -> bool {
        self.update(refresh_windows);
        Pump::run_until_parked(self, go_on)
    }

    fn park(&self, timeout: Duration) {
        std::thread::park_timeout(timeout);
    }
}

fn pending_views_of(app: &mut App) -> Vec<EntityId> {
    app.windows()
        .into_iter()
        .flat_map(|handle| {
            app.update_window(handle, |_, window, _| window.pending_dirty_views())
                .unwrap_or_else(|e| panic!("window {handle:?} closed during a settle: {e:#}"))
        })
        .collect()
}

/// A test app draws a dirty window when the update that dirtied it ends.
fn refresh_windows(app: &mut App) {
    for handle in app.windows() {
        app.update_window(handle, |_, window, _| window.refresh())
            .unwrap_or_else(|e| panic!("window {handle:?} closed during a settle: {e:#}"));
    }
}

/// What the tests read off the screen: every element of the committed frame.
/// `ElementInfo` has no `PartialEq`, so its `Debug` form stands in for it.
fn screen(bounds: &BoundsRegistry) -> BTreeMap<String, String> {
    bounds
        .all_elements()
        .into_iter()
        .map(|(id, info)| (id, format!("{info:?}")))
        .collect()
}

/// Settle the engine with `settle_engine`, then pump the window until a frame
/// reflects the engine's emitted seq, every UI subscription applied its first
/// snapshot, each view driver's tokio task applied the rows and inputs its
/// watermark reports name (`UiWatermark::owed` is empty),
/// no admitted operation is unfinished, one more pump commits no
/// frame, and the emitted seq did not move. A notify during a draw marks its
/// view pending without dirtying the window, and a test app draws only a dirty
/// window, so while a view is pending the settle forces two frames: the window
/// is settled only when the second shows what the first showed. A change that
/// comes only after two unchanged renders is not seen. Between two tries it
/// waits for a change of the watermark or a window task to wake, unless the
/// try itself moved the frame. No clock advance: a gpui timer that must fire
/// is the caller's to advance.
#[allow(clippy::result_large_err)]
pub fn settle_window(
    app: &impl Pump,
    bounds: &BoundsRegistry,
    watermark: &UiWatermark,
    settle_engine: &mut dyn FnMut(),
    deadline: Duration,
) -> Result<RenderEpoch, SettleError> {
    watermark.poke();
    settle_window_poked(app, bounds, watermark, settle_engine, deadline)
}

/// [`settle_window`] after its caller poked the watermark.
#[allow(clippy::result_large_err)]
fn settle_window_poked(
    app: &impl Pump,
    bounds: &BoundsRegistry,
    watermark: &UiWatermark,
    settle_engine: &mut dyn FnMut(),
    deadline: Duration,
) -> Result<RenderEpoch, SettleError> {
    assert!(
        deadline <= HANG_DEADLINE,
        "a settle deadline of {deadline:?} outlives the nextest cap; ask for at most {HANG_DEADLINE:?}"
    );
    let clock = app.clock();
    let until = clock.now() + deadline;
    let timeout = |before: RenderEpoch, forced_frame_changed_screen: bool, runnable: bool| {
        let epoch = bounds.epoch();
        Err(SettleError::Timeout(SettleTimeout {
            deadline,
            epoch,
            target: watermark.emitted().map_err(SettleError::Engine)?,
            routed: watermark.routed().map_err(SettleError::Engine)?,
            dirty: epoch.frame != before.frame || forced_frame_changed_screen,
            pending_views: app.pending_views(),
            owed: watermark.owed(),
            operations_in_flight: watermark.operations_in_flight(),
            held_bridges: holon_gpui::bridge_hold::held_rows(),
            runnable,
        }))
    };
    loop {
        let mut change = std::pin::pin!(tokio::task::unconstrained(watermark.changed()));
        let mut forced_frame_changed_screen = false;
        let mut frame_moved = false;
        settle_engine();
        let before = bounds.epoch();
        let parked = pump_parks(|go_on| app.run_until_parked(go_on));
        bounds.flush();
        if !parked {
            return timeout(before, false, true);
        }
        let target = watermark.emitted().map_err(SettleError::Engine)?;
        let epoch = bounds.epoch();
        if epoch.reflects >= target
            && watermark.owed().is_empty()
            && watermark.operations_in_flight() == 0
        {
            let parked = pump_parks(|go_on| app.run_until_parked(go_on));
            bounds.flush();
            if !parked {
                return timeout(before, false, true);
            }
            let emitted = watermark.emitted().map_err(SettleError::Engine)?;
            frame_moved = bounds.epoch() != epoch;
            if !frame_moved && emitted == target && watermark.operations_in_flight() == 0 {
                if !app.pending_views().is_empty() {
                    // The first forced frame may itself change what the next
                    // one shows.
                    let parked = pump_parks(|go_on| app.force_frame(go_on));
                    bounds.flush();
                    let shown = screen(bounds);
                    let parked = parked && pump_parks(|go_on| app.force_frame(go_on));
                    bounds.flush();
                    if !parked {
                        return timeout(before, false, true);
                    }
                    forced_frame_changed_screen = screen(bounds) != shown;
                }
                if !forced_frame_changed_screen {
                    let epoch = bounds.epoch();
                    let loading = bounds
                        .all_elements()
                        .iter()
                        .any(|(_, info)| info.widget_type.as_ref() == "loading");
                    return match loading {
                        true => Err(SettleError::Loading(epoch)),
                        false => Ok(epoch),
                    };
                }
            }
        }
        let now = clock.now();
        if now >= until {
            return timeout(before, forced_frame_changed_screen, false);
        }
        if !(frame_moved || forced_frame_changed_screen) {
            await_change(app, change.as_mut(), until - now);
        }
    }
}

/// [`settle_window`] until `delivered` holds: for a change that reaches the
/// window over a hop the watermark does not count, such as a condition on the
/// session bus that a tokio task turns into a toast.
#[allow(clippy::result_large_err)]
pub fn settle_window_until(
    app: &impl Pump,
    bounds: &BoundsRegistry,
    watermark: &UiWatermark,
    delivered: &dyn Fn(&BoundsRegistry) -> bool,
    deadline: Duration,
) -> Result<RenderEpoch, SettleError> {
    assert!(
        deadline <= HANG_DEADLINE,
        "a settle deadline of {deadline:?} outlives the nextest cap; ask for at most {HANG_DEADLINE:?}"
    );
    let clock = app.clock();
    let start = clock.now();
    loop {
        // Subscribed after the poke, so only a change after it wakes the wait.
        watermark.poke();
        let mut change = std::pin::pin!(tokio::task::unconstrained(watermark.changed()));
        let left = deadline.saturating_sub(clock.now() - start);
        let epoch = settle_window_poked(app, bounds, watermark, &mut || {}, left)?;
        if delivered(bounds) {
            return Ok(epoch);
        }
        let spent = clock.now() - start;
        if spent >= deadline {
            return Err(SettleError::Undelivered(epoch));
        }
        await_change(app, change.as_mut(), deadline - spent);
    }
}

/// Try `ready` until it holds (true) or `deadline` passes (false). Between two
/// tries it waits for a change of the watermark or a window task to wake, never
/// by the clock; a try that must see the window pumps it itself.
pub fn wait_until(
    app: &impl Pump,
    watermark: &UiWatermark,
    ready: &mut dyn FnMut() -> bool,
    deadline: Duration,
) -> bool {
    assert!(
        deadline <= HANG_DEADLINE,
        "a wait deadline of {deadline:?} outlives the nextest cap; ask for at most {HANG_DEADLINE:?}"
    );
    let clock = app.clock();
    let start = clock.now();
    loop {
        // Subscribed before the try, so a change during the try wakes the wait.
        let mut change = std::pin::pin!(tokio::task::unconstrained(watermark.changed()));
        if ready() {
            return true;
        }
        let spent = clock.now() - start;
        if spent >= deadline {
            return false;
        }
        await_change(app, change.as_mut(), deadline - spent);
    }
}

/// Keep every tokio worker of `runtime` busy for `hold`, so a task woken now
/// runs only after it: a tokio hop then takes longer than a window settle, as
/// it does on a loaded host.
pub fn hold_tokio_workers(runtime: &tokio::runtime::Runtime, hold: Duration) {
    for _ in 0..runtime.metrics().num_workers() {
        runtime.spawn(async move { std::thread::sleep(hold) });
    }
}

/// Every tokio worker of a runtime, blocked until this drops: no tokio task
/// runs while it lives.
pub struct HeldWorkers {
    release: std::sync::Arc<std::sync::Barrier>,
}

impl Drop for HeldWorkers {
    fn drop(&mut self) {
        self.release.wait();
    }
}

/// Block every tokio worker of `runtime`; returns once each one is blocked.
pub fn hold_tokio_workers_until_released(runtime: &tokio::runtime::Runtime) -> HeldWorkers {
    let workers = runtime.metrics().num_workers();
    let held = std::sync::Arc::new(std::sync::Barrier::new(workers + 1));
    let release = std::sync::Arc::new(std::sync::Barrier::new(workers + 1));
    for _ in 0..workers {
        let (held, release) = (held.clone(), release.clone());
        runtime.spawn(async move {
            held.wait();
            release.wait();
        });
    }
    held.wait();
    HeldWorkers { release }
}
