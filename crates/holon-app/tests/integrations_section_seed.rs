//! Seed-shape + deletion-stickiness guards for the left-sidebar
//! **Integrations** discovery section.
//!
//! Martin's ruling: synced MCP-integration data must be observable via an
//! Integrations section in the LEFT sidebar, BELOW the page hierarchy,
//! separated by a divider — and it must be ordinary, user-editable layout DATA
//! (seeded by default), NOT hardcoded UI. The section is expressed inside the
//! seeded left-sidebar render block (`assets/default/index.org`): the
//! page-hierarchy `tree(...)` is wrapped in a `column(...)` followed by
//! `divider()`, an "Integrations" header, and a `live_query` over the
//! `integration_state` table (the queryable mirror of the enablement store).
//!
//! Two guards:
//!  1. `fresh_seed_places_integrations_section_below_hierarchy` — after a fresh
//!     seed the left-sidebar render carries the page hierarchy, THEN a divider,
//!     THEN the Integrations section (header + integration_state query), in
//!     that order.
//!  2. `deleted_integrations_section_does_not_resurrect_on_reseed` — deleting
//!     the render block and re-seeding (the non-fresh boot path) does NOT bring
//!     it back: layout is seeded fresh-only, so a user deletion sticks.
//!
//! WHAT the section resolves to — that its rows are exactly the enabled
//! integrations, each with its boot status — is pinned separately, by
//! `integrations_enablement_projection.rs`.
//!
//! @pbt kind harness
//! @pbt covers integrations-section-seed — left-sidebar Integrations discovery
//! section is seeded below the page hierarchy behind a divider, as ordinary
//! deletable layout data (deletion sticks across reseed)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone renders the
//! layout but does not assert the seeded render-block shape

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon::storage::BLOCK_READ_TABLE;
use holon::sync::EventInfraModule;

const LEFT_SIDEBAR_ID: &str = "block:default-left-sidebar";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// Build a fresh file-backed SqlOnly engine + `BlockOrdering` through the same
/// lazy-DI entry the gpui frontend uses. Mirrors
/// `fresh_db_boot_seed_smoke.rs`; `integration_state` is materialized as an
/// eager schema root by the `BackendEngine` factory, so the seeded Integrations
/// `live_query` has a real table to read.
async fn fresh_engine(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::BackendEngine>,
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            EventInfraModule.configure(injector).map_err(|e| {
                anyhow::anyhow!("configure EventInfraModule for integrations-seed test: {e}")
            })?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            Ok(())
        },
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("fresh-db lazy DI graph must build (BackendEngine + BlockOrdering)")
}

/// The left-sidebar render block's content: the sole `render`-language child of
/// `block:default-left-sidebar` (id resolved from the query, not hard-coded, so
/// the test is robust to the org parser's id-derivation scheme).
async fn left_sidebar_render_content(db: &holon::storage::DbHandle) -> Option<(String, String)> {
    let rows = db
        .query(
            &format!(
                "SELECT id, content FROM {BLOCK_READ_TABLE} \
                 WHERE parent_id = '{LEFT_SIDEBAR_ID}' AND source_language = 'render'"
            ),
            HashMap::new(),
        )
        .await
        .expect("query left-sidebar render block");
    rows.first().map(|row| {
        let id = row
            .get("id")
            .and_then(|v| v.as_string())
            .expect("render row has id")
            .to_string();
        let content = row
            .get("content")
            .and_then(|v| v.as_string())
            .expect("render row has content")
            .to_string();
        (id, content)
    })
}

/// After a fresh seed the left-sidebar render composes, IN ORDER: the
/// page-hierarchy tree, a divider, then the Integrations discovery section
/// (header + an `integration_state` query). This is the ruling made concrete —
/// discovery section BELOW the hierarchy, separated by a divider.
#[test]
fn fresh_seed_places_integrations_section_below_hierarchy() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;

        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout must complete on a fresh file DB");

        let db = engine.db_handle();
        let (_, content) = left_sidebar_render_content(db)
            .await
            .expect("left sidebar must have a seeded render block after fresh seed");

        // The page hierarchy is still there (unchanged tree over Pages).
        let tree_at = content
            .find("tree(")
            .expect("render must still contain the page-hierarchy tree");
        // A divider separates the two sections.
        let divider_at = content
            .find("divider(")
            .expect("render must contain a divider() separating hierarchy from integrations");
        // The Integrations discovery section: header + a query over
        // `integration_state`, the queryable mirror of the enablement store.
        let header_at = content
            .find("Integrations")
            .expect("render must contain the Integrations section header");
        let mirror_at = content
            .find("integration_state")
            .expect("integrations section must query the integration_state mirror");

        assert!(
            tree_at < divider_at && divider_at < header_at && header_at < mirror_at,
            "order must be: page hierarchy, divider, Integrations header, integration_state \
             query — got tree@{tree_at} divider@{divider_at} header@{header_at} \
             integration_state@{mirror_at} in: {content}"
        );
        // Seeded as a live, reactive query surface (not a static snapshot), so
        // toggling an integration re-renders the section without a restart.
        assert!(
            content.contains("live_query"),
            "integrations section must be a live_query so enablement changes surface: {content}"
        );
    });
}

/// The seeded sidebar and the Settings modal render the SAME list.
///
/// Both embed `integrations_section::sidebar_live_query_src()`; the seed
/// carries it as org text, which nothing but this assertion keeps in step.
/// Without it the two surfaces drift into two lists that merely resemble each
/// other — and the modal's would be the one nobody noticed had gone stale.
#[test]
fn the_seeded_section_embeds_the_shared_list_source_verbatim() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout must complete on a fresh file DB");

        let (_, content) = left_sidebar_render_content(engine.db_handle())
            .await
            .expect("left sidebar must have a seeded render block after fresh seed");

        let shared = holon_app::integrations_section::sidebar_live_query_src();
        assert!(
            content.contains(&shared),
            "the seeded Integrations section must embed the shared list source verbatim.\n  \
             expected: {shared}\n  in: {content}"
        );
    });
}

/// A user who deletes the Integrations-bearing render block must have that
/// stick: the layout is seeded ONLY on a fresh boot (root layout absent), so
/// re-running the seed on an already-seeded DB (the every-boot path) never
/// resurrects it.
#[test]
fn deleted_integrations_section_does_not_resurrect_on_reseed() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("fresh.db");
        let (engine, ordering) = fresh_engine(db_path).await;

        // First (fresh) boot seeds the full layout incl. the integrations section.
        holon_app::seed_default_layout(&engine, ordering.clone(), false, false)
            .await
            .expect("first seed");
        let db = engine.db_handle();
        let (render_id, first) = left_sidebar_render_content(db)
            .await
            .expect("render present after first seed");
        assert!(
            first.contains("integration_state"),
            "sanity: first seed carries the integrations section"
        );

        // User deletes the render block (their layout, their call).
        engine
            .execute_operation(
                &holon_api::EntityName::from("block"),
                "delete",
                {
                    let mut p = holon_api::StorageEntity::new();
                    p.insert("id".into(), holon_api::Value::String(render_id.clone()));
                    p
                },
                holon_api::OpOrigin::User,
            )
            .await
            .expect("delete render block");
        assert!(
            left_sidebar_render_content(db).await.is_none(),
            "render block must be gone right after deletion"
        );

        // Every-boot reseed (root layout now exists → fresh=false).
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("reseed on already-seeded DB");

        assert!(
            left_sidebar_render_content(db).await.is_none(),
            "deleted Integrations section MUST stay deleted after reseed — layout is seeded \
             fresh-only, so a user deletion sticks"
        );
    });
}
