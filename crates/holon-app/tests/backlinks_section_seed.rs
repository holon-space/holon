//! Seed-shape + data guards for the main-panel **Linked references**
//! (backlinks) section.
//!
//! Martin's ruling (2026-07-21): backlinks must NOT be hard-coded in Rust —
//! they live in the declarative render pipeline (`assets/default/`). The
//! section is expressed inside the seeded main-panel render block
//! (`assets/default/index.org`): the page outline (`collection_view()`, the
//! block's profile-derived tree/table/board switcher) is wrapped in a
//! `column(...)` followed by `divider()`, a "Linked references" header, and a
//! `live_query` over the `backlinks` matview scoped to the current page via a
//! `focus_roots` join.
//!
//! The one generic Rust capability this needs is the `collection_view()`
//! render marker (expanded by `BlockDomain::render_entity` into the block's
//! default collection view), so the declarative render can compose the outline
//! with surrounding chrome WITHOUT losing view-mode switching and WITHOUT
//! hard-coding the tree.
//!
//! Guards:
//!  1. `fresh_seed_places_backlinks_section_below_outline` — the seeded
//!     main-panel render composes outline → divider → header → backlinks query,
//!     in order.
//!  2. `render_entity_expands_collection_view_marker` — rendering the main
//!     panel expands `collection_view()` into the real view-mode switcher and
//!     keeps the backlinks `live_query`; no raw marker leaks to the frontend.
//!  3. `backlinks_query_lists_incoming_links_for_focused_page` — with the main
//!     region focused on a page, the section query returns the blocks linking
//!     to it; focusing a page with no incoming links returns nothing.
//!
//! @pbt kind harness
//! @pbt covers backlinks-section-seed — main-panel Linked-references section is
//! seeded below the outline behind a divider as declarative layout data, and
//! its query returns incoming resolved links for the focused page.
//! @pbt overlaps block_links_junction — kept: the junction test proves the
//! `backlinks` matview lifecycle; this proves the seeded render wiring +
//! focus-scoped query.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon::storage::BLOCK_READ_TABLE;
use holon::sync::EventInfraModule;
use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::OpOrigin;
use holon_api::Value;

const MAIN_PANEL_ID: &str = "block:default-main-panel";

/// Extract the backlinks `live_query` SQL from the *seeded* main-panel render
/// content — the actual shipped query, not a hand-copied duplicate. Binds this
/// guard to `assets/default/index.org` so a drift in the section SQL can no
/// longer slip past the test (the prior hardcoded `SECTION_SQL` const could).
///
/// The main-panel render composes exactly one `live_query(#{sql: "..."})` (the
/// backlinks section), so scanning for the first `sql: "..."` string literal is
/// unambiguous. The seeded SQL uses single-quoted string literals (`'main'`),
/// so the closing double-quote is the section boundary.
fn extract_backlinks_sql(render_content: &str) -> String {
    let key = "sql: \"";
    let start = render_content
        .find(key)
        .expect("main-panel render must contain a live_query sql literal")
        + key.len();
    let rest = &render_content[start..];
    let end = rest
        .find('"')
        .expect("live_query sql literal must be terminated by a double quote");
    rest[..end].to_string()
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// Fresh file-backed SqlOnly engine + `BlockOrdering` through the same lazy-DI
/// entry the gpui frontend uses (mirrors `integrations_section_seed.rs`).
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
                anyhow::anyhow!("configure EventInfraModule for backlinks-seed test: {e}")
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

/// The main-panel render block's content: the sole `render`-language child of
/// `block:default-main-panel` (id resolved from the query, not hard-coded).
async fn main_panel_render_content(db: &holon::storage::DbHandle) -> Option<String> {
    let rows = db
        .query(
            &format!(
                "SELECT content FROM {BLOCK_READ_TABLE} \
                 WHERE parent_id = '{MAIN_PANEL_ID}' AND source_language = 'render'"
            ),
            HashMap::new(),
        )
        .await
        .expect("query main-panel render block");
    rows.first().map(|row| {
        row.get("content")
            .and_then(|v| v.as_string())
            .expect("render row has content")
            .to_string()
    })
}

/// An id-form (`EntityRef::Internal`) link mark — resolves trivially at the
/// write boundary, so the `backlinks` matview picks it up without needing the
/// target to exist or be Page-tagged.
fn id_link_marks(target_local: &str, label: &str, start: usize, end: usize) -> String {
    holon_api::marks_to_json(&[MarkSpan::new(
        start,
        end,
        InlineMark::Link {
            target: EntityRef::Internal {
                id: EntityUri::block(target_local),
            },
            label: label.to_string(),
        },
    )])
}

async fn create_linking_block(
    engine: &holon::api::BackendEngine,
    block_local: &str,
    content: &str,
    target_local: &str,
    label: &str,
) {
    let block_entity: EntityName = "block".to_string().into();
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{block_local}")));
    p.insert("content".into(), Value::String(content.to_string()));
    p.insert(
        "marks".into(),
        Value::String(id_link_marks(target_local, label, 0, label.len())),
    );
    engine
        .execute_operation(&block_entity, "create", p, OpOrigin::User)
        .await
        .expect("create linking block");
}

/// Point the main region's focus at `block_local` (closes any prior open main
/// focus first, mirroring `focus_replace`).
async fn focus_main(db: &holon::storage::DbHandle, block_local: &str) {
    db.execute_values(
        "UPDATE navigation_history SET closed_at = datetime('now') \
         WHERE region = 'main' AND closed_at IS NULL",
        vec![],
    )
    .await
    .expect("close prior main focus");
    db.execute_values(
        &format!(
            "INSERT INTO navigation_history (region, block_id) \
             VALUES ('main', 'block:{block_local}')"
        ),
        vec![],
    )
    .await
    .expect("insert main focus row");
}

async fn section_result_ids(db: &holon::storage::DbHandle, sql: &str) -> Vec<String> {
    let rows = db
        .query(sql, HashMap::new())
        .await
        .expect("backlinks section query must compile and run (fail loud, not empty)");
    rows.into_iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("section row has id")
                .to_string()
        })
        .collect()
}

