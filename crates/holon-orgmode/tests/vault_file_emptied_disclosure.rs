//! The write-back pump keeps the `vault-file-emptied` condition in step with
//! the documents whose emptied file it refuses: one disclosure per refusal,
//! a fresh one when the refused bytes change, and one all-clear when the
//! refusal lifts.
//!
//! This drives the REAL `FileSyncController` and its write-back net through
//! repeated passes, and counts the disclosure seam's calls.
//!
//! @pbt kind harness
//! @pbt covers vault-file-emptied-disclosure — a refused empty file is
//! disclosed once per refused content and cleared once when the refusal lifts

#![cfg(feature = "di")]

mod org_vault_double;

use holon_api::entity_uri::EntityUri;
use holon_filesystem::writeback_net as wn;
use org_vault_double::DOC_ID;
use org_vault_double::Vault;
use org_vault_double::full_content;
use org_vault_double::vault;

fn count(v: &Vault, prefix: &str) -> usize {
    v.disclosures
        .calls()
        .iter()
        .filter(|c| c.starts_with(prefix))
        .count()
}

/// Empties the file and runs the pass that reads it, so the document's
/// removal is refused.
async fn empty_the_file(v: &mut Vault) {
    let path = v.path.clone();
    std::fs::write(&path, "").unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
}

async fn passes(v: &mut Vault, n: usize) {
    for _ in 0..n {
        v.controller.drive_writeback_net().await.unwrap();
    }
}

async fn ingested_vault() -> Vault {
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    assert_eq!(v.block_ids().len(), 2, "the fixture must land two blocks");
    v
}

#[tokio::test]
async fn a_file_that_stays_empty_is_disclosed_once_across_passes() {
    let mut v = ingested_vault().await;
    empty_the_file(&mut v).await;
    passes(&mut v, 3).await;

    assert_eq!(
        count(&v, "emptied"),
        1,
        "every pass re-raised the condition of a refusal it had already disclosed: {:?}",
        v.disclosures.calls(),
    );
    assert_eq!(
        v.block_ids().len(),
        2,
        "a refused removal must keep the document"
    );
}

/// The disk read the refusal was taken on (`seen`) is part of what was
/// disclosed: a refusal of other bytes is a new fact for the user.
#[tokio::test]
async fn a_refusal_of_other_bytes_is_disclosed_again() {
    let mut v = ingested_vault().await;
    empty_the_file(&mut v).await;
    passes(&mut v, 1).await;
    assert_eq!(count(&v, "emptied"), 1, "{:?}", v.disclosures.calls());

    // A disk read of other bytes, the file emptied again before its ingest:
    // the refusal is taken anew on that read.
    v.net.write(
        wn::ON_DISK,
        EntityUri::block(DOC_ID).as_str(),
        wn::content([("hash", "bytes-read-before-the-file-was-emptied")]),
    );
    passes(&mut v, 3).await;

    assert_eq!(
        count(&v, "emptied"),
        2,
        "a refusal of other bytes must be disclosed exactly once more: {:?}",
        v.disclosures.calls(),
    );
    assert_eq!(
        count(&v, "cleared"),
        0,
        "the document stayed refused, so nothing may clear its condition: {:?}",
        v.disclosures.calls(),
    );
}

#[tokio::test]
async fn a_lifted_refusal_clears_its_condition_once_and_a_new_one_is_disclosed() {
    let mut v = ingested_vault().await;
    empty_the_file(&mut v).await;
    passes(&mut v, 1).await;

    let path = v.path.clone();
    std::fs::write(&path, full_content()).unwrap();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    passes(&mut v, 3).await;

    let cleared = format!("cleared {} vault-file-emptied", path.display());
    let calls = v.disclosures.calls();
    assert_eq!(
        calls.iter().filter(|c| **c == cleared).count(),
        1,
        "the lifted refusal must clear its condition exactly once: {calls:?}",
    );

    empty_the_file(&mut v).await;
    passes(&mut v, 1).await;
    assert_eq!(
        count(&v, "emptied"),
        2,
        "the file emptied after the lift is a new refusal and must be disclosed: {:?}",
        v.disclosures.calls(),
    );
}
