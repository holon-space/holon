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
use holon_api::EntityName;
use holon_api::EntityUri;
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
    default_org_exists: bool,
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

    // Copy-on-write: the default layout (`block:__default__`) is a VIRTUAL
    // seed doc. It re-seeds from the bundled asset UNLESS the user has
    // materialized it — the durable marker for which is a `__default__.org`
    // file in the vault. When that file exists it WINS (the org scan owns the
    // doc), so the default layout is neither seeded nor replaced here.
    let reseed_default = fresh && !default_org_exists;
    if fresh && default_org_exists {
        tracing::info!(
            "[holon-app] __default__.org present on disk — the materialized (user-owned) default              layout wins; skipping default-layout seed/re-seed (copy-on-write)."
        );
    }

    // Build the seed entries from the bundled Org assets. `reseed_default`
    // gates the `__default__` page + parsed `index.org` layout; the journals
    // machinery is seeded regardless (see `build_default_layout_blocks`).
    let mut entries = FrontendSession::<()>::build_default_layout_blocks(reseed_default)?;

    // Parse the bundled index.org layout (root-layout + sidebars + sources).
    // Top-level blocks reparent from the file doc to `__default__`.
    if reseed_default {
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

    // The org document each seed block belongs to = its nearest `Page`-tagged
    // ancestor within the seed set. Resolved ONCE up front so the SqlOnly
    // routing hint (`build_block_params`'s `document_uri`) agrees with the
    // runtime writeback resolver, which walks the same parent chain
    // (`holon_orgmode::di::resolve_doc_for_block`). Fails loud on a block whose
    // chain has no `Page` root — an unroutable seed block (dogfood #5).
    let by_id: HashMap<&EntityUri, &holon_api::block::Block> =
        entries.iter().map(|b| (&b.id, b)).collect();
    let routing_docs: HashMap<EntityUri, EntityUri> = entries
        .iter()
        .map(|b| Ok((b.id.clone(), seed_routing_doc(b, &by_id)?)))
        .collect::<Result<_>>()?;

    for block in &entries {
        let persisted = ordering
            .create_in_tree(
                &block.parent_id,
                None,
                &block.id,
                block.to_block_content(),
                &block.properties,
                &holon_api::BlockEdges::of(block),
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
                let doc_uri = &routing_docs[&block.id];
                let params =
                    holon_orgmode::build_block_params(block, &block.parent_id, doc_uri, None);
                // First-boot seeding is system-authored boundary ingest —
                // never user-undoable.
                engine
                    .execute_operation(
                        &EntityName::from("block"),
                        "create",
                        params,
                        holon_api::OpOrigin::Ingest,
                    )
                    .await?;
            }
        }
    }

    // Copy-on-write auto-update: the default layout (`block:__default__`) is a
    // VIRTUAL seed doc, but the persisted Loro snapshot pins whatever content
    // it was first seeded with — `create_in_tree` above is idempotent and does
    // NOT rewrite an existing block's content. Refresh every default-layout
    // block whose persisted content DRIFTED from the current bundled asset, so
    // a shipped-asset change (e.g. a new/edited layout query) reaches an
    // unmodified vault on the next boot. Churn-free: `reseed_content` compares
    // against the authority and writes only on a real diff. Skipped when a
    // `__default__.org` file wins (`reseed_default` is false).
    if reseed_default {
        let refresh: Vec<(EntityUri, String)> = entries
            .iter()
            .filter(|b| routing_docs.get(&b.id) == Some(&default_doc_uri))
            .map(|b| (b.id.clone(), b.content.clone()))
            .collect();
        let refreshed = ordering
            .reseed_content(&refresh)
            .await
            .map_err(|e| anyhow::anyhow!("seed reseed_content: {e:#}"))?;
        if refreshed > 0 {
            tracing::info!(
                "[holon-app] copy-on-write: refreshed {refreshed} default-layout block(s) from                  the current bundled asset (stale Loro snapshot healed)"
            );
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
            .execute_operation(
                &EntityName::from("navigation"),
                "focus",
                nav_params,
                holon_api::OpOrigin::Ingest,
            )
            .await?;
        tracing::info!(
            "[holon-app] Seeded default layout via intents ({} entries); main panel focused on \
             block:journals",
            entries.len()
        );
    }
    Ok(())
}

