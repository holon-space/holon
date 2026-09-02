//! Shared harness for the Loro→SQL projection tests: a fresh
//! `LoroDocumentStore` over a `TempDir`, the global doc force-initialized, real
//! block nodes inserted into the global tree, and an in-memory sink standing in
//! for both the `OriginTaggedWrites` command bus and the `SinkReader` read side
//! (mirrors `stub_sut.rs`'s `StubOperationProvider`).

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
use holon_loro::STABLE_ID;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use holon_loro::event_bus::EventOrigin;
use loro::TreeID;
use tokio::sync::RwLock;

/// In-memory sink that can be toggled to fail every write.
pub(crate) struct MemorySink {
    /// When `true`, `execute_batch_with_origin` returns `Err` (batch rolled
    /// back).
    fail: AtomicBool,
    /// Every `execute_batch_with_origin` invocation increments this —
    /// successful AND failed — so a test can prove an apply was ATTEMPTED
    /// on failure and re-ATTEMPTED on the following success.
    apply_calls: AtomicUsize,
    /// stable-id → merged param map (the persisted block rows the projection
    /// diffs against via `read_blocks`).
    blocks: StdMutex<HashMap<String, StorageEntity>>,
}

impl MemorySink {
    pub(crate) fn new() -> Self {
        Self {
            fail: AtomicBool::new(false),
            apply_calls: AtomicUsize::new(0),
            blocks: StdMutex::new(HashMap::new()),
        }
    }

    pub(crate) fn set_fail(&self, v: bool) {
        self.fail.store(v, Ordering::SeqCst);
    }

    pub(crate) fn apply_calls(&self) -> usize {
        self.apply_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn row_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.blocks.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
}

#[async_trait]
impl SinkReader for MemorySink {
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
impl OperationProvider for MemorySink {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Vec::new()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        panic!("MemorySink::execute_operation unused; use execute_batch_with_origin");
    }
}

#[async_trait]
impl OriginTaggedWrites for MemorySink {
    async fn execute_operation_with_origin(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
        _: EventOrigin,
    ) -> DatasourceResult<OperationResult> {
        panic!("MemorySink::execute_operation_with_origin unused; use execute_batch_with_origin");
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
            return Err("MemorySink: injected sink-write failure (batch rolled back)".into());
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
                other => panic!("MemorySink: unknown op '{other}'"),
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
pub(crate) async fn insert_root_block(
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
