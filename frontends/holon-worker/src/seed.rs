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

use holon::api::backend_engine::BackendEngine;
use std::sync::Arc;

const ROOT_ID: &str = "block:root-layout";
const DOC_ID: &str = "sentinel:no_parent";

pub async fn seed_default_layout(engine: &Arc<BackendEngine>) -> anyhow::Result<()> {
    let db = engine.db_handle();

    // Idempotent — skip if root block already present.
    let existing = db
        .query(
            &format!("SELECT id FROM block WHERE id = '{ROOT_ID}'"),
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
            DOC_ID, DOC_ID, "", "text", "", "a0",
            r#"{"name":"__default__"}"#,
        ),
        (
            ROOT_ID, DOC_ID, "Holon Layout", "text", "", "a1",
            r#"{"sequence":0,"level":1}"#,
        ),
        (
            "block:root-layout::src::0",
            ROOT_ID,
            "MATCH (root:block)<-[:CHILD_OF]-(d:block)\nWHERE root.id = 'block:root-layout' AND d.content_type = 'text'\nRETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority\nORDER BY d.properties.sequence",
            "source", "holon_gql", "a2",
            r#"{"sequence":1}"#,
        ),
        (
            "block:holon-app-layout::render::0",
            ROOT_ID,
            r#"if_space(600.0,
  columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("content", "Main Panel", block_ref(), spacer(0))}),
  if_space(1024.0,
    columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("content", "Left Sidebar", spacer(0), if_col("collapse_to", "drawer", drawer(col("id"), block_ref()), block_ref()))}),
    columns(#{gap: 4, sort_key: col("sequence"), item_template: if_col("collapse_to", "drawer", drawer(col("id"), block_ref()), block_ref())})))"#,
            "source", "render", "a3",
            r#"{"sequence":2}"#,
        ),
        (
            "block:default-left-sidebar",
            ROOT_ID,
            "Left Sidebar", "text", "", "a4",
            r#"{"sequence":3,"level":2,"collapse_to":"drawer"}"#,
        ),
        (
            "block:default-left-sidebar::render::0",
            "block:default-left-sidebar",
            r#"list(#{sortkey: "name", item_template: selectable(row(icon("notebook"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})"#,
            "source", "render", "a5",
            r#"{"sequence":4}"#,
        ),
        (
            "block:default-left-sidebar::src::0",
            "block:default-left-sidebar",
            "from block\nfilter name != null\nfilter name != \"\" && name != \"index\" && name != \"__default__\"",
            "source", "holon_prql", "a6",
            r#"{"sequence":5}"#,
        ),
        (
            "block:default-main-panel",
            ROOT_ID,
            "Main Panel", "text", "", "a7",
            r#"{"sequence":6,"level":2}"#,
        ),
        (
            "block:default-main-panel::src::0",
            "block:default-main-panel",
            "MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = 'main' AND root.id = fr.root_id RETURN d",
            "source", "holon_gql", "a8",
            r#"{"sequence":7}"#,
        ),
        (
            "block:default-right-sidebar",
            ROOT_ID,
            "Right Sidebar", "text", "", "a9",
            r#"{"sequence":8,"level":2,"collapse_to":"drawer"}"#,
        ),
        (
            "block:default-right-sidebar::src::0",
            "block:default-right-sidebar",
            "from children\n",
            "source", "holon_prql", "b0",
            r#"{"sequence":9}"#,
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
            "INSERT OR IGNORE INTO block \
             (id, parent_id, content, content_type{lang_col}, sort_key, properties, created_at, updated_at) \
             VALUES ('{id}', '{parent_id}', '{content_escaped}', '{content_type}'{lang_val}, \
                     '{sort_key}', '{properties}', {now}, {now})",
        );
        db.execute(&sql, vec![]).await?;
    }

    tracing::info!("[seed] seeded {} default layout blocks", stmts.len());
    Ok(())
}
