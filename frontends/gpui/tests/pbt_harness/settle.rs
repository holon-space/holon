//! The one window settle of the GPUI tests: pump the window until a committed
//! frame reflects every change the engine emitted, and the window is at rest.

use std::time::Duration;
use std::time::Instant;

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
#[derive(Debug)]
pub struct SettleTimeout {
    pub deadline: Duration,
    pub epoch: RenderEpoch,
    pub target: EngineSeq,
    /// The last pump committed a new frame: something keeps the window dirty.
    pub dirty: bool,
    /// The UI subscriptions that hold the watermark, with why.
    pub owed: Vec<String>,
    /// The engine's admitted operations that have not finished.
    pub operations_in_flight: usize,
    /// The rows whose bridge waits on a shut test hold.
    pub held_bridges: Vec<EntityUri>,
}

#[derive(Debug)]
pub enum SettleError {
    Timeout(SettleTimeout),
    Loading(RenderEpoch),
    Engine(anyhow::Error),
}

impl std::fmt::Display for SettleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettleError::Timeout(t) => write!(
                f,
                "window did not settle within {:?}: frame {:?} reflects {:?}, target {:?}, \
                 dirty {}, owed {:?}, operations in flight {}, held bridges {:?}",
                t.deadline,
                t.epoch.frame,
                t.epoch.reflects,
                t.target,
                t.dirty,
                t.owed,
                t.operations_in_flight,
                t.held_bridges
            ),
            SettleError::Loading(epoch) => {
                write!(
                    f,
                    "window settled with a loading placeholder at frame {:?}",
                    epoch.frame
                )
            }
            SettleError::Engine(e) => write!(f, "the engine cannot report its emitted seq: {e:#}"),
        }
    }
}

impl std::error::Error for SettleError {}

/// The app context a window test pumps.
pub trait Pump {
    fn run_until_parked(&self);
}

impl Pump for HeadlessAppContext {
    fn run_until_parked(&self) {
        HeadlessAppContext::run_until_parked(self)
    }
}

impl Pump for TestAppContext {
    fn run_until_parked(&self) {
        TestAppContext::run_until_parked(self)
    }
}

/// Settle the engine with `settle_engine`, then pump the window until a frame
/// reflects the engine's emitted seq, every UI subscription applied its first
/// snapshot, no admitted operation is unfinished, one more pump commits no
/// frame, and the emitted seq did not move. No sleep and no clock advance: a
/// gpui timer that must fire is the caller's to advance.
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
    loop {
        settle_engine();
        watermark.poke();
        let before = bounds.epoch();
        app.run_until_parked();
        bounds.flush();
        let target = watermark.emitted().map_err(SettleError::Engine)?;
        let epoch = bounds.epoch();
        if epoch.reflects >= target
            && watermark.owed().is_empty()
            && watermark.operations_in_flight() == 0
        {
            app.run_until_parked();
            bounds.flush();
            let emitted = watermark.emitted().map_err(SettleError::Engine)?;
            if bounds.epoch() == epoch && emitted == target && watermark.operations_in_flight() == 0
            {
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
        if start.elapsed() >= deadline {
            return Err(SettleError::Timeout(SettleTimeout {
                deadline,
                epoch,
                target,
                dirty: epoch.frame != before.frame,
                owed: watermark.owed(),
                operations_in_flight: watermark.operations_in_flight(),
                held_bridges: holon_gpui::bridge_hold::engaged_rows(),
            }));
        }
    }
}
