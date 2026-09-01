//! Config-driven session construction (relocated from
//! `FrontendSession::new_from_config*` in storage de-leak Stage 6).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use holon::api::BackendEngine;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::preferences::PrefKey;

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
    let (engine, (session, extra, types)) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            injector.add_frontend(holon_config, session_config, config_dir, locked_keys)?;
            extra_setup(injector)?;
            Ok(())
        },
        |injector| async move {
            let session = injector.resolve_async::<FrontendSession>().await;
            let extra = extra_resolve(&injector);
            let types = injector
                .resolve_async::<holon_profiles::TypeRegistry>()
                .await;
            (session, extra, types)
        },
    )
    .await?;

    // CV-E admission over the WHOLE registry (ruling D54.a). The `declare_type`
    // op is not how bundled types become real — registry seeding is — so
    // guarding only the op would leave every seeded type unchecked.
    //
    // Ordering: this runs before the session and engine handles reach any
    // caller, and those handles are the only route to dispatching a write, so
    // no caller-served write can precede it. Write AUTHORITIES are already
    // registered by this point (`FreeStandingTypeViews` derives them during
    // engine construction), which is why a refusal here aborts startup rather
    // than unwinding them — there is no undeclare.
    let profiles = holon_capability::registry::shipped_profiles()
        .map_err(|e| anyhow::anyhow!("the shipped capability profiles must parse: {e}"))?;
    crate::type_admission::sweep_registry(&profiles, &types).map_err(|e| {
        anyhow::anyhow!("refusing to start: the type registry fails capability admission: {e}")
    })?;

    Ok((session, engine, extra))
}
