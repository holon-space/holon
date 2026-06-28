//! Turso DI wiring for the frontend session (relocated from
//! `holon-frontend::frontend_module` in storage de-leak Stage 6).
//!
//! `FrontendInjectorExt::add_frontend()` registers all frontend-specific services
//! (ThemeRegistry, conditional modules, FrontendSession factory)
//! so that callers can `injector.resolve_async::<FrontendSession>().await`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use fluxdi::{Injector, Module, ModuleLifecycleFuture, Provider, Shared};

use holon::api::BackendEngine;
use holon::di::DbHandleProvider;
use holon::sync::{
    EventInfraModule, LoroBlockOperations, LoroConfig, LoroModule, PublishErrorTracker,
};
use holon_frontend::config::{HolonConfig, SessionConfig};
use holon_frontend::preferences::{self, PrefKey};
use holon_frontend::render_services::register_render_services;
use holon_frontend::theme;
use holon_frontend::{FrontendSession, SessionParts};

use crate::mcp_integrations::McpIntegrationRegistry;

/// Configuration directory path, stored in DI.
#[derive(Clone, Debug)]
pub struct ConfigDir(pub PathBuf);

/// Preference keys locked by CLI/env (read-only in UI), stored in DI.
#[derive(Clone, Debug)]
pub struct LockedKeys(pub HashSet<PrefKey>);

/// Extension trait for registering all frontend services in DI.
pub trait FrontendInjectorExt {
    /// Register all frontend services.
    ///
    /// After calling this, `injector.resolve_async::<FrontendSession>()` returns
    /// a fully initialized session with all conditional modules wired.
    fn add_frontend(
        &self,
        holon_config: HolonConfig,
        session_config: SessionConfig,
        config_dir: PathBuf,
        locked_keys: HashSet<PrefKey>,
    ) -> Result<()>;
}

