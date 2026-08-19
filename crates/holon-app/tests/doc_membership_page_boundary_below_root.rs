//! The document membership walk must exclude a `Page` block wherever it sits,
//! including BELOW the document's direct children.
//!
//! `CacheBlockReader::get_blocks` and `doc_block_topology` share one recursive
//! CTE whose recursive arm excludes blocks tagged `Page` — the sub-document
//! boundary. The exclusion has to test the CHILD the arm is about to admit
//! (`b.id`), not the CTE row it descends from (`d.id`): a row already in
//! `descendants` can never carry the tag, so keying the check on it moves the
//! boundary down one level and admits the `Page` itself.
//!
//! Every earlier fixture put its `Page` among the document's DIRECT children,
//! where the base arm rejects it before the recursive arm is ever consulted.
//! Both the correct and the incorrect exclusion agree there, so those fixtures
//! cannot tell them apart — and a proposed rewrite keying the check on the CTE
//! row was measured safe on exactly such a fixture. This test puts the `Page`
//! at depth 1 so the recursive arm alone decides, which is the only place the
//! difference is observable.
//!
//! The keystone cannot cover this. A page under a non-page is prohibited by the
//! interim page-hierarchy ruling, and the composed generator provably never
//! produces one (pages are seed-only, always at `no_parent` — ForkB-B1 R8,
//! guarded by `inv-no-page-under-non-page`). Production still reaches the
//! topology: whole-set `set_field("tags", [.., "Page"])` bypasses the
//! author-time nesting guard, and sync/org-ingest replay MUST be able to
//! reproduce any state that was legal where it was authored (BugFunnel
//! 2026-07-17, OPEN). So the walk has to be right for a shape the generator
//! is forbidden to draw — which is why this lives here as a directed test and
//! not as a keystone transition.

use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_app::turso_seams::CacheBlockReader;
use holon_core::OperationProvider;
use holon_filesystem::BlockReader;
use holon_loro::block_to_params;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::runtime::Runtime;
use uuid::Uuid;

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
    // The writer fans `marks` into `block_links`, and the reader consults
    // `block_redirects` on a lookup miss. Both live here.
    LinkSchemaModule
        .ensure_schema(handle)
        .await
        .expect("LinkSchemaModule");
}

/// ```text
/// doc
///  ├── x1                (depth 0, plain)      -> IN
///  │    ├── nested_page  (depth 1, Page)       -> OUT, boundary
///  │    │    └── under_nested_page             -> OUT, behind the boundary
///  │    └── x2           (depth 1, plain)      -> IN
///  └── root_page         (depth 0, Page)       -> OUT, rejected by the base arm
///       └── under_root_page                    -> OUT, behind the boundary
/// ```
/// `nested_page` is the discriminating block: only the recursive arm can
/// reject it.
#[test]
fn membership_walk_excludes_a_page_nested_below_the_documents_direct_children() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso init");
        setup_production_schema(&handle).await;

        let provider = Arc::new(SqlOperationProvider::with_edge_fields(
            handle.clone(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
            BlockSchemaModule.edge_fields(),
        ));
        let entity: EntityName = "block".to_string().into();

        let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
        let x1 = EntityUri::block("x1-plain-depth0");
        let nested_page = EntityUri::block("nested-page-depth1");
        let under_nested_page = EntityUri::block("under-nested-page");
        let x2 = EntityUri::block("x2-plain-depth1");
        let root_page = EntityUri::block("root-page-depth0");
        let under_root_page = EntityUri::block("under-root-page");

        let doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "Doc");
        let b_x1 = Block::new_text(x1.clone(), doc_id.clone(), "x1");
        let mut b_nested_page = Block::new_text(nested_page.clone(), x1.clone(), "nested page");
        b_nested_page.set_page(true);
        let b_under_nested =
            Block::new_text(under_nested_page.clone(), nested_page.clone(), "hidden");
        let b_x2 = Block::new_text(x2.clone(), x1.clone(), "x2");
        let mut b_root_page = Block::new_text(root_page.clone(), doc_id.clone(), "root page");
        b_root_page.set_page(true);
        let b_under_root = Block::new_text(under_root_page.clone(), root_page.clone(), "hidden");

        // Parents before children: `block_raw` carries a parent FK.
        for blk in [
            doc,
            b_x1,
            b_nested_page,
            b_under_nested,
            b_x2,
            b_root_page,
            b_under_root,
        ] {
            let params = block_to_params(&holon::api::SnapshotBlock {
                block: blk.clone(),
                sort_key: "A0".to_string(),
            });
            provider
                .execute_operation(&entity, "create", params)
                .await
                .unwrap_or_else(|e| panic!("create {}: {e}", blk.id));
        }

        let cache: Arc<QueryableCache<Block>> = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), Block::type_definition())
                .await
                .expect("cache"),
        );
        let reader: Arc<dyn BlockReader> = Arc::new(CacheBlockReader::new(cache));

        let expected = vec![x1.to_string(), x2.to_string()];

        let mut members: Vec<String> = reader
            .get_blocks(&doc_id)
            .await
            .expect("get_blocks")
            .iter()
            .map(|b| b.id.to_string())
            .collect();
        members.sort();
        assert_eq!(
            members, expected,
            "get_blocks must stop at every `Page`, including `{nested_page}` at depth 1. Admitting \
             it means the recursive arm keys its exclusion on the CTE row it descends from instead \
             of the child it is about to admit."
        );

        // The write-back shape gate reads the same membership through a second
        // query. It has to agree block-for-block, or the gate passes on a
        // document whose contents were rendered from a different set.
        let mut shape: Vec<String> = reader
            .doc_block_topology(&doc_id)
            .await
            .expect("doc_block_topology")
            .iter()
            .map(|(id, _parent)| id.to_string())
            .collect();
        shape.sort();
        assert_eq!(
            shape, expected,
            "doc_block_topology must report the same membership as get_blocks"
        );
    });
}
