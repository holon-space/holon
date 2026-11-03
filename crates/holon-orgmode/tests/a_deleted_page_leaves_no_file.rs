//! A page Holon deletes leaves no file behind, whichever writer created its
//! file, in whatever order the deletion's changes arrive, and while the page
//! registry still lists the deleted page. A page that is not deleted keeps its
//! file.
//!
//! Production's page registry is a view with its own CDC stream, apart from
//! the block feed that drives write-back, so it can still list a page the
//! store has already deleted.

use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::MapAliasRegistrar;
use org_vault_double::Vault;
use org_vault_double::vault;
use proptest::prelude::*;

use crate::org_vault_double;

#[derive(Clone, Copy, Debug)]
enum Writer {
    /// A change of a block inside the page renders the page's file.
    Render,
    /// An upsert of the page block itself runs the identity-file materializer.
    Materialize,
}

#[derive(Clone, Copy, Debug)]
enum Deletion {
    /// The undo of "turn into page": the page goes, its block returns to the
    /// source document.
    UndoConvert,
    /// The page and its block both go.
    Delete,
}

#[derive(Clone, Debug)]
struct Case {
    creation: Vec<Writer>,
    deletion: Deletion,
    /// The order the deletion's changes reach the controller, as indexes into
    /// `deletion_changes`.
    order: Vec<usize>,
    /// How many of those changes reach the controller while the registry
    /// still lists the deleted page.
    registry_lag: usize,
}

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

/// The changes a deletion sends, each with the document it is homed in.
fn deletion_changes(minted: &MintedPage, deletion: Deletion) -> Vec<(EntityUri, BlockDelta)> {
    let page = &minted.page.id;
    let mut changes = vec![
        (page.clone(), BlockDelta::Remove(minted.kid.id.clone())),
        (page.clone(), BlockDelta::Remove(page.clone())),
    ];
    if let Deletion::UndoConvert = deletion {
        changes.push((EntityUri::block(DOC_ID), upsert(&kid_back_home(minted))));
    }
    changes
}

fn kid_back_home(minted: &MintedPage) -> Block {
    let mut back = minted.kid.clone();
    back.parent_id = EntityUri::block(DOC_ID);
    back
}

fn upsert(block: &Block) -> BlockDelta {
    BlockDelta::Upsert {
        block: block.clone(),
        prev: None,
    }
}

async fn change(v: &mut Vault, doc: &EntityUri, delta: &BlockDelta) -> bool {
    v.controller
        .on_block_changed(doc, delta)
        .await
        .unwrap_or_else(|e| panic!("on_block_changed({doc}): {e:#}"))
}

async fn create(v: &mut Vault, minted: &MintedPage, creation: &[Writer]) {
    v.put(minted.page.clone());
    v.put(minted.kid.clone());
    for writer in creation {
        let block = match writer {
            Writer::Render => &minted.kid,
            Writer::Materialize => &minted.page,
        };
        change(v, &minted.page.id, &upsert(block)).await;
    }
}

/// Deletes the page in the store at once and in the registry after
/// `case.registry_lag` changes. Returns whether any change asked for the bulk
/// pass.
async fn delete(v: &mut Vault, minted: &MintedPage, case: &Case) -> bool {
    let page = &minted.page.id;
    let listed = v.docs.by_id.lock().unwrap()[page].clone();
    v.forget(page);
    match case.deletion {
        Deletion::UndoConvert => v.put(kid_back_home(minted)),
        Deletion::Delete => v.forget(&minted.kid.id),
    }
    v.docs.by_id.lock().unwrap().insert(page.clone(), listed);

    let changes = deletion_changes(minted, case.deletion);
    let mut handled = true;
    for (sent, &i) in case.order.iter().enumerate() {
        if sent == case.registry_lag {
            v.docs.by_id.lock().unwrap().remove(page);
        }
        let (doc, delta) = &changes[i];
        handled &= change(v, doc, delta).await;
    }
    v.docs.by_id.lock().unwrap().remove(page);
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

/// What the case left on disk that it should not have, or `None`.
async fn run(case: &Case) -> Option<String> {
    let mut v = vault().with_alias_registrar(Arc::new(MapAliasRegistrar::default()));
    let notes = v.path.clone();
    let _ = v.controller.on_file_changed(&notes).await.unwrap();

    let doomed = MintedPage::new("20261102T100000", "20261102T100001", "Origin");
    let keeper = MintedPage::new("20261102T110000", "20261102T110001", "Keeper");
    create(&mut v, &doomed, &case.creation).await;
    create(&mut v, &keeper, &case.creation).await;
    let before = files_naming(&v, &doomed.page.id);
    assert_eq!(before.len(), 1, "the new page owns one file: {before:?}");

    if delete(&mut v, &doomed, case).await {
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

fn check(case: &Case) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Some(failure) = rt.block_on(run(case)) {
        panic!("{case:?}: {failure}");
    }
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let writer = prop_oneof![Just(Writer::Render), Just(Writer::Materialize)];
    let deletion = prop_oneof![Just(Deletion::UndoConvert), Just(Deletion::Delete)];
    (prop::collection::vec(writer, 1..=3), deletion).prop_flat_map(|(creation, deletion)| {
        // A delete removes leaves before their parent
        // (holon-loro loro_sync_controller.rs, "Delete leaves first"); a page
        // removed before its block is ungrounded and the removal guard refuses it.
        let (n, order) = match deletion {
            Deletion::UndoConvert => (3usize, Just(vec![0, 1, 2]).prop_shuffle().boxed()),
            Deletion::Delete => (2, Just(vec![0, 1]).boxed()),
        };
        (order, 0..=n).prop_map(move |(order, registry_lag)| Case {
            creation: creation.clone(),
            deletion,
            order,
            registry_lag,
        })
    })
}

/// Cases kept from earlier failures, run through the same property first.
fn fixed_cases() -> Vec<Case> {
    let in_order = |creation: Vec<Writer>, deletion, n, registry_lag| Case {
        creation,
        deletion,
        order: (0..n).collect(),
        registry_lag,
    };
    vec![
        // The render path wrote the page's file without recording its alias.
        in_order(vec![Writer::Render], Deletion::UndoConvert, 3, 0),
        in_order(vec![Writer::Render], Deletion::Delete, 2, 0),
        // The registry still listed the deleted page when its removal was
        // rendered, so the file kept the page's header.
        in_order(vec![Writer::Materialize], Deletion::UndoConvert, 3, 3),
        in_order(vec![Writer::Render], Deletion::UndoConvert, 3, 3),
    ]
}

#[test]
fn a_deleted_page_leaves_no_file_fixed_cases() {
    for case in fixed_cases() {
        check(&case);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_deleted_page_leaves_no_file_and_a_live_page_keeps_its_file(case in case_strategy()) {
        check(&case);
    }
}
