//! A watch that is over must leave no membership row behind.
//!
//! `watch_context` is what makes ONE descendant matview serve every focus
//! root: the root enters the shared view as a joined row instead of as a
//! literal. That trade buys away the per-block `CREATE MATERIALIZED VIEW`, and
//! it buys a new unbounded growth in exchange — a row that outlives its watch
//! keeps a whole subtree incrementally maintained for nobody, on every write,
//! forever. The row count returning to where it started is the property that
//! says the trade is safe.
//!
//! The row is owned by the watch's `WatchContextGuard`, so it goes when the
//! watch ends — on a quiet database too, which is the normal state of an app
//! whose panel was just closed. These tests therefore write NOTHING after
//! dropping a watch: a prune that needed a later write would not be a
//! lifetime at all.
//!
//! The file also holds the other half of the trade: with one view serving
//! many watches, each watch must receive ITS rows and no one else's. That is
//! asserted end to end, over a real shared view, at the bottom.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use holon::api::backend_engine::BackendEngine;
use holon::api::backend_engine::WatchViewKind;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::runtime::Handle;
use tokio_stream::StreamExt as _;

const ROOT_PARENT: &str = "sentinel:no_parent";
/// Enough watches that a partial prune is visible as a count, not as a flake.
const WATCHES: usize = 5;

/// The production SqlOnly block wiring. `E2ETestContext::new()` boots an
/// engine with NO block provider, and a leak test that cannot write blocks
/// never produces the CDC batch the property depends on. Same construction as
/// `capability_certification.rs`.
async fn block_engine() -> Result<Arc<BackendEngine>> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = Handle::current();
                    // ALLOW(block_on): wrapped in `block_in_place`, which is what makes a
                    // blocking wait legal on a multi-thread runtime thread.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .context("the leak test's engine must boot with the block provider")
}

async fn count_membership(ctx: &E2ETestContext) -> Result<i64> {
    let rows = ctx
        .service()
        .engine()
        .db_handle()
        .query("SELECT count(*) AS n FROM watch_context", HashMap::new())
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.get("n"))
        .and_then(|v| v.as_i64())
        .expect("count(*) always returns one integer row"))
}

