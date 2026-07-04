//! Org write-back emits the user's AUTHORED link bytes, never a resolved form.
//!
//! `[[Some Page]]` is stored as an `EntityRef::Name` mark and reaches disk as
//! `[[Some Page]]` — even when the `block_links` junction has resolved it to a
//! real block. Resolution lives in the junction; the id-rewrite belongs to
//! NAVIGATE (`docs/Explanation/DESIGN_LINKS.md` Phase 2-3), not to write-back.
//! An id-form mark (`[[block:…][Label]]`, equally legal authored input) is
//! likewise rendered exactly as stored.
//!
//! The render therefore takes the marks VERBATIM from the slice it is handed,
//! so the bytes cannot depend on which read produced the values.

#![cfg(feature = "di")]

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as OrderingResult;
use holon_filesystem::BlockDelta;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::RealFileSystem;
use holon_orgmode::file_sync_controller::new_org_sync_controller;

const LABEL: &str = "Linked Page";
const RESOLVED_ID: &str = "block:550e8400-e29b-41d4-a716-446655440000";

/// A block whose only content is a wiki link, stored with the target the user
/// authored — `Name` for `[[Label]]`, `Scheme` for `[[block:…][Label]]`.
fn linking_block(id: &str, parent: &EntityUri, authored: EntityRef) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent.clone(), LABEL);
    b.marks = Some(vec![MarkSpan {
        start: 0,
        end: LABEL.len(),
        mark: InlineMark::Link {
            target: authored,
            label: LABEL.to_string(),
        },
    }]);
    b
}

/// Stands in for `CacheBlockReader`. It has no way to reach the `block_links`
/// junction from here, and that is the point: the render consumes stored marks
/// only, so a store's resolution state cannot enter the file bytes.
struct StoreReader {
    doc_id: EntityUri,
    blocks: Mutex<Vec<Block>>,
}

#[async_trait]
impl BlockReader for StoreReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.blocks.lock().unwrap().clone())
    }

    /// Delegates to `get_blocks`: this double has no cheaper projection.
    /// Never an empty stub — an empty shape would let the write-back
    /// fold-completeness gate PASS on an incomplete document.
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
            .iter()
            .find(|b| b.id == *id)
            .cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(vec![(
            self.doc_id.clone(),
            self.blocks.lock().unwrap().clone(),
        )])
    }
}

struct StubDocManager {
    doc: Block,
}

#[async_trait]
impl DocumentManager for StubDocManager {
    async fn find_by_parent_and_name(
        &self,
        _: &EntityUri,
        _: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok((self.doc.id == *id).then(|| self.doc.clone()))
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
    async fn name_chain(&self, _: &EntityUri) -> anyhow::Result<Vec<String>> {
        Ok(vec!["doc".to_string()])
    }
}

struct LiveOrderOrdering {
    reader: Arc<StoreReader>,
}

#[async_trait]
impl BlockOrdering for LiveOrderOrdering {
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
            .reader
            .blocks
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id.clone())
            .collect())
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> OrderingResult<()> {
        Ok(())
    }
}

/// Previous sibling of `block` in the authoritative sequence: the last block
/// before it that shares its parent, `None` if it is first in its group.
fn prev_sibling(blocks: &[Block], block: &Block) -> Option<EntityUri> {
    blocks
        .iter()
        .take_while(|b| b.id != block.id)
        .filter(|b| b.parent_id == block.parent_id)
        .last()
        .map(|b| b.id.clone())
}

struct Harness {
    controller: holon_filesystem::FileSyncController,
    reader: Arc<StoreReader>,
    doc: Block,
    linker: Block,
    path: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

fn build_harness(authored: EntityRef) -> Harness {
    let doc_id = EntityUri::block("doc-1");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "My Document");
    doc.set_page(true);
    let linker = linking_block("b1", &doc_id, authored);

    let reader = Arc::new(StoreReader {
        doc_id: doc_id.clone(),
        blocks: Mutex::new(vec![linker.clone()]),
    });
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let controller = new_org_sync_controller(
        reader.clone(),
        Arc::new(StubDocManager { doc: doc.clone() }),
        root.clone(),
        Arc::new(LiveOrderOrdering {
            reader: reader.clone(),
        }),
        Arc::new(RealFileSystem),
    );
    let path = holon_core::CanonicalPath::new(&root)
        .into_path_buf()
        .join("doc.org");
    Harness {
        controller,
        reader,
        doc,
        linker,
        path,
        _tmp: tmp,
    }
}

/// Drive one block edit the way production does: the holder is seeded from the
/// authority (production seeds it from the block feed's initial snapshot), then
/// the edit is applied at the position the authority already gives the block.
async fn write_back(h: &mut Harness) -> String {
    h.controller
        .seed_holder_from_authority(&h.doc.id)
        .await
        .expect("seeding the holder must not fail");
    let blocks = h.reader.get_blocks(&h.doc.id).await.unwrap();
    let prev = prev_sibling(&blocks, &h.linker);
    let wrote = h
        .controller
        .on_block_changed(
            &h.doc.id,
            &BlockDelta::Upsert {
                block: h.linker.clone(),
                prev,
            },
        )
        .await
        .expect("write-back must not fail");
    assert!(wrote, "the doc must resolve to a tracked file");
    std::fs::read_to_string(&h.path).expect("write-back must have produced the file")
}

/// A name-form link reaches disk as the user typed it.
#[tokio::test]
async fn a_name_form_link_writes_back_as_authored() {
    let mut h = build_harness(EntityRef::Name {
        name: LABEL.to_string(),
    });
    let org = write_back(&mut h).await;
    assert!(
        org.contains(&format!("[[{LABEL}]]")),
        "write-back must emit the authored name form.\n--- file ---\n{org}"
    );
    assert!(
        !org.contains(RESOLVED_ID),
        "nothing may substitute a resolved id for the authored name.\n--- file ---\n{org}"
    );
}

/// The input side is untouched: an id-form link — legal authored input, and
/// what every file written before this ruling holds — round-trips verbatim.
#[tokio::test]
async fn an_authored_id_link_writes_back_as_authored() {
    let mut h = build_harness(EntityRef::Scheme {
        raw: RESOLVED_ID.to_string(),
    });
    let org = write_back(&mut h).await;
    assert!(
        org.contains(&format!("[[{RESOLVED_ID}][{LABEL}]]")),
        "an authored id link must survive write-back.\n--- file ---\n{org}"
    );
}
