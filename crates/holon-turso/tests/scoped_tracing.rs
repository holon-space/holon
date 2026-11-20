//! The one global tracing subscriber of this test binary. A test routes the
//! events of the whole process to its own layer for the life of a [`Scope`], so
//! tests that read events from actor and watchdog threads share one process.
//! Scopes are serialized: a second one waits for the first to drop.

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::PoisonError;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::Layered;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;

type Filtered = Layered<reload::Layer<EnvFilter, Registry>, Registry>;
type Route = Box<dyn Layer<Filtered> + Send + Sync>;

struct Installed {
    filter: reload::Handle<EnvFilter, Registry>,
    route: reload::Handle<Option<Route>, Filtered>,
    serial: Mutex<()>,
}

fn installed() -> &'static Installed {
    static INSTALLED: OnceLock<Installed> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let (filter, filter_handle) = reload::Layer::new(EnvFilter::new("off"));
        let (route, route_handle) = reload::Layer::new(None::<Route>);
        tracing::subscriber::set_global_default(Registry::default().with(filter).with(route))
            .expect("this test binary installs its global subscriber only here");
        Installed {
            filter: filter_handle,
            route: route_handle,
            serial: Mutex::new(()),
        }
    })
}

/// While alive, every event of the process that passes `filter` reaches the
/// layer given to [`Scope::new`]; dropping it silences the process again.
pub struct Scope {
    installed: &'static Installed,
    _serial: MutexGuard<'static, ()>,
}

impl Scope {
    pub fn new(filter: &str, layer: impl Layer<Filtered> + Send + Sync + 'static) -> Self {
        let installed = installed();
        // A panicking test drops its scope while unwinding, which restores the
        // state.
        let serial = installed
            .serial
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        installed
            .route
            .reload(Some(Box::new(layer) as Route))
            .expect("the route layer lives as long as the process");
        installed
            .filter
            .reload(EnvFilter::new(filter))
            .expect("the filter layer lives as long as the process");
        Self {
            installed,
            _serial: serial,
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.installed
            .filter
            .reload(EnvFilter::new("off"))
            .expect("the filter layer lives as long as the process");
        self.installed
            .route
            .reload(None::<Route>)
            .expect("the route layer lives as long as the process");
    }
}
