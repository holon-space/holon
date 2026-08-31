//! A block can be moved TO ROOT, and root's contract at the write chokepoint.
//!
//! Root is spelled `sentinel:no_parent` everywhere a block is READ. The WRITE
//! path could not reach the same place: `move_block` resolved its destination
//! with `get_by_id`, which reads the `block` matview, which excludes the
//! sentinel FK-anchor row on purpose — so the one destination the read path
//! names was the one destination the write path could not look up, and the move
//! failed "Parent not found".
//!
//! The fix makes the sentinel a legal EXPLICIT destination: its existence read
//! is skipped (it is the anchor row, it always exists), and its page-ness is
//! supplied directly as `false`. Both halves are pinned here — the skip must be
//! keyed on that exact id, and the `false` must still reach the
//! no-pages-under-non-pages guard.
//!
//! The same chokepoint's OTHER contract is pinned here too: the
//! no-pages-under-non-pages guard has to receive a page-ness that was actually
//! read. `get_by_id` decodes through the derived `TryFromEntity`, which
//! defaults every `#[edge_field]` to empty, so a `Page` in the store reads back
//! as a non-page — a guard fed from there never fires. Both of its inputs come
//! from `is_page_authoritative` instead.
//!
//! @pbt kind harness
//! @pbt covers move-block-to-root — a leaf reaches root through the production
//! read path; an unknown destination is still refused; a page is still refused
//! @pbt covers move-block-page-guard — a page is refused under a non-page from
//! either side of the tree, root-level and nested alike

use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::BlockOperations;
use holon_core::BlockQueryHelpers;
use holon_core::DataSource;
use holon_core::OperationProvider;
use holon_core::traits::BlockMovePrefetched;
use holon_core::traits::MovePrefetch;
use holon_loro::block_to_params;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::runtime::Runtime;

async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
    use holon_turso::schema_modules::CoreSchemaModule;
    use holon_turso::schema_modules::LinkSchemaModule;

    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");
    CoreSchemaModule
        .ensure_schema(handle)
        .await
        .expect("CoreSchemaModule");
    BlockSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockSchemaModule");
    BlockMatviewSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockMatviewSchemaModule");
    LinkSchemaModule
        .ensure_schema(handle)
        .await
        .expect("LinkSchemaModule");
}

/// The production block wiring: the CRUD authority plus the structural provider
/// that owns `move_block`.
async fn wiring(
    handle: &holon::storage::turso::DbHandle,
) -> (Arc<SqlOperationProvider>, SqlBlockOperations) {
    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    ));
    let cache: Arc<QueryableCache<Block>> = Arc::new(
        QueryableCache::<Block>::new(handle.clone(), Block::type_definition())
            .await
            .expect("cache"),
    );
    let ops = SqlBlockOperations::new(provider.clone(), cache);
    (provider, ops)
}

async fn create(provider: &SqlOperationProvider, block: Block) {
    let entity: EntityName = "block".to_string().into();
    let params = block_to_params(&holon::api::SnapshotBlock {
        block: block.clone(),
        sort_key: "A0".to_string(),
    });
    provider
        .execute_operation(&entity, "create", params)
        .await
        .unwrap_or_else(|e| panic!("create {}: {e}", block.id));
}

