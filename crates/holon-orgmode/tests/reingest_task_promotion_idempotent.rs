//! Re-ingesting an on-disk projection nobody edited must NOT re-derive a task
//! keyword into `task_state` (BugFunnel F4 — silent TODO promotion across
//! restart).
//!
//! A block a user authored as plain text whose content merely STARTS with a
//! task keyword ("TODO buy milk") renders to `* TODO buy milk`. On the next
//! boot the org parser hoists "TODO" into `task_state`, silently promoting
//! persisted plain text to a task — a mutation the user never authored.
//!
//! This drives the REAL `FileSyncController` twice:
//!   1. ingest `* buy milk` → the store holds `milk-block` as plain text;
//!   2. simulate the live-typed (but un-promoted) keyword by editing the stored
//!      content to "TODO buy milk", then re-ingest the byte-identical disk
//!      projection `* TODO buy milk`.
//! The store block must still carry NO `task_state` after step 2.
//!
//! The twin below convicts the reconciler, not the fixture: a GENUINE on-disk
//! keyword edit (the stored content did NOT already carry the keyword) still
//! promotes — org interop preserved.
//!
//! @pbt kind harness
//! @pbt covers reingest-task-promotion-idempotent — a persisted plain block
//!   whose text starts with a task keyword is not silently promoted on
//! re-ingest

#![cfg(feature = "di")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::StorageEntity;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

#[derive(Clone, Default)]
struct FakeStore {
    blocks: Arc<Mutex<HashMap<String, StorageEntity>>>,
    docs: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

impl FakeStore {
    /// The store row whose `id` ends with `bare` (scheme-prefixed on write).
    fn row_of(&self, bare: &str) -> StorageEntity {
        self.blocks
            .lock()
            .unwrap()
            .values()
            .find(|r| row_field(r, "id").ends_with(bare))
            .cloned()
            .unwrap_or_else(|| panic!("no store row for block ending in {bare:?}"))
    }

    fn task_state_of(&self, bare: &str) -> Option<String> {
        self.row_of(bare)
            .get("task_state")
            .and_then(|v| v.as_string().map(str::to_string))
    }

    fn content_of(&self, bare: &str) -> String {
        row_field(&self.row_of(bare), "content").to_string()
    }

    /// Simulate a live edit that put the keyword into the block's TEXT without
    /// promoting it (the shape the live authoring path leaves behind today).
    fn set_content(&self, bare: &str, content: &str) {
        let mut guard = self.blocks.lock().unwrap();
        let key = guard
            .values()
            .find(|r| row_field(r, "id").ends_with(bare))
            .map(|r| row_field(r, "id").to_string())
            .unwrap_or_else(|| panic!("no store row for block ending in {bare:?}"));
        let row = guard.get_mut(&key).unwrap();
        row.insert("content".into(), holon_api::Value::String(content.into()));
    }

    /// Apply HALF A's outcome directly to the store: the stripped content plus
    /// the promoted `task_state` — the two writes
    /// `block.promote_task_keyword` performs.
    fn apply_promotion(&self, bare: &str, promotion: &holon_org_format::Promotion) {
        self.set_content(bare, &promotion.stripped);
        let mut guard = self.blocks.lock().unwrap();
        let key = guard
            .values()
            .find(|r| row_field(r, "id").ends_with(bare))
            .map(|r| row_field(r, "id").to_string())
            .unwrap_or_else(|| panic!("no store row for block ending in {bare:?}"));
        let row = guard.get_mut(&key).unwrap();
        row.insert(
            "task_state".into(),
            holon_api::Value::String(promotion.keyword.keyword.clone()),
        );
        row.insert(
            "task_state_category".into(),
            holon_api::Value::String(promotion.keyword.category.as_str().to_string()),
        );
    }
}

#[async_trait]
impl BlockOrdering for FakeStore {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> OrderingResult<()> {
        Ok(())
    }
    async fn prev_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> OrderingResult<Option<EntityUri>> {
        Ok(None)
    }
    async fn children(&self, parent_id: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .values()
            .map(row_to_block)
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id)
            .collect())
    }
    async fn update_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        self.blocks.lock().unwrap().insert(id, params);
        Ok(())
    }
    async fn delete_in_tree(&self, params: StorageEntity) -> OrderingResult<()> {
        let id = row_field(&params, "id").to_string();
        self.blocks.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[async_trait]
impl DocumentManager for FakeStore {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .docs
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let row = self.blocks.lock().unwrap().get(&id.to_string()).cloned();
        if let Some(row) = row {
            let block = row_to_block(&row);
            if block.is_page() {
                return Ok(Some(block));
            }
        }
        Ok(self
            .docs
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.docs
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

#[async_trait]
impl BlockReader for FakeStore {
    async fn get_blocks(&self, document_uri: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .values()
            .filter(|row| row_doc_uri(row) == *document_uri)
            .map(row_to_block)
            .filter(|b| b.id != *document_uri)
            .collect())
    }
    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }
    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .blocks
            .lock()
            .unwrap()
            .get(&id.to_string())
            .map(row_to_block))
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

fn row_uri(raw: &str) -> EntityUri {
    EntityUri::parse(raw)
        .unwrap_or_else(|e| panic!("store row holds an unparseable uri {raw}: {e}"))
}

fn row_field<'a>(row: &'a StorageEntity, key: &str) -> &'a str {
    row.get(key)
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("store row {row:?} has no `{key}`"))
}

