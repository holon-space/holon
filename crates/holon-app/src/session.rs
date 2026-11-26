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
    conditions: Arc<holon_conditions::ConditionBus>,
) -> Result<Arc<FrontendSession>> {
    let (session, _engine, ()) = new_from_config_with_di(
        holon_config,
        session_config,
        config_dir,
        locked_keys,
        conditions,
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
    conditions: Arc<holon_conditions::ConditionBus>,
    extra_setup: F,
    extra_resolve: G,
) -> Result<(Arc<FrontendSession>, Arc<BackendEngine>, T)>
where
    F: FnOnce(&fluxdi::Injector) -> Result<()> + Send + 'static,
    G: FnOnce(&fluxdi::Injector) -> T + Send + 'static,
    T: Send + 'static,
{
    let db_path = holon_config.resolve_db_path(&config_dir);
    #[cfg(not(target_arch = "wasm32"))]
    let vault = crate::vault_lock::SessionVault::acquire(holon_config.vault.root.as_deref())?;

    // `create_backend_engine_with_extras` resolves the `BackendEngine` ONCE
    // (root_async, cached) and returns it. Thread that exact instance back to
    // callers that need a handle — re-resolving it elsewhere (especially
    // synchronously) risks a duplicate engine with its own CDC/matview state
    // and background tasks (see lifecycle.rs TOCTOU note).
    let vault_root = holon_config.vault.root.clone();
    let home_conditions = Arc::clone(&conditions);
    let timings = session_config.timings.clone();
    let clock = session_config.clock.clone();
    let (engine, (session, extra, types)) = holon::di::create_backend_engine_with_extras(
        db_path,
        holon::di::DatabaseRole::for_crdt_enabled(holon_config.crdt_enabled()),
        conditions,
        timings,
        clock,
        move |injector| {
            #[cfg(not(target_arch = "wasm32"))]
            vault.register(injector);
            injector.add_frontend(holon_config, session_config, config_dir, locked_keys)?;
            extra_setup(injector)?;
            // Before the engine resolves: the profile resolver seeds its vault
            // profile mirror from the registry's file rows.
            if let Some(root) = &vault_root {
                injector
                    .resolve::<holon_profiles::TypeRegistry>()
                    .load_vault_profile_home(root, &home_conditions);
            }
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

/// Stop a session in the one order that works: the tasks its boot spawned stop
/// first, then the storage actor closes.
///
/// A watcher still running when the actor closes reads a dead store for the
/// rest of the process, and the org-writeback supervisor spends its restart
/// budget and declares derived state permanently stale.
///
/// Every quit path goes through here, so the ordering has one definition. A
/// watcher that does not stop within
/// [`DEFAULT_SHUTDOWN_TIMEOUT`](holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT)
/// is an `Err` naming it — never a silent detach.
pub async fn shutdown_session(injector: &fluxdi::Injector) -> Result<()> {
    // Before anything stops: the write-back loop lets shutdown win over its
    // backlog, so an edit still on its way to disk would stay only in the
    // store, and the next boot with another DB reads the stale file as a user
    // edit that deletes it. A write-back that does not settle does not stop
    // the shutdown; its error is returned once the rest has run.
    #[cfg(not(target_arch = "wasm32"))]
    let settled = match &*session_vault(injector)? {
        crate::vault_lock::SessionVault::Held(_) => {
            wait_for_writeback(injector, WRITEBACK_SETTLE_BUDGET).await
        }
        crate::vault_lock::SessionVault::Absent => Ok(()),
    };
    #[cfg(target_arch = "wasm32")]
    let settled: Result<()> = Ok(());

    stop_session(injector, Teardown::Quit).await?;
    settled
}

/// End a session as a killed process ends: the same teardown as
/// [`shutdown_session`], without waiting for the write-back and without the
/// Loro checkpoint, so the store and the files stay as the last acknowledged
/// write left them. What survives is the next boot's to find.
#[cfg(all(feature = "crash-injection", not(target_arch = "wasm32")))]
pub async fn kill_session(injector: &fluxdi::Injector) -> Result<()> {
    stop_session(injector, Teardown::Kill).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Teardown {
    Quit,
    #[cfg(all(feature = "crash-injection", not(target_arch = "wasm32")))]
    Kill,
}

#[cfg(not(target_arch = "wasm32"))]
fn session_vault(
    injector: &fluxdi::Injector,
) -> Result<fluxdi::Shared<crate::vault_lock::SessionVault>> {
    injector
        .try_resolve::<crate::vault_lock::SessionVault>()
        .map_err(|e| {
            anyhow::anyhow!("shutdown: the container registers no SessionVault to release: {e}")
        })
}

/// The tasks stop, then the store closes, then the vault is released.
async fn stop_session(injector: &fluxdi::Injector, teardown: Teardown) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    let vault = session_vault(injector)?;

    injector
        .resolve::<holon_api::lifecycle::SessionShutdown>()
        .shutdown(holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT)
        .await?;

    // `LoroConfig` is registered exactly when the CRDT layer is on.
    if teardown == Teardown::Quit
        && injector
            .try_resolve::<holon_loro_wiring::LoroConfig>()
            .is_ok()
    {
        let store = injector
            .try_resolve::<holon_loro::LoroDocumentStore>()
            .map_err(|e| {
                anyhow::anyhow!(
                    "shutdown: a CRDT session must resolve its LoroDocumentStore to save it, but \
                     resolution failed: {e}"
                )
            })?;
        store
            .checkpoint()
            .await
            .map_err(|e| anyhow::anyhow!("shutdown: checkpointing the Loro store failed: {e:#}"))?;
    }

    // Which substrate this container holds is a registered value, so a Turso
    // wiring whose engine will not resolve is a failure, not "no actor here".
    match *injector.resolve::<holon::di::StorageSelector>() {
        holon::di::StorageSelector::LoroMemory => {}
        holon::di::StorageSelector::Turso => {
            let engine = injector.try_resolve::<BackendEngine>().map_err(|e| {
                anyhow::anyhow!(
                    "shutdown: a Turso container must resolve its BackendEngine to close the \
                     storage actor, but resolution failed: {e}"
                )
            })?;
            engine
                .db_handle()
                .shutdown()
                .await
                .map_err(|e| anyhow::anyhow!("Turso actor shutdown failed: {e}"))?;
        }
    }

    // Last: nothing of this session writes the vault any more, so the next
    // writer may take it.
    #[cfg(not(target_arch = "wasm32"))]
    if let crate::vault_lock::SessionVault::Held(lock) = &*vault {
        lock.release()?;
    }
    tracing::info!("session shut down");
    Ok(())
}

/// The exit status of a step followed by another that runs whatever the first
/// returned, such as a frontend's run and then its [`shutdown_session`].
pub fn first_then(first: Result<()>, then: Result<()>) -> Result<()> {
    match (first, then) {
        (Err(first), Err(then)) => Err(anyhow::anyhow!("{first:#}; then {then:#}")),
        (first, then) => first.and(then),
    }
}

#[cfg(test)]
mod first_then_tests {
    use super::*;

    #[test]
    fn the_exit_status_keeps_every_error() {
        let failed = |what: &str| Err(anyhow::anyhow!("{what} failed"));
        let cases = [
            (Ok(()), Ok(()), None),
            (failed("serving"), Ok(()), Some("serving failed")),
            (Ok(()), failed("shutdown"), Some("shutdown failed")),
            (
                failed("serving"),
                failed("shutdown"),
                Some("serving failed; then shutdown failed"),
            ),
        ];
        for (first, then, expected) in cases {
            let exit = first_then(first, then).map_err(|e| format!("{e:#}"));
            assert_eq!(exit.err().as_deref(), expected);
        }
    }
}

/// How long a shutdown waits for the org write-back to write what it owes.
pub const WRITEBACK_SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// Wait until every change the store holds has reached its org file. The
/// backlog import is sealed first: while it runs, every file it imports moves
/// the Loro document. Then the Loro projection catches up with the document,
/// the block feed's write-back consumer applies every CDC batch emitted up to
/// that point, and the write-back owes nothing. Each wait wakes on the event
/// that can end it; `budget` is only the hang guard.
async fn wait_for_writeback(
    injector: &fluxdi::Injector,
    budget: std::time::Duration,
) -> Result<()> {
    let idle = injector
        .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
        .map_err(|e| {
            anyhow::anyhow!(
                "shutdown: a session with a vault must resolve its org write-back idle signal, \
                 but resolution failed: {e}"
            )
        })?;
    let loro_sync = if injector
        .try_resolve::<holon_loro_wiring::LoroConfig>()
        .is_ok()
    {
        Some(
            injector
                .try_resolve_async::<holon_loro::LoroSyncControllerHandle>()
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "shutdown: a CRDT session must resolve its Loro sync controller to wait \
                         for the projection, but resolution failed: {e}"
                    )
                })?,
        )
    } else {
        None
    };
    let engine = match *injector.resolve::<holon::di::StorageSelector>() {
        holon::di::StorageSelector::LoroMemory => None,
        holon::di::StorageSelector::Turso => Some(injector.resolve::<BackendEngine>()),
    };

    let stage = std::sync::Mutex::new(Stage::SealingBacklog);
    let settled = tokio::time::timeout(
        budget,
        writeback_settles(&idle, loro_sync.as_deref(), engine.as_deref(), &stage),
    )
    .await;
    let settled = match settled {
        Ok(settled) => settled,
        Err(_elapsed) => {
            let stage = *stage.lock().expect("stage mutex");
            budget_spent(
                budget,
                stage,
                idle.writeback_owes_handed_over(),
                idle.queued_renders().documents(),
            )
        }
    };
    let refused = idle
        .writeback_net()
        .read(holon_filesystem::writeback_net::failed_writebacks);
    if refused.is_empty() {
        return settled;
    }
    let refusal = format!(
        "shutdown: the file system refused the org write-back, so edits are in the store but \
         not in these files: {}",
        refused.join("; ")
    );
    Err(match settled {
        Ok(()) => anyhow::anyhow!(refusal),
        Err(e) => e.context(refusal),
    })
}

/// How far [`writeback_settles`] got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    SealingBacklog,
    /// The Loro projection has not caught up with the document. The Loro
    /// checkpoint keeps every edit, so this alone loses nothing.
    ReadModelBehind,
    /// The block feed's write-back consumer has not applied through this CDC
    /// sequence.
    FeedBehind(u64),
    WritebackOwes,
}

