//! Boot of the real shared wiring over a temp vault, for the undo-precondition
//! id-scheme suite. In a subdirectory so cargo does not compile it as a test
//! binary of its own.

use std::collections::HashSet;
use std::sync::Arc;

use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

/// The child block the suite edits. Written to disk BARE, exactly as
/// `docs/Reference/ORG_SYNTAX.md` specifies org files store ids; the projection
/// keys the same block as `block:PROBE_CHILD`.
pub const PROBE_CHILD: &str = "undo-scheme-probe-child";
pub const PROBE_PAGE: &str = "undo-scheme-probe-page";

const VAULT_ORG: &str = "\
* Undo scheme probe page
:PROPERTIES:
:ID: undo-scheme-probe-page
:END:
** A child block
:PROPERTIES:
:ID: undo-scheme-probe-child
:END:
Some text so the write-back fold has a document to render.
";

pub struct Booted {
    pub engine: Arc<holon::api::BackendEngine>,
    _session: Arc<FrontendSession>,
    _reactive: Arc<holon_frontend::reactive::ReactiveEngine>,
    _live: holon_frontend::reactive::LiveBlock,
    _dir: tempfile::TempDir,
}

/// Boot the shared production wiring — the leg the GPUI application boots per
/// D113.a, with no editor-cell registry — over a vault with content.
pub async fn boot_a_working_session() -> Booted {
    let dir = tempfile::tempdir().expect("create tempdir for the vault");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault file");

    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("undo-scheme.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };

    let (session, engine, reactive) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().to_path_buf(),
        HashSet::new(),
        |injector| {
            use holon_frontend::reactive::RenderInterpreterInjectorExt;
            let slot = injector.resolve::<holon_frontend::reactive::BuilderServicesSlot>();
            injector.set_render_interpreter(holon_frontend::reactive::make_interpret_fn(
                slot.0.clone(),
            ));
            Ok(())
        },
        |injector| {
            let reactive = injector.resolve::<holon_frontend::reactive::ReactiveEngine>();
            let slot = injector.resolve::<holon_frontend::reactive::BuilderServicesSlot>();
            let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
            let _ = slot.0.set(services);
            reactive
        },
    )
    .await
    .expect("the shared wiring must boot a session");

    let services = reactive.clone() as Arc<dyn holon_frontend::reactive::BuilderServices>;
    let live = reactive.watch_live(&holon_api::EntityUri::block(PROBE_PAGE), services);

    Booted {
        engine,
        _session: session,
        _reactive: reactive,
        _live: live,
        _dir: dir,
    }
}