async fn create_block(ctx: &E2ETestContext, id: &str, parent: &str) -> Result<()> {
    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(format!("block:{id}")));
    params.insert("content".into(), Value::String(format!("{id} content")));
    params.insert("parent_id".into(), Value::String(parent.to_string()));
    ctx.execute_op("block", "create", params).await
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_watches_returns_watch_context_to_its_starting_count() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "leak-root", ROOT_PARENT).await?;
    for i in 0..WATCHES {
        create_block(&ctx, &format!("leak-child-{i}"), "block:leak-root").await?;
    }

    let start = count_membership(&ctx).await?;

    // The shape the leaf render path watches: the block enters through
    // `watch_context`, so all WATCHES subscribers share one view.
    let sql = leaf_sql();

    let mut streams = Vec::new();
    for i in 0..WATCHES {
        let block = format!("block:leak-child-{i}");
        streams.push(
            engine
                .query_and_watch_keyed(
                    sql.clone(),
                    &format!("leak:{block}"),
                    &block,
                    WatchViewKind::Leaf,
                )
                .await?,
        );
    }
    assert_eq!(
        count_membership(&ctx).await?,
        start + WATCHES as i64,
        "each open watch must own exactly one membership row"
    );

    drop(streams);

    // NOTHING IS WRITTEN AFTER THIS POINT. A quiet database is the normal
    // state of an app whose panel was just closed, and it is the case a
    // write-triggered prune cannot serve: the row must go because the watch
    // ended, not because something else happened to change later.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = -1;
    while Instant::now() < deadline {
        seen = count_membership(&ctx).await?;
        if seen == start {
            assert_eq!(
                engine.watch_release_failures(),
                0,
                "a membership delete failed; the row is standing work on every later write"
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    panic!(
        "{} membership rows outlived their watches ({seen} rows, started at {start}) on a QUIET \
         database: every leaked row keeps its subtree incrementally maintained on every later \
         write, for nobody",
        seen - start
    );
}

/// A watch that fails to set up must not leave its membership row behind.
///
/// The row is inserted before the view is minted, so every `?` between the
/// insert and the subscription is a leak unless the row's lifetime is owned by
/// something that unwinds with them.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_watch_setup_leaves_no_membership_row() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "fail-root", ROOT_PARENT).await?;
    let start = count_membership(&ctx).await?;

    // A bad COLUMN, not a bad table. Both are real ways a watch fails, but
    // they fail through different machinery: an unknown TABLE is an
    // unsatisfied DDL dependency, so `execute_ddl_with_deps` waits the full
    // `DEPENDENCY_TIMEOUT` (120 s, crates/holon-turso/src/turso.rs:1113)
    // before erroring — longer than the runner's own cap. Every table here
    // exists, so the DDL is admitted immediately and the engine rejects the
    // column, which is the fast path this property needs.
    let sql = format!(
        "SELECT b.no_such_column_for_the_leak_test, wc.watch_key AS watch_key FROM {table} b \
         JOIN watch_context wc ON b.id = wc.context_id",
        table = holon::storage::BLOCK_READ_TABLE,
    );

    let began = Instant::now();
    let outcome = engine
        .query_and_watch_keyed(sql, "leak:fail", "block:fail-root", WatchViewKind::Leaf)
        .await;
    let waited = began.elapsed();

    assert!(
        outcome.is_err(),
        "a watch whose view cannot be created must return an error, not a stream: waited {waited:?}"
    );
    println!("[failed-setup] error returned after {waited:?}");
    assert!(
        waited < Duration::from_secs(20),
        "the failure path must return promptly, not park in a dependency wait: {waited:?}"
    );

    // The guard's `Drop` cannot await, so it hands the delete to the actor;
    // the property is that the row does not SURVIVE the failure, bounded.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = -1;
    while Instant::now() < deadline {
        seen = count_membership(&ctx).await?;
        if seen == start {
            assert_eq!(
                engine.watch_release_failures(),
                0,
                "a membership delete failed; the row is standing work on every later write"
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a failed watch setup left its membership row behind ({seen} rows, started at {start})");
}

/// The leaf shape, deduplicated by key exactly as production builds it.
fn leaf_sql() -> String {
    format!(
        "SELECT b.*, wc.watch_key AS watch_key FROM {table} b \
         JOIN watch_context wc ON b.id = wc.context_id WHERE wc.kind = 'leaf'",
        table = holon::storage::BLOCK_READ_TABLE,
    )
}

/// Two panels can show one block — `is_focus_root` is region-agnostic and the
/// main panel and a sidebar can rest on the same block — and they address the
/// same place, so they share a `watch_key`. The NEWER one closing first must
/// not take the older one's subtree away.
///
/// One place holds ONE row — a row per watch is not available here
/// (navigation.sql names the measurements) — so ownership is a set beside the
/// watches and only the LAST owner releases. The property is asserted on what
/// the older watch RECEIVES, not only on a row count.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_survives_a_second_watch_on_the_same_place_ending_first() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "shared-place", ROOT_PARENT).await?;
    let start = count_membership(&ctx).await?;
    let place = "leaf:block:shared-place";

    let mut older = engine
        .query_and_watch_keyed(leaf_sql(), place, "block:shared-place", WatchViewKind::Leaf)
        .await?;
    let newer = engine
        .query_and_watch_keyed(leaf_sql(), place, "block:shared-place", WatchViewKind::Leaf)
        .await?;

    assert_eq!(
        count_membership(&ctx).await?,
        start + 1,
        "one place holds ONE row however many watches look at it"
    );

    // Drain the snapshot batch both watches open with, so the next batch the
    // older one sees is a real change.
    let _ = tokio::time::timeout(Duration::from_secs(5), older.next()).await;

    drop(newer);
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        count_membership(&ctx).await?,
        start + 1,
        "the newer watch ending took the place's row the older watch still needs"
    );

    // The decisive assertion: the older watch is still fed by the shared view.
    let mut edit: holon_api::StorageEntity = HashMap::new();
    edit.insert("id".into(), Value::String("block:shared-place".to_string()));
    edit.insert("content".into(), Value::String("edited".to_string()));
    ctx.execute_op("block", "update", edit).await?;

    let batch = tokio::time::timeout(Duration::from_secs(20), older.next())
        .await
        .expect("the surviving watch received nothing after an edit to its block")
        .expect("the surviving watch's stream ended");
    assert!(
        !batch.inner.items.is_empty(),
        "the surviving watch received an empty batch after an edit to its block"
    );

    drop(older);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if count_membership(&ctx).await? == start {
            assert_eq!(engine.watch_release_failures(), 0);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the last watch on the place left its claim behind");
}

/// A release that lands AFTER a reboot purged the table must spare the row the
/// new boot's watch registered.
///
/// `Drop` cannot await, so the release is spawned, and a reboot inside a live
/// process truncates `watch_context` and re-registers its watches ~1 ms later
/// (`schema_modules.rs`, hand-authored case
/// `reboot-orphans-the-previous-boots-watchers`). Whether the release or the
/// purge wins is the scheduler's choice, so the ordering is forced here: the
/// purge and the successor both happen before the old watch is dropped. The
/// release names its own `(watch_key, nonce)`, so the successor's row — a
/// different nonce under the same key — cannot be what it deletes.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_that_lands_after_a_boot_purge_spares_the_new_boots_row() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "reopen-place", ROOT_PARENT).await?;
    let place = "leaf:block:reopen-place";

    let first = engine
        .query_and_watch_keyed(leaf_sql(), place, "block:reopen-place", WatchViewKind::Leaf)
        .await?;

    // The reboot: the table is truncated, then the new boot registers its own
    // watch on the same place.
    ctx.service()
        .engine()
        .db_handle()
        .execute("DELETE FROM watch_context", Vec::new())
        .await?;
    let mut second = engine
        .query_and_watch_keyed(leaf_sql(), place, "block:reopen-place", WatchViewKind::Leaf)
        .await?;

    // Only now does the previous boot's watch end, so its release is the last
    // statement of the three.
    drop(first);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        count_membership(&ctx).await?,
        1,
        "the previous boot's release took the row the new boot registered"
    );

    // Drain the snapshot batch, then prove the successor is still fed.
    let _ = tokio::time::timeout(Duration::from_secs(5), second.next()).await;
    let mut edit: holon_api::StorageEntity = HashMap::new();
    edit.insert("id".into(), Value::String("block:reopen-place".to_string()));
    edit.insert("content".into(), Value::String("edited".to_string()));
    ctx.execute_op("block", "update", edit).await?;

    let batch = tokio::time::timeout(Duration::from_secs(20), second.next())
        .await
        .expect("the successor watch received nothing after an edit to its block")
        .expect("the successor watch's stream ended");
    assert!(
        !batch.inner.items.is_empty(),
        "the successor watch received an empty batch after an edit to its block"
    );

    drop(second);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if count_membership(&ctx).await? == 0 {
            assert_eq!(engine.watch_release_failures(), 0);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the successor watch's row outlived it");
}

/// Re-opening a watch on the SAME place, then dropping the OLD one, must
/// leave the live watch's row alone.
///
/// The re-render order where the old watch outlives the new one's opening.
/// Both watches hold the same place, so the old one leaving must leave the
/// place — and its closure — standing for the watch that remains.
#[tokio::test(flavor = "multi_thread")]
async fn a_superseded_watch_does_not_delete_its_replacements_row() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "sup-root", ROOT_PARENT).await?;
    let start = count_membership(&ctx).await?;

    let sql = leaf_sql();
    let place = "leaf:sup-place";

    let first = engine
        .query_and_watch_keyed(sql.clone(), place, "block:sup-root", WatchViewKind::Leaf)
        .await?;
    // The re-open: same place, so it REPLACES the row and takes ownership.
    let second = engine
        .query_and_watch_keyed(sql.clone(), place, "block:sup-root", WatchViewKind::Leaf)
        .await?;
    assert_eq!(
        count_membership(&ctx).await?,
        start + 1,
        "one place holds ONE row however often it is re-opened"
    );

    // The superseded watch ends AFTER its replacement exists.
    drop(first);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        count_membership(&ctx).await?,
        start + 1,
        "the superseded watch deleted the row its replacement still needs"
    );

    assert_eq!(
        engine.watch_release_failures(),
        0,
        "being superseded is the correct outcome, not a failed delete"
    );

    // And the live watch still cleans up after itself.
    drop(second);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if count_membership(&ctx).await? == start {
            assert_eq!(engine.watch_release_failures(), 0);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the surviving watch's row outlived it");
}

