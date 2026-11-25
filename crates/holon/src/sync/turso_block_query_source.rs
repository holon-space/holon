//! Turso implementation of [`BlockQuerySource`].
//!
//! This is the production / "real" side of the block read seam (ADR 0004
//! Phase 9): it reads Turso's CDC-driven `LiveData` mirrors of the `block` and
//! `focus_roots` materialized views and captures them into a sync
//! [`BlockSnapshot`]. The only async concern is settling — waiting for the CDC
//! stream to go quiescent — after which the read is a synchronous pass over the
//! in-memory mirror (see the module docs on
//! `holon_core::storage::block_query`).
//!
//! The `block` matview is keyed by id and unordered; canonical sibling order
//! comes from the substrate `sort_key` column (ADR 0005 / Phase 8), so this
//! mirror carries `sort_key` alongside the parsed [`Block`] and re-establishes
//! `ORDER BY parent_id, sort_key, id` at snapshot time.
//!
//! The row→[`Block`] parser is **injected** rather than reimplemented here, so
//! the PBT harness (which owns `parse_block_row`, depending on org helpers) and
//! production can each supply their own without this module pulling in those
//! dependencies or risking divergence.

use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::reporting_to;
use holon_api::streaming::BatchWithMetadata;
use holon_core::storage::block_query::BlockQuerySource;
use holon_core::storage::block_query::BlockSnapshot;
use holon_core::storage::block_query::FocusRoot;
use holon_core::storage::block_query::Result as BlockQueryResult;
use holon_core::storage::types::StorageEntity;

use crate::api::BackendEngine;
use crate::storage::BLOCK_READ_TABLE;
use crate::storage::turso::RowChangeStream;
use crate::sync::LiveData;

/// A parsed block paired with the substrate `sort_key` that orders it among its
/// siblings. Kept together in the mirror so order stays CDC-consistent with the
/// block content.
#[derive(Clone)]
struct OrderedBlock {
    block: Block,
    sort_key: String,
}

/// A row→`Block` parser, injected by the caller (PBT vs production).
/// Fail-loud: a parse failure carries the offending column + block id and is
/// propagated by the mirror, never dropped.
pub type BlockRowParser = Arc<dyn Fn(&StorageEntity) -> anyhow::Result<Block> + Send + Sync>;

/// Reads the Turso `block` + `focus_roots` matview mirrors and captures them
/// into a [`BlockSnapshot`].
pub struct TursoBlockQuerySource {
    blocks: Arc<LiveData<OrderedBlock>>,
    focus_roots: Arc<LiveData<FocusRoot>>,
    /// How long the CDC stream must be silent before a snapshot is considered
    /// settled, and the overall bound on waiting for that silence.
    quiet_for: Duration,
    settle_timeout: Duration,
}

impl TursoBlockQuerySource {
    /// Default quiescence window: 50ms of CDC silence, bounded at 5s.
    ///
    /// 50ms (not 150ms) because by the time any caller settles a snapshot the
    /// upstream write path has already driven the CDC *emission* stream to
    /// quiescence — so this mirror only has to drain batches that were emitted
    /// strictly earlier, never await an in-flight one. Confirming a drained
    /// broadcast receiver needs only a short silence window.
    pub const DEFAULT_QUIET_FOR: Duration = Duration::from_millis(50);
    pub const DEFAULT_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

    /// Build the source using the canonical [`Block::try_from`] row parser —
    /// the same one `CacheBlockReader` uses. This is the production default.
    pub async fn watch_default(engine: &BackendEngine) -> BlockQueryResult<Self> {
        let parser: BlockRowParser = Arc::new(|row| Block::try_from(row.clone()));
        Self::watch(engine, parser).await
    }

    /// Build the source by opening CDC watches on the `block` and `focus_roots`
    /// matviews of a started engine. `parse_block` turns a `block` matview row
    /// into a [`Block`] (the same parser the rest of the system uses).
    pub async fn watch(
        engine: &BackendEngine,
        parse_block: BlockRowParser,
    ) -> BlockQueryResult<Self> {
        Self::watch_with_settle(
            engine,
            parse_block,
            Self::DEFAULT_QUIET_FOR,
            Self::DEFAULT_SETTLE_TIMEOUT,
        )
        .await
    }

    /// Like [`watch`](Self::watch) with explicit quiescence tuning.
    pub async fn watch_with_settle(
        engine: &BackendEngine,
        parse_block: BlockRowParser,
        quiet_for: Duration,
        settle_timeout: Duration,
    ) -> BlockQueryResult<Self> {
        // `SELECT *` so the injected parser sees every column (`source_name`,
        // `created_at`, the `properties` JSON carrying level/sequence/planning,
        // the json_group_array-hydrated `tags`/`requires`, …). `sort_key` is
        // among them and drives the canonical sibling re-order at snapshot
        // time.
        let blocks_sql = format!("SELECT * FROM {BLOCK_READ_TABLE}");
        let (blocks_initial, blocks_stream) = watch_reporting(engine, &blocks_sql).await?;
        let parse_for_mirror = Arc::clone(&parse_block);
        let blocks = LiveData::new(
            blocks_initial,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("block row missing 'id'"))
            },
            move |row| {
                let block = (parse_for_mirror)(row)
                    .map_err(|e| anyhow::anyhow!("block parser failed for row {row:?}: {e}"))?;
                let sort_key = row
                    .get("sort_key")
                    .and_then(value_as_string)
                    .unwrap_or_default();
                Ok(OrderedBlock { block, sort_key })
            },
        );
        blocks.subscribe("block", blocks_stream);

        let (roots_initial, roots_stream) =
            watch_reporting(engine, "SELECT region, root_id FROM focus_roots").await?;
        let focus_roots = LiveData::new(
            roots_initial,
            |row| {
                let region = row
                    .get("region")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?;
                let root_id = row
                    .get("root_id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?;
                Ok(format!("{region}\u{1F}{root_id}"))
            },
            |row| {
                Ok(FocusRoot::new(
                    row.get("region")
                        .and_then(value_as_string)
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?,
                    row.get("root_id")
                        .and_then(value_as_string)
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?,
                ))
            },
        );
        // The source name is what tells the e2e correlator this mirror's rows
        // are focus roots (a navigation's observable), not block rows.
        focus_roots.subscribe(holon_api::latency_e2e::FOCUS_ROOTS_SOURCE, roots_stream);

        Ok(Self {
            blocks,
            focus_roots,
            quiet_for,
            settle_timeout,
        })
    }
}

