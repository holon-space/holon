//! A page Holon deletes leaves no file behind, whichever writer created its
//! file: the render of the page's blocks, or the page's identity-file
//! materializer. A page that is not deleted keeps its file.
//!
//! The inputs are the product of two small sets (four creation orders, two
//! deletions), so every case runs instead of a sample.

use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::MapAliasRegistrar;
use org_vault_double::Vault;
use org_vault_double::vault;

use crate::org_vault_double;

#[derive(Clone, Copy, Debug)]
enum Writer {
    /// A change of a block inside the page renders the page's file.
    Render,
    /// An upsert of the page block itself runs the identity-file materializer.
    Materialize,
}

const CREATIONS: [&[Writer]; 4] = [
    &[Writer::Render],
    &[Writer::Materialize],
    &[Writer::Render, Writer::Materialize],
    &[Writer::Materialize, Writer::Render],
];

#[derive(Clone, Copy, Debug)]
enum Deletion {
    /// The undo of "turn into page": the page goes, its block returns to the
    /// source document.
    UndoConvert,
    /// The page and its block both go.
    Delete,
}

const DELETIONS: [Deletion; 2] = [Deletion::UndoConvert, Deletion::Delete];

struct MintedPage {
    page: Block,
    kid: Block,
}

impl MintedPage {
    fn new(id: &str, kid_id: &str, title: &str) -> Self {
        let mut page = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), title);
        page.set_page(true);
        let kid = Block::new_text(EntityUri::block(kid_id), page.id.clone(), "a task");
        Self { page, kid }
    }
}

async fn change(v: &mut Vault, doc: &EntityUri, delta: BlockDelta) -> bool {
    v.controller
        .on_block_changed(doc, &delta)
        .await
        .unwrap_or_else(|e| panic!("on_block_changed({doc}): {e:#}"))
}

fn upsert(block: &Block) -> BlockDelta {
    BlockDelta::Upsert {
        block: block.clone(),
        prev: None,
    }
}

async fn create(v: &mut Vault, minted: &MintedPage, creation: &[Writer]) {
    v.put(minted.page.clone());
    v.put(minted.kid.clone());
    for writer in creation {
        let block = match writer {
            Writer::Render => &minted.kid,
            Writer::Materialize => &minted.page,
        };
        change(v, &minted.page.id, upsert(block)).await;
    }
}

/// Returns whether any change asked for the bulk pass.
async fn delete(v: &mut Vault, minted: &MintedPage, deletion: Deletion) -> bool {
    let page = &minted.page.id;
    let mut handled = true;
    v.forget(page);
    match deletion {
        Deletion::UndoConvert => {
            let mut back = minted.kid.clone();
            back.parent_id = EntityUri::block(DOC_ID);
            v.put(back.clone());
            handled &= change(v, page, BlockDelta::Remove(minted.kid.id.clone())).await;
            handled &= change(v, &EntityUri::block(DOC_ID), upsert(&back)).await;
        }
        Deletion::Delete => {
            v.forget(&minted.kid.id);
            handled &= change(v, page, BlockDelta::Remove(minted.kid.id.clone())).await;
        }
    }
    handled &= change(v, page, BlockDelta::Remove(page.clone())).await;
    !handled
}

/// The vault files that name `page` in their bytes. `.holon/` holds
/// content-addressed merge bases, which a sweep removes, not page files.
fn files_naming(v: &Vault, page: &EntityUri) -> Vec<String> {
    let root = v.path.parent().unwrap();
    let mut found = Vec::new();
    let mut open = vec![root.to_path_buf()];
    while let Some(dir) = open.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().unwrap() != ".holon" {
                    open.push(path);
                }
            } else if std::fs::read_to_string(&path).unwrap().contains(page.id()) {
                found.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    found
}

/// What one case left on disk that it should not have, or `None`.
async fn run(creation: &[Writer], deletion: Deletion) -> Option<String> {
    let mut v = vault().with_alias_registrar(Arc::new(MapAliasRegistrar::default()));
    let notes = v.path.clone();
    let _ = v.controller.on_file_changed(&notes).await.unwrap();

    let doomed = MintedPage::new("20261102T100000", "20261102T100001", "Origin");
    let keeper = MintedPage::new("20261102T110000", "20261102T110001", "Keeper");
    create(&mut v, &doomed, creation).await;
    create(&mut v, &keeper, creation).await;
    let before = files_naming(&v, &doomed.page.id);
    assert_eq!(before.len(), 1, "the new page owns one file: {before:?}");

    if delete(&mut v, &doomed, deletion).await {
        v.controller
            .re_render_all_tracked(&Default::default())
            .await
            .unwrap_or_else(|e| panic!("bulk pass: {e:#}"));
    }

    let doomed_files = files_naming(&v, &doomed.page.id);
    let keeper_files = files_naming(&v, &keeper.page.id);
    (!doomed_files.is_empty() || keeper_files.len() != 1)
        .then(|| format!("deleted page still in {doomed_files:?}; live page in {keeper_files:?}"))
}

#[tokio::test]
async fn a_deleted_page_leaves_no_file_and_a_live_page_keeps_its_file() {
    let mut failures = Vec::new();
    for creation in CREATIONS {
        for deletion in DELETIONS {
            if let Some(failure) = run(creation, deletion).await {
                failures.push(format!(
                    "created by {creation:?}, removed by {deletion:?}: {failure}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
