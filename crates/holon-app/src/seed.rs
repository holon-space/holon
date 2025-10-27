//! Default-layout seeding (relocated from `holon-frontend` in storage de-leak
//! Stage 6 — it names `BackendEngine`, `DbHandle` SQL and orgmode params, all
//! wiring-crate concerns). The pure block-construction half stays in
//! `holon-frontend` as [`FrontendSession::build_default_layout_blocks`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use holon::api::BackendEngine;
use holon::storage::BLOCK_READ_TABLE;
use holon_api::{EntityName, EntityUri};
use holon_frontend::FrontendSession;

/// Seed a default layout from the bundled `index.org`.
///
/// Available on native and wasm32-wasip1-threads (which has std::time and
/// std::path). NOT available on wasm32-unknown-unknown (browser main thread)
/// where the org parser's path/time dependencies are absent.
///
/// Loro is the authority and is seeded DIRECTLY from the bundled Org assets
/// via intents (`BlockOrdering::create_in_tree`) — never from Turso. The
/// outbound projector writes `block_raw`. In SqlOnly mode (no Loro)
/// `create_in_tree` returns `false`, so each block falls back to the block
/// `OperationProvider`'s `create` (idempotent) to populate `block_raw`.
/// Document order is preserved (`create_in_tree` appends), so the layout
/// columns keep their order without a separate place pass.
#[tracing::instrument(skip(engine, ordering), name = "seed_default_layout")]
pub async fn seed_default_layout(
    engine: &BackendEngine,
    ordering: Arc<dyn holon_core::block_ordering::BlockOrdering>,
    user_index_org_exists: bool,
) -> Result<()> {
    let db = engine.db_handle();
    let default_doc_uri = FrontendSession::<()>::default_doc_uri();

    // Idempotent: the root layout existing means a prior boot already seeded.
    //
    // A user `index.org` in the vault also suppresses the default layout:
    // its root heading carries the well-known `:ID: root-layout` and the org
    // scan ingests it as THE layout. Checking only the DB raced the scan —
    // seed-before-scan booted the default layout on top of the user's
    // (leftover sidebars under the re-parented root, nondeterministic render
    // pick), scan-before-seed adopted the user layout. The explicit file
    // check makes the outcome deterministic regardless of boot interleaving.
    let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID;
    let fresh = !user_index_org_exists
        && db
            .query(
                &format!("SELECT id FROM {BLOCK_READ_TABLE} WHERE id = '{root_id}'"),
                HashMap::new(),
            )
            .await?
            .is_empty();

    // Build the seed entries from the bundled Org assets.
    let mut entries = FrontendSession::<()>::build_default_layout_blocks(fresh)?;

    // Parse the bundled index.org layout (root-layout + sidebars + sources).
    // Top-level blocks reparent from the file doc to `__default__`.
    if fresh {
        let content = include_str!("../../../assets/default/index.org");
        let parse_result = holon_orgmode::parse_org_file(
            Path::new("index.org"),
            content,
            &default_doc_uri,
            Path::new(""),
        )?;
        let file_doc_uri = parse_result.document.id.clone();
        for mut block in parse_result.blocks {
            if block.parent_id == file_doc_uri {
                block.parent_id = default_doc_uri.clone();
            }
            entries.push(block);
        }
    }

    for block in &entries {
        let persisted = ordering
            .create_in_tree(
                &block.parent_id,
                None,
                &block.id,
                block.to_block_content(),
                &block.properties,
                &block.tags,
                &block.requires,
            )
            .await
            .map_err(|e| anyhow::anyhow!("seed create_in_tree({}): {e:#}", block.id))?;
        if !persisted {
            // SqlOnly: no Loro authority — write through the block
            // OperationProvider, skipping rows that already exist.
            let exists = !db
                .query(
                    &format!(
                        "SELECT 1 FROM {BLOCK_READ_TABLE} WHERE id = '{}'",
                        block.id.as_str()
                    ),
                    HashMap::new(),
                )
                .await?
                .is_empty();
            if !exists {
                let doc_uri = if block.parent_id.is_no_parent() {
                    &block.id
                } else {
                    &default_doc_uri
                };
                let params = holon_orgmode::build_block_params(block, &block.parent_id, doc_uri);
                engine
                    .execute_operation(&EntityName::from("block"), "create", params)
                    .await?;
            }
        }
    }

    if fresh {
        // Land first-launch users on the Journals overview block. Going
        // through `navigation::focus` keeps navigation_history + cursor
        // atomically in sync so the focus matviews resolve on first render.
        let mut nav_params = holon_api::StorageEntity::new();
        nav_params.insert(
            "region".into(),
            holon_api::Value::from(holon_api::Region::Main),
        );
        nav_params.insert(
            "block_id".into(),
            holon_api::Value::String(EntityUri::block("journals").as_str().to_string()),
        );
        engine
            .execute_operation(&EntityName::from("navigation"), "focus", nav_params)
            .await?;
        tracing::info!(
            "[holon-app] Seeded default layout via intents ({} entries); \
             main panel focused on block:journals",
            entries.len()
        );
    }
    Ok(())
}
