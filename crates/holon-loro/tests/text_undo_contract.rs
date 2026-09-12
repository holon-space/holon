//! What the vault's text-undo manager promises, measured rather than assumed.

use std::sync::Arc;

use anyhow::Result;
use holon_loro::LoroDocument;
use holon_loro::TextUndo;
use holon_loro::WriteOrigin;
use holon_loro::loro_document_store::DocScope;
use holon_loro::text_undo::MERGE_INTERVAL_MS;

fn typed(doc: &LoroDocument, at: usize, text: &str) -> Result<()> {
    doc.with_write(WriteOrigin::UiEditorKeystroke, |txn| {
        txn.get_text("content").insert(at, text)?;
        Ok(())
    })
}

/// The guard must fire on the path that actually moves Loro's peer id.
///
/// `LoroDocument::peer_id` returns a value cached when the wrapper was built,
/// so a write straight to the inner doc moves the real id and leaves the cache
/// stale. A guard reading the cache would never fire — which is the same as
/// having no guard, since Loro silently clears both undo stacks on the change.
#[test]
fn a_peer_id_change_behind_the_wrapper_is_refused_loudly() -> Result<()> {
    let doc = Arc::new(LoroDocument::new("peer-guard".to_string())?);
    let undo = TextUndo::install(doc.clone());
    typed(&doc, 0, "typed")?;
    assert!(undo.can_undo()?, "the manager must hold the typing");

    let before = doc.doc().peer_id();
    doc.doc().set_peer_id(before.wrapping_add(1))?;

    let err = undo
        .can_undo()
        .expect_err("a moved peer id must be refused, not reported as an empty stack");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("peer id changed"),
        "the refusal must name what happened; got: {msg}"
    );
    Ok(())
}

/// How a new undo group is observed, measured.
///
/// The journal keeps one text-epoch marker per manager group, so it needs to
/// know what closes a group. Two mechanisms are candidates: the merge interval
/// and an explicit checkpoint. This records what each one actually does in the
/// pinned fork, so the journal relies on the one that works.
#[test]
fn what_closes_an_undo_group() -> Result<()> {
    // The merge interval: two keystrokes either side of it are two groups.
    let doc = Arc::new(LoroDocument::new("group-interval".to_string())?);
    let undo = TextUndo::install(doc.clone());
    typed(&doc, 0, "a")?;
    std::thread::sleep(std::time::Duration::from_millis(
        MERGE_INTERVAL_MS as u64 + 200,
    ));
    typed(&doc, 1, "b")?;
    assert_eq!(
        undo.undo_count()?,
        2,
        "a pause longer than the merge interval must start a new undo group"
    );

    // Two keystrokes INSIDE the interval are one group.
    let doc = Arc::new(LoroDocument::new("group-merged".to_string())?);
    let undo = TextUndo::install(doc.clone());
    typed(&doc, 0, "a")?;
    typed(&doc, 1, "b")?;
    assert_eq!(
        undo.undo_count()?,
        1,
        "keystrokes inside the merge interval must coalesce into one undo group"
    );

    // MEASURED, and why `TextUndo` exposes no checkpoint: Loro takes the merge
    // decision at write time from the elapsed interval alone
    // (`loro-internal/src/undo.rs` `in_merge_interval`), and
    // `record_new_checkpoint` does not reset that clock. Two keystrokes inside
    // the interval stay ONE group across a checkpoint, so a checkpoint cannot
    // force a split and the journal never assumes one.
    let doc = Arc::new(LoroDocument::new("group-checkpoint".to_string())?);
    // ALLOW(loro_doc_escape): drives the manager directly to measure the
    // fork's own behaviour, which is the point of this arm.
    let raw = doc.doc();
    let mut manager = loro::UndoManager::new(&raw);
    manager.set_merge_interval(MERGE_INTERVAL_MS);
    typed(&doc, 0, "a")?;
    manager.record_new_checkpoint()?;
    typed(&doc, 1, "b")?;
    assert_eq!(
        manager.undo_count(),
        1,
        "if this ever reads 2 the fork gained checkpoint-forced splits, and `TextUndo` may then \
         offer one"
    );
    Ok(())
}

/// Arming must be safe while the document is busy, and must arm exactly once.
///
/// Loro's subscriber registry holds either the subscriber map or a marker that
/// this emitter is mid-emit on some thread; registering a subscriber against
/// the marker panics with `unwrap_left` on a `Right`
/// (`loro-internal/src/utils/subscription.rs:336`). `UndoManager::new`
/// registers two subscriptions, so arming races every commit unless it is
/// serialised against emission.
#[test]
fn arming_under_concurrent_commits_never_panics_and_arms_once() -> Result<()> {
    for attempt in 0..20 {
        let dir = tempfile::tempdir()?;
        let store = holon_loro::LoroDocumentStore::new(dir.path().to_path_buf());
        let doc = futures::executor::block_on(store.get_doc(DocScope::Global))?;

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut writers = Vec::new();
        for w in 0..4 {
            let doc = doc.clone();
            let stop = stop.clone();
            writers.push(std::thread::spawn(move || {
                let mut i = 0usize;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    doc.with_write(WriteOrigin::Probe("arm_race_writer"), |txn| {
                        txn.get_text("content").insert(0, "x")?;
                        Ok(())
                    })
                    .expect("a writer thread's commit failed");
                    i += 1;
                    if i > 400 {
                        break;
                    }
                }
                let _ = w;
            }));
        }

        // Several arming threads, so a double-arm would also show up here.
        let armers: Vec<_> = (0..3)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    futures::executor::block_on(store.ensure_text_undo())
                        .map(|u| Arc::as_ptr(&u) as usize)
                })
            })
            .collect();

        let mut ids = Vec::new();
        for a in armers {
            ids.push(
                a.join()
                    .expect("an arming thread panicked — see the Loro subscription note")
                    .unwrap_or_else(|e| panic!("arming failed on attempt {attempt}: {e:#}")),
            );
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for w in writers {
            w.join().expect("a writer thread panicked");
        }

        assert!(
            ids.windows(2).all(|p| p[0] == p[1]),
            "three arming calls produced different managers on attempt {attempt}; the journal \
             would then count markers against one manager while another records the typing"
        );
    }
    Ok(())
}
