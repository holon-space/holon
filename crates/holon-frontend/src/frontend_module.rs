//! DI module for frontend services.
//!
//! `FrontendInjectorExt::add_frontend()` registers all frontend-specific services
//! (ThemeRegistry, conditional modules, FrontendSession factory)
//! so that callers can `injector.resolve_async::<FrontendSession>().await`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fluxdi::{Injector, Module, ModuleLifecycleFuture, Provider, Shared};

use holon::api::BackendEngine;
use holon::di::DbHandleProvider;
use holon::sync::{
    CacheEventSubscriberHandle, EventInfraModule, LoroConfig, LoroModule, PublishErrorTracker,
};
#[cfg(not(target_arch = "wasm32"))]
use holon_orgmode::di::FileWatcherReadySignal;
#[cfg(not(target_arch = "wasm32"))]
use holon_todoist::di::TodoistInjectorExt;

use crate::config::{HolonConfig, SessionConfig};
use crate::memory_monitor;
use crate::preferences::{self, PrefKey};
use crate::theme;
use crate::FrontendSession;
#[cfg(not(target_arch = "wasm32"))]
use crate::McpIntegrationRegistry;

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
        let has_todoist_key = holon_config
            .todoist
            .api_key
            .as_ref()
            .is_some_and(|k| !k.is_empty());
        #[cfg(not(target_arch = "wasm32"))]
        let todoist_fake = session_config.todoist_fake;
        #[cfg(not(target_arch = "wasm32"))]
        let mcp_integrations_dir = holon_config.resolve_mcp_integrations_dir(&config_dir);

        // Event infrastructure (needed by Loro and OrgMode)
        if loro_enabled || orgmode_root.is_some() {
            EventInfraModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register EventInfraModule: {}", e))?;
        }

        // Loro CRDT (must be before OrgMode so OrgMode can detect it)
        let resolved_loro_dir = if loro_enabled {
            let loro_dir = loro_storage_dir
                .clone()
                .or_else(|| orgmode_root.as_ref().map(|r| r.join(".loro")))
                .unwrap_or_else(|| db_path.parent().unwrap_or(&db_path).join(".loro"));
            let loro_dir_for_provider = loro_dir.clone();
            self.provide::<LoroConfig>(Provider::root(move |_| {
                Shared::new(LoroConfig::new(loro_dir_for_provider.clone()))
            }));
            LoroModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register LoroModule: {}", e))?;

            // Register MutableText provider — async factory that resolves
            // LoroDocumentStore and gets the global doc. Frontends wire it
            // into ReactiveEngine.editable_text_provider in on_start.
            self.provide::<crate::editable_text_provider::LoroEditableTextProvider>(
                Provider::root_async(|resolver| async move {
                    let store =
                        resolver.resolve::<holon::sync::loro_document_store::LoroDocumentStore>();
                    let collab = store
                        .get_global_doc()
                        .await
                        .expect("Failed to get global LoroDoc for MutableText");
                    let doc = collab.doc();
                    let resolver =
                        Arc::new(crate::editable_text_provider::LoroDocTextResolver { doc });
                    Shared::new(
                        crate::editable_text_provider::LoroEditableTextProvider::new(resolver),
                    )
                }),
            );

            Some(loro_dir)
        } else {
            None
        };

        // OrgMode (native-only — holon-orgmode uses tokio::fs + tokio::process)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = orgmode_root {
            use holon_orgmode::di::{OrgModeConfig, OrgModeModule};

            // Ensure the org root exists. On a truly empty org root (no .org
            // files), seed a notes.org so the sidebar shows at least one
            // document on first launch. "index.org" is deliberately excluded
            // from the sidebar query, so we use a different filename.
            if !root.exists() {
                std::fs::create_dir_all(&root).expect("Failed to create org root directory");
            }
            let no_org_files = std::fs::read_dir(&root)
                .map(|mut d| {
                    d.all(|e| {
                        !e.map(|e| e.path().extension().is_some_and(|x| x == "org"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(true);
            if no_org_files {
                for asset in crate::DEFAULT_ASSETS {
                    std::fs::write(root.join(asset.filename), asset.content)
                        .unwrap_or_else(|e| panic!("Failed to write {}: {}", asset.filename, e));
                }
            }

            let mut org_config = if let Some(loro_dir) = resolved_loro_dir {
                OrgModeConfig::with_loro_storage(root, loro_dir)
            } else {
                OrgModeConfig::new(root)
            };
            org_config.post_org_write_hook = post_org_write_hook;
            self.provide::<OrgModeConfig>(Provider::root(move |_| Shared::new(org_config.clone())));
            OrgModeModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register OrgModeModule: {}", e))?;
        }

        // Todoist (native-only — requires network stack not available on wasm32)
        #[cfg(not(target_arch = "wasm32"))]
        {
            if has_todoist_key {
                self.add_todoist(holon_config.todoist.clone())?;
            } else if todoist_fake {
                self.add_todoist_fake()?;
            }
        }

        // MCP integrations (native-only)
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
        self.provide::<FrontendSession>(Provider::root_async(move |resolver| async move {
             tracing::info!("[FrontendSession] factory: entering");
            let engine = resolver.resolve_async::<BackendEngine>().await;
             tracing::info!("[FrontendSession] factory: BackendEngine resolved");

            // Trigger lazy background wiring
            let _ = resolver
                .try_resolve_async::<CacheEventSubscriberHandle>()
                .await;
             tracing::info!(
                "[FrontendSession] factory: CacheEventSubscriberHandle try_resolve_async done"
            );

            // Subscribe dir/file caches (holon-filesystem types)
            subscribe_filesystem_caches(&resolver).await;
             tracing::info!("[FrontendSession] factory: subscribe_filesystem_caches done");

            // Register FDW-backed tables and set the matview hook for auto-subscription
            #[cfg(not(target_arch = "wasm32"))]
            {
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

            let error_tracker: PublishErrorTracker = resolver
                .try_resolve::<PublishErrorTracker>()
                .map(|t| (*t).clone())
                .unwrap_or_else(|_| PublishErrorTracker::new());
            // ALLOW(ok): optional DI service — native only (holon-orgmode not on wasm32)
            #[cfg(not(target_arch = "wasm32"))]
            let ready_signal: Option<holon_orgmode::di::FileWatcherReadySignal> = resolver
                .try_resolve::<holon_orgmode::di::FileWatcherReadySignal>()
                .ok()
                .map(|s| (*s).clone());

            // Transition DB to ready
            if let Ok(db_handle_provider) = resolver.try_resolve::<dyn DbHandleProvider>() {
                let handle = db_handle_provider.handle();
                if let Err(e) = handle.transition_to_ready().await {
                    tracing::warn!("Failed to transition actor to ready: {}", e);
                }
            }

            // Seed default layout (native only — org parser pulls notify which
            // has no wasm backend; wasi path uses seed.rs in holon-worker instead).
            #[cfg(not(target_arch = "wasm32"))]
            FrontendSession::<()>::seed_default_layout(&engine)
                .await
                .expect("Failed to seed default layout");

            // Start action watchers — streaming discovery picks up action blocks
            // as OrgSyncController inserts them. Must be after seed_default_layout
            // so the block table and seed data (block:journals) exist.
            #[cfg(not(target_arch = "wasm32"))]
            holon::api::action_watcher::start_action_watchers(engine.clone())
                .await
                .expect("Failed to start action watchers");

            // Wait for orgmode readiness — errors propagate (never swallowed)
            #[cfg(not(target_arch = "wasm32"))]
            if wait_for_ready {
                if let Some(ref signal) = ready_signal {
                    signal.wait_ready().await.expect("OrgMode startup failed");
                }
            }

            // Resolve the Loro sync controller AFTER `seed_default_layout`
            // AND OrgMode readiness. The controller's factory runs
            // `seed_loro_from_persistent_store`, which mirrors every row in
            // the `block` table into Loro. If this resolve happens before
            // those two phases complete, the mirror sees an incomplete table
            // and later share/accept ops fail with
            // "block X not found in Loro tree".
             tracing::info!(
                "[FrontendSession] factory: about to try_resolve_async::<LoroSyncControllerHandle> (post-seed+orgmode)"
            );
            let lsch_result = resolver
                .try_resolve_async::<holon::sync::LoroSyncControllerHandle>()
                .await;
             tracing::info!(
                "[FrontendSession] factory: try_resolve_async::<LoroSyncControllerHandle> returned: {}",
                if lsch_result.is_ok() { "Ok" } else { "Err" }
            );
            if let Err(ref e) = lsch_result {
                 tracing::info!("[FrontendSession] factory: LoroSyncControllerHandle error: {e}");
            }
            let _ = lsch_result;

            let config_dir_val = resolver.resolve::<ConfigDir>();
            let theme_registry = resolver.resolve::<theme::ThemeRegistry>();
            let preference_defs = resolver.resolve::<Vec<preferences::PreferenceDef>>();
            let holon_config = resolver.resolve::<HolonConfig>();
            let locked_keys = resolver.resolve::<LockedKeys>();

            Shared::new(FrontendSession {
                engine,
                error_tracker,
                #[cfg(not(target_arch = "wasm32"))]
                ready_signal,
                extras: (),
                _memory_monitor: memory_monitor::MemoryMonitorHandle::start(),
                preference_defs,
                theme_registry,
                holon_config: Mutex::new((*holon_config).clone()),
                config_dir: config_dir_val.0.clone(),
                locked_keys: locked_keys.0.clone(),
            })
        }));

        // BuilderServicesSlot — OnceLock for circular dep breaking (GPUI)
        self.provide::<crate::reactive::BuilderServicesSlot>(Provider::root(|_| {
            Shared::new(crate::reactive::BuilderServicesSlot(Arc::new(
                std::sync::OnceLock::new(),
            )))
        }));

        // Shared shadow interpreter — built ONCE here and handed to every
        // consumer via DI. This is the only place in the entire frontend
        // where the interpreter is constructed; see `build_shadow_interpreter`
        // for the rationale behind crate-private visibility.
        //
        // Everything downstream reaches interpretation via
        // `BuilderServices::interpret` / `interpret_with_source` — no call
        // site ever names `RenderInterpreter` directly. Registered as
        // `RenderInterpreter<ReactiveViewModel>` so fluxdi resolves it as
        // `Shared<_>` (i.e. `Arc<_>`).
        let shadow_interpreter_shared =
            Shared::new(crate::shadow_builders::build_shadow_interpreter());
        self.provide::<crate::render_interpreter::RenderInterpreter<crate::ReactiveViewModel>>(
            Provider::root({
                let s = shadow_interpreter_shared;
                move |_| s.clone()
            }),
        );

        // ReactiveEngine factory — resolves FrontendSession + RenderInterpreterFn
        // + the shared shadow interpreter from DI.
        // Only registered if set_render_interpreter was called (Flutter doesn't use ReactiveEngine).
        self.provide::<crate::reactive::ReactiveEngine>(Provider::root(|resolver| {
            let session = resolver.resolve::<FrontendSession>();
            let interpret = resolver.resolve::<crate::reactive::RenderInterpreterFn>();
            let interpreter = resolver
                .resolve::<crate::render_interpreter::RenderInterpreter<crate::ReactiveViewModel>>(
                );
            let f = interpret.0.clone();
            Shared::new(crate::reactive::ReactiveEngine::new(
                session,
                tokio::runtime::Handle::current(),
                interpreter,
                move |expr, rows| f(expr, rows),
            ))
        }));

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

/// Subscribe dir/file caches to EventBus (if OrgModeModule registered them).
/// No-op on wasm32: holon-filesystem is not available.
async fn subscribe_filesystem_caches(resolver: &Injector) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let event_bus = match resolver
            .try_resolve_async::<holon::sync::TursoEventBus>()
            .await
        {
            Ok(eb) => eb,
            Err(_) => return,
        };

        use holon::core::queryable_cache::QueryableCache;
        use holon::sync::cache_event_subscriber::CacheEventSubscriber;
        use holon::sync::event_bus::{AggregateType, EventBus};

        let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();

        if let Ok(dc) =
            resolver.try_resolve::<QueryableCache<holon_filesystem::directory::Directory>>()
        {
            if let Err(e) = CacheEventSubscriber::subscribe_entity(
                AggregateType::Directory,
                dc,
                event_bus_arc.clone(),
            )
            .await
            {
                tracing::error!("Failed to subscribe directory cache: {}", e);
            }
        }
        if let Ok(fc) = resolver.try_resolve::<QueryableCache<holon_filesystem::File>>() {
            if let Err(e) =
                CacheEventSubscriber::subscribe_entity(AggregateType::File, fc, event_bus_arc).await
            {
                tracing::error!("Failed to subscribe file cache: {}", e);
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = resolver;
}