/// Two engines in ONE process, on two databases, must not share ownership.
///
/// `watch_key` is `leaf:{block_id}`, and the same block id exists in both
/// databases whenever two instances share a subtree —
/// `pbt/composed/two_instance.rs` boots exactly that. Ownership therefore
/// belongs to the ENGINE that owns the database: held process-wide, one
/// engine's watch ending decides the fate of the other engine's row, in
/// another database, and neither the row count nor any counter shows it.
#[tokio::test(flavor = "multi_thread")]
async fn two_engines_in_one_process_own_their_rows_separately() -> Result<()> {
    let left = E2ETestContext::from_engine(block_engine().await?);
    let right = E2ETestContext::from_engine(block_engine().await?);
    create_block(&left, "twin", ROOT_PARENT).await?;
    create_block(&right, "twin", ROOT_PARENT).await?;
    let place = "leaf:block:twin";

    let left_watch = left
        .service()
        .engine()
        .query_and_watch_keyed(leaf_sql(), place, "block:twin", WatchViewKind::Leaf)
        .await?;
    let right_watch = right
        .service()
        .engine()
        .query_and_watch_keyed(leaf_sql(), place, "block:twin", WatchViewKind::Leaf)
        .await?;
    assert_eq!(
        (
            count_membership(&left).await?,
            count_membership(&right).await?
        ),
        (1, 1),
        "each database holds the row of the watch opened against it"
    );

    drop(left_watch);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        (
            count_membership(&left).await?,
            count_membership(&right).await?
        ),
        (0, 1),
        "the left watch ending must clear the left row and only the left row"
    );

    drop(right_watch);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if (
            count_membership(&left).await?,
            count_membership(&right).await?,
        ) == (0, 0)
        {
            assert_eq!(left.service().engine().watch_release_failures(), 0);
            assert_eq!(right.service().engine().watch_release_failures(), 0);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a watch's row outlived it in one of the two databases");
}

