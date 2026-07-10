//! Default layout seeding for the browser worker.
//!
//! `holon-orgmode` cannot compile on wasm32-wasip1-threads (it pulls `notify`
//! which has no wasm backend). This module seeds the same blocks that
//! `FrontendSession::seed_default_layout` would produce by executing the SQL
//! directly, using block IDs that were verified against the org parser.
//!
//! Block IDs and parent chain were captured by running the org parser on
//! `assets/default/index.org` with doc_uri = `sentinel:no_parent`:
//!
//!   document:             sentinel:no_parent  (parent sentinel:no_parent)
//!   root-layout:          block:root-layout   (parent sentinel:no_parent)
//!   root-layout gql:      block:root-layout::src::0
//!   app-layout render:    block:holon-app-layout::render::0
//!   left sidebar:         block:default-left-sidebar
//!   left sidebar render:  block:default-left-sidebar::render::0
//!   left sidebar prql:    block:default-left-sidebar::src::0
//!   main panel:           block:default-main-panel
//!   main panel gql:       block:default-main-panel::src::0
//!   right sidebar:        block:default-right-sidebar
//!   right sidebar prql:   block:default-right-sidebar::src::0

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::storage::BLOCK_READ_TABLE;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Region;
use holon_api::Value;

const ROOT_ID: &str = "block:root-layout";
const DOC_ID: &str = "sentinel:no_parent";

