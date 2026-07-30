//! Increment 3a SUT-side seeding: graft the **fixed shared** `parent`/`c1`/`c2`
//! tree into a live window's backend so the windowed `inv-displayed-text`
//! oracle has known blocks to compare against (approach **B1** — graft under
//! the Main panel's rendered focus root).
//!
//! The Main panel renders the descendants of `focus_roots.root_id` (region
//! `main`). Attaching `parent` directly under that root makes
//! `parent`/`c1`/`c2` render as text widgets, and because the ids are the same
//! fixed ids the ref
//! seeds ([`crate::pbt::composed::subsystem_seed::seed_ref_tree`] /
//! [`super::builders::window_ref_caps_seeded`]), the rendered widgets resolve
//! to ref-known blocks and the text comparison bites. The vault's pre-existing
//! random-UUID blocks stay unknown to the ref and are skipped.
//!
//! `inv-displayed-text` compares **content by id only** — the grafted parent
//! need not match the ref's notion of *where* `parent` lives (the ref seeds it
//! under `no_parent`); only the per-id content has to agree. So this graft is
//! decoupled from the ref's tree shape.

use anyhow::Context;
use anyhow::Result;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::Value;

use crate::pbt::composed::seed_primitives::C1;
use crate::pbt::composed::seed_primitives::C2;
use crate::pbt::composed::seed_primitives::PARENT;
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::test_environment::TestEnvironment;

/// Query the `root_id` the Main panel renders descendants of. Fail-loud: the
/// windowed slice boots a Turso session via `start_app`, so `focus_roots` must
/// exist and carry a `main` row once the window has settled.
async fn main_focus_root(env: &TestEnvironment) -> Result<String> {
    let rows = env
        .engine()
        .execute_query(
            "SELECT root_id FROM focus_roots WHERE region = 'main'".to_string(),
            std::collections::HashMap::new(),
            None,
        )
        .await
        .context("query focus_roots for the Main render root")?;
    let row = rows
        .first()
        .context("focus_roots has no 'main' row — window not settled / no focus")?;
    match row.get("root_id") {
        Some(Value::String(s)) => Ok(s.clone()),
        other => anyhow::bail!("focus_roots.root_id is not a string: {other:?}"),
    }
}

/// Graft the fixed `parent`/`c1`/`c2` tree under the Main focus root of a live,
/// settled window. Mirrors
/// [`seed_ref_tree`](crate::pbt::composed::subsystem_seed::seed_ref_tree)'s
/// content (`PARENT`/`C1`/`C2`) and ids ([`fixed_ids`]) so the ref↔SUT id
/// mapping is the identity. The caller must re-settle the window afterwards so
/// the new blocks paint before the invariants read geometry/VM.
pub async fn graft_displayed_text_tree(env: &TestEnvironment) -> Result<()> {
    let root = main_focus_root(env).await?;
    let ids = fixed_ids();
    env.create_block(ids.parent.as_str(), &root, PARENT)
        .await
        .context("graft parent under Main focus root")?;
    env.create_block(ids.c1.as_str(), ids.parent.as_str(), C1)
        .await
        .context("graft c1 under parent")?;
    env.create_block(ids.c2.as_str(), ids.parent.as_str(), C2)
        .await
        .context("graft c2 under parent")?;
    Ok(())
}

/// Block id of the ClaudeCode-shaped query headline grafted by
/// [`graft_nested_query_block`].
pub const NESTED_QUERY_HEAD_ID: &str = "nq-head";
/// Parent of the three rows the grafted query selects.
pub const NESTED_QUERY_DATA_ID: &str = "nq-data";
/// Content prefix of the rows the grafted query returns. The data blocks are
/// SOURCE-typed, so the outline filters them out of its own rows — anything on
/// screen carrying this prefix was painted by the nested widget and nothing
/// else.
pub const NESTED_QUERY_ROW_MARKER: &str = "QROW-";
/// Number of rows the grafted query returns.
pub const NESTED_QUERY_ROW_COUNT: usize = 3;

/// Graft a ClaudeCode-shaped query headline under the Main focus root: a plain
/// text headline owning a `holon_sql` source child and a `render` child, so the
/// block profile resolves it to the `query_block_titled` variant
/// (`column(row(headline…), live_block(), drop_zone())`) — i.e. its widget
/// renders through a NESTED `ReactiveShell`, embedded as one row of the main
/// outline rather than parented by a panel.
///
/// The query selects the three `nq-row-*` blocks by exact `parent_id`
/// (equality, not `LIKE`, so the watcher's matview stays inside the supported
/// IVM subset), so counting [`NESTED_QUERY_ROW_MARKER`] on screen counts widget
/// rows and nothing else.
///
/// The caller must re-settle the window afterwards.
pub async fn graft_nested_query_block(env: &TestEnvironment) -> Result<()> {
    let root = main_focus_root(env).await?;

    env.create_block(NESTED_QUERY_DATA_ID, &root, "NQ Data")
        .await
        .context("graft the query's data parent under Main focus root")?;
    for i in 1..=NESTED_QUERY_ROW_COUNT {
        // Source-typed so the outline skips them: the ONLY place their content
        // can appear on screen is inside the nested widget under test. The
        // language is deliberately neither a query nor a rule language, so these
        // rows do not turn `nq-data` into a query owner of its own.
        env.create_source_block(
            &format!("nq-row-{i}"),
            NESTED_QUERY_DATA_ID,
            SourceLanguage::Other("qdata".to_string()),
            &format!("{NESTED_QUERY_ROW_MARKER}{i}"),
        )
        .await
        .with_context(|| format!("graft query data row {i}"))?;
    }

    env.create_block(NESTED_QUERY_HEAD_ID, &root, "Nested Query Head")
        .await
        .context("graft the query headline under Main focus root")?;
    env.create_source_block(
        &format!("{NESTED_QUERY_HEAD_ID}::src::0"),
        NESTED_QUERY_HEAD_ID,
        SourceLanguage::Query(QueryLanguage::HolonSql),
        "SELECT id, content FROM block_raw WHERE parent_id = 'block:nq-data' ORDER BY content",
    )
    .await
    .context("graft the headline's holon_sql source child")?;
    env.create_source_block(
        &format!("{NESTED_QUERY_HEAD_ID}::render::0"),
        NESTED_QUERY_HEAD_ID,
        SourceLanguage::Render,
        r#"list(#{sortkey: "content", item_template: rendered_text(col("content"))})"#,
    )
    .await
    .context("graft the headline's render child")?;
    Ok(())
}