/// The descendant shape, exactly as `render_region_root` builds it.
fn root_sql() -> String {
    format!(
        "WITH RECURSIVE subtree(watch_key, node_id, depth, visited) AS ( \
           SELECT wc.watch_key, b.id, 0, CAST(b.id AS TEXT) \
           FROM watch_context wc JOIN {table} b ON b.id = wc.context_id \
           WHERE wc.kind = 'root' \
           UNION ALL \
           SELECT subtree.watch_key, child.id, subtree.depth + 1, \
                  subtree.visited || ',' || CAST(child.id AS TEXT) \
           FROM subtree \
           JOIN {table} child ON child.parent_id = subtree.node_id \
           LEFT JOIN block_tags pt ON pt.block_id = subtree.node_id AND pt.tag = 'Page' \
           WHERE subtree.depth < 20 \
             AND ',' || subtree.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%' \
             AND (subtree.depth = 0 OR pt.block_id IS NULL) \
         ) \
         SELECT d.*, subtree.watch_key AS watch_key \
         FROM subtree JOIN {table} d ON d.id = subtree.node_id",
        table = holon::storage::BLOCK_READ_TABLE,
    )
}

/// Every id a watch is told about, over `window`, as (created/updated,
/// deleted).
async fn drain(
    stream: &mut holon_turso::turso::RowChangeStream,
    window: Duration,
) -> (Vec<String>, Vec<String>) {
    let mut present = Vec::new();
    let mut gone = Vec::new();
    let deadline = Instant::now() + window;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(Some(batch)) = tokio::time::timeout(remaining, stream.next()).await else {
            break;
        };
        for change in batch.inner.items {
            match change.change {
                holon_turso::turso::ChangeData::Deleted { id, .. } => gone.push(id),
                holon_turso::turso::ChangeData::Created { data, .. }
                | holon_turso::turso::ChangeData::Updated { data, .. } => {
                    if let Some(Value::String(id)) = data.get("id") {
                        present.push(id.clone());
                    }
                }
                holon_turso::turso::ChangeData::FieldsChanged { entity_id, .. } => {
                    present.push(entity_id)
                }
            }
        }
    }
    present.sort();
    present.dedup();
    gone.sort();
    gone.dedup();
    (present, gone)
}

