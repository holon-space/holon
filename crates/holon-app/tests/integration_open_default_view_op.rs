//! Clicking an Integrations row opens that integration's view page.
//!
//! `integration.open_default_view` resolves the sidecar's `default_view` — the
//! BARE id of a page block — and hands it to `navigation.focus` for region
//! `main`, which is the same door a sidebar page-click goes through. The
//! sidecar is the authority; `integration_state.default_view` mirrors it.
//!
//! Every way the resolution can fail is a REFUSAL here, because each one would
//! otherwise present as a click that does nothing: no `default_view` in the
//! sidecar, and a `default_view` naming a block that does not exist.
//!
//! These tests drive the REAL dispatch path — `McpIntegrationsModule` registers
//! the provider exactly as the app does — for the reason
//! `integration_set_field_op` states: a hand-built provider would keep passing
//! after the DI registration was dropped.
//!
//! @pbt kind harness
//! @pbt covers integration-open-default-view — dispatching
//! `integration.open_default_view` focuses the sidecar's view page in the main
//! panel, and refuses by name when there is no page to focus
//! @pbt overlaps integration_set_field_op — kept: that file pins the
//! operation → store WRITE; this one pins the operation → NAVIGATION read

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;

/// The page `assets/integrations/claude-history.yaml` names as its default
/// view. Spelled out rather than read from the sidecar so the assertion states
/// the contract the sidebar lane authors its page against.
const CLAUDE_HISTORY_VIEW: &str = "claude-history-view";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

struct NoBrowser;

impl holon_mcp_client::oauth_bootstrap::BrowserOpener for NoBrowser {
    fn open(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn engine_over(
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
) -> Arc<holon::api::BackendEngine> {
    let (engine, ()) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            holon_loro_wiring::EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule for the op test: {e}"))?;
            // The `block` entity's write door, so the rung can author the view
            // page through an operation rather than an INSERT.
            injector.provide_into_set::<dyn holon_core::OperationProvider>(fluxdi::Provider::root(
                |r| {
                    let db = r.resolve::<dyn holon::di::DbHandleProvider>().handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            // `open_default_view` refuses a view page the store does not hold,
            // and asks the backend-blind reader seam. `OrgModeModule` registers
            // it in the app; registering the seam alone here keeps the test off
            // the file-sync machinery it has no use for.
            injector.provide::<dyn holon_filesystem::sync_ports::BlockReader>(
                fluxdi::Provider::root_async(|r| async move {
                    let cache = r
                        .resolve_async::<holon::core::queryable_cache::QueryableCache<
                            holon_api::block::Block,
                        >>()
                        .await;
                    Arc::new(holon_app::turso_seams::CacheBlockReader::new(cache))
                        as Arc<dyn holon_filesystem::sync_ports::BlockReader>
                }),
            );
            holon_app::mcp_integrations::McpIntegrationsModule::from_dir(&state_dir, &state_dir)
                .with_browser(Arc::new(NoBrowser))
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure McpIntegrationsModule for op test: {e}"))
        },
        |_| async {},
    )
    .await
    .expect("fresh-db lazy DI graph must build");
    engine
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_dir = dir.path().join("integrations");
    std::fs::create_dir_all(&state_dir).expect("state dir");
    (dir, state_dir)
}

fn row(provider: &str) -> StorageEntity {
    let mut p: StorageEntity = HashMap::new();
    p.insert(
        "id".into(),
        Value::String(format!("integration:{provider}")),
    );
    p
}

async fn open(engine: &holon::api::BackendEngine, provider: &str) -> anyhow::Result<()> {
    engine
        .execute_operation(
            &EntityName::new("integration"),
            "open_default_view",
            row(provider),
            OpOrigin::User,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Author the page the sidecar points at. The sidebar lane authors it in
/// `assets/default/index.org` for the app; a rung that leaned on that seed
/// would be testing the asset, not the operation.
async fn author_view_page(engine: &holon::api::BackendEngine, bare_id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(format!("block:{bare_id}")));
    params.insert(
        "content".into(),
        Value::String("Claude History".to_string()),
    );
    params.insert("sort_key".into(), Value::String("a0".to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::User)
        .await
        .unwrap_or_else(|e| panic!("authoring the view page `{bare_id}`: {e}"));
}

/// What the main region is focused on, read from the navigation tables the
/// provider writes.
async fn focused_in_main(engine: &holon::api::BackendEngine) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            "SELECT h.block_id FROM navigation_cursor c \
             JOIN navigation_history h ON h.id = c.history_id \
             WHERE c.region = 'main'",
            HashMap::new(),
        )
        .await
        .expect("read the main region's cursor");
    rows.into_iter().next().and_then(|r| {
        r.get("block_id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
    })
}

#[test]
fn opening_claude_history_focuses_its_view_page_in_main() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let engine = engine_over(dir.path().join("fresh.db"), state_dir).await;
        author_view_page(&engine, CLAUDE_HISTORY_VIEW).await;

        open(&engine, "claude-history")
            .await
            .expect("claude-history declares a default_view and its page exists");

        assert_eq!(
            focused_in_main(&engine).await.as_deref(),
            Some(format!("block:{CLAUDE_HISTORY_VIEW}").as_str()),
            "the operation must focus the sidecar's view page in the main region"
        );
    });
}

/// A provider whose sidecar states no `default_view` has nothing to open, and
/// says so. A guard that merely hid the row would leave the same click doing
/// nothing with no way to find out why.
#[test]
fn a_provider_with_no_default_view_is_refused_by_name() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let engine = engine_over(dir.path().join("fresh.db"), state_dir).await;

        let err = open(&engine, "todoist")
            .await
            .expect_err("todoist declares no default_view")
            .to_string();

        assert!(
            err.contains("todoist"),
            "the refusal must name the provider: {err}"
        );
        assert!(
            err.contains("default_view"),
            "the refusal must name the missing key: {err}"
        );
        assert_eq!(
            focused_in_main(&engine).await,
            None,
            "a refused open must not move the main region"
        );
    });
}

/// The sidecar's `default_view` is a REFERENCE, so it can dangle — a page
/// renamed or never authored. Focusing a block the store does not hold would
/// blank the main panel, which is the degradation this refusal replaces.
#[test]
fn a_default_view_naming_no_block_is_refused_by_name() {
    let rt = runtime();
    rt.clone().block_on(async {
        let (dir, state_dir) = fixture();
        let engine = engine_over(dir.path().join("fresh.db"), state_dir).await;
        // Deliberately do NOT author the page.

        let err = open(&engine, "claude-history")
            .await
            .expect_err("the view page was never authored")
            .to_string();

        assert!(
            err.contains(CLAUDE_HISTORY_VIEW),
            "the refusal must name the block it could not find: {err}"
        );
        assert_eq!(
            focused_in_main(&engine).await,
            None,
            "a refused open must not move the main region"
        );
    });
}
