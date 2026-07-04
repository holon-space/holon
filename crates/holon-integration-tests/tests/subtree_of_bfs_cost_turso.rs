//! Option C §9.6 / §10.2.5 — measure `BlockHomeAuthority::subtree_of` against a
//! REAL Turso engine before the Inc-2 cutover.
//!
//! `subtree_of` walks descendants breadth-first, issuing one
//! `BlockOrdering::children` per visited node. In the SqlOnly wiring each of
//! those is a `SELECT … WHERE parent_id = ?` against `block_raw` — the WORST
//! case, since the Loro-enabled wiring answers `children` from the in-memory
//! tree via `cell_registry.live_children` and never touches SQL. The costed
//! alternative is a single doc-scoped read (`BlockReader::get_blocks`, one
//! recursive CTE) filtered to the subtree in memory.
//!
//! The numbers this prints decide §10.2.5. Both alternatives are exercised on
//! the same database in the same process, and `locate_batch` — the fan-out cost
//! already deemed acceptable at Inc 1 — is measured beside them as the
//! yardstick. Wall time on an in-memory Turso is a floor, not a production
//! latency; the READ COUNT is the backend-independent quantity.
//!
//! @pbt kind harness
//! @pbt covers option-c-subtree-of-cost — Inc-2 prerequisite measurement

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Instant;

use anyhow::Result;
use async_trait::async_trait;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_setup;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::live_data::home_by::HomeAuthority;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::HomeBurstMemo;
use holon_turso::schema_modules::BlockSchemaModule;

/// Blocks in the document that owns the measured subtree. Fixed while the
/// subtree size varies, so the two costs move independently: BFS is
/// O(subtree) reads, the doc-scoped alternative is 1 read over O(document)
/// rows.
const DOC_BLOCKS: usize = 1000;

/// Counts `children` calls while delegating to the real Turso ordering, so the
/// measured read count is the production one rather than an assumption about
/// BFS shape.
struct CountingOrdering {
    inner: Arc<dyn BlockOrdering>,
    children_calls: Arc<AtomicU64>,
}

#[async_trait]
impl BlockOrdering for CountingOrdering {
    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        self.children_calls.fetch_add(1, AtomicOrdering::Relaxed);
        self.inner.children(parent_id).await
    }

    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        self.inner.place(uri, parent_id, after_id).await
    }

    async fn prev_sibling(&self, id: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        self.inner.prev_sibling(id).await
    }

    async fn next_sibling(&self, id: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        self.inner.next_sibling(id).await
    }

    async fn first_child(&self, parent_id: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        self.inner.first_child(parent_id).await
    }

    async fn last_child(&self, parent_id: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        self.inner.last_child(parent_id).await
    }

    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        self.inner.update_in_tree(params).await
    }

    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        self.inner.delete_in_tree(params).await
    }
}

/// One page holding `DOC_BLOCKS` blocks. The first `subtree` of them hang
/// under `host` in a branching-factor-4 tree; the rest are flat filler
/// children of the page, so the document is realistically larger than the
/// moved subtree.
///
/// Returns `(page_uri, host_uri, subtree_ids)`.
async fn seed(
    db: &holon::storage::DbHandle,
    subtree: usize,
) -> Result<(EntityUri, EntityUri, Vec<String>)> {
    assert!(
        subtree + 2 <= DOC_BLOCKS,
        "subtree of {subtree} must fit in a {DOC_BLOCKS}-block document"
    );
    let page = EntityUri::block("meas-page");
    let host = EntityUri::block("meas-host");

    let insert = |id: String, parent: String, sort_key: String| {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id));
        params.insert("parent_id".to_string(), Value::String(parent));
        params.insert("sort_key".to_string(), Value::String(sort_key));
        params
    };

    let sql = format!(
        "INSERT INTO {BLOCK_WRITE_TABLE} (id, parent_id, sort_key, content, content_type) VALUES \
         ($id, $parent_id, $sort_key, 'x', 'text')"
    );

    db.query(
        &sql,
        insert(page.to_string(), "sentinel:no_parent".into(), "80".into()),
    )
    .await
    .map_err(|e| anyhow::anyhow!("seed page: {e}"))?;
    let mut tag = HashMap::new();
    tag.insert("id".to_string(), Value::String(page.to_string()));
    db.query(
        "INSERT INTO block_tags (block_id, tag) VALUES ($id, 'Page')",
        tag,
    )
    .await
    .map_err(|e| anyhow::anyhow!("tag page: {e}"))?;

    db.query(
        &sql,
        insert(host.to_string(), page.to_string(), "8080".into()),
    )
    .await
    .map_err(|e| anyhow::anyhow!("seed host: {e}"))?;

    // Branching factor 4 under `host`, level-order so every parent exists
    // before its children are inserted.
    let mut subtree_ids = Vec::with_capacity(subtree);
    for i in 0..subtree {
        let id = format!("block:meas-d{i}");
        let parent = if i < 4 {
            host.to_string()
        } else {
            format!("block:meas-d{}", i / 4 - 1)
        };
        db.query(&sql, insert(id.clone(), parent, format!("{:06}", i % 4)))
            .await
            .map_err(|e| anyhow::anyhow!("seed descendant {i}: {e}"))?;
        subtree_ids.push(id);
    }

    for i in 0..(DOC_BLOCKS - subtree - 2) {
        db.query(
            &sql,
            insert(
                format!("block:meas-f{i}"),
                page.to_string(),
                format!("9{i:06}"),
            ),
        )
        .await
        .map_err(|e| anyhow::anyhow!("seed filler {i}: {e}"))?;
    }

    Ok((page, host, subtree_ids))
}

