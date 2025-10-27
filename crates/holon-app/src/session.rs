//! Config-driven session construction (relocated from
//! `FrontendSession::new_from_config*` in storage de-leak Stage 6).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use holon::api::BackendEngine;
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::preferences::PrefKey;
use holon_frontend::FrontendSession;

use crate::wiring::FrontendInjectorExt;

/// Create a new frontend session from a premortem-loaded `HolonConfig`.
///
/// This is the preferred constructor. CLI frontends use
/// `holon_frontend::cli::build_session()` which produces the arguments.
/// Uses FluxDI to wire all services.
pub async fn new_from_config(
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
) -> Result<Arc<FrontendSession>> {
    let (session, _engine, ()) = new_from_config_with_di(
        holon_config,
        session_config,
        config_dir,
        locked_keys,
        |_| Ok(()),
        |_| (),
    )
    .await?;
    Ok(session)
}

/// Create a new frontend session with additional DI registrations.
///
/// The `extra_setup` closure runs on the DI injector after the frontend
/// services are registered but before anything is resolved. Use it to register
/// frontend-specific services (e.g. `set_render_interpreter`).
///
/// The `extra_resolve` closure runs after session creation and can resolve
/// additional services from the same DI container (e.g. `ReactiveEngine`).
pub async fn new_from_config_with_di<F, G, T>(
    holon_config: HolonConfig,
    session_config: SessionConfig,
    config_dir: PathBuf,
    locked_keys: HashSet<PrefKey>,
    extra_setup: F,
    extra_resolve: G,
) -> Result<(Arc<FrontendSession>, Arc<BackendEngine>, T)>
where
    F: FnOnce(&fluxdi::Injector) -> Result<()> + Send + 'static,
    G: FnOnce(&fluxdi::Injector) -> T + Send + 'static,
    T: Send + 'static,
{
    let db_path = holon_config.resolve_db_path(&config_dir);

    // `create_backend_engine_with_extras` resolves the `BackendEngine` ONCE
    // (root_async, cached) and returns it. Thread that exact instance back to
    // callers that need a handle — re-resolving it elsewhere (especially
    // synchronously) risks a duplicate engine with its own CDC/matview state
    // and background tasks (see lifecycle.rs TOCTOU note).
    let (engine, (session, extra)) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            injector.add_frontend(holon_config, session_config, config_dir, locked_keys)?;
            extra_setup(injector)?;
            Ok(())
        },
        |injector| async move {
            let session = injector.resolve_async::<FrontendSession>().await;
            let extra = extra_resolve(&injector);
            (session, extra)
        },
    )
    .await?;

    Ok((session, engine, extra))
}
