//! The seam: a block that arrives through the share projection leg must be
//! REFUSED when the user then tries to edit it.
//!
//! The two halves were pinned separately and neither proved the join. The
//! share backend's own tests end at `ReadOnlyDocuments::refusal_for_block`, and
//! the dispatcher's end at a block a test adopted by hand. This test carries
//! one block across: a peer's update enters through the real
//! `LoroShareBackend` projection worker, and the edit that follows goes through
//! the real `OperationDispatcher` write-tier gate.
//!
//! Entry `2026-09-08-a-synced-block-under-a-read-only-document-stays-editable`,
//! Residual #1.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use holon::api::AuthoredInput;
use holon::api::OperationDispatcher;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::ReadOnlyDocuments;
use holon_core::ReadOnlyMembers;
use holon_core::Result;
use holon_core::WriteTierAuthority;
use holon_loro::degraded_signal_bus::DegradedSignalBus;
use holon_loro::device_key_store::load_or_create_device_key;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::iroh_sync_adapter::SharedTreeSyncManager;
use holon_loro::loro_document_store::LoroDocumentStore;
use holon_loro::loro_share_backend::LoroShareBackend;
use holon_loro::multi_peer::STABLE_ID;
use holon_loro::multi_peer::TREE_NAME;
use loro::LoroDoc;
use loro::LoroText;
use loro::TreeID;
use tempfile::TempDir;
use tokio::sync::RwLock;

const RECIPE: &str = "Pancakes.cook";
const RECIPE_PATH: &str = "/vault/Pancakes.cook";
const STEP: &str = "Pancakes.cook::b::0";
const IMPORTED: &str = "peer-added";

/// Stands for the composition root's `ReadOnlyFormatGate`, minus its degraded
/// bus: the disclosure leg is not what this test is about, and linking
/// `holon-app` from here would invert the dependency.
struct Tier(Arc<ReadOnlyDocuments>);

#[async_trait]
impl WriteTierAuthority for Tier {
    fn any_read_only_documents(&self) -> bool {
        !self.0.is_empty()
    }

    async fn refusal_for(&self, block_id: &str) -> Result<Option<holon_core::EditRefused>> {
        Ok(self.0.refusal_for_block(&EntityUri::parse(block_id)?))
    }

    async fn adopt_sync_import(&self, block_id: &str, parent_id: &str) -> Result<bool> {
        Ok(self
            .0
            .adopt(&EntityUri::parse(parent_id)?, &EntityUri::parse(block_id)?))
    }

    fn disclose(&self, _: &holon_core::EditRefused) {}
}

/// The share backend's SQL sink. Records nothing but presence — this test
/// asserts the tier decision, and the row's own shape is pinned in
/// `holon-loro`.
#[derive(Default)]
struct RecordingSink(std::sync::Mutex<Vec<String>>);

impl RecordingSink {
    fn has(&self, id: &str) -> bool {
        self.0.lock().unwrap().iter().any(|s| s == id)
    }

    fn note(&self, params: &StorageEntity) {
        if let Some(id) = params.get("id").and_then(|v| v.as_string()) {
            self.0.lock().unwrap().push(id.to_string());
        }
    }
}

#[async_trait]
impl OperationProvider for RecordingSink {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![]
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        self.note(&params);
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl OriginTaggedWrites for RecordingSink {
    async fn execute_operation_with_origin(
        &self,
        _: &EntityName,
        _: &str,
        params: StorageEntity,
        _: holon_loro::event_bus::EventOrigin,
    ) -> Result<OperationResult> {
        self.note(&params);
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn execute_batch_with_origin(
        &self,
        _: &EntityName,
        operations: Vec<holon_core::BatchOp>,
        _: holon_loro::event_bus::EventOrigin,
    ) -> Result<Vec<OperationResult>> {
        let mut out = Vec::with_capacity(operations.len());
        for op in operations {
            self.note(&op.params);
            out.push(OperationResult::irreversible(vec![]));
        }
        Ok(out)
    }
}

/// The block provider behind the dispatcher. It must never run here: the
/// write-tier gate refuses before routing, and a call would mean the refusal
/// came too late to protect anything.
struct MustNotRun;

#[async_trait]
impl OperationProvider for MustNotRun {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![descriptor("set_field")]
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        op: &str,
        _: StorageEntity,
    ) -> Result<OperationResult> {
        panic!("the write-tier gate let '{op}' reach the provider");
    }
}

fn descriptor(op_name: &str) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: op_name.to_string(),
        display_name: op_name.to_string(),
        description: op_name.to_string(),
        required_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Block,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::Test,
        },
        trigger: None,
        bound_params: Default::default(),
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Undeclared,
    }
}

