//! Shared harness for the two session-shutdown test binaries. Lives in a
//! subdirectory so cargo does not compile it as a test binary of its own; both
//! binaries pull it in with `#[path]`.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::lifecycle::SessionShutdown;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;

/// Collects every ERROR event. An orphaned watcher leaves no other trace: it
/// logs and keeps going rather than returning an `Err` to anyone.
#[derive(Clone, Default)]
pub struct ErrorLog(Arc<Mutex<Vec<String>>>);

impl ErrorLog {
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("error log poisoned"))
    }
}

impl<S: tracing::Subscriber> Layer<S> for ErrorLog {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if *event.metadata().level() != tracing::Level::ERROR {
            return;
        }
        let mut rendered = String::new();
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
                if field.name() == "message" {
                    rendered = format!("{value:?}");
                }
            },
        );
        self.0
            .lock()
            .expect("error log poisoned")
            .push(format!("{}: {rendered}", event.metadata().target()));
    }
}

/// GLOBAL, not thread-local: the watchers under test run on tokio worker
/// threads, and a `set_default` dispatcher never reaches them.
pub fn capture_errors() -> ErrorLog {
    let log = ErrorLog::default();
    let subscriber = tracing_subscriber::registry().with(log.clone());
    tracing::subscriber::set_global_default(subscriber)
        .expect("this test binary installs the only global subscriber");

    // Self-check: a capture that silently sees nothing would make every
    // assertion in this suite vacuous, and that is exactly how the first
    // version of these tests passed.
    tracing::error!("[capture-self-check] the error log is armed");
    let seen = log.take();
    assert_eq!(
        seen.len(),
        1,
        "the ERROR capture layer is not receiving events — every assertion built on it is vacuous"
    );

    log
}

/// Render captured errors for an assertion message.
pub fn listed(errors: &[String]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {e}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The page the probe vault defines and the harness watches.
pub const PROBE_PAGE: &str = "shutdown-probe-page";

const VAULT_ORG: &str = "\
* Shutdown probe page
:PROPERTIES:
:ID: shutdown-probe-page
:END:
** A child block
:PROPERTIES:
:ID: shutdown-probe-child
:END:
Some text so the write-back fold has a document to render.
";

pub struct Booted {
    pub engine: Arc<holon::api::BackendEngine>,
    /// Read only by `session_shutdown_stops_watchers`; the other binary that
    /// includes this harness boots the same session and never drives it.
    #[allow(dead_code)]
    pub shutdown: Arc<SessionShutdown>,
    /// The container the production teardown is driven through. Same as
    /// `shutdown`: one of the two binaries has no teardown to drive.
    #[allow(dead_code)]
    pub injector: fluxdi::Injector,
    /// Held, never read: dropping either would tear down the very watchers
    /// these tests exist to observe.
    _session: Arc<FrontendSession>,
    _reactive: Arc<holon_frontend::reactive::ReactiveEngine>,
    _live: holon_frontend::reactive::LiveBlock,
    dir: tempfile::TempDir,
}

impl Booted {
    /// Give the file-sync controller work to do. Called AFTER the store closes,
    /// this is what makes both shutdown tests deterministic: a surviving
    /// watcher is guaranteed a read to fail on, instead of the test hoping one
    /// was still in flight.
    pub fn touch_vault(&self) {
        let path = self.dir.path().join("probe.org");
        let content = std::fs::read_to_string(&path).expect("read the probe vault file");
        std::fs::write(&path, format!("{content}\n*** Another child\n"))
            .expect("touch the probe vault file");
    }
}

/// Boot the real shared wiring over a vault with content, and put the session's
/// watchers to work: the org write-back fold has a document, and a live UI
/// watch is registered so the render path is subscribed to CDC. Without that
/// last step the session has no `UiWatcher` at all and any shutdown assertion
/// would pass vacuously.
pub async fn boot_a_working_session() -> Booted {
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault file");

    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("shutdown.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };

    let (session, engine, (shutdown, reactive, injector)) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().to_path_buf(),
        HashSet::new(),
        |injector| {
            // The render stack the gpui frontend installs. Without it the
            // `ReactiveEngine` provider does not resolve, so this boot would
            // have no UI watcher to orphan.
            use holon_frontend::reactive::RenderInterpreterInjectorExt;
            let slot = injector.resolve::<holon_frontend::reactive::BuilderServicesSlot>();
            injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                slot.0.clone(),
            ));
            Ok(())
        },
        |injector| {
            let reactive = injector.resolve::<holon_frontend::reactive::ReactiveEngine>();
            // Break the engine↔interpreter cycle by publishing the ONE engine
            // instance the interpreter renders through.
            let slot = injector.resolve::<holon_frontend::reactive::BuilderServicesSlot>();
            let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
            let _ = slot.0.set(services);
            (
                injector.resolve::<SessionShutdown>(),
                reactive,
                injector.clone(),
            )
        },
    )
    .await
    .expect("the shared wiring must boot a session");

    // Put the render path to work: a live UI watch is what subscribes the
    // session to CDC and produces the `[UiWatcher] render_entity(...)` reads
    // that an orphaned watcher keeps issuing against a dead actor.
    let services = reactive.clone() as Arc<dyn holon_frontend::reactive::BuilderServices>;
    let live = reactive.watch_live(&holon_api::EntityUri::block(PROBE_PAGE), services);

    Booted {
        engine,
        shutdown,
        injector,
        _session: session,
        _reactive: reactive,
        _live: live,
        dir,
    }
}
