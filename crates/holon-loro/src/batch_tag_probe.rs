//! Probe: can a per-commit tag carry a batch identity that a rollback can
//! audit?
//!
//! A whole-document `revert_to` is only honest when every change it undoes
//! belongs to the batch asking for it. The write authority attributes an op to
//! a PEER, so a local write that lands inside the window is indistinguishable
//! from the batch's own (bugfunnel
//! `2026-09-19-rollback-destroys-a-concurrent-local-write`). A commit-level
//! tag would make it distinguishable — if loro carries one that is durable,
//! replicated, and readable back over a counter range.
//!
//! The pinned loro (1.13.9, rev `6f5b2d7e`) offers two per-commit slots.
//! `CommitOptions::origin` is documented as NOT persisted
//! (`loro-internal/src/loro.rs`), which is why `WriteOrigin` can only serve
//! live subscribers. `CommitOptions::commit_msg` claims to persist. These
//! tests measure whether it does, and whether change compaction can smear a
//! tagged change together with an untagged one.
//!
//! Each test names its own verdict.

use loro::LoroDoc;

const TAG: &str = "holon.batch:probe";

/// Write one op into the doc's text container and commit it under `message`.
fn commit_tagged(doc: &LoroDoc, text: &str, message: Option<&str>) {
    if let Some(message) = message {
        doc.set_next_commit_message(message);
    }
    doc.get_text("t").insert(0, text).unwrap();
    doc.commit();
}

/// Every change this peer authored, as `(first counter, op count, message)`,
/// walked the way a rollback audit would walk it: step the counter range by
/// each change's own length.
fn changes_of(doc: &LoroDoc, peer: u64) -> Vec<(i32, usize, Option<String>)> {
    let end = *doc.oplog_vv().get(&peer).unwrap();
    let mut out = Vec::new();
    let mut counter = 0;
    while counter < end {
        let change = doc
            .get_change(loro::ID::new(peer, counter))
            .expect("every counter below the version vector belongs to some change");
        out.push((counter, change.len, change.message.map(|m| m.to_string())));
        counter += i32::try_from(change.len).unwrap();
    }
    out
}

/// VERDICT 1 — `commit_msg` is readable back over a counter range, and it
/// survives export/import into a peer that never saw the original commits.
///
/// This is what lets a rollback audit ask "does every change in the window
/// carry THIS batch's id?" instead of counting ops and hoping.
#[test]
fn a_commit_message_is_readable_per_change_and_survives_export_import() {
    let doc = LoroDoc::new();
    doc.set_peer_id(1).unwrap();
    commit_tagged(&doc, "a", Some(TAG));
    commit_tagged(&doc, "b", None);
    commit_tagged(&doc, "c", Some(TAG));

    let local = changes_of(&doc, 1);
    assert_eq!(
        local
            .iter()
            .map(|(_, _, m)| m.as_deref())
            .collect::<Vec<_>>(),
        vec![Some(TAG), None, Some(TAG)],
        "the tag must be readable per change, and the untagged change must \
         stay distinguishable: {local:?}"
    );

    let peer = LoroDoc::new();
    peer.set_peer_id(2).unwrap();
    peer.import(&doc.export(loro::ExportMode::all_updates()).unwrap())
        .unwrap();
    assert_eq!(
        changes_of(&peer, 1),
        local,
        "a replica must read the same per-change tags as the author"
    );
}