impl FrontendInjectorExt for Injector {
    fn add_frontend(
        &self,
        holon_config: HolonConfig,
        session_config: SessionConfig,
        config_dir: PathBuf,
        locked_keys: HashSet<PrefKey>,
    ) -> Result<()> {
        let db_path = holon_config.resolve_db_path(&config_dir);

        // Register configs as singletons (pre-wrap in Arc for non-Clone types)
        let holon_config_arc: Shared<HolonConfig> = Shared::new(holon_config.clone());
        self.provide::<HolonConfig>(Provider::root({
            let c = holon_config_arc;
            move |_| c.clone()
        }));
        let session_config_arc: Shared<SessionConfig> = Shared::new(session_config.clone());
        self.provide::<SessionConfig>(Provider::root({
            let c = session_config_arc;
            move |_| c.clone()
        }));
        self.provide::<ConfigDir>(Provider::root({
            let d = config_dir.clone();
            move |_| Shared::new(ConfigDir(d.clone()))
        }));
        self.provide::<LockedKeys>(Provider::root({
            let k = locked_keys.clone();
            move |_| Shared::new(LockedKeys(k.clone()))
        }));

        // ThemeRegistry + PreferenceDefs
        let post_org_write_hook = holon_config.hooks.post_org_write.clone();

        let theme_registry = theme::ThemeRegistry::load(None);
        let preference_defs = preferences::define_preferences(&theme_registry);
        self.provide::<theme::ThemeRegistry>(Provider::root({
            let tr = Shared::new(theme_registry);
            move |_| tr.clone()
        }));
        self.provide::<Vec<preferences::PreferenceDef>>(Provider::root({
            let pd = Shared::new(preference_defs);
            move |_| pd.clone()
        }));

        // UiInfo
        self.provide::<holon_api::UiInfo>(Provider::root({
            let ui = session_config.ui_info.clone();
            move |_| Shared::new(ui.clone())
        }));

        // Conditional modules
        let orgmode_root = holon_config.orgmode.root_directory.clone();
        let loro_enabled = holon_config.loro_enabled();
        let loro_storage_dir = holon_config.loro.storage_dir.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let mcp_integrations_dir = holon_config.resolve_mcp_integrations_dir(&config_dir);

        // Event infrastructure (needed by Loro and OrgMode)
        if loro_enabled || orgmode_root.is_some() {
            EventInfraModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register EventInfraModule: {}", e))?;
        }

        // Loro CRDT (must be before OrgMode so OrgMode can detect it)
        if loro_enabled {
            let loro_dir = loro_storage_dir
                .clone()
                .or_else(|| orgmode_root.as_ref().map(|r| r.join(".loro")))
                .unwrap_or_else(|| db_path.parent().unwrap_or(&db_path).join(".loro"));
            self.provide::<LoroConfig>(Provider::root(move |_| {
                Shared::new(LoroConfig::new(loro_dir.clone()))
            }));
            LoroModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register LoroModule: {}", e))?;

            // BlockCellRegistry is registered by LoroModule above; no
            // additional registration needed here. Frontends resolve
            // `Arc<BlockCellRegistry>` from DI and assign it to
            // `ReactiveEngine.block_cell_registry` in their `on_start`.

            // Loro-backed alias registrar (doc_id ↔ path). The file-sync
            // controller resolves `dyn AliasRegistrar` from DI; in the Turso
            // container the Loro module owns the doc store, so the registrar is
            // derived from `LoroBlockOperations` here at the composition root
            // (only the wiring crate may name a concrete backend) — holon-orgmode
            // stays storage-agnostic.
            self.provide::<dyn holon_filesystem::AliasRegistrar>(Provider::root(|resolver| {
                let ops = resolver.resolve::<LoroBlockOperations>();
                Arc::new(crate::loro_seams::LoroAliasRegistrar {
                    doc_store: ops.shared_doc_store(),
                }) as Arc<dyn holon_filesystem::AliasRegistrar>
            }));
        }

        // OrgMode (native-only — holon-orgmode uses tokio::fs + tokio::process)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = orgmode_root {
            use holon_orgmode::di::{OrgModeConfig, OrgModeModule};

            // Ensure the org root exists on the real disk so the config-time
            // canonicalize below resolves. The empty-vault default-document
            // seeding happens later through the FileSystem port (ADR 0011) —
            // see `seed_assets` and OrgModeModule's factory. "index.org" is
            // deliberately excluded from the sidebar query, so the seed uses
            // a different filename (notes.org).
            if !root.exists() {
                std::fs::create_dir_all(&root).expect("Failed to create org root directory");
            }

            let mut org_config = OrgModeConfig::new(root);
            org_config.post_org_write_hook = post_org_write_hook;
            org_config.seed_assets = holon_frontend::DEFAULT_ASSETS
                .iter()
                .map(|a| (a.filename.to_string(), a.content.to_string()))
                .collect();
            self.provide::<OrgModeConfig>(Provider::root(move |_| Shared::new(org_config.clone())));
            OrgModeModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register OrgModeModule: {}", e))?;
        }