/// The org document a seed block belongs to: its nearest `Page`-tagged ancestor
/// within the seed set (the block itself when it is a `Page`), walking
/// `parent_id`. This MIRRORS the runtime org-writeback resolver
/// (`holon_orgmode::di::resolve_doc_for_block`) so the SqlOnly seed's routing
/// hint agrees with the document a block's parent chain actually leads to.
///
/// Dogfood #5 restart-loss: the previous rule routed EVERY non-root block to
/// `block:__default__`, so `block:journals::src::0` / `::render::0` (children
/// of the `block:journals` Page) were tagged for a document their parent chain
/// never reaches — a routing-doc / parent-doc disagreement that stranded them
/// in no org file, deleting them on the next boot's file-authority ingest.
///
/// Fails loud (`Err`) on a block whose chain leaves the seed set without a
/// `Page` root: such a block cannot be routed to any file and must never be
/// seeded silently.
fn seed_routing_doc(
    block: &holon_api::block::Block,
    by_id: &HashMap<&EntityUri, &holon_api::block::Block>,
) -> Result<EntityUri> {
    let mut current = block;
    // Depth-bounded like the runtime resolver (guards a malformed cycle).
    for _ in 0..50 {
        if current.is_page() {
            return Ok(current.id.clone());
        }
        match by_id.get(&current.parent_id) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    anyhow::bail!(
        "seed block {} has no Page ancestor in the seed set (parent chain ends at {}); it cannot \
         be routed to an org document — refusing to seed an unroutable block (dogfood #5 \
         restart-loss class)",
        block.id,
        current.parent_id
    )
}

#[cfg(test)]
mod routing_tests {
    use std::collections::HashMap;

    use holon_api::EntityUri;
    use holon_api::block::Block;
    use holon_frontend::FrontendSession;

    use super::seed_routing_doc;

    /// The class-closing invariant for dogfood #5 (BugFunnel row 144): EVERY
    /// seeded non-`Page` block's routing document is its nearest `Page`
    /// ancestor in the seed set — i.e. it renders into exactly ONE org file,
    /// the file its parent chain leads to. The pre-fix rule routed
    /// `block:journals::src::0` / `::render::0` to `block:__default__` while
    /// their parent is the `block:journals` Page; this asserts the agreement.
    #[test]
    fn every_seed_block_routes_to_its_page_ancestor() {
        // `fresh` seeds the full layout (journals page + `__default__` +
        // index.org sidebars/sources) — the widest entry set.
        let entries = FrontendSession::<()>::build_default_layout_blocks(true)
            .expect("build default layout blocks");
        let by_id: HashMap<&EntityUri, &Block> = entries.iter().map(|b| (&b.id, b)).collect();

        for block in &entries {
            let doc = seed_routing_doc(block, &by_id)
                .unwrap_or_else(|e| panic!("seed block {} is unroutable: {e}", block.id));

            // The routing doc must be a Page in the seed set (or the block
            // itself when it is a Page).
            let doc_block = by_id
                .get(&doc)
                .unwrap_or_else(|| panic!("routing doc {doc} for {} not in seed set", block.id));
            assert!(
                doc_block.is_page(),
                "routing doc {doc} for {} is not a Page",
                block.id
            );

            // The routing doc must be reachable from the block via parent_id —
            // i.e. it genuinely OWNS the block (no cross-page misrouting).
            let mut cur = block;
            let mut reached = false;
            for _ in 0..50 {
                if cur.id == doc {
                    reached = true;
                    break;
                }
                match by_id.get(&cur.parent_id) {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            assert!(
                reached,
                "routing doc {doc} is not an ancestor of {} — cross-page misroute",
                block.id
            );
        }
    }

    /// The specific dogfood #5 blocks: the journals view definition routes to
    /// the `block:journals` page, NOT `block:__default__`.
    #[test]
    fn journals_view_blocks_route_to_journals_not_default() {
        let entries = FrontendSession::<()>::build_default_layout_blocks(true)
            .expect("build default layout blocks");
        let by_id: HashMap<&EntityUri, &Block> = entries.iter().map(|b| (&b.id, b)).collect();

        for id in ["block:journals::src::0", "block:journals::render::0"] {
            let uri = EntityUri::parse(id).expect("parse id");
            let block = by_id
                .get(&uri)
                .unwrap_or_else(|| panic!("{id} missing from seed set"));
            let doc = seed_routing_doc(block, &by_id).expect("route journals view block");
            assert_eq!(
                doc.as_str(),
                "block:journals",
                "{id} must route to the journals page, not {doc}"
            );
        }
    }
}