/// Two watches on OVERLAPPING subtrees of one shared view: a block that moves
/// from one subtree to the other must leave the first and arrive at the
/// second, each watch hearing only about its own.
///
/// This is the end-to-end half of the per-key routing that
/// `coalesce_row_changes` is unit-tested for: one relation, two subscribers,
/// one entity id in both their pasts.
#[tokio::test(flavor = "multi_thread")]
async fn two_overlapping_watches_each_receive_only_their_own_subtree() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "left", ROOT_PARENT).await?;
    create_block(&ctx, "right", ROOT_PARENT).await?;
    create_block(&ctx, "mover", "block:left").await?;

    let mut left = engine
        .query_and_watch_keyed(
            root_sql(),
            "root:block:left",
            "block:left",
            WatchViewKind::Root,
        )
        .await?;
    let mut right = engine
        .query_and_watch_keyed(
            root_sql(),
            "root:block:right",
            "block:right",
            WatchViewKind::Root,
        )
        .await?;

    let (left_seed, _) = drain(&mut left, Duration::from_secs(3)).await;
    let (right_seed, _) = drain(&mut right, Duration::from_secs(3)).await;
    assert!(
        left_seed.contains(&"block:mover".to_string()),
        "the left watch must open with its own subtree: {left_seed:?}"
    );
    assert!(
        !right_seed.contains(&"block:mover".to_string()),
        "the right watch must not open with the left subtree's block: {right_seed:?}"
    );

    let mut move_params: holon_api::StorageEntity = HashMap::new();
    move_params.insert("id".into(), Value::String("block:mover".to_string()));
    move_params.insert("parent_id".into(), Value::String("block:right".to_string()));
    ctx.execute_op("block", "update", move_params).await?;

    let (left_after, left_gone) = drain(&mut left, Duration::from_secs(10)).await;
    let (right_after, _) = drain(&mut right, Duration::from_secs(10)).await;

    assert!(
        left_gone.contains(&"block:mover".to_string()),
        "the left watch must be told the block left its subtree; got present={left_after:?} \
         deleted={left_gone:?}"
    );
    assert!(
        right_after.contains(&"block:mover".to_string()),
        "the right watch must be told the block entered its subtree; got {right_after:?}"
    );
    Ok(())
}

/// The state oracle names a row no live watch owns, and says nothing about
/// the rows that are owned.
///
/// `inv-watch-context-rows-owned` asks the engine this after every keystone
/// transition. It is the detector for the leak classes that report NO error —
/// a release never issued, or one that matched nothing — so its own teeth are
/// pinned here rather than inferred from a suite that happens to stay green.
#[tokio::test(flavor = "multi_thread")]
async fn the_ownership_oracle_names_a_row_no_watch_owns() -> Result<()> {
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    create_block(&ctx, "owned-place", ROOT_PARENT).await?;
    let watch = engine
        .query_and_watch_keyed(
            leaf_sql(),
            "leaf:block:owned-place",
            "block:owned-place",
            WatchViewKind::Leaf,
        )
        .await?;
    assert!(
        engine.unowned_watch_context_rows().await?.is_empty(),
        "a live watch's own row must not be reported as unowned"
    );

    // The state a skipped or mismatched release leaves: a row for a place no
    // watch of this engine holds. Written directly, because every way of
    // producing it through the guard is a bug that is now fixed.
    ctx.service()
        .engine()
        .db_handle()
        .execute(
            "INSERT INTO watch_context (watch_key, context_id, kind, nonce) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Text("leaf:block:ghost".into()),
                turso::Value::Text("block:owned-place".into()),
                turso::Value::Text("leaf".into()),
                turso::Value::Text("ghost-nonce".into()),
            ],
        )
        .await?;

    let unowned = engine.unowned_watch_context_rows().await?;
    assert_eq!(
        unowned.len(),
        1,
        "expected exactly the ghost row: {unowned:?}"
    );
    assert!(
        unowned[0].contains("leaf:block:ghost"),
        "the report must name the orphaned place: {unowned:?}"
    );

    drop(watch);
    Ok(())
}
