//! Atomic base-advance fault-injection test for the incremental Loro→SQL
//! projection (`LoroProjection::project`).
//!
//! Contract under test: when the sink write fails (`consolidator.apply` →
//! `Err`, driven here by a `ToggleFailSink` whose `execute_batch_with_origin`
//! returns `Err`), `project()`:
//!   1. returns `Err`,
//!   2. does NOT advance the incremental diff base `live`,
//!   3. does NOT advance the `last_synced` watermark,
//!   4. flips `seeded` to `false` (forcing a full reseed next pass), and
//!   5. RE-EMITS the change on the next successful pass — never silently drops
//!      it.
//!
//! The harness models `stub_sut.rs`: a fresh `LoroDocumentStore` over a
//! `TempDir`, the global doc force-initialized, real block nodes inserted into
//! the global tree, and an in-memory sink standing in for both the
//! `OriginTaggedWrites` command bus and the `SinkReader` read side.
//!
//! @pbt kind harness
//! @pbt covers loro-projection-atomic — atomic base-advance fault injection for
//! Loro→SQL projection

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EdgeField;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::SnapshotBlock;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result as DatasourceResult;
use holon_loro::CONTENT_RAW;
use holon_loro::CONTENT_TYPE;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::PendingChange;
use holon_loro::STABLE_ID;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use holon_loro::event_bus::EventOrigin;
use loro::Frontiers;
use loro::TreeID;
use loro::TreeParentId;
use tokio::sync::RwLock;

/// In-memory sink that can be toggled to fail every write. Serves as BOTH the
/// projection's `OriginTaggedWrites` command bus and its `SinkReader` read side
/// (mirrors `StubOperationProvider`).
struct ToggleFailSink {
    /// When `true`, `execute_batch_with_origin` returns `Err` (batch rolled
    /// back).
    fail: AtomicBool,
    /// Every `execute_batch_with_origin` invocation increments this —
    /// successful AND failed — so a test can prove an apply was ATTEMPTED
    /// on failure and re-ATTEMPTED on the following success. (The spec's
    /// phrasing only calls out the failure increment; counting every call
    /// is a superset that satisfies both the "apply attempted" and
    /// "incremented again" assertions.)
    apply_calls: AtomicUsize,
    /// stable-id → merged param map (the persisted block rows the projection
    /// diffs against via `read_blocks`).
    blocks: StdMutex<HashMap<String, StorageEntity>>,
}

impl ToggleFailSink {
    fn new() -> Self {
        Self {
            fail: AtomicBool::new(false),
            apply_calls: AtomicUsize::new(0),
            blocks: StdMutex::new(HashMap::new()),
        }
    }

    fn set_fail(&self, v: bool) {
        self.fail.store(v, Ordering::SeqCst);
    }

    fn apply_calls(&self) -> usize {
        self.apply_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SinkReader for ToggleFailSink {
    async fn read_blocks(&self) -> Result<HashMap<String, SnapshotBlock>> {
        let blocks = self.blocks.lock().unwrap();
        let mut out = HashMap::with_capacity(blocks.len());
        for (id, params) in blocks.iter() {
            let sort_key = params
                .get("sort_key")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "A0".to_string());
            // `block_to_params` writes only `block_raw`; the junction-synthesized
            // edge columns are absent. `TursoSinkReader` COALESCEs every one of
            // them to `'[]'` — mirror that over `EdgeField::ALL` so the strict
            // `Block::try_from` decode succeeds and a newly added edge field
            // cannot leave this stub behind.
            let mut row = params.clone();
            for field in EdgeField::ALL {
                row.entry(field.column().into())
                    .or_insert_with(|| Value::String("[]".to_string()));
            }
            let block = Block::try_from(row)?;
            out.insert(id.clone(), SnapshotBlock { block, sort_key });
        }
        Ok(out)
    }
}

#[async_trait]
impl OperationProvider for ToggleFailSink {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Vec::new()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        panic!("ToggleFailSink::execute_operation unused; use execute_batch_with_origin");
    }
}

#[async_trait]
impl OriginTaggedWrites for ToggleFailSink {
    async fn execute_operation_with_origin(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
        _: EventOrigin,
    ) -> DatasourceResult<OperationResult> {
        panic!(
            "ToggleFailSink::execute_operation_with_origin unused; use execute_batch_with_origin"
        );
    }