        // MCP integrations (native-only). External providers (Todoist, etc.)
        // are configured here from `{config_dir}/integrations/*.yaml`.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref dir) = mcp_integrations_dir {
            let module = crate::mcp_integrations::McpIntegrationsModule::from_dir(dir);
            module
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register McpIntegrationsModule: {}", e))?;
        }

        // FrontendSession factory — resolves all dependencies from DI
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        let wait_for_ready = session_config.wait_for_ready;
        self.provide::<FrontendSession>(Provider::root_async(move |resolver| {
            use tracing::Instrument;
            async move {
                tracing::info!("[FrontendSession] factory: entering");
                let engine = async { resolver.resolve_async::<BackendEngine>().await }
                    .instrument(tracing::info_span!(
                        "di.factory.FrontendSession.resolve_engine"
                    ))
                    .await;
                tracing::info!("[FrontendSession] factory: BackendEngine resolved");

                // Option B: dir/file caches are fed directly from the
                // OrgModeSyncProvider broadcast in holon-orgmode's DI wiring —
                // no EventBus subscription here anymore.

                // Register FDW-backed tables and set the matview hook for auto-subscription
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    let mcp_result = resolver.try_resolve_async::<McpIntegrationRegistry>().await;
                    if let Ok(mcp_registry) = mcp_result {
                        for table in mcp_registry.fdw_backed_tables() {
                            engine.register_fdw_table(&table).await;
                        }
                        if let Some(integration) = mcp_registry.integrations().first() {
                            engine
                                .set_matview_hook(integration.sync_engine.clone())
                                .await;
                        }
                    }
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.mcp_fdw_register"
                ))
                .await;

                let error_tracker: PublishErrorTracker = resolver
                    .try_resolve::<PublishErrorTracker>()
                    .map(|t| (*t).clone())
                    .unwrap_or_else(|_| PublishErrorTracker::new());
                #[cfg(not(target_arch = "wasm32"))]
                let ready_signal: Option<
                    tokio::sync::watch::Receiver<Option<Result<(), String>>>,
                > = resolver
                    .try_resolve::<holon_orgmode::di::FileWatcherReadySignal>()
                    .ok()
                    .map(|s| (*s).clone().into_receiver());

                // Transition DB to ready
                async {
                    if let Ok(db_handle_provider) = resolver.try_resolve::<dyn DbHandleProvider>() {
                        let handle = db_handle_provider.handle();
                        if let Err(e) = handle.transition_to_ready().await {
                            tracing::warn!("Failed to transition actor to ready: {}", e);
                        }
                    }
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.transition_to_ready"
                ))
                .await;

                // Seed default layout (native only — org parser pulls notify which
                // has no wasm backend; wasi path uses seed.rs in holon-worker instead).
                // Seeds the bundled Org assets into Loro via `BlockOrdering`
                // intents (Loro is the authority; SQL is the projection).
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    let ordering = resolver
                        .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                        .await;
                    // A user `index.org` whose root heading carries
                    // `:ID: root-layout` is THE layout (the well-known
                    // `block:root-layout` contract) — suppress the default
                    // seed so boot doesn't depend on whether this factory
                    // races the org initial scan. An index.org WITHOUT that
                    // id never takes the layout over, so the default must
                    // still seed (otherwise there would be no layout at all).
                    // Checked through the FileSystem port (in-memory test FS
                    // included).
                    let user_index_org_exists = match (
                        resolver.try_resolve::<holon_orgmode::di::OrgModeConfig>(),
                        resolver.try_resolve::<dyn holon_filesystem::FileSystem>(),
                    ) {
                        (Ok(cfg), Ok(fs)) => fs
                            .read_to_string(&cfg.root_directory.join("index.org"))
                            .await
                            .is_ok_and(|content| content.contains(":ID: root-layout")),
                        _ => false,
                    };
                    crate::seed::seed_default_layout(&engine, ordering, user_index_org_exists)
                        .await
                        .expect("Failed to seed default layout");
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.seed_default_layout"
                ))
                .await;

                // Project the freshly-seeded layout into the SQL sink (`block_raw`)
                // immediately, so the 3-column shell renders without waiting for
                // the org initial scan or the (ready-gated) projection run-loop.
                //
                // `seed_default_layout` writes the layout into Loro (the
                // authority) via `create_in_tree` but does not itself project to
                // SQL; previously the layout only reached `block_raw` once the
                // org scan's first flush ran (seconds in) or the run-loop started
                // after orgmode readiness — so the UI shell appeared empty for
                // seconds after launch. This flush uses the shared, *unarmed*
                // projection (creates flow, deletes withheld) — the same instance
                // the scan flushes and the controller later runs — so it is a
                // safe bootstrap projection of the seed layout.
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    // Optional: a `DownstreamProjection` is only registered by
                    // `LoroModule`. In the degraded SQL-only config (no Loro)
                    // it's absent — skip the best-effort seed flush rather than
                    // panic; the run-loop reconciles either way. Mirrors
                    // `holon-orgmode`'s `optional_resolve_async` for the same
                    // service.
                    if let Some(projection) = resolver
                        .optional_resolve_async::<dyn holon_core::DownstreamProjection>()
                        .await
                    {
                        if let Err(e) = projection.flush().await {
                            tracing::warn!(
                                "[FrontendSession] seed-layout projection flush failed \
                                 (run-loop will reconcile): {e:#}"
                            );
                        }
                    } else {
                        tracing::debug!(
                            "[FrontendSession] no DownstreamProjection registered \
                             (SQL-only/no-Loro); skipping seed-layout flush"
                        );
                    }
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.flush_seed_layout"
                ))
                .await;

                // Start action watchers — streaming discovery picks up action blocks
                // as FileSyncController inserts them. Must be after seed_default_layout
                // so the block table and seed data (block:journals) exist.
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    holon::api::action_watcher::start_action_watchers(engine.clone())
                        .await
                        .expect("Failed to start action watchers");
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.start_action_watchers"
                ))
                .await;

                // Wait for orgmode readiness, then resolve the Loro sync
                // controller so its outbound Loro → SQL projector starts after
                // the bundled Org assets have been seeded into Loro via intents.
                //
                // Production (`wait_for_ready=false`): spawn this in the
                // background so the window paints immediately and data
                // streams in via the reactive layer.
                // Tests (`wait_for_ready=true`): await before returning so
                // assertions see fully-seeded state.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let resolver_bg = resolver.clone();
                    let ready_signal_bg = ready_signal.clone();
                    let post_ready_work = async move {
                        if let Some(mut signal) = ready_signal_bg {
                            let result = signal
                                .wait_for(|v| v.is_some())
                                .await
                                .expect("FileWatcherReadySignal sender dropped");
                            match result.as_ref().unwrap() {
                                Ok(()) => {}
                                Err(msg) => panic!("OrgMode startup failed: {msg}"),
                            }
                        }
                        let _ = resolver_bg
                            .try_resolve_async::<holon::sync::LoroSyncControllerHandle>()
                            .await;
                    }
                    .instrument(tracing::info_span!(
                        "di.factory.FrontendSession.post_ready",
                        wait_for_ready = wait_for_ready
                    ));

                    if wait_for_ready {
                        post_ready_work.await;
                    } else {
                        tokio::spawn(post_ready_work);
                    }
                }

                let config_dir_val = resolver.resolve::<ConfigDir>();
                let theme_registry = resolver.resolve::<theme::ThemeRegistry>();
                let preference_defs = resolver.resolve::<Vec<preferences::PreferenceDef>>();
                let holon_config = resolver.resolve::<HolonConfig>();
                let locked_keys = resolver.resolve::<LockedKeys>();

                let profiles = engine.profile_resolver().clone();
                let query_engine = Some(engine.clone() as Arc<dyn holon::api::QueryEngine>);
                let operation_engine = Some(engine.clone() as Arc<dyn holon::api::OperationEngine>);
                let ui_watcher = engine.clone() as Arc<dyn holon::api::UiWatcher>;
                // The block read seam over Turso's CDC mirrors. Built here so
                // `block_query()` is total across both wirings (the no-Turso arm
                // builds a `LoroBlockQuerySource`); consumers read blocks uniformly
                // through the capability with no backend branch.
                let block_query = Arc::new(
                    holon::sync::turso_block_query_source::TursoBlockQuerySource::watch_default(
                        &engine,
                    )
                    .await
                    .expect(
                        "FrontendSession factory: TursoBlockQuerySource::watch_default failed \
                         (the `block` + `focus_roots` matviews must exist by this point)",
                    ),
                )
                    as Arc<dyn holon_core::storage::BlockQuerySource>;
                Shared::new(FrontendSession::from_parts(SessionParts {
                    query_engine,
                    block_query,
                    operation_engine,
                    ui_watcher,
                    profiles,
                    error_tracker,
                    ready_signal,
                    preference_defs,
                    theme_registry,
                    holon_config: (*holon_config).clone(),
                    config_dir: config_dir_val.0.clone(),
                    locked_keys: locked_keys.0.clone(),
                }))
            }
            .instrument(tracing::info_span!("di.factory.FrontendSession"))
        }));

        // Render stack (slot + shared shadow interpreter + ReactiveEngine
        // factory) — backend-agnostic, lives in holon-frontend.
        register_render_services(self);

        Ok(())
    }
}

/// Reusable module for frontend services: config, conditional modules, session factory.
///
/// - `configure()`: registers all frontend services via [`FrontendInjectorExt::add_frontend`]
/// - `on_start()`: resolves `FrontendSession` (triggers the async factory chain)
///
/// Compose this into frontend-specific modules via explicit delegation
/// (not `imports()`, which creates child injector scopes).
pub struct HolonFrontendModule {
    pub holon_config: HolonConfig,
    pub session_config: SessionConfig,
    pub config_dir: PathBuf,
    pub locked_keys: HashSet<PrefKey>,
}

impl Module for HolonFrontendModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector
            .add_frontend(
                self.holon_config.clone(),
                self.session_config.clone(),
                self.config_dir.clone(),
                self.locked_keys.clone(),
            )
            .map_err(|e| {
                fluxdi::Error::module_lifecycle_failed(
                    "HolonFrontendModule",
                    "configure",
                    &e.to_string(),
                )
            })
    }

    fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
        Box::pin(async move {
            let _session = injector.resolve_async::<FrontendSession>().await;
            Ok(())
        })
    }
}