/// The seeded main-panel render composes, IN ORDER: the page outline
/// (`collection_view()`), a divider, the "Linked references" header, then a
/// `live_query` over the `backlinks` matview scoped by `focus_roots`.
#[test]
fn fresh_seed_places_backlinks_section_below_outline() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;

        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout must complete on a fresh file DB");

        let db = engine.db_handle();
        let content = main_panel_render_content(&db)
            .await
            .expect("main panel must have a seeded render block after fresh seed");

        let outline_at = content
            .find("collection_view(")
            .expect("render must embed the page outline via collection_view()");
        let divider_at = content
            .find("divider(")
            .expect("render must contain a divider() separating outline from backlinks");
        let header_at = content
            .find("Linked references")
            .expect("render must contain the Linked references header");
        let query_at = content
            .find("live_query")
            .expect("backlinks section must be a live_query (reactive)");

        assert!(
            outline_at < divider_at && divider_at < header_at && header_at < query_at,
            "order must be: outline, divider, header, backlinks query — got \
             outline@{outline_at} divider@{divider_at} header@{header_at} \
             query@{query_at} in: {content}"
        );
        assert!(
            content.contains("backlinks") && content.contains("focus_roots"),
            "section must query the backlinks matview scoped to the current page \
             via focus_roots: {content}"
        );
    });
}

/// Rendering the main panel expands the `collection_view()` marker into the
/// real view-mode switcher (view modes preserved) and keeps the backlinks
/// `live_query` — no raw marker reaches the frontend.
#[test]
fn render_entity_expands_collection_view_marker() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout");

        let main_uri = EntityUri::block("default-main-panel");
        let (expr, _stream) = engine
            .blocks()
            .render_entity(&main_uri, &None)
            .await
            .expect("render_entity(main panel) must resolve");
        let dump = format!("{expr:?}");

        assert!(
            dump.contains("view_mode_switcher"),
            "collection_view() must expand to the profile view-mode switcher \
             (view modes preserved), got: {dump}"
        );
        assert!(
            dump.contains("backlinks"),
            "the composed render must keep the backlinks live_query: {dump}"
        );
        assert!(
            !dump.contains("collection_view"),
            "the collection_view() marker must be fully substituted, not leaked \
             to the frontend: {dump}"
        );
    });
}

/// With the main region focused on a page, the section query returns the
/// blocks linking to it (alphabetical by content); focusing a page with no
/// incoming links returns nothing (the section renders empty, never
/// fabricated).
#[test]
fn backlinks_query_lists_incoming_links_for_focused_page() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering.clone(), false, false)
            .await
            .expect("seed_default_layout");

        // Two blocks link to page `alice`; none link to page `bob`.
        create_linking_block(&engine, "ref-b", "beta mentions", "alice", "Alice").await;
        create_linking_block(&engine, "ref-a", "alpha mentions", "alice", "Alice").await;

        let db = engine.db_handle();
        let render = main_panel_render_content(&db)
            .await
            .expect("main panel must have a seeded render block");
        let sql = extract_backlinks_sql(&render);

        // Bind the executed query to the SEEDED asset, not a hand-copied const.
        let render = main_panel_render_content(&db)
            .await
            .expect("main panel must have a seeded render block after fresh seed");
        let section_sql = extract_backlinks_sql(&render);

        focus_main(&db, "alice").await;
        assert_eq!(
            section_result_ids(&db, &section_sql).await,
            vec!["block:ref-a".to_string(), "block:ref-b".to_string()],
            "focused on alice → its incoming links, ordered by content (alpha, beta)"
        );

        focus_main(&db, "bob").await;
        assert!(
            section_result_ids(&db, &section_sql).await.is_empty(),
            "focused on bob (no incoming links) → empty section, never fabricated"
        );
    });
}