fn recipe_documents() -> Arc<ReadOnlyDocuments> {
    let path = Path::new(RECIPE_PATH);
    let documents = Arc::new(ReadOnlyDocuments::new());
    documents.record(
        &EntityUri::block(RECIPE),
        "cooklang",
        path,
        &ReadOnlyMembers::from_persisted_row(path, vec![EntityUri::block(STEP)])
            .expect("a non-empty membership"),
    );
    documents
}

fn share_backend(
    dir: &TempDir,
    sink: Arc<RecordingSink>,
    documents: Arc<ReadOnlyDocuments>,
) -> Arc<LoroShareBackend> {
    let store = Arc::new(RwLock::new(LoroDocumentStore::new(
        dir.path().to_path_buf(),
    )));
    let bus = Arc::new(DegradedSignalBus::new());
    let snapshot_store = Arc::new(holon_loro::shared_snapshot_store::SharedSnapshotStore::new(
        dir.path().to_path_buf(),
        bus.clone(),
    ));
    let manager = Arc::new(SharedTreeSyncManager::new());
    let key = load_or_create_device_key(dir.path()).expect("device key");
    let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
    LoroShareBackend::new_with_sql(
        store,
        snapshot_store,
        manager,
        advertiser,
        bus,
        key,
        Arc::new(holon_loro::share_credentials::ShareCredentials::in_memory(
            &dir.path().to_string_lossy(),
        )),
        Some(sink as Arc<dyn OriginTaggedWrites>),
        None,
        Some(Arc::new(Tier(documents)) as Arc<dyn WriteTierAuthority>),
    )
}

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), Value::String((*v).to_string())))
        .collect()
}

/// A peer's block lands through the projection worker, and the user's very
/// next edit to it is refused by the dispatcher naming the recipe file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_imported_by_the_share_projection_refuses_the_users_next_edit() {
    let dir = TempDir::new().expect("temp dir");
    let documents = recipe_documents();
    let sink = Arc::new(RecordingSink::default());
    let backend = share_backend(&dir, sink.clone(), documents.clone());

    // The shared doc: its root IS the recipe step, and the peer hung a block
    // under it. Attaching the worker before the commit makes the commit the
    // import the worker projects.
    let shared = Arc::new(LoroDoc::new());
    backend
        .attach_projection_worker(
            "stid-seam".to_string(),
            shared.clone(),
            "block:mount".to_string(),
        )
        .await
        .expect("the projection worker attaches");

    {
        let tree = shared.get_tree(TREE_NAME);
        let root = tree.create(None::<TreeID>).unwrap();
        let root_meta = tree.get_meta(root).unwrap();
        root_meta
            .insert(STABLE_ID, loro::LoroValue::from(STEP))
            .unwrap();
        let root_text: LoroText = root_meta.ensure_mergeable_text("content_raw").unwrap();
        root_text.insert(0, "Crack the eggs").unwrap();

        let child = tree.create(Some(root)).unwrap();
        let child_meta = tree.get_meta(child).unwrap();
        child_meta
            .insert(STABLE_ID, loro::LoroValue::from(IMPORTED))
            .unwrap();
        let child_text: LoroText = child_meta.ensure_mergeable_text("content_raw").unwrap();
        child_text.insert(0, "and whisk them").unwrap();
        shared.commit();
    }

    let imported = EntityUri::block(IMPORTED);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !sink.has(imported.as_str()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the peer's block never reached the SQL sink — a sync import must land, not be refused"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // The other half of the seam: the real dispatcher, the real gate.
    let mut dispatcher = OperationDispatcher::new(vec![Arc::new(MustNotRun)]);
    dispatcher.set_write_tier_authority(Arc::new(Tier(documents)) as Arc<dyn WriteTierAuthority>);

    let refusal = dispatcher
        .execute_operation_with_provenance(
            &EntityName::new("block"),
            "set_field",
            params(&[
                ("id", imported.as_str()),
                ("field", "content"),
                ("value", "TYPED"),
            ]),
            AuthoredInput::Live,
            OpOrigin::User,
        )
        .await
        .expect_err(
            "the user edited a block imported under a recipe step and the dispatcher accepted it \
             — the edit can never reach the recipe file",
        );

    let refusal = refusal.to_string();
    assert!(
        refusal.contains("read-only format"),
        "the refusal must name the tier, got: {refusal}"
    );
    assert!(
        refusal.contains(RECIPE),
        "the refusal must name the file the edit cannot reach, got: {refusal}"
    );
}
