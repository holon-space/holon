//! The render-time link-mark upgrade belongs to the RENDER, not to the read
//! that produced the values.
//!
//! `[[Some Page]]` is stored as a dangling `EntityRef::Name` mark. Once the
//! `block_links` junction resolves it, write-back must emit the ratified
//! `[[<id>][Some Page]]` form. That upgrade used to hang off
//! `CacheBlockReader::get_blocks` / `get_block_authoritative`, which made the
//! rendered BYTES depend on where the values came from — the identical `Block`
//! taken from the block feed (whose matview projects `marks` verbatim) rendered
//! the bare form instead.
//!
//! These tests hand the controller values that are **feed-shaped** — marks left
//! unresolved, exactly as the `home_by` holder will supply them after the
//! Option C cutover — and pin that the file on disk still gets the resolved
//! form, because the renderer applies the seam to whatever slice it renders.

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

/// A block whose only content is a wiki link, stored the way every store
/// stores it: a dangling `Name` target. This is the feed-shaped value.
fn linking_block(id: &str, parent: &EntityUri) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent.clone(), LABEL);
    b.marks = Some(vec![MarkSpan {
        start: 0,
        end: LABEL.len(),
        mark: InlineMark::Link {
            target: EntityRef::Name {
                name: LABEL.to_string(),
            },
            label: LABEL.to_string(),
        },
    }]);
    b
}

/// Stands in for `CacheBlockReader`: the stored marks are dangling, and the
/// junction is consulted only through `resolve_link_marks`.
///
/// `resolves` off models a store that has no resolution for the link (or, for
/// Loro, no junction at all) — the link must then keep rendering bare.
struct JunctionReader {
    doc_id: EntityUri,
    blocks: Mutex<Vec<Block>>,
    resolves: bool,
}

#[async_trait]
impl BlockReader for JunctionReader {
    async fn get_blocks(&self, _: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self.blocks.lock().unwrap().clone())
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

    /// The junction lookup, as `CacheBlockReader` does it: rewrite the TARGET
    /// of every dangling `Name` link that has resolved, leaving the label and
    /// the stored marks alone.
    async fn resolve_link_marks(&self, blocks: &mut [Block]) -> anyhow::Result<()> {
        if !self.resolves {
            return Ok(());
        }
        for b in blocks {
            let Some(marks) = b.marks.as_mut() else {
                continue;
            };
            for span in marks {
                if let InlineMark::Link { target, .. } = &mut span.mark {
                    if matches!(target, EntityRef::Name { name } if name == LABEL) {
                        *target = EntityRef::Scheme {
                            raw: RESOLVED_ID.to_string(),
                        };
                    }
                }
            }
        }
        Ok(())
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
    reader: Arc<JunctionReader>,
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

struct Harness {
    controller: holon_filesystem::FileSyncController,
    doc: Block,
    linker: Block,
    path: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

fn build_harness(resolves: bool) -> Harness {
    let doc_id = EntityUri::block("doc-1");
    let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), "My Document");
    doc.set_page(true);
    let linker = linking_block("b1", &doc_id);

    let reader = Arc::new(JunctionReader {
        doc_id: doc_id.clone(),
        blocks: Mutex::new(vec![linker.clone()]),
        resolves,
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
        doc,
        linker,
        path,
        _tmp: tmp,
    }
}

async fn write_back(h: &mut Harness) -> String {
    let wrote = h
        .controller
        .on_block_changed(&h.doc.id, &BlockDelta::Upsert(h.linker.clone()))
        .await
        .expect("write-back must not fail");
    assert!(wrote, "the doc must resolve to a tracked file");
    std::fs::read_to_string(&h.path).expect("write-back must have produced the file")
}

/// The load-bearing case: values arrive with marks UNRESOLVED (feed-shaped),
/// and the file still gets the ratified form because the renderer — not the
/// read — applies the upgrade.
#[tokio::test]
async fn writeback_emits_the_resolved_form_for_feed_shaped_values() {
    let mut h = build_harness(true);
    let org = write_back(&mut h).await;
    assert!(
        org.contains(&format!("[[{RESOLVED_ID}][{LABEL}]]")),
        "expected the ratified link form in the written org.\n--- file ---\n{org}"
    );
    assert!(
        !org.contains(&format!("[[{LABEL}]]")),
        "the bare form must not survive once the link has resolved.\n--- file ---\n{org}"
    );
}

/// The negative half, so the seam cannot be satisfied by unconditionally
/// rewriting links: a store with nothing to resolve renders the link exactly
/// as authored. This is the documented `LoroBlockReader` behaviour.
#[tokio::test]
async fn an_unresolved_link_still_renders_bare() {
    let mut h = build_harness(false);
    let org = write_back(&mut h).await;
    assert!(
        org.contains(&format!("[[{LABEL}]]")),
        "an unresolved link must render as authored.\n--- file ---\n{org}"
    );
    assert!(
        !org.contains(RESOLVED_ID),
        "nothing may invent a resolution.\n--- file ---\n{org}"
    );
}
