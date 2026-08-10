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

// ── Band-geometry scenario (bug #69) ────────────────────────────────────────

/// Content prefix of the plain outline rows preceding the nested query band.
pub const BAND_PRE_MARKER: &str = "BANDPRE-";
/// Content prefix of the rows the band's query returns.
pub const BAND_ROW_MARKER: &str = "BANDROW-";
/// Content of the plain outline row that immediately FOLLOWS the band. The
/// overlap invariant reads this one row's top edge against the band's painted
/// bottom edge.
pub const BAND_SIBLING_CONTENT: &str = "BANDSIB-1";
/// Content prefix of the plain outline rows after the sibling, present only to
/// push the page's content past the viewport so scrolling is a real operation.
pub const BAND_POST_MARKER: &str = "BANDPOST-";
/// Content of the LAST row of the page — the reachability invariant's target.
pub const BAND_LAST_CONTENT: &str = "BANDLAST";
/// Number of rows the band's query returns. Large enough that the band is
/// several times taller than the ~4 rows the defective build reserves for it.
pub const BAND_ROW_COUNT: usize = 18;
/// Filler count for the OVERLAP scenario: the band and its immediate sibling
/// are all that invariant reads, so keep the vault small and the settle fast.
pub const BAND_POST_COUNT_OVERLAP: usize = 6;
/// Filler count for the REACHABILITY scenario: enough rows that the page's
/// content is several viewports tall and scrolling is a real operation.
pub const BAND_POST_COUNT_SCROLL: usize = 60;

/// Graft the bug-#69 page shape under the Main focus root, in outline order:
///
/// ```text
///   BANDPRE-1, BANDPRE-2          plain rows above the section
///   Band Section                  a plain headline — the band's PARENT
///     Band Data                   the query's data parent (its rows are
///                                 source-typed, so the outline hides them)
///     Band Query Head             the NESTED query band — paints BAND_ROW_COUNT rows
///     BANDSIB-1                   the sibling that must start BELOW the band
///   BANDPOST-1 … BANDPOST-N       filler that pushes content past the viewport
///   BANDLAST                      the last row, reachable only by scrolling
/// ```
///
/// The band sits at outline DEPTH 2, under a section headline, because that is
/// the shape Martin's ClaudeCode page has and the depth at which he reproduced
/// the overlap (depth 2 and 3). The band and the sibling it must not overlap
/// are siblings under the same section, exactly as a real page's query block
/// and the note beneath it are.
///
/// Creation order is outline order: `create_block` appends. `post_count` sizes
/// the filler tail — see [`BAND_POST_COUNT_OVERLAP`] /
/// [`BAND_POST_COUNT_SCROLL`].
///
/// The caller must re-settle the window afterwards.
pub async fn graft_band_geometry_page(env: &TestEnvironment, post_count: usize) -> Result<()> {
    let root = main_focus_root(env).await?;

    for i in 1..=2 {
        env.create_block(
            &format!("band-pre-{i}"),
            &root,
            &format!("{BAND_PRE_MARKER}{i}"),
        )
        .await
        .with_context(|| format!("graft plain row above the band ({i})"))?;
    }

    // The section headline that puts the band at DEPTH 2 — the depth at which
    // Martin reproduced the overlap. Its children below are depth-2 rows.
    env.create_block("band-section", &root, "Band Section")
        .await
        .context("graft the section headline that parents the band")?;

    env.create_block("band-data", "band-section", "Band Data")
        .await
        .context("graft the band query's data parent")?;
    for i in 1..=BAND_ROW_COUNT {
        // Source-typed so the outline skips them: their content can only reach
        // the screen through the nested band under test.
        env.create_source_block(
            &format!("band-row-{i:02}"),
            "band-data",
            SourceLanguage::Other("qdata".to_string()),
            &format!("{BAND_ROW_MARKER}{i:02}"),
        )
        .await
        .with_context(|| format!("graft band data row {i}"))?;
    }

    env.create_block("band-head", "band-section", "Band Query Head")
        .await
        .context("graft the band headline")?;
    env.create_source_block(
        "band-head::src::0",
        "band-head",
        SourceLanguage::Query(QueryLanguage::HolonSql),
        "SELECT id, content FROM block_raw WHERE parent_id = 'block:band-data' ORDER BY content",
    )
    .await
    .context("graft the band headline's holon_sql source child")?;
    env.create_source_block(
        "band-head::render::0",
        "band-head",
        SourceLanguage::Render,
        r#"list(#{sortkey: "content", item_template: rendered_text(col("content"))})"#,
    )
    .await
    .context("graft the band headline's render child")?;

    // Sibling of the band under the SAME section, so the seam this test judges
    // is the one a real page has between a query block and the note under it.
    env.create_block("band-sib-1", "band-section", BAND_SIBLING_CONTENT)
        .await
        .context("graft the sibling row directly below the band")?;

    for i in 1..=post_count {
        env.create_block(
            &format!("band-post-{i:03}"),
            &root,
            &format!("{BAND_POST_MARKER}{i:03}"),
        )
        .await
        .with_context(|| format!("graft filler row {i}"))?;
    }

    env.create_block("band-last", &root, BAND_LAST_CONTENT)
        .await
        .context("graft the last row of the page")?;
    Ok(())
}