#[test]
fn a_leaf_moves_to_root_through_the_production_read_path() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso init");
        setup_production_schema(&handle).await;
        let (provider, ops) = wiring(&handle).await;

        let page_id = EntityUri::block("page-holding-the-leaf");
        let leaf_id = EntityUri::block("leaf-that-leaves");

        let mut page = Block::new_text(page_id.clone(), EntityUri::no_parent(), "Page");
        page.set_page(true);
        create(&provider, page).await;
        create(
            &provider,
            Block::new_text(leaf_id.clone(), page_id.clone(), "leaf"),
        )
        .await;

        // The leaf starts inside the page.
        let before: Option<Block> = ops.get_by_id(leaf_id.as_str()).await.expect("read leaf");
        assert_eq!(
            before.expect("leaf exists before the move").parent_id,
            page_id,
            "fixture: the leaf must start under the page"
        );

        // The skip is keyed on the root sentinel alone. Every OTHER destination
        // still has to exist, or a move that cannot be honoured would land
        // silently as an orphan.
        let unknown = EntityUri::block("no-such-destination");
        let refused = ops
            .move_block(&leaf_id, &unknown, None)
            .await
            .expect_err("a move to a destination that does not exist must be refused");
        let msg = format!("{refused:#}");
        assert!(
            msg.contains("Parent not found"),
            "the refusal must still name the missing parent, got: {msg}"
        );

        // The move under test: destination is root, named the way every reader
        // names it.
        ops.move_block(&leaf_id, &EntityUri::no_parent(), None)
            .await
            .expect(
                "moving a leaf to root must succeed — root is a destination the read path names, \
                 so the write path has to be able to reach it",
            );

        let after: Block = ops
            .get_by_id(leaf_id.as_str())
            .await
            .expect("read leaf after the move")
            .expect("the leaf must still be readable after moving to root");
        assert!(
            after.parent_id.is_no_parent(),
            "after moving to root the leaf's parent must read as root, got {}",
            after.parent_id
        );

        // The page it left must not still claim it.
        let page_children = ops.children_ordered(&page_id).await.expect("page children");
        assert!(
            page_children.iter().all(|b| b.id != leaf_id),
            "the page must no longer hold the leaf it lost"
        );

        // And back: root is an ORIGIN as well as a destination. Every move to
        // root records an inverse that starts here, so a chokepoint that cannot
        // read a sentinel parent makes the move a one-way door — which is how
        // undoing `rehome_entity` failed with "Cannot move root block".
        ops.move_block(&leaf_id, &page_id, None).await.expect(
            "moving a leaf back off root must succeed — a block AT the root is not the root, and \
             its inverse move has to be executable",
        );
        let back: Block = ops
            .get_by_id(leaf_id.as_str())
            .await
            .expect("read leaf after moving back")
            .expect("the leaf must still be readable after moving back off root");
        assert_eq!(
            back.parent_id, page_id,
            "after moving back off root the leaf must sit under the page again"
        );
    });
}

/// Root is not a page, and a page may only nest under a page — so a PAGE cannot
/// be moved to root.
///
/// This is what pins the `false` the sentinel arm hands the guard. Nothing else
/// does: with that arm flipped to `true` the rest of the suite still passes,
/// and the keystone only catches it on a draw that happens to move a page to
/// root. The guard itself is never skipped for the sentinel — only the
/// destination LOOKUP is — and that distinction is observable only here.
#[test]
fn a_page_cannot_be_moved_to_root_because_root_is_not_a_page() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso init");
        setup_production_schema(&handle).await;
        let (provider, ops) = wiring(&handle).await;

        let host_id = EntityUri::block("host-page");
        let nested_id = EntityUri::block("nested-page");

        let mut host = Block::new_text(host_id.clone(), EntityUri::no_parent(), "Host");
        host.set_page(true);
        create(&provider, host).await;
        // A page under a page — the legal nesting, so the fixture itself is not
        // what the move gets refused for.
        let mut nested = Block::new_text(nested_id.clone(), host_id.clone(), "Nested");
        nested.set_page(true);
        create(&provider, nested).await;

        // NON-VACUITY, asserted against the STORE: the block under test really
        // is a Page there. Measured rather than assumed, because the decoder
        // used below does not carry it.
        let stored_tags = handle
            .query(
                "SELECT tags FROM block WHERE id = 'block:nested-page'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("read the stored tags")
            .first()
            .and_then(|r| r.get("tags"))
            .and_then(|v| v.as_string())
            .unwrap_or_default()
            .to_string();
        assert!(
            stored_tags.contains("Page"),
            "fixture is vacuous: the block under test must be stored as a Page, got {stored_tags:?}"
        );

        // `moved_is_page` comes through the prefetch, the way production supplies
        // it, because the alternative pins nothing: `get_by_id` decodes through
        // the DERIVED `TryFromEntity`, which defaults every `#[edge_field]` to
        // empty, so `block.is_page()` read that way is ALWAYS false even though
        // the matview holds `["Page"]` (asserted just above). That gap is a
        // separate defect, reported with this change.
        //
        // `new_parent` is deliberately LEFT UNSET so the destination arm under
        // test is the one that answers — supplying it would bypass exactly what
        // this test exists to pin.
        let prefetch = MovePrefetch {
            block: Some((host_id.clone(), true)),
            ..MovePrefetch::default()
        };
        let refused = ops
            .move_block_prefetched(&nested_id, &EntityUri::no_parent(), None, prefetch)
            .await
            .expect_err("moving a PAGE to root must be refused — root is not a page");
        let msg = format!("{refused:#}");
        assert!(
            msg.contains("refusing to reparent page block")
                && msg.contains("pages under non-pages are prohibited"),
            "the refusal must be the no-pages-under-non-pages guard, got: {msg}"
        );

        // And the move really did not happen.
        let nested_after: Block = ops
            .get_by_id(nested_id.as_str())
            .await
            .expect("read nested after")
            .expect("the nested page must still exist");
        assert_eq!(
            nested_after.parent_id, host_id,
            "a refused move must leave the page where it was"
        );
    });
}