pub async fn seed_default_layout(engine: &Arc<BackendEngine>) -> anyhow::Result<()> {
    let db = engine.db_handle();

    // Idempotent — skip if root block already present.
    let existing = db
        .query(
            &format!("SELECT id FROM {BLOCK_READ_TABLE} WHERE id = '{ROOT_ID}'"), /* ALLOW(sql):
                                                                                   * idempotency
                                                                                   * check during
                                                                                   * seed */
            Default::default(),
        )
        .await?;
    if !existing.is_empty() {
        return Ok(());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64;

    // All INSERT OR IGNORE so re-running after a partial seed is safe.
    let stmts: &[(&str, &str, &str, &str, &str, &str, &str)] = &[
        // (id, parent_id, content, content_type, source_language, sort_key, properties_json)
        (
            DOC_ID,
            DOC_ID,
            "",
            "text",
            "",
            "a0",
            r#"{"name":"__default__"}"#,
        ),
        (
            ROOT_ID,
            DOC_ID,
            "Holon Layout",
            "text",
            "",
            "a1",
            r#"{"sequence":0,"level":1}"#,
        ),
        (
            "block:root-layout::src::0",
            ROOT_ID,
            "MATCH (root:block)<-[:CHILD_OF]-(d:block)\nWHERE root.id = 'block:root-layout' AND \
             d.content_type = 'text'\nRETURN d, d.properties.sequence AS sequence, \
             d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, \
             d.properties.column_priority AS priority\nORDER BY d.properties.sequence",
            "source",
            "holon_gql",
            "a2",
            r#"{"sequence":1}"#,
        ),
        (
            "block:holon-app-layout::render::0",
            ROOT_ID,
            r#"if_space(600.0,
  columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("content", "Main Panel", live_block(), spacer(0))}),
  if_space(1024.0,
    columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("content", "Left Sidebar", spacer(0), if_col("collapse_to", "drawer", drawer(col("id"), live_block()), live_block()))}),
    columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("collapse_to", "drawer", drawer(col("id"), live_block()), live_block())})))"#,
            "source",
            "render",
            "a3",
            r#"{"sequence":2}"#,
        ),
        (
            "block:default-left-sidebar",
            ROOT_ID,
            "Left Sidebar",
            "text",
            "",
            "a4",
            r#"{"sequence":3,"level":2,"collapse_to":"drawer"}"#,
        ),
        // Sidebar mirrors assets/default/index.org: pages are blocks tagged
        // 'Page' (see the block_tags seeding below), displayed by content.
        // The old seed filtered on a `name` column that the `block` matview
        // does not project — the generated watch_view failed to create and
        // the sidebar rendered an error banner (HANDOFF gap #1).
        (
            "block:default-left-sidebar::render::0",
            "block:default-left-sidebar",
            r#"tree(#{parent_id: col("parent_id"), sortkey: col("sort_key"), item_template: selectable(row(icon("notebook"), spacer(6), text(col("content"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})"#,
            "source",
            "render",
            "a5",
            r#"{"sequence":4}"#,
        ),
        (
            "block:default-left-sidebar::src::0",
            "block:default-left-sidebar",
            "SELECT b.* FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = \
             'Page' AND b.id != 'block:__default__'",
            "source",
            "holon_sql",
            "a6",
            r#"{"sequence":5}"#,
        ),
        (
            "block:default-main-panel",
            ROOT_ID,
            "Main Panel",
            "text",
            "",
            "a7",
            r#"{"sequence":6,"level":2}"#,
        ),
        (
            "block:default-main-panel::src::0",
            "block:default-main-panel",
            "MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = \
             'main' AND root.id = fr.root_id RETURN d",
            "source",
            "holon_gql",
            "a8",
            r#"{"sequence":7}"#,
        ),
        (
            "block:default-right-sidebar",
            ROOT_ID,
            "Right Sidebar",
            "text",
            "",
            "a9",
            r#"{"sequence":8,"level":2,"collapse_to":"drawer"}"#,
        ),
        (
            "block:default-right-sidebar::src::0",
            "block:default-right-sidebar",
            "from children\n",
            "source",
            "holon_prql",
            "b0",
            r#"{"sequence":9}"#,
        ),
        // Welcome document — gives the left sidebar at least one entry to render
        // and the main panel something to focus on. Lives under DOC_ID, not the
        // root layout subtree, so it matches the sidebar PRQL `from block | filter name != null`.
        (
            "block:welcome",
            DOC_ID,
            "Welcome",
            "text",
            "",
            "c0",
            r#"{"name":"Welcome","sequence":100,"level":1}"#,
        ),
        (
            "block:welcome::para::0",
            "block:welcome",
            "Holon is now running in your browser. Click \"Welcome\" in the left sidebar to focus \
             it here.",
            "text",
            "",
            "c1",
            r#"{"sequence":101,"level":2}"#,
        ),
        // A few sibling blocks so structural interactions (drag & drop,
        // indent/outdent, split/join) have material to work with on first boot.
        (
            "block:welcome::para::1",
            "block:welcome",
            "Try dragging a block by its bullet and dropping it on another block.",
            "text",
            "",
            "c2",
            r#"{"sequence":102,"level":2}"#,
        ),
        (
            "block:welcome::para::2",
            "block:welcome",
            "Enter splits a block, Backspace at the start joins it, Tab / Shift-Tab indent and \
             outdent.",
            "text",
            "",
            "c3",
            r#"{"sequence":103,"level":2}"#,
        ),
    ];

    for (id, parent_id, content, content_type, source_language, sort_key, properties) in stmts {
        let content_escaped = content.replace('\'', "''");
        let lang_col = if source_language.is_empty() {
            "".to_string()
        } else {
            format!(", source_language")
        };
        let lang_val = if source_language.is_empty() {
            "".to_string()
        } else {
            format!(", '{source_language}'")
        };
        let sql = format!(
            // ALLOW(sql): seed INSERT for the bundled default layout
            "INSERT OR IGNORE INTO {BLOCK_WRITE_TABLE} (id, parent_id, content, \
             content_type{lang_col}, sort_key, properties, created_at, updated_at) VALUES \
             ('{id}', '{parent_id}', '{content_escaped}', '{content_type}'{lang_val}, \
             '{sort_key}', '{properties}', {now}, {now})",
        );
        // ALLOW(sole_block_writer): bootstrap seed for the bundled default
        // layout. The wasm worker can't use the org/Loro block-creation path
        // (holon-orgmode doesn't build on wasm — see this module's doc), so the
        // initial layout is written via hand-rolled SQL. Runs once on a fresh
        // in-memory DB before any BlockOperations writer exists.
        db.execute(&sql, vec![]).await?;
    }

    // Journals page: the SAME blocks the native seed builds
    // (`build_default_layout_blocks` → `journals_page_blocks`: shell + display
    // query + render), translated to the worker's hand-rolled SQL. Sharing one
    // block spec keeps the journals page identical across the browser and native
    // frontends. The `block:journals` page parents under the no-parent sentinel,
    // matching this module's DOC_ID convention for the other bundled pages.
    for (i, block) in holon_frontend::journals_page_blocks()
        .into_iter()
        .enumerate()
    {
        let content_escaped = block.content.replace('\'', "''");
        let (lang_col, lang_val) = match block.source_language.as_ref() {
            Some(lang) => (", source_language".to_string(), format!(", '{lang}'")),
            None => (String::new(), String::new()),
        };
        let content_type = block.content_type.to_string();
        let sort_key = format!("d{i}");
        let sql = format!(
            // ALLOW(sql): seed INSERT for the bundled default layout
            "INSERT OR IGNORE INTO {BLOCK_WRITE_TABLE} (id, parent_id, content, \
             content_type{lang_col}, sort_key, properties, created_at, updated_at) VALUES \
             ('{id}', '{parent_id}', '{content_escaped}', '{content_type}'{lang_val}, \
             '{sort_key}', '{{}}', {now}, {now})",
            id = block.id.as_str(),
            parent_id = block.parent_id.as_str(),
        );
        // ALLOW(sole_block_writer): bootstrap seed for the bundled default layout
        // (see the doc-comment on the tuple loop above).
        db.execute(&sql, vec![]).await?;
    }

    // Pages surface in the left sidebar via the 'Page' tag (same convention
    // the native org ingest uses); the sidebar query joins block_tags.
    for page_id in ["block:welcome", "block:journals"] {
        db.execute(
            &format!(
                // ALLOW(sql): seed INSERT for the bundled default layout
                "INSERT OR IGNORE INTO block_tags (block_id, tag) VALUES ('{page_id}', 'Page')"
            ),
            vec![],
        )
        .await?;
    }

    // FU-10 browser parity: land first-launch users on `block:welcome`. Going
    // through `navigation::focus` (rather than raw INSERT into navigation_history)
    // keeps navigation_history and navigation_cursor atomically in sync, so the
    // focus_roots / current_focus matviews resolve correctly on first render.
    // Reached only on the fresh-DB path (after the early return above), so
    // existing DBs preserve whatever the user last navigated to.
    let mut nav_params: holon_api::StorageEntity = HashMap::new();
    nav_params.insert("region".into(), Value::from(Region::Main));
    nav_params.insert(
        "block_id".into(),
        Value::String(EntityUri::block("welcome").as_str().to_string()),
    );
    engine
        .execute_operation(&EntityName::from("navigation"), "focus", nav_params)
        .await?;

    tracing::info!(
        "[seed] seeded {} default layout blocks; main panel focused on block:welcome",
        stmts.len()
    );
    Ok(())
}
