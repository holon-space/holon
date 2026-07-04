//! In-process per-case reset support (Phase 1 Option A).
//!
//! [`build_fresh_sut`] boots a production-faithful `FrontendSession` +
//! `ReactiveEngine` on fresh, seeded paths WITHOUT starting an MCP server (that
//! is `GpuiModule`-only). The reset pump then `rebind`s the live window onto the
//! fresh SUT. See `docs/Testing/Phase1_OptionA_RebindReset_Plan.md`.
//!
//! Mirrors `HeadlessFrontendComponent::new_with_loro`
//! (`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs`) with
//! two deltas for the real app: (a) REAL filesystem — the caller materializes
//! seed files onto `org_root` on disk (no `InMemoryFileSystem`); (b) it lives in
//! the gpui crate for colocation with `rebind` + `render_supported_widgets`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::FrontendSession;

/// A freshly-booted, seeded SUT for an in-process rebind reset. Holds the
/// `BackendEngine` so the retirement list (plan F) can keep it (and its temp
/// dirs) alive/inert after the window rebinds away from it.
pub struct FreshSut {
    pub session: Arc<FrontendSession>,
    pub engine: Arc<ReactiveEngine>,
    pub backend: Arc<BackendEngine>,
}

/// Build a production-faithful `FrontendSession` + `ReactiveEngine` on the given
/// fresh, seeded paths, WITHOUT starting an MCP server (that is `GpuiModule`-only,
/// so the already-running server is reused — see plan C2 for the live-cell swap).
///
/// `org_root` must already contain the seed `.org` files (caller writes them).
/// All three paths MUST be fresh per reset (plan D/F): the retired engine's
/// file-sync controller keeps watching the old root and writes `:ID:` drawers
/// back into it, so in-place reuse lets the retired engine corrupt the new seed.
pub async fn build_fresh_sut(
    db_path: PathBuf,
    org_root: PathBuf,
    config_dir: PathBuf,
    settle: Duration,
) -> anyhow::Result<FreshSut> {
    use holon_frontend::config::VaultConfig;
    use holon_frontend::reactive::{
        BuilderServices, BuilderServicesSlot, RenderInterpreterInjectorExt,
    };
    use holon_frontend::{HolonConfig, SessionConfig};

    let mut holon_config = HolonConfig {
        db_path: Some(db_path),
        vault: VaultConfig {
            root: Some(org_root),
        },
        ..Default::default()
    };
    holon_config.crdt.enabled = Some(true); // mirror mobile.rs boot

    let ui_info = holon_api::UiInfo {
        available_widgets: crate::render_supported_widgets(),
        screen_size: None,
    };
    let session_config = SessionConfig::new(ui_info);

    // Capture the DI injector so we can resolve the Loro `BlockCellRegistry`
    // after build (the windowless path bypasses frontend `on_start`).
    let injector_slot: Arc<std::sync::OnceLock<fluxdi::Injector>> =
        Arc::new(std::sync::OnceLock::new());
    let injector_slot_c = injector_slot.clone();

    let (session, backend, reactive) = holon_app::new_from_config_with_di(
        holon_config,
        session_config,
        config_dir,
        std::collections::HashSet::new(),
        move |injector| {
            // `GpuiModule::configure` minus MCP (di.rs). NO add_mcp_server, NO
            // register_debug_services — the existing MCP server is reused (C2).
            let slot = injector.resolve::<BuilderServicesSlot>();
            injector.set_render_interpreter(crate::make_interpret_fn(slot.0.clone()));
            Ok(())
        },
        move |injector| {
            // `GpuiModule::on_start` minus MCP (di.rs).
            let engine = injector.resolve::<ReactiveEngine>();
            let slot = injector.resolve::<BuilderServicesSlot>();
            let services: Arc<dyn BuilderServices> = engine.clone();
            slot.0.set(services).ok(); // ALLOW(ok): OnceLock set — idempotent
            injector_slot_c.set(injector.clone()).ok(); // ALLOW(ok): OnceLock set
            engine
        },
    )
    .await?;

    // Loro editor-cell registry (crdt on) — else typing errs "no MutableText".
    // Mirrors components.rs:208-221.
    {
        let injector = injector_slot
            .get()
            .expect("injector captured in extra_resolve");
        let registry: Arc<holon::sync::block_cell_registry::BlockCellRegistry> = injector
            .resolve_async::<holon::sync::block_cell_registry::BlockCellRegistry>()
            .await;
        let registry_dyn: Arc<dyn holon_frontend::cell::EntityCellRegistry> = registry;
        reactive
            .block_cell_registry
            .lock()
            .unwrap()
            .replace(registry_dyn);
    }

    // Boot settle. The window runs its own settle-to-fixed-point after rebind,
    // so a bounded sleep here is enough to let the org drain + first projection
    // land before we hand the SUT off.
    if settle > Duration::ZERO {
        tokio::time::sleep(settle).await;
    }

    Ok(FreshSut {
        session,
        engine: reactive,
        backend,
    })
}

/// Materialize `files` into fresh temp paths and boot a [`FreshSut`], returning
/// it as the mcp-crate [`ResetBuildOutput`] the `reset_vault` tool consumes.
///
/// This is the concrete builder installed on `DebugServices::reset_builder`
/// (see `mobile.rs`). It owns a single `TempDir` holding the fresh org root,
/// db, and config dir; that `TempDir` is handed back in `retire` so the tool's
/// retirement list keeps the retired engine's paths alive-but-abandoned.
pub async fn build_fresh_sut_from_files(
    files: Vec<(String, String)>,
) -> anyhow::Result<holon_mcp::server::ResetBuildOutput> {
    let temp = tempfile::TempDir::new()?;
    let base = temp.path().to_path_buf();
    let org_root = base.join("vault");
    let config_dir = base.join("config");
    std::fs::create_dir_all(&org_root)?;
    std::fs::create_dir_all(&config_dir)?;
    for (name, content) in &files {
        std::fs::write(org_root.join(name), content)?;
    }
    let db_path = base.join("holon.db");

    let fresh = build_fresh_sut(db_path, org_root, config_dir, Duration::from_millis(500)).await?;

    Ok(holon_mcp::server::ResetBuildOutput {
        session: fresh.session,
        engine: fresh.engine,
        backend: fresh.backend,
        retire: Box::new(temp),
    })
}
