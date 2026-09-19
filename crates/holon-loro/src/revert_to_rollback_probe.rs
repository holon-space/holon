//! Probe: can `LoroDoc::revert_to(&Frontiers)` serve as the rollback a failed
//! multi-op block batch needs?
//!
//! The `dense_patch` all-or-nothing work (bugfunnel
//! `2026-09-17-dense-patch-apply-is-not-atomic`) found no transactional write
//! seam and shipped a loud partial-apply report instead. `revert_to` is the
//! candidate that would replace the report with a real undo. It exists in the
//! pinned loro (1.13.9, rev `6f5b2d7e`, `crates/loro/src/lib.rs:1469`) and has
//! ZERO uses in Holon, so its behaviour on OUR shape — the block tree, not a
//! text container — is unmeasured. This file measures it.
//!
//! What `revert_to` says about itself: it generates a series of LOCAL ops that
//! carry the doc back, so it is COMPENSATING, not a transaction abort; and
//! "if the document is shallow and the target is before the shallow start,
//! revert will fail".
//!
//! Each test below names its own verdict.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::BlockContent;
use holon_api::EntityUri;
use loro::LoroDoc;
use loro::TreeParentId;

use crate::LoroDocument;
use crate::loro_backend::LoroBackend;
use crate::loro_backend::TREE_NAME;

/// Ids of the live nodes, in tree order.
fn live_ids(doc: &LoroDoc) -> Vec<String> {
    let tree = doc.get_tree(TREE_NAME);
    let mut out = Vec::new();
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        let Ok(meta) = tree.get_meta(node.id) else {
            continue;
        };
        if let Some(v) = meta.get(crate::loro_backend::STABLE_ID) {
            out.push(format!("{v:?}"));
        }
    }
    out
}

async fn create(backend: &LoroBackend, parent: EntityUri, id: &str) {
    backend
        .create_block_with_properties(
            parent,
            BlockContent::text(id),
            Some(EntityUri::block(id)),
            &HashMap::new(),
            &holon_api::BlockEdges::default(),
        )
        .await
        .unwrap();
}

/// VERDICT 1 — `revert_to` DOES undo a partly-applied block batch.
///
/// The `dense_patch` shape exactly: op 1 creates, op 2 fails, and the frontier
/// captured before the batch is handed back to `revert_to`.
#[tokio::test]
async fn revert_to_undoes_the_creates_a_failed_batch_left_behind() {
    let doc = Arc::new(LoroDocument::new("revert-probe".to_string()).unwrap());
    let backend = LoroBackend::from_document(doc.clone());
    create(&backend, EntityUri::no_parent(), "root").await;

    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw = doc.doc();
    raw.commit();
    let before_batch = raw.oplog_frontiers();
    let base = live_ids(&raw);

    // The batch: op 1 lands, op 2 is where the engine would fail.
    create(&backend, EntityUri::block("root"), "op1").await;
    raw.commit();
    let mid = live_ids(&raw);
    assert_eq!(
        mid.len(),
        base.len() + 1,
        "op 1 must be in the doc before the revert: {mid:?}"
    );

    raw.revert_to(&before_batch).expect("revert must succeed");
    raw.commit();

    assert_eq!(
        live_ids(&raw),
        base,
        "revert_to must restore the pre-batch node set"
    );
}

/// VERDICT 2 — the revert also closes the pending-op leak that
/// `with_write_is_isolation_not_rollback` documents: a later unrelated write
/// does NOT resurrect the reverted create.
#[tokio::test]
async fn a_later_write_does_not_resurrect_the_reverted_batch() {
    let doc = Arc::new(LoroDocument::new("revert-leak".to_string()).unwrap());
    let backend = LoroBackend::from_document(doc.clone());
    create(&backend, EntityUri::no_parent(), "root").await;

    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw = doc.doc();
    raw.commit();
    let before_batch = raw.oplog_frontiers();

    create(&backend, EntityUri::block("root"), "op1").await;
    raw.commit();
    raw.revert_to(&before_batch).expect("revert must succeed");
    raw.commit();

    create(&backend, EntityUri::block("root"), "later").await;
    raw.commit();

    let ids = live_ids(&raw);
    assert!(
        !ids.iter().any(|i| i.contains("op1")),
        "the reverted create must stay gone after an unrelated write: {ids:?}"
    );
    assert!(
        ids.iter().any(|i| i.contains("later")),
        "the unrelated write must survive: {ids:?}"
    );
}