    async fn execute_batch_with_origin(
        &self,
        entity_name: &EntityName,
        operations: Vec<holon_core::BatchOp>,
        _: EventOrigin,
    ) -> DatasourceResult<Vec<OperationResult>> {
        assert_eq!(entity_name, "block", "sink only knows the 'block' entity");
        self.apply_calls.fetch_add(1, Ordering::SeqCst);

        if self.fail.load(Ordering::SeqCst) {
            return Err("ToggleFailSink: injected sink-write failure (batch rolled back)".into());
        }

        let mut blocks = self.blocks.lock().unwrap();
        for op in &operations {
            let params = &op.params;
            match op.op_name.as_str() {
                "create" | "update" => {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .expect("create/update missing 'id'");
                    let entry = blocks.entry(id).or_default();
                    for (k, v) in params {
                        entry.insert(k.clone(), v.clone());
                    }
                }
                "delete" => {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .expect("delete missing 'id'");
                    blocks.remove(&id);
                }
                other => panic!("ToggleFailSink: unknown op '{other}'"),
            }
        }
        Ok(vec![
            OperationResult::irreversible(Vec::new());
            operations.len()
        ])
    }
}

/// Insert a real root-level block node (STABLE_ID meta + text content, with the
/// global tree's auto-assigned fractional index) into the global doc, returning
/// its `TreeID`. Mirrors the meta the prod create path writes.
async fn insert_root_block(
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    stable_id: &str,
    content: &str,
) -> Result<TreeID> {
    let collab = doc_store.read().await.get_doc(DocScope::Global).await?;
    let doc = collab.doc();
    let tree = doc.get_tree(TREE_NAME);
    let node = tree.create(None)?; // root-level; fi auto-assigned by schema
    let meta = tree.get_meta(node)?;
    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id))?;
    meta.insert(CONTENT_TYPE, loro::LoroValue::from("text"))?;
    let text = meta.ensure_mergeable_text(CONTENT_RAW)?;
    text.insert(0, content)?;
    doc.commit();
    Ok(node)
}

#[tokio::test]
async fn failed_sink_write_neither_advances_base_nor_drops_change() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    // Force global doc init (schema: fractional index enabled) so the tree
    // accepts nodes and the projection has something to read.
    doc_store.read().await.get_doc(DocScope::Global).await?;

    let sink = Arc::new(ToggleFailSink::new());
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sc.sync"),
    );
    projection.arm();

    // ── (a) fail=false: insert B, seed `live` via a full (cold-boot) pass ──────
    sink.set_fail(false);
    insert_root_block(&doc_store, "B-id", "block B").await?;
    projection.project().await.expect("seed pass succeeds");

    assert!(projection.is_seeded(), "first successful pass seeds `live`");
    assert!(
        projection.live_snapshot().contains_key("block:B-id"),
        "`live` holds B after the seed pass: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    let l1 = projection.last_synced_value();
    let calls1 = sink.apply_calls();
    assert_eq!(calls1, 1, "one apply (the seed create) so far");

    // ── (b) insert C, stage its create fact, arm the failure ──────────────────
    let c_tid = insert_root_block(&doc_store, "C-id", "block C").await?;
    projection
        .pending()
        .lock()
        .unwrap()
        .push(PendingChange::Create {
            parent: TreeParentId::Root,
            target: c_tid,
        });
    sink.set_fail(true);

    // ── (c) failing incremental pass: Err, base untouched, seeded flipped ──────
    let r = projection.project().await;
    assert!(r.is_err(), "a failed sink write surfaces as Err");
    assert!(
        !projection.is_seeded(),
        "failure flips `seeded` false to force a full reseed"
    );
    assert!(
        !projection.live_snapshot().contains_key("block:C-id"),
        "staging is NOT applied on failure — `live` must not gain C: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    assert_eq!(
        projection.last_synced_value(),
        l1,
        "`last_synced` must not advance past the last committed sink write"
    );
    assert!(
        sink.apply_calls() > calls1,
        "the sink apply was ATTEMPTED (and failed): {} !> {}",
        sink.apply_calls(),
        calls1
    );
    let calls_after_fail = sink.apply_calls();

    // ── (d) recovery: next successful pass RE-EMITS C (never silently dropped) ─
    sink.set_fail(false);
    projection.project().await.expect("recovery pass succeeds");
    assert!(
        projection.live_snapshot().contains_key("block:C-id"),
        "C is re-emitted on recovery — the change was never dropped: {:?}",
        projection.live_snapshot().keys().collect::<Vec<_>>()
    );
    assert!(
        sink.apply_calls() > calls_after_fail,
        "the recovery pass re-attempted the apply: {} !> {}",
        sink.apply_calls(),
        calls_after_fail
    );

    Ok(())
}
