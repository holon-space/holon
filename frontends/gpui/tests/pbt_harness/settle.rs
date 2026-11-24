//! The one window settle of the GPUI tests: pump the window until a committed
//! frame reflects every change the engine emitted, and the window is at rest.

use std::collections::BTreeMap;
use std::future::Future as _;
use std::task::Poll;
use std::time::Duration;
use std::time::Instant;

use gpui::App;
use gpui::AppContext as _;
use gpui::BackgroundExecutor;
use gpui::EntityId;
use gpui::HeadlessAppContext;
use gpui::TestAppContext;
use holon_api::EntityUri;
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
    /// A window task was still runnable at the deadline: a task that wakes
    /// itself on every poll keeps the executor from parking.
    pub runnable: bool,
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
                 dirty {}, pending views {:?}, owed {:?}, operations in flight {}, \
                 held bridges {:?}, task still runnable {}",
                t.deadline,
                t.epoch.frame,
                t.epoch.reflects,
                t.target,
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
    /// Run window tasks until none is runnable; false when one still is at
    /// `until`.
    fn run_until_parked(&self, until: Instant) -> bool;
    /// Every view of every window that a notify marked dirty and that no frame
    /// has drawn since.
    fn pending_views(&self) -> Vec<EntityId>;
    /// Draw every window once more, dirty or not, then pump like
    /// [`Pump::run_until_parked`].
    fn force_frame(&self, until: Instant) -> bool;
}

/// Tick `executor` until it parks or `until` passes, with tokio's task budget
/// lifted. A harness step pumps the window inside one `Runtime::block_on`
/// poll; once that poll spends its budget, every tokio channel a window task
/// awaits answers Pending and wakes the task at once.
fn tick_until_parked(executor: &BackgroundExecutor, until: Instant) -> bool {
    let mut run = std::pin::pin!(tokio::task::unconstrained(async move {
        while executor.tick() {
            if Instant::now() >= until {
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
    fn run_until_parked(&self, until: Instant) -> bool {
        tick_until_parked(self.background_executor(), until)
    }

    fn pending_views(&self) -> Vec<EntityId> {
        pending_views_of(&mut self.app.borrow_mut())
    }

    fn force_frame(&self, until: Instant) -> bool {
        refresh_windows(&mut self.app.borrow_mut());
        Pump::run_until_parked(self, until)
    }
}

impl Pump for TestAppContext {
    fn run_until_parked(&self, until: Instant) -> bool {
        tick_until_parked(&self.executor(), until)
    }

    fn pending_views(&self) -> Vec<EntityId> {
        self.update(pending_views_of)
    }

    fn force_frame(&self, until: Instant) -> bool {
        self.update(refresh_windows);
        Pump::run_until_parked(self, until)
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
/// comes only after two unchanged renders is not seen. No sleep and no clock
/// advance: a gpui timer that must fire is the caller's to advance.
#[allow(clippy::result_large_err)]
pub fn settle_window(
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
    let start = Instant::now();
    let until = start + deadline;
    let timeout = |before: RenderEpoch, forced_frame_changed_screen: bool, runnable: bool| {
        let epoch = bounds.epoch();
        Err(SettleError::Timeout(SettleTimeout {
            deadline,
            epoch,
            target: watermark.emitted().map_err(SettleError::Engine)?,
            dirty: epoch.frame != before.frame || forced_frame_changed_screen,
            pending_views: app.pending_views(),
            owed: watermark.owed(),
            operations_in_flight: watermark.operations_in_flight(),
            held_bridges: holon_gpui::bridge_hold::held_rows(),
            runnable,
        }))
    };
    loop {
        let mut forced_frame_changed_screen = false;
        settle_engine();
        watermark.poke();
        let before = bounds.epoch();
        let parked = app.run_until_parked(until);
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
            let parked = app.run_until_parked(until);
            bounds.flush();
            if !parked {
                return timeout(before, false, true);
            }
            let emitted = watermark.emitted().map_err(SettleError::Engine)?;
            if bounds.epoch() == epoch && emitted == target && watermark.operations_in_flight() == 0
            {
                if !app.pending_views().is_empty() {
                    // The first forced frame may itself change what the next
                    // one shows.
                    let parked = app.force_frame(until);
                    bounds.flush();
                    let shown = screen(bounds);
                    let parked = parked && app.force_frame(until);
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
        if start.elapsed() >= deadline {
            return timeout(before, forced_frame_changed_screen, false);
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
    let start = Instant::now();
    loop {
        let left = deadline.saturating_sub(start.elapsed());
        let epoch = settle_window(app, bounds, watermark, &mut || {}, left)?;
        if delivered(bounds) {
            return Ok(epoch);
        }
        if start.elapsed() >= deadline {
            return Err(SettleError::Undelivered(epoch));
        }
        std::thread::sleep(Duration::from_millis(5));
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