/// VERDICT 3 — THE CAVEAT THAT DECIDES THE DESIGN. `revert_to` is a
/// whole-document rewind, so a REMOTE op that landed inside the batch window
/// is reverted with ours. Blind rollback would silently drop a peer's write.
#[tokio::test]
async fn a_remote_op_inside_the_batch_window_is_reverted_too() {
    let doc_a = Arc::new(LoroDocument::new("revert-remote-a".to_string()).unwrap());
    let backend_a = LoroBackend::from_document(doc_a.clone());
    create(&backend_a, EntityUri::no_parent(), "root").await;

    let doc_b = Arc::new(LoroDocument::new("revert-remote-b".to_string()).unwrap());
    let backend_b = LoroBackend::from_document(doc_b.clone());
    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw_a = doc_a.doc();
    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw_b = doc_b.doc();

    raw_a.commit();
    raw_b
        .import(
            &raw_a
                .export(loro::ExportMode::updates(&raw_b.oplog_vv()))
                .unwrap(),
        )
        .unwrap();

    let before_batch = raw_a.oplog_frontiers();

    // Our op 1 lands...
    create(&backend_a, EntityUri::block("root"), "ours").await;
    raw_a.commit();
    // ...and a peer's op arrives before our op 2 fails.
    create(&backend_b, EntityUri::block("root"), "theirs").await;
    raw_b.commit();
    raw_a
        .import(
            &raw_b
                .export(loro::ExportMode::updates(&raw_a.oplog_vv()))
                .unwrap(),
        )
        .unwrap();
    raw_a.commit();

    let before_revert = live_ids(&raw_a);
    assert!(
        before_revert.iter().any(|i| i.contains("theirs")),
        "the peer's block must be present before the revert: {before_revert:?}"
    );

    raw_a.revert_to(&before_batch).expect("revert must succeed");
    raw_a.commit();

    let after = live_ids(&raw_a);
    println!("PROBE remote-op revert: before={before_revert:?} after={after:?}");
    assert!(
        !after.iter().any(|i| i.contains("ours")),
        "our op must be reverted: {after:?}"
    );
    // Whichever way this lands, it is the load-bearing fact for the design.
    let peer_survived = after.iter().any(|i| i.contains("theirs"));
    println!("PROBE peer write survived the revert: {peer_survived}");
    assert!(
        !peer_survived,
        "MEASURED: revert_to to a pre-batch frontier also reverts a concurrent \
         REMOTE op — rollback must therefore refuse when the doc advanced from \
         a peer inside the window. after={after:?}"
    );
}

/// VERDICT 4 — the shallow-snapshot boundary. `revert_to` refuses when the
/// target predates the trim, which is the second condition a rollback path
/// must detect rather than assume away.
#[tokio::test]
async fn revert_across_a_shallow_snapshot_boundary_fails() {
    let doc = Arc::new(LoroDocument::new("revert-shallow".to_string()).unwrap());
    let backend = LoroBackend::from_document(doc.clone());
    create(&backend, EntityUri::no_parent(), "root").await;
    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw = doc.doc();
    raw.commit();
    let before_batch = raw.oplog_frontiers();

    create(&backend, EntityUri::block("root"), "op1").await;
    raw.commit();

    // Trim history at the CURRENT frontier, so `before_batch` is older than
    // the shallow start — the exact situation `export_compact_snapshot`
    // creates in production.
    let snapshot = raw
        .export(loro::ExportMode::shallow_snapshot(&raw.oplog_frontiers()))
        .unwrap();
    let trimmed = LoroDoc::new();
    trimmed.import(&snapshot).unwrap();

    let outcome = trimmed.revert_to(&before_batch);
    println!("PROBE shallow revert outcome: {outcome:?}");
    assert!(
        outcome.is_err(),
        "revert across a shallow boundary must fail loudly, not silently no-op: {outcome:?}"
    );
}

/// The guard VERDICT 3 forces on any rollback path: a revert is safe only
/// while the doc has not advanced from a peer other than us.
///
/// Checked against the oplog version vector rather than the frontiers because
/// only the vector says WHOSE ops arrived.
fn foreign_peer_advanced(doc: &LoroDoc, before: &loro::VersionVector, ours: loro::PeerID) -> bool {
    doc.oplog_vv()
        .iter()
        .any(|(peer, count)| *peer != ours && before.get(peer).copied().unwrap_or(0) < *count)
}

/// VERDICT 5 — the guard separates the two cases the rollback path must tell
/// apart, so "refuse when a peer wrote" is implementable and not a hope.
#[tokio::test]
async fn the_guard_admits_a_local_only_window_and_refuses_a_peer_touched_one() {
    let doc_a = Arc::new(LoroDocument::new("guard-a".to_string()).unwrap());
    let ours = doc_a.peer_id();
    let backend_a = LoroBackend::from_document(doc_a.clone());
    create(&backend_a, EntityUri::no_parent(), "root").await;

    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw_a = doc_a.doc();
    raw_a.commit();

    // (a) local-only window → admitted.
    let before_vv = raw_a.oplog_vv();
    create(&backend_a, EntityUri::block("root"), "ours").await;
    raw_a.commit();
    assert!(
        !foreign_peer_advanced(&raw_a, &before_vv, ours),
        "a window containing only our own ops must be admitted"
    );

    // (b) same window, plus a peer's op → refused.
    let doc_b = Arc::new(LoroDocument::new("guard-b".to_string()).unwrap());
    let backend_b = LoroBackend::from_document(doc_b.clone());
    // ALLOW(loro_doc_escape): measurement probe — `revert_to` and `oplog_frontiers`
    // are not exposed through the doc-boundary API, and single-threaded probe
    // reads cannot observe another writer.
    let raw_b = doc_b.doc();
    raw_b
        .import(
            &raw_a
                .export(loro::ExportMode::updates(&raw_b.oplog_vv()))
                .unwrap(),
        )
        .unwrap();
    create(&backend_b, EntityUri::block("root"), "theirs").await;
    raw_b.commit();
    raw_a
        .import(
            &raw_b
                .export(loro::ExportMode::updates(&raw_a.oplog_vv()))
                .unwrap(),
        )
        .unwrap();
    raw_a.commit();

    assert!(
        foreign_peer_advanced(&raw_a, &before_vv, ours),
        "a window a peer wrote into must be refused — reverting it destroys their write"
    );
}
