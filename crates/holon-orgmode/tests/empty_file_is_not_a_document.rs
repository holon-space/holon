//! A 0-byte org file is an atomic-save intermediate, never a document.
//!
//! Editors (and every `write to temp + rename` saver) make the final path
//! observable at zero length for a moment before the real bytes land. The
//! watcher sees that moment as a content change like any other, and parsing it
//! yields a document with no blocks — which the ingest diff turns into a delete
//! of every block the document had. The bytes that arrive a millisecond later
//! then have to re-create everything, and anything derived from the store in
//! between saw the document blank.
//!
//! This drives the REAL `FileSyncController::on_file_changed` with the REAL org
//! adapter, and its store doubles APPLY deletes — a double that swallows
//! `delete_in_tree` cannot see the clobber this file is about.
//!
//! @pbt kind harness
//! @pbt covers empty-doc-skip-watcher — a 0-byte file neither ingests as an
//! empty document nor clobbers the blocks of the document that lives at its
//! path

#![cfg(feature = "di")]

use org_vault_double::full_content;
use org_vault_double::vault;

use crate::org_vault_double;

/// THE regression: the 0-byte intermediate of an atomic save must leave the
/// document exactly as it was, and the content that lands next must ingest
/// normally.
#[tokio::test]
async fn a_zero_byte_save_intermediate_leaves_the_documents_blocks_alone() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("the document ingests");
    let before = v.block_ids();
    assert_eq!(
        before.len(),
        2,
        "the fixture must land two blocks before the empty-file step, else this test proves \
         nothing: {before:?}",
    );

    // The atomic save's first observable state.
    std::fs::write(&path, "").unwrap();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("a 0-byte file must not be an ingest error");

    assert_eq!(
        v.block_ids(),
        before,
        "a 0-byte file was ingested as an empty document and deleted the blocks that live at \
         its path — the save's real bytes had not even landed yet",
    );

    // The bytes the editor was actually writing.
    std::fs::write(&path, full_content()).unwrap();
    let _ = v
        .controller
        .on_file_changed(&path)
        .await
        .expect("the completed save ingests");
    assert_eq!(
        v.block_ids(),
        before,
        "the completed save must restore the document to exactly what it held",
    );
}

/// The skip must not disguise itself as a write-back opportunity: nothing may
/// rewrite the file while it is empty, because the writer that emptied it is
/// still mid-save.
#[tokio::test]
async fn a_zero_byte_file_is_not_written_back() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "",
        "Holon rewrote a file whose save was still in flight — the bytes the editor is about \
         to write would land on top of a projection nobody asked for",
    );
}

/// A whitespace-only file is a file someone WROTE. Only `is_empty()` may
/// refuse: an atomic save's intermediate is exactly zero-length, and widening
/// the guard to `trim().is_empty()` would silently drop a real, if degenerate,
/// edit — and with it the document that file creates.
#[tokio::test]
async fn a_whitespace_only_file_is_still_ingested() {
    let mut v = vault();
    let scratch = v.path.parent().unwrap().join("Scratch.org");
    std::fs::write(&scratch, "\n   \n").unwrap();

    let _ = v
        .controller
        .on_file_changed(&scratch)
        .await
        .expect("a whitespace-only file ingests like any other");

    let titles: Vec<String> = v
        .docs
        .by_id
        .lock()
        .unwrap()
        .values()
        .map(|d| d.title().to_string())
        .collect();
    assert!(
        titles.iter().any(|t| t == "Scratch"),
        "a whitespace-only file was refused, so the document it names never came into being — \
         only a ZERO-length file is a save in flight; titles were {titles:?}",
    );
}

/// An empty file is a file a user emptied. Holon keeps the document it last
/// read — an empty file names no document, so nothing in it can authorize
/// deleting one — which leaves the app showing content the file no longer has.
/// That divergence must reach the user, not just the log.
#[tokio::test]
async fn a_file_that_stays_empty_is_disclosed_as_degraded() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    let before = v.block_ids();
    assert_eq!(before.len(), 2, "fixture must land two blocks: {before:?}");

    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    v.controller.poll_new_files().await.unwrap();
    v.controller.poll_new_files().await.unwrap();

    let emptied: Vec<String> = v
        .disclosures
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("emptied"))
        .collect();
    assert_eq!(
        emptied.len(),
        1,
        "a file the user emptied must raise its degraded condition exactly once, naming the \
         file — silently keeping a document whose file is empty is the divergence nobody can \
         see; calls were {:?}",
        v.disclosures.calls(),
    );
    assert!(emptied[0].contains("Notes.org"), "{emptied:?}");
    assert_eq!(
        v.block_ids(),
        before,
        "the document must be KEPT: an empty file identifies no document, so nothing in it can \
         authorize deleting one",
    );

    // The all-clear: content is back, so the condition it raised is lifted.
    std::fs::write(&path, full_content()).unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    assert!(
        v.disclosures
            .calls()
            .iter()
            .any(|c| c.starts_with("recovered")),
        "the emptiness condition is sticky and nothing lifted it: {:?}",
        v.disclosures.calls(),
    );
}

/// The other half of the 0-byte story, and the one rev 1 broke: a title-less
/// page with no blocks RENDERS to zero bytes — `render_document_header`
/// deliberately emits no synthetic `#+TITLE:` for a doc-root whose name its
/// filename carries — so a 0-byte `.org` file is that page's canonical on-disk
/// form. Refusing every 0-byte file means such a page can never be created from
/// disk and never comes back after a restart.
///
/// The refusal protects the blocks of the document at the path. A path Holon
/// holds no content for has none, so there is nothing there to protect.
#[tokio::test]
async fn a_new_zero_byte_file_becomes_its_page() {
    let mut v = vault();
    let fresh = v.path.parent().unwrap().join("Fresh.org");
    std::fs::write(&fresh, "").unwrap();

    let _ = v
        .controller
        .on_file_changed(&fresh)
        .await
        .expect("a new 0-byte file must not be an ingest error");

    let titles: Vec<String> = v
        .docs
        .by_id
        .lock()
        .unwrap()
        .values()
        .map(|d| d.title().to_string())
        .collect();
    assert!(
        titles.iter().any(|t| t == "Fresh"),
        "a 0-byte file at a path Holon holds nothing for was refused, so the page it IS never \
         came into being — and that file is exactly what an empty title-less page renders to; \
         titles were {titles:?}",
    );
}

/// ANTI-OVERCORRECTION: scoping the refusal must not reopen the clobber for the
/// path the refusal exists for — a file Holon HAS content for.
#[tokio::test]
async fn a_zero_byte_file_at_a_path_with_content_is_still_refused() {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    let before = v.block_ids();
    assert_eq!(before.len(), 2, "fixture must land two blocks: {before:?}");

    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();

    assert_eq!(
        v.block_ids(),
        before,
        "the save-in-flight clobber is back: a 0-byte read deleted the blocks of a document \
         Holon holds content for",
    );
}