/// Corpus guard: EVERY query source block shipped in the default seed
/// (`holon_sql` / `holon_gql` / `holon_prql`) must compile to SQL and execute
/// against a freshly-booted, freshly-seeded vault. Compile-only is the floor;
/// this goes to the delivery floor — the query actually runs (empty result is
/// fine; a compile or execution error is not).
///
/// This is the coverage that would have caught a seeded query body silently
/// rotting (BugFunnel 2026-07-22 landmine 1: the main-panel recursive-CTE that
/// never delivered). It binds directly to the SEEDED asset content (read back
/// from the DB after `seed_default_layout`), so it auto-covers new src blocks
/// with no test edit. It CANNOT catch the vault-scale never-deliver pathology
/// (that only bites at ~real-vault size) — see the BugFunnel row.
#[test]
fn every_seeded_source_block_compiles_and_executes_against_booted_vault() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout");

        let db = engine.db_handle();
        let rows = db
            .query(
                &format!(
                    "SELECT id, source_language, content FROM {BLOCK_READ_TABLE} \
                     WHERE content_type = 'source' AND source_language IN \
                     ('holon_sql', 'holon_gql', 'holon_prql')"
                ),
                HashMap::new(),
            )
            .await
            .expect("query seeded source blocks");

        assert!(
            !rows.is_empty(),
            "expected the default seed to contain query source blocks"
        );

        let mut failures: Vec<String> = Vec::new();
        for row in &rows {
            let id = row
                .get("id")
                .and_then(|v| v.as_string())
                .expect("source block row has id")
                .to_string();
            let lang_str = row
                .get("source_language")
                .and_then(|v| v.as_string())
                .expect("source block row has source_language")
                .to_string();
            let body = row
                .get("content")
                .and_then(|v| v.as_string())
                .expect("source block row has content")
                .to_string();

            let lang: holon_api::QueryLanguage = match lang_str.parse() {
                Ok(l) => l,
                Err(e) => {
                    failures.push(format!(
                        "{id}: unparseable source_language {lang_str:?}: {e}"
                    ));
                    continue;
                }
            };

            let sql = match engine.compile_to_sql(&body, lang) {
                Ok(sql) => sql,
                Err(e) => {
                    failures.push(format!(
                        "{id} ({lang_str}): compile failed: {e}\n    {body}"
                    ));
                    continue;
                }
            };

            if let Err(e) = engine
                .execute_query(sql.clone(), HashMap::new(), None)
                .await
            {
                failures.push(format!(
                    "{id} ({lang_str}): execute failed: {e}\n    body: {body}\n    sql: {sql}"
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "seeded query source blocks failed to compile/execute against a booted \
             vault (seed rot):\n{}",
            failures.join("\n")
        );
    });
}


/// Structural lock on the SHIPPED backlinks SQL: it must join `backlinks` to
/// `focus_roots` on the **equi-join ON** shape `ON bl.target_id = fr.root_id`.
/// Turso IVM can only maintain the join incrementally when the join key is in
/// the ON clause; the region filter must be a WHERE predicate, not the ON. The
/// earlier constant-ON form (`... ON fr.region = 'main' WHERE bl.target_id =
/// fr.root_id`) made the join a cross-join-then-filter that IVM cannot
/// maintain. Extracting from the asset (not a copied constant) means a
/// regression to that form in `assets/default/index.org` fails HERE.
#[test]
fn section_sql_locks_ivm_equijoin_on_shape() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, ordering) = fresh_engine(dir.path().join("fresh.db")).await;
        holon_app::seed_default_layout(&engine, ordering, false, false)
            .await
            .expect("seed_default_layout");

        let db = engine.db_handle();
        let render = main_panel_render_content(&db)
            .await
            .expect("main panel must have a seeded render block");
        let sql = extract_backlinks_sql(&render);

        assert!(
            sql.contains("backlinks") && sql.contains("focus_roots"),
            "backlinks section must join the backlinks matview to focus_roots: {sql}"
        );
        assert!(
            sql.contains("ON bl.target_id = fr.root_id"),
            "IVM-load-bearing: join key must be in the ON clause \
             (ON bl.target_id = fr.root_id), not a constant-ON cross-join: {sql}"
        );
    });
}