#[async_trait]
impl BlockQuerySource for TursoBlockQuerySource {
    async fn snapshot(&self) -> BlockQueryResult<BlockSnapshot> {
        // Settle: wait for both mirrors' CDC streams to go quiet before
        // reading. The two mirrors are independent (separate matviews, no
        // ordering dependency), so wait on them CONCURRENTLY — sequential
        // awaits paid `2 × quiet_for` (300ms at the 150ms default) on every
        // snapshot even when both were already idle; joined, a snapshot of a
        // settled store costs a single `quiet_for` window.
        tokio::join!(
            self.blocks
                .wait_for_quiescent(self.quiet_for, self.settle_timeout),
            self.focus_roots
                .wait_for_quiescent(self.quiet_for, self.settle_timeout),
        );

        // Re-establish canonical order: ORDER BY parent_id, sort_key, id.
        let mut ordered: Vec<Arc<OrderedBlock>> = self.blocks.read().values().cloned().collect();
        ordered.sort_by(|a, b| {
            a.block
                .parent_id
                .as_str()
                .cmp(b.block.parent_id.as_str())
                .then_with(|| a.sort_key.cmp(&b.sort_key))
                .then_with(|| a.block.id.as_str().cmp(b.block.id.as_str()))
        });

        let blocks = ordered.into_iter().map(|ob| ob.block.clone());
        let focus_roots: Vec<FocusRoot> = self
            .focus_roots
            .read()
            .values()
            .map(|fr| (**fr).clone())
            .collect();
        Ok(BlockSnapshot::from_ordered(blocks, focus_roots))
    }
}

/// Register `Arc<dyn BlockQuerySource>` backed by Turso's CDC mirrors.
///
/// This is the Turso arm of the Phase-9 resolution rule (the `LoroMemory` arm
/// registers a `LoroBlockQuerySource` instead). The producer is built lazily:
/// the `BackendEngine` is resolved and the matview watches opened on first
/// resolution, so registration is cheap and order-independent. Consumers
/// `resolve_async::<dyn BlockQuerySource>()`.
pub fn register_turso_block_query_source(injector: &fluxdi::Injector) {
    injector.provide::<dyn BlockQuerySource>(fluxdi::Provider::root_async(|inj| async move {
        let engine = inj.resolve_async::<BackendEngine>().await;
        Arc::new(
            TursoBlockQuerySource::watch_default(&engine)
                .await
                .expect("register_turso_block_query_source: watch_default failed"),
        ) as Arc<dyn BlockQuerySource>
    }));
}

/// Open a watch whose stream reports each batch its consumer applied to the
/// engine's delivery bookkeeping, so a settle on the engine waits for the
/// mirror.
async fn watch_reporting(
    engine: &BackendEngine,
    sql: &str,
) -> BlockQueryResult<(Vec<StorageEntity>, AppliedOnNextPull<RowChangeStream>)> {
    let cursor = Arc::new(DeliveryCursor::default());
    let watch = reporting_to(cursor.clone(), engine.watch_view(sql))
        .await
        .map_err(to_boxed_err)?;
    let stream = AppliedOnNextPull {
        inner: watch.stream,
        cursor,
        handed: None,
    };
    Ok((watch.initial_rows, stream))
}

/// Reports a batch applied when its consumer pulls the next one: the
/// `LiveData` actor pulls only after it applied the batch it holds.
struct AppliedOnNextPull<S> {
    inner: S,
    cursor: Arc<DeliveryCursor>,
    handed: Option<u64>,
}

impl<S, C> tokio_stream::Stream for AppliedOnNextPull<S>
where
    S: tokio_stream::Stream<Item = BatchWithMetadata<C>> + Unpin,
{
    type Item = BatchWithMetadata<C>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(seq) = self.handed.take() {
            self.cursor.applied_through(seq);
        }
        let next = Pin::new(&mut self.inner).poll_next(cx);
        if let Poll::Ready(Some(batch)) = &next {
            self.handed = Some(batch.metadata.seq);
        }
        next
    }
}

fn value_as_string(v: &Value) -> Option<String> {
    v.as_string().map(|s| s.to_string())
}

fn to_boxed_err<E: std::fmt::Display>(e: E) -> Box<dyn std::error::Error + Send + Sync> {
    format!("TursoBlockQuerySource watch failed: {e}").into()
}