/// A page may only nest under a page — pinned for a page starting AT THE ROOT.
///
/// This placement was masked before `move_block` could read a sentinel parent:
/// the old "Cannot move root block" bail fired first, so the guard was never
/// reached and the hole underneath it never showed.
#[test]
fn a_root_level_page_is_refused_under_a_non_page() {
    page_under_non_page_is_refused(Start::AtRoot);
}

/// The same rule for a page that starts NESTED under another page.
///
/// Nothing masked this one — it was simply never refused, because the guard's
/// `moved_is_page` came from a decode that always answers false.
#[test]
fn a_nested_page_is_refused_under_a_non_page() {
    page_under_non_page_is_refused(Start::Nested);
}

/// Where the page under test sits before the move.
#[derive(Clone, Copy)]
enum Start {
    AtRoot,
    Nested,
}

/// Move a stored `Page` under a stored non-page through the production wiring
/// and require the no-pages-under-non-pages guard to refuse it.
///
/// Nothing is supplied by prefetch, so the arms under test are the ones that
/// have to do the reading themselves — which is the whole point: a guard fed
/// from `get_by_id`'s derived decode sees every page as a non-page and never
/// fires. Both fixtures assert their page-ness against the stored `tags` first,
/// so a green here cannot come from a vacuous fixture.
fn page_under_non_page_is_refused(start: Start) {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso init");
        setup_production_schema(&handle).await;
        let (provider, ops) = wiring(&handle).await;

        let host_id = EntityUri::block("guard-host-page");
        let plain_id = EntityUri::block("guard-plain-non-page");
        let moved_id = EntityUri::block("guard-moved-page");

        let mut host = Block::new_text(host_id.clone(), EntityUri::no_parent(), "Host");
        host.set_page(true);
        create(&provider, host).await;
        // The destination: a stored NON-page, so the refusal under test is the
        // guard's and not a missing-parent one.
        create(
            &provider,
            Block::new_text(plain_id.clone(), host_id.clone(), "plain"),
        )
        .await;
        let starts_under = match start {
            Start::AtRoot => EntityUri::no_parent(),
            Start::Nested => host_id.clone(),
        };
        let mut moved = Block::new_text(moved_id.clone(), starts_under.clone(), "Moved");
        moved.set_page(true);
        create(&provider, moved).await;

        assert!(
            stored_tags(&handle, &moved_id).await.contains("Page"),
            "fixture is vacuous: the block under test must be STORED as a Page"
        );
        assert!(
            !stored_tags(&handle, &plain_id).await.contains("Page"),
            "fixture is vacuous: the destination must be STORED as a non-page"
        );

        let refused = match ops.move_block(&moved_id, &plain_id, None).await {
            Err(e) => e,
            Ok(_) => panic!(
                "moving a page under a NON-page was ACCEPTED — the prohibited topology landed in \
                 the store"
            ),
        };
        let msg = format!("{refused:#}");
        assert!(
            msg.contains("refusing to reparent page block")
                && msg.contains("pages under non-pages are prohibited"),
            "the refusal must be the no-pages-under-non-pages guard, got: {msg}"
        );

        let after: Block = ops
            .get_by_id(moved_id.as_str())
            .await
            .expect("read the page after the refused move")
            .expect("the page must still exist");
        assert_eq!(
            after.parent_id, starts_under,
            "a refused move must leave the page exactly where it was"
        );
    });
}

/// The `tags` column as the STORE holds it — the write authority, not the
/// derived decode the guard must not trust.
async fn stored_tags(handle: &holon::storage::turso::DbHandle, id: &EntityUri) -> String {
    handle
        .query(
            &format!("SELECT tags FROM block WHERE id = '{}'", id.as_str()),
            std::collections::HashMap::new(),
        )
        .await
        .expect("read the stored tags")
        .first()
        .and_then(|r| r.get("tags"))
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string()
}