async fn measure(subtree: usize) -> Result<()> {
    let engine = create_test_engine_with_setup(":memory:".into(), |_| Ok(())).await?;
    let db = engine.db_handle().clone();
    let (page, host, subtree_ids) = seed(&db, subtree).await?;

    let mut block_raw_type_def = Block::type_definition();
    block_raw_type_def.name = "block_raw".to_string();
    let cache = Arc::new(
        QueryableCache::<Block>::new(db.clone(), block_raw_type_def)
            .await
            .map_err(|e| anyhow::anyhow!("block_raw cache: {e}"))?,
    );
    let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
        db.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    ));
    let real_ordering: Arc<dyn BlockOrdering> =
        Arc::new(SqlBlockOperations::new(sql_ops, cache.clone()));
    let children_calls = Arc::new(AtomicU64::new(0));
    let ordering: Arc<dyn BlockOrdering> = Arc::new(CountingOrdering {
        inner: real_ordering,
        children_calls: children_calls.clone(),
    });
    let reader: Arc<dyn BlockReader> =
        Arc::new(holon_app::turso_seams::CacheBlockReader::new(cache.clone()));
    let authority = BlockHomeAuthority::new(reader.clone(), ordering);

    // (a) today's BFS.
    let t = Instant::now();
    let found = authority
        .subtree_of(host.as_str(), &mut HomeBurstMemo::default())
        .await?;
    let bfs_elapsed = t.elapsed();
    let bfs_children = children_calls.load(AtomicOrdering::Relaxed);
    assert_eq!(
        found.len(),
        subtree,
        "BFS must find every descendant of the host block"
    );

    // (b) the costed alternative: ONE doc-scoped recursive CTE, then filter to
    // the subtree in memory over the returned `parent_id` edges.
    //
    // NOTE the semantic gap this fixture does not expose: `get_blocks`' CTE
    // stops at `Page`-tagged blocks, while the BFS crosses them. The two agree
    // here only because the fixture nests no page inside the subtree. A real
    // swap would need a subtree CTE that does NOT stop at page boundaries —
    // a new seam method, not a change confined to `home_authority.rs`.
    let t = Instant::now();
    let doc_blocks = reader.get_blocks(&page).await?;
    let mut kids: HashMap<String, Vec<String>> = HashMap::new();
    for b in &doc_blocks {
        kids.entry(b.parent_id.as_str().to_string())
            .or_default()
            .push(b.id.as_str().to_string());
    }
    let mut via_doc_read = Vec::new();
    let mut frontier = vec![host.as_str().to_string()];
    while let Some(node) = frontier.pop() {
        for k in kids.get(&node).into_iter().flatten() {
            via_doc_read.push(k.clone());
            frontier.push(k.clone());
        }
    }
    let doc_read_elapsed = t.elapsed();
    assert_eq!(
        via_doc_read.len(),
        subtree,
        "the doc-scoped alternative must return the SAME membership as the BFS — otherwise the \
         comparison is between two different answers"
    );

    // (b2) the FLOOR for any single-read alternative: a purpose-built subtree
    // CTE selecting ids only — no edge hydration, no page-boundary stop, no
    // `Block` deserialization. `get_blocks` cannot reach this because it must
    // return whole hydrated blocks; a seam that could would be a new method.
    let mut params = HashMap::new();
    params.insert("root".to_string(), Value::String(host.to_string()));
    let t = Instant::now();
    let rows = db
        .query(
            &format!(
                "WITH RECURSIVE sub(id, d) AS (SELECT id, 0 FROM {BLOCK_WRITE_TABLE} WHERE \
                 parent_id = $root UNION ALL SELECT b.id, s.d + 1 FROM {BLOCK_WRITE_TABLE} b JOIN \
                 sub s ON b.parent_id = s.id WHERE s.d < 100) SELECT id FROM sub"
            ),
            params,
        )
        .await
        .map_err(|e| anyhow::anyhow!("subtree CTE: {e}"))?;
    let subtree_cte_elapsed = t.elapsed();
    assert_eq!(
        rows.len(),
        subtree,
        "the id-only subtree CTE must return the same membership as the BFS"
    );

    // (c) the yardstick: `locate_batch` over the same ids, the Inc-1 fan-out
    // cost already deemed acceptable.
    authority.reset_locate_reads();
    let t = Instant::now();
    let placed = authority
        .locate_batch(&subtree_ids, &mut HomeBurstMemo::default())
        .await?;
    let batch_elapsed = t.elapsed();
    let batch_reads = authority.locate_reads();
    assert_eq!(placed.len(), subtree, "locate_batch must place every id");

    println!(
        "SUBTREE_OF_COST doc_blocks={DOC_BLOCKS} subtree={subtree} \
         bfs_children_reads={bfs_children} bfs_elapsed={bfs_elapsed:?} \
         doc_scoped_reads=1 doc_scoped_rows={} doc_scoped_elapsed={doc_read_elapsed:?} \
         subtree_cte_reads=1 subtree_cte_elapsed={subtree_cte_elapsed:?} \
         locate_batch_reads={batch_reads} locate_batch_elapsed={batch_elapsed:?}",
        doc_blocks.len()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subtree_of_bfs_cost_against_real_turso() -> Result<()> {
    for subtree in [10usize, 100, 1000 - 2] {
        measure(subtree).await?;
    }
    Ok(())
}
