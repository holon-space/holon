//! The shipped GPUI container hands its MCP tools the vault root write-back
//! uses.
//!
//! `dense_patch` checks every row against the file that holds it, under the
//! root in `DebugServices`; without that root every patch is refused
//! (bug-funnel `2026-10-06-gpui-mcp-dense-patch-names-no-vault-root`).
//!
//! @pbt kind harness
//! @pbt covers gpui-mcp-debug-services-wiring — booting `GpuiModule` fills
//! `DebugServices` with the session's vault root, org filesystem and
//! write-back renderer
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone never
//! assembles the gpui module graph, so it cannot see a missing population

use std::collections::HashSet;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::McpConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_gpui::di::GpuiModule;
use holon_mcp::server::DebugServices;

#[test]
fn booting_the_gpui_module_gives_the_mcp_tools_the_vault_root() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = fluxdi::Application::new(GpuiModule {
            holon_config: HolonConfig {
                db_path: Some(dir.path().join("holon.db")),
                vault: VaultConfig {
                    root: Some(dir.path().to_path_buf()),
                },
                mcp: McpConfig {
                    enabled: Some(false),
                },
                ..Default::default()
            },
            session_config: SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
            config_dir: dir.path().to_path_buf(),
            locked_keys: HashSet::new(),
            conditions: std::sync::Arc::new(holon_api::ConditionBus::new()),
        });
        app.bootstrap().await.expect("GpuiModule boots");
        let injector = app.injector();

        let debug = injector.resolve::<DebugServices>();
        let cell = debug
            .live_debug
            .read()
            .expect("live_debug cell poisoned")
            .clone();
        assert_eq!(
            cell.org_root,
            Some(std::fs::canonicalize(dir.path()).expect("canonical vault root")),
            "the MCP tools must see the vault root write-back uses"
        );
        assert!(
            cell.writeback_renderer.is_some(),
            "the MCP tools must see the write-back renderer"
        );
        assert!(
            debug.org_fs.get().is_some(),
            "the MCP tools must read org files through the session's filesystem"
        );

        holon_app::shutdown_session(&injector)
            .await
            .expect("the session shuts down");
    });
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