/// VERDICT 2 — change compaction cannot smear a tagged change into an
/// untagged neighbour.
///
/// Adjacent same-peer changes normally merge, which would leave one change
/// covering both a batch op and somebody else's write and make the tag
/// unreadable for the second. `Change::can_merge_right` requires equal commit
/// messages, so a differing tag is itself the barrier. The first assertion is
/// the control: without the tag these two commits DO merge, so the second
/// assertion is measuring the tag and not a doc that never merges.
#[test]
fn a_differing_commit_message_stops_two_adjacent_changes_merging() {
    let merged = LoroDoc::new();
    merged.set_peer_id(1).unwrap();
    commit_tagged(&merged, "a", None);
    commit_tagged(&merged, "b", None);
    assert_eq!(
        changes_of(&merged, 1),
        vec![(0, 2, None)],
        "control: two untagged adjacent commits must merge into one change"
    );

    let split = LoroDoc::new();
    split.set_peer_id(1).unwrap();
    commit_tagged(&split, "a", Some(TAG));
    commit_tagged(&split, "b", None);
    assert_eq!(
        changes_of(&split, 1),
        vec![(0, 1, Some(TAG.to_string())), (1, 1, None)],
        "a tagged change must not absorb an untagged neighbour"
    );
}

/// VERDICT 3 — an unlocked read of a document another thread is mid-transaction
/// on is STALE-OR-DIRTY, never torn.
///
/// The cell backing's reads (`current`, `anchor_cursor`, `resolve_cursor`) take
/// no doc lock. What they can observe is another writer's UNCOMMITTED op, since
/// loro applies a local op to document state immediately and the commit only
/// moves it into the oplog. The read itself is serialised by loro's own state
/// lock, so it returns a whole consistent string either way — the risk is
/// reading a batch interior, not reading rubble.
#[test]
fn an_unlocked_read_during_another_writers_transaction_is_dirty_not_torn() {
    use std::sync::mpsc;

    let doc = std::sync::Arc::new(LoroDoc::new());
    doc.set_peer_id(1).unwrap();
    let text = doc.get_text("t");
    text.insert(0, "base").unwrap();
    doc.commit();

    let (pending_tx, pending_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writing = doc.clone();
    let writer = std::thread::spawn(move || {
        writing.get_text("t").insert(4, "-uncommitted").unwrap();
        pending_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        writing.commit();
    });

    pending_rx.recv().unwrap();
    let observed = doc.get_text("t").to_string();
    release_tx.send(()).unwrap();
    writer.join().unwrap();

    assert_eq!(
        observed, "base-uncommitted",
        "an unlocked read sees the other writer's uncommitted op, and sees it whole"
    );
    assert_eq!(doc.get_text("t").to_string(), "base-uncommitted");
}

/// VERDICT 4 — a MULTI-op write exposes its interior to an unlocked read: the
/// intermediate is a value that neither precedes nor follows the write.
///
/// Verdict 3 used a single op, where "dirty" can only mean "already the final
/// value". Production writes are multi-op: `write_content_to_meta` sets
/// `content_type` and then replaces the text (itself a delete plus an insert
/// per diff hunk), and `update_block_marked` clears marks before reapplying
/// them. Between those ops the document holds a state no writer ever intended
/// — here the empty string. The editor's unlocked reads (`current`,
/// `resolve_cursor`) can render it; they converge on the commit event that
/// follows.
#[test]
fn an_unlocked_read_inside_a_multi_op_write_sees_an_intermediate_state() {
    use std::sync::mpsc;

    let doc = std::sync::Arc::new(LoroDoc::new());
    doc.set_peer_id(1).unwrap();
    let text = doc.get_text("t");
    text.insert(0, "before").unwrap();
    doc.commit();

    let (midway_tx, midway_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let writing = doc.clone();
    let writer = std::thread::spawn(move || {
        let text = writing.get_text("t");
        text.delete(0, "before".len()).unwrap();
        midway_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        text.insert(0, "after").unwrap();
        writing.commit();
    });

    midway_rx.recv().unwrap();
    let observed = doc.get_text("t").to_string();
    release_tx.send(()).unwrap();
    writer.join().unwrap();

    assert_eq!(
        observed, "",
        "the read must be able to observe the write's interior — neither \
         'before' nor 'after'"
    );
    assert_eq!(doc.get_text("t").to_string(), "after");
}
