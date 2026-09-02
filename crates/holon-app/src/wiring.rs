//! Turso DI wiring for the frontend session (relocated from
//! `holon-frontend::frontend_module` in storage de-leak Stage 6).
//!
//! `FrontendInjectorExt::add_frontend()` registers all frontend-specific
//! services (ThemeRegistry, conditional modules, FrontendSession factory)
//! so that callers can `injector.resolve_async::<FrontendSession>().await`.
//!
//! **Relationship to the PBT `CapMap` (ADR 0019 §5) — NESTED, not parallel.**
//! This `fluxdi` wiring *assembles the real application*. The PBT composition
//! spine (`holon_pbt_core::composition::CapMap`) is a capability-*disclosure
//! facade* that wraps a fluxdi-assembled app: the headless PBT host boots this
//! exact wiring via `new_from_config_with_di` and exposes the result as
//! `Sut*`/`Ref*` capabilities. `CapMap` does NOT duplicate this wiring, and
//! prod does NOT boot through `CapMap`. The two DI containers must stay
//! separate — see ADR 0019 (do-not-unify).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::ModuleLifecycleFuture;
use fluxdi::Provider;
use fluxdi::Shared;
use holon::api::BackendEngine;
use holon::di::DbHandleProvider;
use holon_core::CrudAuthority;
use holon_core::OperationProvider;
use holon_frontend::FrontendSession;
use holon_frontend::SessionParts;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::platform::BootDisclosure;
use holon_frontend::platform::BootStep;
use holon_frontend::platform::PlatformCapabilities;
use holon_frontend::preferences::PrefKey;
use holon_frontend::preferences::{self};
use holon_frontend::render_services::register_render_services;
use holon_frontend::theme;
use holon_loro::LoroBlockOperations;
use holon_loro::PublishErrorTracker;
use holon_loro_wiring::EventInfraModule;
use holon_loro_wiring::LoroConfig;
use holon_loro_wiring::LoroModule;

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
    /// After calling this, `injector.resolve_async::<FrontendSession>()`
    /// returns a fully initialized session with all conditional modules
    /// wired.
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

        // Ledger of which boot steps this assembly actually performs. Steps that
        // a `#[cfg]` compiles out — or that the hand-mirrored worker assembly
        // never had — are warned about by `finish()` at the end of the session
        // factory, so a silently missing feature becomes a startup log line.
        let disclosure = BootDisclosure::new(PlatformCapabilities::current());

        // Lets a legacy-loro-state rejection name this process's actual files.
        holon_loro::mergeable_child::disclose_migration_paths(
            holon_loro::mergeable_child::MigrationPaths {
                db_path: db_path.clone(),
                crdt_storage_dir: holon_config.resolve_crdt_storage_dir(&config_dir),
            },
        );

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

        // Boot-ordering gate for provider syncs. Starts `DeferredUntilScan`;
        // the `post_ready` scan barrier opens it once the org initial scan has
        // finished, so MCP/provider syncs never contend with boot ingest on the
        // serialized DatabaseActor.
        self.provide::<holon_core::SyncGate>(Provider::root({
            let gate = holon_core::SyncGate::new();
            move |_| Shared::new(gate.clone())
        }));

        // Degraded-state disclosure bus. Registered UNCONDITIONALLY, before any
        // conditional module: it is the only channel through which a frontend
        // learns that something degraded (dead MCP integration, failed org
        // ingest, share write-back gap), and a container without it degrades
        // invisibly — blank pages, no banner. It is a plain broadcast channel
        // with no Loro/iroh dependency, so mode has no say in whether it exists.
        self.provide::<Arc<holon_loro::DegradedSignalBus>>(Provider::root(|_| {
            Shared::new(Arc::new(holon_loro::DegradedSignalBus::new()))
        }));
        disclosure.performed(BootStep::DegradedSignalBus);

        // Write-back supervision disclosure. Registered UNCONDITIONALLY for the
        // same reason as the bus itself: org write-back runs in every mode, so
        // the one signal saying "your edits stopped reaching disk" must not be
        // reachable only in Loro mode.
        self.provide::<dyn holon_filesystem::WritebackDisclosure>(Provider::root(|resolver| {
            let bus = resolver.resolve::<Arc<holon_loro::DegradedSignalBus>>();
            Arc::new(crate::loro_seams::WritebackDegradedDisclosure {
                bus: (*bus).clone(),
            }) as Arc<dyn holon_filesystem::WritebackDisclosure>
        }));
        disclosure.performed(BootStep::WritebackDisclosure);

        // ThemeRegistry + PreferenceDefs
        let post_write_hook = holon_config.hooks.post_org_write.clone();

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
        disclosure.performed(BootStep::ThemeAndPreferences);

        // UiInfo
        self.provide::<holon_api::UiInfo>(Provider::root({
            let ui = session_config.ui_info.clone();
            move |_| Shared::new(ui.clone())
        }));
        disclosure.performed(BootStep::UiInfo);

        // Conditional modules
        let orgmode_root = holon_config.vault.root.clone();
        let loro_enabled = holon_config.crdt_enabled();
        let loro_storage_dir = holon_config.crdt.storage_dir.clone();
        // Derived ONCE and reused for both LoroConfig and the epoch guard's
        // durable-state descriptor, so the guard always protects/wipes the dir
        // Loro actually writes.
        let loro_dir = loro_storage_dir
            .clone()
            .or_else(|| orgmode_root.as_ref().map(|r| r.join(".loro")))
            .unwrap_or_else(|| db_path.parent().unwrap_or(&db_path).join(".loro"));

        // Model.md invariant 10: consolidator handover is an epoch, not a runtime
        // lookup. Fail loud if a durable vault was consolidated by a different
        // mode than the one configured now (escape hatch:
        // HOLON_CONSOLIDATOR_MIGRATE=1). The identity comes from the same
        // SessionCapabilities pin the rest of the session uses; each component
        // describes its own durable footprint.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let configured =
                holon_api::capability::SessionCapabilities::detect_and_pin(loro_enabled)
                    .consolidator_id();
            let turso_state = holon_turso::durable_state::TursoDurableState::new(&db_path);
            let loro_state =
                holon_loro::durable_state::LoroDurableState::new(&loro_dir, loro_enabled);
            // Marker home: next to the durable db, or (ephemeral db + durable
            // CRDT) next to the CRDT dir — it must survive the migrate wipe.
            let marker_dir = holon_turso::durable_state::instance_state_root(&db_path)
                .or_else(|| loro_dir.parent().map(|p| p.join(".holon")));
            crate::consolidator_epoch::guard_consolidator_epoch(
                marker_dir.as_deref(),
                &configured,
                &[&turso_state, &loro_state],
            )?;
            disclosure.performed(BootStep::ConsolidatorEpochGuard);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let mcp_integrations_dir = holon_config.resolve_mcp_integrations_dir(&config_dir);

        // Event infrastructure (needed by Loro and OrgMode)
        if loro_enabled || orgmode_root.is_some() {
            EventInfraModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register EventInfraModule: {}", e))?;
        }

        // SqlOnly is a deliberate mode, and the entities it drops must say so.
        // `LoroShareBackend` serves `tree` only inside the branch below, so
        // with the layer off `tree.share_subtree` would otherwise fail as a
        // bare missing registration — indistinguishable from a broken build.
        if !loro_enabled {
            tracing::warn!(
                "[wiring] CRDT layer disabled (crdt.enabled = false) — sharing and offline merge \
                 are unavailable in this session"
            );
            self.provide::<holon::api::operation_dispatcher::UnavailableEntities>(Provider::root(
                |_| {
                    Shared::new(holon::api::operation_dispatcher::UnavailableEntities::new(
                        [(
                            holon_loro::loro_share_backend::TREE_ENTITY,
                            "the CRDT layer is off (`crdt.enabled = false`); sharing needs it"
                                .to_string(),
                        )],
                    ))
                },
            ));
        }

        // Loro CRDT (must be before OrgMode so OrgMode can detect it)
        if loro_enabled {
            let loro_dir = loro_dir.clone();
            let loro_peer_id = session_config.loro_peer_id;
            self.provide::<LoroConfig>(Provider::root(move |_| {
                Shared::new(LoroConfig::new(loro_dir.clone()).with_peer_id(loro_peer_id))
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

            // Block-CRUD authority: when Loro is enabled it owns block CRUD (the
            // source of truth the SQL projection mirrors), so register the Loro
            // provider as the `CrudAuthority`. The org file-sync wiring resolves
            // this marker to pick the authority without naming a concrete backend
            // (SqlOnly registers none and falls back to its local SQL provider).
            self.provide::<CrudAuthority>(Provider::root(|resolver| {
                let ops = resolver.resolve::<LoroBlockOperations>();
                Shared::new(CrudAuthority(ops as Arc<dyn OperationProvider>))
            }));

            // Shared-subtree write-back disclosure (Inc 1). Forwards a
            // not-yet-materialized shared edit to the `DegradedSignalBus` (which
            // the composition root provides in every mode) so the frontend
            // banners it instead of the edit silently failing to reach disk.
            // Only wired in Loro mode — shares don't exist in SqlOnly, so its
            // absence there (di.rs WARN-logs) is correct.
            self.provide::<dyn holon_filesystem::ShareWritebackDisclosure>(Provider::root(
                |resolver| {
                    let bus = resolver.resolve::<Arc<holon_loro::DegradedSignalBus>>();
                    Arc::new(crate::loro_seams::ShareDegradedDisclosure {
                        bus: (*bus).clone(),
                    }) as Arc<dyn holon_filesystem::ShareWritebackDisclosure>
                },
            ));

            // Authoritative mount registry (Inc 3): the org ingest guard skips a
            // shared-subtree projection file ONLY when its page id is a real
            // mount node in the global Loro tree — never on drawer content alone
            // (which round-trips from any user file). Backed by LoroShareBackend
            // (async-provided), so resolve async.
            self.provide::<dyn holon_filesystem::MountRegistry>(Provider::root_async(
                |resolver| async move {
                    let backend = resolver
                        .resolve_async::<Arc<holon_loro::loro_share_backend::LoroShareBackend>>()
                        .await;
                    Arc::new(crate::loro_seams::LoroMountRegistry {
                        backend: (*backend).clone(),
                    }) as Arc<dyn holon_filesystem::MountRegistry>
                },
            ));
        }

        // OrgMode (native-only — holon-orgmode uses tokio::fs + tokio::process)
        #[cfg(not(target_arch = "wasm32"))]
        if orgmode_root.is_none() {
            disclosure.absent_by_config(BootStep::OrgModeIngest, "no vault root configured");
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(root) = orgmode_root {
            use holon_orgmode::di::OrgModeConfig;

            use crate::turso_seams::OrgModeModule;

            // Ensure the org root exists on the real disk so the config-time
            // canonicalize below resolves. The empty-vault default-document
            // seeding happens later through the FileSystem port (ADR 0011) —
            // see `seed_assets` and OrgModeModule's factory. "index.org" is
            // deliberately excluded from the sidebar query, so the seed uses
            // a different filename (notes.org).
            if !root.exists() {
                std::fs::create_dir_all(&root).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to create org root directory {}: {e}",
                        root.display()
                    )
                })?;
            }

            let mut org_config = OrgModeConfig::new(root);
            org_config.post_write_hook = post_write_hook;
            org_config.seed_assets = holon_frontend::DEFAULT_ASSETS
                .iter()
                .map(|a| (a.filename.to_string(), a.content.to_string()))
                .collect();
            self.provide::<OrgModeConfig>(Provider::root(move |_| Shared::new(org_config.clone())));

            // The vault's file formats. Org pages are read AND written; `.cook`
            // recipes are authoritative input, so the controller refuses to
            // write them back. The markdown adapters are deliberately NOT here:
            // both claim `md`, which the registry refuses at construction until
            // a vault-flavor discriminator picks between them (D56.a).
            let formats = std::sync::Arc::new(
                holon_core::FormatRegistry::new(vec![
                    std::sync::Arc::new(holon_orgmode::file_format::OrgFormatAdapter::new()),
                    std::sync::Arc::new(holon_kitchen::CookFormatAdapter::new()),
                ])
                .map_err(|e| anyhow::anyhow!("vault format registry: {e}"))?,
            );
            self.provide::<holon_core::FormatRegistry>(Provider::root(move |_| formats.clone()));

            // 3-way text merger for the no-store conflict path (spec 0008 §3.1).
            // Registered unconditionally here — NOT inside the `loro_enabled`
            // block — because the conflict path it serves is SqlOnly/`Direct`
            // mode (no Loro store). The impl is a transient LoroText (the merge
            // *function* only; it needs the `loro` crate, not a live doc), so it
            // is safe to construct in both modes; the file-sync controller
            // consults it only when `Consolidator::Store` owns the store.
            self.provide::<dyn holon_filesystem::ThreeWayTextMerge>(Provider::root(|_| {
                Arc::new(holon_loro::TransientLoroTextMerge)
                    as Arc<dyn holon_filesystem::ThreeWayTextMerge>
            }));

            OrgModeModule
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register OrgModeModule: {}", e))?;
            disclosure.performed(BootStep::OrgModeIngest);
        }

        // MCP integrations (native-only). External providers (Todoist, etc.)
        // are configured here from `{config_dir}/integrations/*.yaml`.
        #[cfg(not(target_arch = "wasm32"))]
        if mcp_integrations_dir.is_none() {
            disclosure.absent_by_config(
                BootStep::McpIntegrations,
                "no MCP integrations directory configured",
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref dir) = mcp_integrations_dir {
            let module = crate::mcp_integrations::McpIntegrationsModule::from_dir(dir, &config_dir);
            module
                .configure(self)
                .map_err(|e| anyhow::anyhow!("Failed to register McpIntegrationsModule: {}", e))?;
            disclosure.performed(BootStep::McpIntegrations);
        }

        // FrontendSession factory — resolves all dependencies from DI
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        let wait_for_ready = session_config.wait_for_ready;
        let disclosure = disclosure.clone();
        self.provide::<FrontendSession>(Provider::root_async(move |resolver| {
            let disclosure = disclosure.clone();
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
                        let hooks = mcp_registry
                            .integrations()
                            .iter()
                            .map(|i| {
                                i.sync_engine.clone() as std::sync::Arc<dyn holon_core::MatviewHook>
                            })
                            .collect();
                        if let Some(hook) = holon_core::combine_matview_hooks(hooks) {
                            engine.set_matview_hook(hook).await;
                        }
                        disclosure.performed(BootStep::McpFdwTables);
                    } else {
                        disclosure.absent_by_config(
                            BootStep::McpFdwTables,
                            "no MCP integration registry in this container",
                        );
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
                disclosure.performed(BootStep::PublishErrorTracker);
                #[cfg(not(target_arch = "wasm32"))]
                let ready_signal: Option<
                    tokio::sync::watch::Receiver<Option<Result<(), String>>>,
                > = resolver
                    .try_resolve::<holon_orgmode::di::FileWatcherReadySignal>()
                    .ok()
                    .map(|s| (*s).clone().into_receiver());
                #[cfg(not(target_arch = "wasm32"))]
                if ready_signal.is_some() {
                    disclosure.performed(BootStep::FileWatcherReadySignal);
                } else {
                    // No org module registered it — the vault-less configuration.
                    disclosure.absent_by_config(
                        BootStep::FileWatcherReadySignal,
                        "no org file watcher in this configuration",
                    );
                }

                // Transition DB to ready
                async {
                    if let Ok(db_handle_provider) = resolver.try_resolve::<dyn DbHandleProvider>() {
                        let handle = db_handle_provider.handle();
                        if let Err(e) = handle.transition_to_ready().await {
                            // Names the boot step, because the ledger cannot see
                            // WHY a step is unmarked: without this line the boot
                            // report blames `transition-db-to-ready` on wiring
                            // drift and sends the reader hunting a divergence
                            // that is not there.
                            tracing::error!(
                                "boot [component=platform-capabilities]: step \
                                 `transition-db-to-ready` FAILED at runtime: {e}. The DB actor is \
                                 still in boot mode. This — not a divergence from the shared \
                                 wiring — is why the boot report lists that step as skipped."
                            );
                        } else {
                            // Records success, not the attempt: a DB left in
                            // boot mode is a missing step, and the ledger says
                            // so instead of reporting the transition as done.
                            disclosure.performed(BootStep::TransitionDbToReady);
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
                    // Copy-on-write: a materialized `__default__.org` on disk is
                    // the durable "user modified the seed layout" marker. When
                    // present, that file WINS — the org scan owns `__default__`
                    // and `seed_default_layout` must NOT re-seed/replace it.
                    // When absent, the seed layout is virtual and re-seeds from
                    // the current bundled asset (auto-update).
                    let default_org_exists = match (
                        resolver.try_resolve::<holon_orgmode::di::OrgModeConfig>(),
                        resolver.try_resolve::<dyn holon_filesystem::FileSystem>(),
                    ) {
                        (Ok(cfg), Ok(fs)) => fs
                            .read_to_string(&cfg.root_directory.join("__default__.org"))
                            .await
                            .is_ok(),
                        _ => false,
                    };
                    crate::seed::seed_default_layout(
                        &engine,
                        ordering,
                        user_index_org_exists,
                        default_org_exists,
                    )
                    .await
                    .expect(
                        "boot [component=session stage=session-resolve]: \
                             seed_default_layout failed",
                    );
                    disclosure.performed(BootStep::SeedDefaultLayout);
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
                        match projection.flush().await {
                            Err(e) => tracing::warn!(
                                "[FrontendSession] seed-layout projection flush failed (run-loop \
                                 will reconcile): {e:#}"
                            ),
                            Ok(pass) if pass.withheld() > 0 => tracing::warn!(
                                "[FrontendSession] seed-layout projection flush withheld {} \
                                 FK-ungrounded op(s) (run-loop will reconcile)",
                                pass.withheld()
                            ),
                            Ok(_) => {}
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

                // Mirror the enablement store into `integration_state`, which
                // the seeded left-sidebar Integrations section queries. Runs
                // here rather than in the (lazily resolved) integration
                // registry: a container that never touches an integration would
                // otherwise render the section empty with everything switched
                // on. Absent view model = no integrations directory configured,
                // and an empty section is then the truth.
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    if let Ok(vm) = resolver
                        .try_resolve::<crate::integrations_settings::IntegrationsSettingsVm>()
                    {
                        std::sync::Arc::new(
                            crate::integration_projection::IntegrationStateProjector::new(
                                engine.db_handle().clone(),
                                vm,
                            ),
                        )
                        .start()
                        .await
                        .expect(
                            "boot [component=session stage=session-resolve]: \
                             IntegrationStateProjector::start failed",
                        );
                    }
                }
                .instrument(tracing::info_span!(
                    "di.factory.FrontendSession.project_integration_state"
                ))
                .await;

                // Start action watchers — streaming discovery picks up action blocks
                // as FileSyncController inserts them. Must be after seed_default_layout
                // so the block table and seed data (block:journals) exist.
                #[cfg(not(target_arch = "wasm32"))]
                async {
                    holon::api::action_watcher::start_action_watchers(engine.clone())
                        .await
                        .expect(
                            "boot [component=session stage=session-resolve]: \
                             start_action_watchers failed",
                        );
                    disclosure.performed(BootStep::StartActionWatchers);
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
                            // Copy the outcome out and drop the non-`Send`
                            // `watch::Ref` BEFORE any `.await` below.
                            let scan_error: Option<String> = {
                                let result = signal.wait_for(|v| v.is_some()).await.expect(
                                    "boot [component=org-sync stage=session-resolve]: \
                                         FileWatcherReadySignal sender dropped",
                                );
                                match result.as_ref().unwrap() {
                                    Ok(()) => None,
                                    Err(msg) => Some(msg.clone()),
                                }
                            };
                            if let Some(msg) = scan_error {
                                // A per-file initial-scan failure. Do NOT panic
                                // this detached worker — that left the window
                                // looking healthy with file sync silently dead
                                // (dogfood 2026-07-10 ship-blocker). The org
                                // watch loop is already armed (holon-orgmode
                                // di.rs no longer early-returns on failure), so
                                // the OTHER files keep syncing. The user-facing
                                // banner is raised (and cleared) per refused
                                // file by the controller's
                                // `WritebackDisclosure`, naming the format that
                                // refused it.
                                tracing::error!(
                                    "OrgMode initial scan degraded — some vault files were not \
                                     ingested; other files continue syncing: {msg}"
                                );
                            }
                        }
                        // Org initial scan is done (success, degraded, or no
                        // org module at all) — the serialized DatabaseActor is
                        // no longer saturated by boot ingest. Open the sync gate
                        // so deferred provider syncs run against an idle actor.
                        // Opened on EVERY path so a deferred sync always
                        // eventually runs (fail-loud: never a silent never-sync).
                        match resolver_bg.try_resolve::<holon_core::SyncGate>() {
                            Ok(gate) => {
                                gate.open();
                                tracing::info!("[post_ready] org scan complete — sync gate opened");
                            }
                            Err(e) => tracing::error!(
                                "[post_ready] SyncGate failed to resolve — deferred MCP syncs \
                                 will fall back to the 600s watchdog: {e}"
                            ),
                        }
                        let _ = resolver_bg
                            .try_resolve_async::<holon_loro::LoroSyncControllerHandle>()
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
                    disclosure.performed(BootStep::PostReady);
                }

                let config_dir_val = resolver.resolve::<ConfigDir>();
                let theme_registry = resolver.resolve::<theme::ThemeRegistry>();
                let preference_defs = resolver.resolve::<Vec<preferences::PreferenceDef>>();
                let holon_config = resolver.resolve::<HolonConfig>();
                let locked_keys = resolver.resolve::<LockedKeys>();
                // Registry-backed: it answers for entity types registered after
                // boot (an MCP sidecar connecting) without re-wiring.
                let link_classifier =
                    (*resolver.resolve::<holon_api::link_parser::LinkTargetClassifier>()).clone();
                disclosure.performed(BootStep::RegistryLinkClassifier);

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
                        "boot [component=turso stage=session-resolve]: FrontendSession factory: \
                         TursoBlockQuerySource::watch_default failed (the `block` + `focus_roots` \
                         matviews must exist by this point)",
                    ),
                )
                    as Arc<dyn holon_core::storage::BlockQuerySource>;
                Shared::new(FrontendSession::from_parts(SessionParts {
                    query_engine,
                    block_query,
                    operation_engine,
                    ui_watcher,
                    profiles,
                    link_classifier,
                    error_tracker,
                    ready_signal,
                    preference_defs,
                    theme_registry,
                    holon_config: (*holon_config).clone(),
                    config_dir: config_dir_val.0.clone(),
                    locked_keys: locked_keys.0.clone(),
                    // Closes the ledger and logs every step that never ran.
                    // Required by SessionParts, so it cannot be dropped.
                    boot_report: disclosure.finish(),
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

/// Reusable module for frontend services: config, conditional modules, session
/// factory.
///
/// - `configure()`: registers all frontend services via
///   [`FrontendInjectorExt::add_frontend`]
/// - `on_start()`: resolves `FrontendSession` (triggers the async factory
///   chain)
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