fn row_doc_uri(row: &StorageEntity) -> EntityUri {
    row_uri(row_field(row, holon_api::ROUTING_DOC_URI_KEY))
}

fn row_to_block(row: &StorageEntity) -> Block {
    Block::new_text(
        row_uri(row_field(row, "id")),
        row_uri(row_field(row, "parent_id")),
        row_field(row, "content"),
    )
}

async fn ingest(store: &FakeStore, root: &std::path::Path, source: &str) {
    let path = root.join("Milk.org");
    std::fs::write(&path, source).unwrap();
    let mut controller = new_org_sync_controller(
        Arc::new(store.clone()),
        Arc::new(store.clone()),
        root.to_path_buf(),
        Arc::new(store.clone()),
        Arc::new(RealFileSystem),
    );
    controller
        .on_file_changed(&path)
        .await
        .expect("ingest must succeed");
}

const PLAIN: &str = "\
#+ID: milk-doc
#+TITLE: Milk
* buy milk
:PROPERTIES:
:ID: milk-block
:END:
";

/// The disk projection of a plain block whose TEXT starts with a keyword — the
/// exact bytes our own renderer emits for `content = \"TODO buy milk\"`.
const PROJECTED_WITH_KEYWORD: &str = "\
#+ID: milk-doc
#+TITLE: Milk
* TODO buy milk
:PROPERTIES:
:ID: milk-block
:END:
";

#[tokio::test]
async fn persisted_plain_text_starting_with_todo_is_not_promoted_on_reingest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();

    // 1. Author it plain: the store holds `milk-block` as plain text.
    let store = FakeStore::default();
    ingest(&store, &root, PLAIN).await;
    assert_eq!(store.task_state_of("milk-block"), None, "seeded plain");

    // 2. The live authoring path put the keyword into the block's TEXT without
    //    promoting it (task_state still absent), and write-back rendered the
    //    byte-identical `* TODO buy milk` to disk.
    store.set_content("milk-block", "TODO buy milk");

    // 3. Re-ingest that projection nobody edited.
    ingest(&store, &root, PROJECTED_WITH_KEYWORD).await;

    assert_eq!(
        store.task_state_of("milk-block"),
        None,
        "re-ingesting our own render must NOT silently promote plain text to a task"
    );
    assert_eq!(
        store.content_of("milk-block"),
        "TODO buy milk",
        "the plain text must survive verbatim across the re-ingest"
    );
}

/// Twin: a GENUINE on-disk keyword edit (stored content did NOT carry the
/// keyword) still promotes — org interop is preserved, only the round-trip
/// artifact is suppressed.
#[tokio::test]
async fn a_genuine_on_disk_keyword_edit_still_promotes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();

    let store = FakeStore::default();
    ingest(&store, &root, PLAIN).await;
    assert_eq!(store.task_state_of("milk-block"), None);
    // Stored content stays "buy milk"; the user hand-adds the keyword on disk.
    ingest(&store, &root, PROJECTED_WITH_KEYWORD).await;

    assert_eq!(
        store.task_state_of("milk-block").as_deref(),
        Some("TODO"),
        "a real disk edit that adds a leading keyword must promote to a task"
    );
    assert_eq!(store.content_of("milk-block"), "buy milk");
}

/// G12 — HALF A composed with HALF B is a fixed point. The live promotion
/// strips the keyword into `task_state`; the renderer puts it back on the line;
/// re-ingesting that line must leave the block exactly where the promotion left
/// it — one keyword on disk, one `task_state`, `buy milk` as content — and the
/// detector must refuse to fire a second time on the re-ingested text.
#[tokio::test]
async fn halfa_then_halfb_is_a_fixed_point() {
    use holon_org_format::TaskKeywordVocabulary;
    use holon_org_format::detect_keyword_promotion;

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let vocabulary = TaskKeywordVocabulary::default();

    let store = FakeStore::default();
    ingest(&store, &root, PLAIN).await;
    assert_eq!(store.task_state_of("milk-block"), None, "seeded plain");

    // HALF A: the author prepends the keyword; the detector fires once.
    let promotion = detect_keyword_promotion("buy milk", None, "TODO buy milk", &vocabulary)
        .expect("HALF A must promote the authoring gesture");
    store.apply_promotion("milk-block", &promotion);
    assert_eq!(store.content_of("milk-block"), "buy milk");

    // The renderer re-emits `keyword + ' ' + content`; re-ingest that render.
    ingest(&store, &root, PROJECTED_WITH_KEYWORD).await;

    assert_eq!(
        store.task_state_of("milk-block").as_deref(),
        Some("TODO"),
        "the promoted task must survive the round trip"
    );
    assert_eq!(
        store.content_of("milk-block"),
        "buy milk",
        "the keyword must NOT be re-absorbed into the content"
    );

    // And the detector itself is at rest: the block now carries a task_state,
    // so no further keystroke can re-promote it.
    assert_eq!(
        detect_keyword_promotion(
            "buy milk",
            Some(&promotion.keyword),
            "TODO buy milk",
            &vocabulary
        ),
        None,
        "a promoted block must never re-promote"
    );
}
