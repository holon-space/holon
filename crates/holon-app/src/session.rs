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
    #[cfg(not(target_arch = "wasm32"))]
    let vault = crate::vault_lock::SessionVault::acquire(holon_config.vault.root.as_deref())?;

    // `create_backend_engine_with_extras` resolves the `BackendEngine` ONCE
    // (root_async, cached) and returns it. Thread that exact instance back to
    // callers that need a handle — re-resolving it elsewhere (especially
    // synchronously) risks a duplicate engine with its own CDC/matview state
    // and background tasks (see lifecycle.rs TOCTOU note).
    let (engine, (session, extra, types)) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            #[cfg(not(target_arch = "wasm32"))]
            vault.register(injector);
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
    #[cfg(not(target_arch = "wasm32"))]
    let vault = injector
        .try_resolve::<crate::vault_lock::SessionVault>()
        .map_err(|e| {
            anyhow::anyhow!("shutdown: the container registers no SessionVault to release: {e}")
        })?;

    // Before anything stops: the write-back loop lets shutdown win over its
    // backlog, so an edit still on its way to disk would stay only in the
    // store, and the next boot with another DB reads the stale file as a user
    // edit that deletes it. A write-back that does not settle does not stop
    // the shutdown; its error is returned once the rest has run.
    #[cfg(not(target_arch = "wasm32"))]
    let settled = match &*vault {
        crate::vault_lock::SessionVault::Held(_) => {
            wait_for_writeback(injector, WRITEBACK_SETTLE_BUDGET).await
        }
        crate::vault_lock::SessionVault::Absent => Ok(()),
    };
    #[cfg(target_arch = "wasm32")]
    let settled: Result<()> = Ok(());

    injector
        .resolve::<holon_api::lifecycle::SessionShutdown>()
        .shutdown(holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT)
        .await?;

    // `LoroConfig` is registered exactly when the CRDT layer is on.
    if injector
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
            .save_all()
            .await
            .map_err(|e| anyhow::anyhow!("shutdown: saving the Loro snapshot failed: {e:#}"))?;
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
    settled
}

/// How long a shutdown waits for the org write-back to write what it owes.
pub const WRITEBACK_SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// The write-back chain is quiet for this long before a shutdown trusts it.
const WRITEBACK_QUIET_FLOOR: std::time::Duration = std::time::Duration::from_millis(100);

/// Wait until every change the store holds has reached its org file: the Loro
/// projection caught up with the document, CDC stopped moving, and the
/// write-back has nothing queued, folding or rendering — all at once, for
/// [`WRITEBACK_QUIET_FLOOR`].
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

    let probe = || {
        let idle = idle.clone();
        let loro_sync = loro_sync.clone();
        let engine = engine.clone();
        async move {
            let mut busy: Vec<String> = idle
                .writeback_unsettled()
                .into_iter()
                .map(str::to_string)
                .collect();
            if let Some(sync) = &loro_sync {
                if !sync.is_settled().await? {
                    busy.push("the Loro projection is behind the document".to_string());
                }
            }
            Ok(WritebackProbe {
                busy,
                cdc: engine
                    .as_ref()
                    .map(|e| e.db_handle().cdc_emitted_watermark()),
                tick: idle.current_tick(),
            })
        }
    };
    let settled = settle(
        probe,
        || idle.queued_renders().documents(),
        budget,
        WRITEBACK_QUIET_FLOOR,
    )
    .await;
    let refused = idle.refused_writebacks().documents();
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

#[derive(Debug, PartialEq, Eq)]
struct WritebackProbe {
    /// What is still running; empty when nothing is.
    busy: Vec<String>,
    cdc: Option<u64>,
    tick: u64,
}

/// Poll `probe` until it reports nothing busy and the same watermarks for
/// `floor`, or `budget` runs out — then name what was not written.
async fn settle<P, F>(
    mut probe: P,
    unwritten: impl Fn() -> Vec<String>,
    budget: std::time::Duration,
    floor: std::time::Duration,
) -> Result<()>
where
    P: FnMut() -> F,
    F: std::future::Future<Output = Result<WritebackProbe>>,
{
    let deadline = tokio::time::Instant::now() + budget;
    let mut last = probe().await?;
    let mut quiet_since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let now = probe().await?;
        if !now.busy.is_empty() || now != last {
            quiet_since = tokio::time::Instant::now();
        } else if quiet_since.elapsed() >= floor {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "shutdown: the org write-back did not settle within {budget:?}, so edits may be \
                 missing from disk. Documents not written: {:?}. Still running: {:?}",
                unwritten(),
                now.busy
            ));
        }
        last = now;
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    #[tokio::test]
    async fn a_write_back_that_never_settles_is_an_error_naming_the_unwritten_documents() {
        let err = settle(
            || async {
                Ok(WritebackProbe {
                    busy: vec!["document re-renders are queued".to_string()],
                    cdc: Some(1),
                    tick: 1,
                })
            },
            || vec!["doc:page.org".to_string()],
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect_err("a write-back that stays busy must not settle");
        let msg = err.to_string();
        assert!(
            msg.contains("doc:page.org") && msg.contains("document re-renders are queued"),
            "the error must name the unwritten document and what is still running: {msg}"
        );
    }

    #[tokio::test]
    async fn a_quiet_write_back_settles() {
        settle(
            || async {
                Ok(WritebackProbe {
                    busy: Vec::new(),
                    cdc: Some(7),
                    tick: 3,
                })
            },
            Vec::new,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(10),
        )
        .await
        .expect("a quiet write-back settles within the floor");
    }

    #[tokio::test]
    async fn a_moving_watermark_is_not_quiet() {
        let mut tick = 0;
        let err = settle(
            move || {
                tick += 1;
                async move {
                    Ok(WritebackProbe {
                        busy: Vec::new(),
                        cdc: Some(0),
                        tick,
                    })
                }
            },
            Vec::new,
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(20),
        )
        .await;
        assert!(
            err.is_err(),
            "a tick that advances on every probe is activity, not quiet"
        );
    }
}