async fn writeback_settles(
    idle: &holon_orgmode::OrgSyncIdleSignal,
    loro_sync: Option<&holon_loro::LoroSyncControllerHandle>,
    engine: Option<&BackendEngine>,
    stage: &std::sync::Mutex<Stage>,
) -> Result<()> {
    let set = |s: Stage| *stage.lock().expect("stage mutex") = s;
    idle.seal_backlog().await;

    if let Some(sync) = loro_sync {
        set(Stage::ReadModelBehind);
        let mut synced = sync.synced();
        loop {
            synced.borrow_and_update();
            if sync.is_settled().await? {
                break;
            }
            synced
                .changed()
                .await
                .map_err(|_| anyhow::anyhow!("shutdown: the Loro projection ended unsettled"))?;
        }
    }

    let feed = idle.writeback_feed();
    let mut delivered = engine.map(|e| e.matview_manager().delivery_changed());
    let start = engine.map(|e| e.db_handle().cdc_emitted_watermark());
    let mut net = idle.writeback_net().subscribe();
    loop {
        let progressed = idle.progressed();
        if let Some(delivered) = delivered.as_mut() {
            delivered.borrow_and_update();
        }
        net.borrow_and_update();
        let behind = match (engine, start) {
            (Some(engine), Some(start)) if feed.is_taken() => {
                let applied = engine.matview_manager().complete_through_for(feed);
                (applied < holon_api::delivery_ledger::Seq(start)).then_some(start)
            }
            _ => None,
        };
        match behind {
            Some(start) => set(Stage::FeedBehind(start)),
            None if idle.writeback_owes_handed_over().is_empty() => return Ok(()),
            None => set(Stage::WritebackOwes),
        }
        let delivery = async {
            match delivered.as_mut() {
                Some(d) => d.changed().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = progressed => {}
            changed = delivery => changed
                .map_err(|_| anyhow::anyhow!("shutdown: the CDC router ended"))?,
            changed = net.changed() => changed
                .map_err(|_| anyhow::anyhow!("shutdown: the write-back net ended"))?,
        }
    }
}

/// The verdict once the hang guard fires: an error naming what was not
/// written, unless only the read model is behind, which is logged.
fn budget_spent(
    budget: std::time::Duration,
    stage: Stage,
    busy: Vec<&'static str>,
    unwritten: Vec<String>,
) -> Result<()> {
    if stage == Stage::ReadModelBehind && busy.is_empty() && unwritten.is_empty() {
        tracing::warn!(
            "shutdown: the read model is still behind the document after {budget:?}; every \
             document is written and the Loro checkpoint keeps every edit"
        );
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "shutdown: the org write-back did not settle within {budget:?}, so edits may be missing \
         from disk. Waiting at: {stage:?}. Documents not written: {unwritten:?}. Still running: \
         {busy:?}"
    ))
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    const STAGES: [Stage; 4] = [
        Stage::SealingBacklog,
        Stage::ReadModelBehind,
        Stage::FeedBehind(7),
        Stage::WritebackOwes,
    ];

    /// Every verdict shape at the budget: only a lagging read model, with
    /// nothing running and nothing unwritten, may end the shutdown Ok; every
    /// error names the stage, the unwritten documents and what still runs.
    #[test]
    fn only_a_lagging_read_model_is_ok_at_the_budget() {
        for stage in STAGES {
            for (busy, unwritten) in [(false, false), (true, false), (false, true), (true, true)] {
                let busy = if busy {
                    vec!["document re-renders are queued"]
                } else {
                    Vec::new()
                };
                let unwritten = if unwritten {
                    vec!["doc:page.org".to_string()]
                } else {
                    Vec::new()
                };
                let ok = stage == Stage::ReadModelBehind && busy.is_empty() && unwritten.is_empty();
                let got = budget_spent(
                    std::time::Duration::from_secs(1),
                    stage,
                    busy.clone(),
                    unwritten.clone(),
                );
                match got {
                    Ok(()) => assert!(ok, "{stage:?} {busy:?} {unwritten:?} must be an error"),
                    Err(e) => {
                        assert!(!ok, "{stage:?} with nothing owed must be Ok: {e:#}");
                        let msg = e.to_string();
                        for named in busy
                            .iter()
                            .copied()
                            .chain(unwritten.iter().map(String::as_str))
                        {
                            assert!(msg.contains(named), "the error must name {named}: {msg}");
                        }
                        assert!(msg.contains(&format!("{stage:?}")), "{msg}");
                    }
                }
            }
        }
    }
}
