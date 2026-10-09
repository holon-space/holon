//! A block edited in the app and, before the write-back, in its org file
//! merges both edits: text the app deleted stays deleted.
//!
//! Direct mode (no Loro store): the file-sync controller 3-way merges the
//! block's text with the merger the app wires (`TransientLoroTextMerge`).
//!
//! @pbt kind harness
//! @pbt covers file-sync-3way-text-merge — an external org edit and an in-app
//! edit of one block merge without resurrecting deleted text

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::consolidator::Seen;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::IngestOutcome;
use holon_filesystem::RealFileSystem;
use holon_loro::TransientLoroTextMerge;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const DOC_ID: &str = "20261009T090000";
const ALPHA: &str = "20261009T090001";

type Store = Arc<Mutex<HashMap<EntityUri, Block>>>;

/// The SQL store of Direct mode: it is the consolidator, and an ingest's
/// content update lands in it.
#[derive(Clone)]
struct StoreTree {
    store: Store,
}

#[async_trait]
impl BlockOrdering for StoreTree {
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
    async fn children(&self, parent: &EntityUri) -> OrderingResult<Vec<EntityUri>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.parent_id == *parent)
            .map(|b| b.id.clone())
            .collect())
    }
    async fn ever_seen(&self, _: &EntityUri) -> OrderingResult<Seen> {
        Ok(Seen::Live)
    }
    fn has_upstream_consolidator(&self) -> bool {
        false
    }
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("every ingest op names the block it acts on");
        let id = EntityUri::parse(id).expect("the ingest names blocks by uri");
        let mut store = self.store.lock().unwrap();
        let block = store
            .get_mut(&id)
            .unwrap_or_else(|| panic!("the ingest updates {id}, which the store does not hold"));
        if let Some(text) = params.get("content").and_then(|v| v.as_string()) {
            block.content = text.to_string();
        }
        Ok(())
    }
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> OrderingResult<()> {
        panic!("this ingest deletes nothing, got delete_in_tree({params:?})");
    }
}

struct StoreReader(Store);

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|b| b.id != *doc_id)
            .cloned()
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
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Default)]
struct PageStore {
    by_id: Arc<Mutex<HashMap<EntityUri, Block>>>,
}

#[async_trait]
impl DocumentManager for PageStore {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self
            .by_id
            .lock()
            .unwrap()
            .get(id)
            .filter(|b| b.is_page())
            .cloned())
    }
    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        self.by_id
            .lock()
            .unwrap()
            .insert(doc.id.clone(), doc.clone());
        Ok(())
    }
}

fn org_file(alpha: &str) -> String {
    format!(
        ":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n\
         * {alpha}\n:PROPERTIES:\n:ID: {ALPHA}\n:END:\n"
    )
}

#[tokio::test]
async fn a_file_edit_does_not_bring_back_text_the_app_deleted() {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), "abcdef"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, org_file("abcdef")).unwrap();

    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(
        store.lock().unwrap()[&alpha].content,
        "abcdef",
        "first ingest: {first:?}"
    );

    // The app deletes "cde"; before the write-back, an editor replaces "d"
    // with "ABC" and inserts "D" before "f".
    store.lock().unwrap().get_mut(&alpha).unwrap().content = "abf".to_string();
    std::fs::write(&path, org_file("abcABCeDf")).unwrap();
    let outcome = controller.on_file_changed(&path).await;

    assert_eq!(
        store.lock().unwrap()[&alpha].content,
        "abABCDf",
        "the merge of the app's delete of \"cde\" and the file's edit must keep \
         \"cde\" deleted and keep the file's \"ABC\" and \"D\" (ingest outcome {outcome:?})"
    );
}

/// The text of `ALPHA` that an overruled file edit discloses, and Holon's
/// change that overrules it.
fn overruled_text(bus: &holon_api::ConditionBus) -> Option<(String, holon_api::HolonChange)> {
    bus.current().into_iter().find_map(|c| match c.reason {
        holon_api::ConditionKind::FileEditOverruled {
            file_text, change, ..
        } if c.subject == EntityUri::block(ALPHA).as_str() => Some((file_text, change)),
        _ => None,
    })
}

#[tokio::test]
async fn edits_too_large_to_merge_keep_both_texts_and_disclose_the_files() {
    let doc = EntityUri::block(DOC_ID);
    let alpha = EntityUri::block(ALPHA);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let base: String = (0..20_000)
        .map(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            b"abcd"[(seed >> 62) as usize] as char
        })
        .collect();
    let mut page = Block::new_text(doc.clone(), EntityUri::no_parent(), "Notes");
    page.set_page(true);
    let docs = PageStore::default();
    docs.by_id.lock().unwrap().insert(doc.clone(), page);
    let store: Store = Arc::default();
    store.lock().unwrap().insert(
        alpha.clone(),
        Block::new_text(alpha.clone(), doc.clone(), base.clone()),
    );

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Notes.org");
    std::fs::write(&path, org_file(&base)).unwrap();

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut controller = new_org_sync_controller(
        Arc::new(StoreReader(store.clone())),
        Arc::new(docs),
        root,
        Arc::new(StoreTree {
            store: store.clone(),
        }),
        Arc::new(RealFileSystem),
    )
    .with_text_merge(Arc::new(TransientLoroTextMerge::default()))
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    let first = controller
        .on_file_changed(&path)
        .await
        .expect("first ingest of the file");
    assert_eq!(first, IngestOutcome::Ingested);

    // The app deletes one char; before the write-back, an editor reverses the
    // whole line.
    let mine = format!("{}{}", &base[..100], &base[101..]);
    let theirs: String = base.chars().rev().collect();
    store.lock().unwrap().get_mut(&alpha).unwrap().content = mine.clone();
    std::fs::write(&path, org_file(&theirs)).unwrap();
    let outcome = controller.on_file_changed(&path).await;

    assert!(
        store.lock().unwrap()[&alpha].content == mine,
        "the app's text must stand (ingest outcome {outcome:?})"
    );
    let (disclosed, change) =
        overruled_text(&bus).expect("the file's edit is disclosed as overruled");
    assert_eq!(change, holon_api::HolonChange::Edited);
    assert!(
        disclosed.contains(&theirs),
        "the disclosure must hold the file's text whole"
    );
}