/// Block id [`graft_chord_target_row`] grafts.
pub const CHORD_TARGET_ID: &str = "chord-target";
/// Content of the grafted chord target row.
pub const CHORD_TARGET_CONTENT: &str = "Chord target row";

/// Graft a single plain row under the Main focus root: a caret target for
/// tests that press a chord into a real editor. Deliberately childless so a
/// structural op's effect on the block count is unambiguous.
pub async fn graft_chord_target_row(env: &TestEnvironment) -> Result<()> {
    let root = main_focus_root(env).await?;
    env.create_block(CHORD_TARGET_ID, &root, CHORD_TARGET_CONTENT)
        .await
        .context("graft the chord target row under Main focus root")?;
    Ok(())
}

/// Block id [`graft_promotion_target_row`] grafts.
pub const PROMOTION_TARGET_ID: &str = "promotion-target";
/// Content of the grafted promotion target row. Non-empty so the row paints a
/// text element the driver can aim a real click at, and NOT keyword-headed so
/// prepending one is a genuine promotion rather than a re-commit.
pub const PROMOTION_TARGET_CONTENT: &str = "milk";

/// Graft a single plain, task-less row under the Main focus root: the target
/// for typing a task keyword at the head of a real editor.
pub async fn graft_promotion_target_row(env: &TestEnvironment) -> Result<()> {
    let root = main_focus_root(env).await?;
    env.create_block(PROMOTION_TARGET_ID, &root, PROMOTION_TARGET_CONTENT)
        .await
        .context("graft the promotion target row under Main focus root")?;
    Ok(())
}

/// Content of the row [`graft_undo_blur_pair`] edits. Two words, so an appended
/// suffix is a plain content edit with no keyword, no trigger and no structural
/// meaning.
pub const UNDO_EDIT_CONTENT: &str = "alpha two";
/// Content of the blur target. Distinct from [`UNDO_EDIT_CONTENT`] so a
/// mis-aimed click lands on visibly different text.
pub const UNDO_BLUR_SIBLING_CONTENT: &str = "beta three";

/// Graft the two sibling rows the undo/blur rung needs: one to edit and undo,
/// one to click afterwards so focus genuinely leaves the first. Returns
/// `(edit_id, blur_sibling_id)`.
///
/// `suffix` keys the ids per storage-mode arm. The re-seed gesture's staleness
/// signal (`local_edit_epoch`) is a PROCESS-GLOBAL map keyed by row id, so two
/// arms sharing a row id would sample each other's edits.
pub async fn graft_undo_blur_pair(env: &TestEnvironment, suffix: &str) -> Result<(String, String)> {
    let root = main_focus_root(env).await?;
    let edit_id = format!("undo-edit-target-{suffix}");
    let blur_id = format!("undo-blur-sibling-{suffix}");
    env.create_block(&edit_id, &root, UNDO_EDIT_CONTENT)
        .await
        .context("graft the undo edit target under Main focus root")?;
    env.create_block(&blur_id, &root, UNDO_BLUR_SIBLING_CONTENT)
        .await
        .context("graft the blur sibling under Main focus root")?;
    Ok((edit_id, blur_id))
}
