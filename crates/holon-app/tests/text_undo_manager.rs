//! The vault document carries a text-undo manager, and it records only typing.
//!
//! Increment 1 of the cell-undo lane: the manager exists and is honest about
//! what it holds. Nothing is wired to cmd-z yet — `OperationEngine::undo` still
//! replays the inverse journal and knows nothing about this.

// The harness serves three other binaries; this one drives only its boot.
#[allow(dead_code)]
#[path = "session_shutdown/harness.rs"]
mod harness;

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::cell::Cell;
use holon_core::cell::TextOp;
use holon_frontend::cell::EntityCellRegistry;
use holon_loro::TextUndo;
use holon_loro::loro_document_store::DocScope;
use holon_loro::loro_document_store::LoroDocumentStore;

const PROBE_CHILD: &str = "shutdown-probe-child";

struct Booted {
    inner: harness::Booted,
    undo: Arc<TextUndo>,
}

async fn boot_with_undo() -> Booted {
    let inner = harness::boot_a_working_session().await;
    let store = inner
        .injector
        .try_resolve::<LoroDocumentStore>()
        .expect("a booted CRDT session registers a LoroDocumentStore");
    // Force the global doc, which is what the manager is built over.
    store
        .get_doc(DocScope::Global)
        .await
        .expect("the global Loro document");
    let undo = store
        .ensure_text_undo()
        .await
        .expect("a booted session must be able to arm a text-undo manager on the vault document");
    Booted { inner, undo }
}

/// One keystroke through the cell leg, the path the editor drives.
async fn keystroke(booted: &harness::Booted, at: usize, text: &str) {
    let registry = booted
        .injector
        .optional_resolve_async::<holon_loro::block_cell_registry::BlockCellRegistry>()
        .await
        .expect("a booted CRDT session registers a BlockCellRegistry");
    let uri = EntityUri::block(PROBE_CHILD);
    let cell: Cell<String> = registry
        .editable_field_any(&uri, "content", TypeId::of::<String>())
        .expect("the probe block's content cell")
        .downcast::<Cell<String>>()
        .map(|c| (*c).clone())
        .expect("content resolves to a Cell<String>");
    cell.apply_text_op(TextOp::Insert {
        pos_codepoint: at,
        text: text.to_string(),
    })
    .expect("a keystroke into the content cell");
}

/// Wait for a manager fact to become true.
///
/// Loro dispatches the manager's recording subscriber on commit, and under load
/// that can land after the typing call returns. Polling asserts the fact rather
/// than the scheduling, which is what these tests are about.
fn settle_until(undo: &TextUndo, what: &str, mut cond: impl FnMut(&TextUndo) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if cond(undo) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("waited 5s and {what} never became true");
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_through_the_cell_is_undoable() {
    let booted = boot_with_undo().await;
    assert!(
        !booted.undo.can_undo().unwrap(),
        "a freshly booted session offers an undo before the user has typed anything"
    );

    keystroke(&booted.inner, 0, "x").await;

    settle_until(&booted.undo, "the keystroke was recorded", |u| {
        u.can_undo().unwrap()
    });
    assert_eq!(booted.undo.undo_count().unwrap(), 1);
}

/// The exclusion, proven rather than assumed.
///
/// The mechanism is the ORIGIN, not timing: an ingest write commits under a
/// `sys.`-prefixed origin, which the manager was told to exclude, so it cannot
/// enter the stack however soon after the keystroke it lands. The assertion is
/// therefore on the stack DEPTH being unmoved, which is what "excluded" means —
/// nothing here depends on a merge group being closed first.
#[tokio::test(flavor = "multi_thread")]
async fn a_boundary_ingest_write_is_not_undoable() {
    let booted = boot_with_undo().await;
    keystroke(&booted.inner, 0, "typed").await;
    settle_until(&booted.undo, "the keystroke was recorded", |u| {
        u.undo_count().unwrap() == 1
    });
    let before = booted.undo.undo_count().unwrap();

    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(format!("block:{PROBE_CHILD}")));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert(
        "value".into(),
        Value::String("rewritten by the file on disk".to_string()),
    );
    booted
        .inner
        .engine
        .execute_operation(
            &holon_api::EntityName::from("block"),
            "set_field",
            params,
            OpOrigin::Ingest,
        )
        .await
        .expect("block/set_field through the production dispatcher");

    assert_eq!(
        booted.undo.undo_count().unwrap(),
        before,
        "an org-ingest write entered the user's text-undo stack; the system-origin exclusion is \
         not doing its job"
    );
}

/// An undo must not become a new undoable step, or cmd-z would toggle between
/// two states forever instead of walking back.
#[tokio::test(flavor = "multi_thread")]
async fn an_undo_is_not_recorded_as_a_new_undo_step() {
    let booted = boot_with_undo().await;
    keystroke(&booted.inner, 0, "typed").await;
    settle_until(&booted.undo, "the keystroke was recorded", |u| {
        u.undo_count().unwrap() == 1
    });

    assert!(booted.undo.undo().unwrap(), "the undo took nothing back");

    assert_eq!(
        booted.undo.undo_count().unwrap(),
        0,
        "the undo recorded itself as a fresh undo step"
    );
    assert!(
        booted.undo.can_redo().unwrap(),
        "the undone step did not reach the redo stack, so the undo is not reversible"
    );
}

/// The compaction question, now on the real wiring rather than a standalone
/// document: saving a history-trimmed snapshot must not empty a live manager.
#[tokio::test(flavor = "multi_thread")]
async fn history_compaction_leaves_the_live_manager_able_to_undo() {
    let booted = boot_with_undo().await;
    keystroke(&booted.inner, 0, "typed").await;
    settle_until(&booted.undo, "the keystroke was recorded", |u| {
        u.can_undo().unwrap()
    });

    let store = booted
        .inner
        .injector
        .try_resolve::<LoroDocumentStore>()
        .unwrap();
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let bytes = doc
        .export_compact_snapshot()
        .expect("export a history-compacted snapshot");
    assert!(!bytes.is_empty());

    assert!(
        booted.undo.can_undo().unwrap(),
        "history compaction emptied the vault's live undo manager"
    );
}

// ── Increment 2: the journal's text-epoch markers ────────────────────────────

/// Amendment 3's invariant: *journal text markers ↔ manager undo-groups, 1:1*.
///
/// The manager decides what one step IS (it merges keystrokes inside its
/// interval), so the journal reads the manager's count rather than counting
/// keystrokes for itself. Two bursts separated by more than the merge interval
/// must produce two markers, not four.
#[tokio::test(flavor = "multi_thread")]
async fn journal_markers_match_the_managers_undo_groups_one_to_one() {
    let booted = boot_with_undo().await;

    keystroke(&booted.inner, 0, "a").await;
    keystroke(&booted.inner, 1, "b").await;
    tokio::time::sleep(std::time::Duration::from_millis(
        (holon_loro::text_undo::MERGE_INTERVAL_MS as u64) + 200,
    ))
    .await;
    keystroke(&booted.inner, 2, "c").await;
    keystroke(&booted.inner, 3, "d").await;

    booted
        .inner
        .engine
        .sync_text_epochs()
        .await
        .expect("materialise the markers for the groups the manager holds");

    let groups = booted.undo.undo_count().unwrap();
    let markers = booted.inner.engine.text_epoch_count().await;
    // NOT an exact count: under load two keystrokes meant as one burst can
    // drift past the merge interval and become two groups. The invariant is
    // the correspondence, and it must hold at whatever grouping the manager
    // chose; the pause only guarantees there is more than one group to check
    // it against.
    assert!(
        groups >= 2,
        "the pause did not produce a second undo group, so 1:1 is untested here"
    );
    assert_eq!(
        markers, groups,
        "the journal holds {markers} text markers for {groups} manager undo-groups; the two \
         stacks have desynchronised and cmd-z would walk them at different rates"
    );
}

/// A marker delegates one step, and the journal pops exactly one with it.
#[tokio::test(flavor = "multi_thread")]
async fn undoing_a_marker_takes_back_one_text_group_and_consumes_one_marker() {
    let booted = boot_with_undo().await;

    keystroke(&booted.inner, 0, "a").await;
    tokio::time::sleep(std::time::Duration::from_millis(
        (holon_loro::text_undo::MERGE_INTERVAL_MS as u64) + 200,
    ))
    .await;
    keystroke(&booted.inner, 1, "b").await;
    settle_until(&booted.undo, "both bursts were recorded", |u| {
        u.undo_count().unwrap() >= 2
    });
    booted.inner.engine.sync_text_epochs().await.unwrap();
    let groups = booted.undo.undo_count().unwrap();
    assert_eq!(booted.inner.engine.text_epoch_count().await, groups);

    let outcome = booted
        .inner
        .engine
        .undo()
        .await
        .expect("undo the top marker");
    assert!(
        matches!(outcome, holon_api::UndoOutcome::Applied),
        "expected the marker to delegate an applied undo, got {outcome:?}"
    );

    assert_eq!(
        booted.inner.engine.text_epoch_count().await,
        groups - 1,
        "one delegated undo must consume exactly one marker"
    );
    assert_eq!(
        booted.undo.undo_count().unwrap(),
        groups - 1,
        "the manager and the journal must come down together"
    );
}

/// A marker whose group the manager no longer holds is reconciled away, with
/// a notice, rather than wedging the stack.
///
/// Draining the manager behind the journal's back is the shape a rebuilt
/// manager or a peer-id change leaves. The press that follows must make
/// progress; erroring here is what left the old build's stack dead for the
/// rest of the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_marker_whose_group_is_gone_is_reconciled_away_not_wedged() {
    let booted = boot_with_undo().await;

    keystroke(&booted.inner, 0, "a").await;
    settle_until(&booted.undo, "the keystroke was recorded", |u| {
        u.undo_count().unwrap() == 1
    });
    booted.inner.engine.sync_text_epochs().await.unwrap();
    assert_eq!(booted.inner.engine.text_epoch_count().await, 1);

    // Drain the manager behind the journal's back, the state a restart leaves:
    // the marker persisted, the manager was rebuilt empty.
    assert!(booted.undo.undo().unwrap());
    assert_eq!(booted.undo.undo_count().unwrap(), 0);

    // The next press reconciles the orphaned marker away and reports an empty
    // stack. It must NOT error (that would wedge every later press) and must
    // NOT claim an undo happened.
    let outcome = booted
        .inner
        .engine
        .undo()
        .await
        .expect("a marker whose group is gone must not wedge the stack");
    assert!(
        matches!(outcome, holon_api::UndoOutcome::Empty),
        "expected an empty stack once the orphaned marker is reconciled away, got {outcome:?}"
    );
    assert_eq!(
        booted.inner.engine.text_epoch_count().await,
        0,
        "the orphaned marker survived, so the next press would hit it again"
    );
}

/// The layout document must NOT get a manager: rearranging the interface is
/// not the user's typing, and an undo that took a pane back would be a
/// surprise. `LoroDocumentStore` builds one for the global scope only.
#[tokio::test(flavor = "multi_thread")]
async fn the_layout_document_has_no_text_undo_manager() {
    let booted = harness::boot_a_working_session().await;
    let store = booted
        .injector
        .try_resolve::<LoroDocumentStore>()
        .expect("a booted CRDT session registers a LoroDocumentStore");

    // Open the LAYOUT document first and only.
    let layout = store
        .get_doc(DocScope::Layout)
        .await
        .expect("the layout Loro document");
    assert_eq!(layout.doc_id(), "holon_layout");
    assert!(
        store.text_undo().is_none(),
        "opening the layout document installed a text-undo manager; only the vault document \
         carries one"
    );
}

/// The manager must NOT exist merely because the vault document was opened.
///
/// A Loro manager is a subscriber, and a subscriber makes Loro materialise an
/// event for every commit. Installed at document-open it rides the whole boot
/// ingest, where there is no typing to record: measured at 42-54 s against
/// 23-31 s on `quick_open_search_at_vault_scale`, i.e. roughly double. Arming
/// is the editor's signal instead.
#[tokio::test(flavor = "multi_thread")]
async fn opening_the_vault_document_does_not_arm_the_manager() {
    let booted = harness::boot_a_working_session().await;
    let store = booted
        .injector
        .try_resolve::<LoroDocumentStore>()
        .expect("a booted CRDT session registers a LoroDocumentStore");
    store
        .get_doc(DocScope::Global)
        .await
        .expect("the global Loro document");

    assert!(
        store.text_undo().is_none(),
        "opening the vault document armed the undo manager; its subscriber then costs an event \
         per commit for the whole boot ingest, where nothing is being typed"
    );

    // The editor asking for a content cell is what arms it.
    keystroke(&booted, 0, "x").await;
    assert!(
        store.text_undo().is_some(),
        "an editor cell was handed out and the manager was still not armed, so the typing that \
         just happened is not undoable"
    );
}

// ── Round 2: the stack must always make progress ─────────────────────────────

async fn ingest_rewrite(booted: &harness::Booted, text: &str) {
    let mut params: holon_api::StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(format!("block:{PROBE_CHILD}")));
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String(text.to_string()));
    booted
        .engine
        .execute_operation(
            &holon_api::EntityName::from("block"),
            "set_field",
            params,
            OpOrigin::Ingest,
        )
        .await
        .expect("block/set_field through the production dispatcher");
}

/// A file change between two typing bursts supersedes the first burst's group.
///
/// The manager then pops that group and reports nothing undone. Treating that
/// as an error left the marker on the stack, so every later cmd-z hit the same
/// refusal and the stack never moved again — the user's undo was dead for the
/// rest of the session after an ordinary event.
#[tokio::test(flavor = "multi_thread")]
async fn ingest_rewrite_between_bursts_then_walk_the_stack_back() {
    let booted = boot_with_undo().await;

    keystroke(&booted.inner, 0, "AAA").await;
    settle_until(&booted.undo, "the first burst was recorded", |u| {
        u.undo_count().unwrap() >= 1
    });
    ingest_rewrite(&booted.inner, "rewritten by the file on disk").await;
    tokio::time::sleep(std::time::Duration::from_millis(
        (holon_loro::text_undo::MERGE_INTERVAL_MS as u64) + 200,
    ))
    .await;
    keystroke(&booted.inner, 0, "BBB").await;
    settle_until(&booted.undo, "the second burst was recorded", |u| {
        u.undo_count().unwrap() >= 2
    });

    booted.inner.engine.sync_text_epochs().await.unwrap();
    let markers_before = booted.inner.engine.text_epoch_count().await;
    assert!(markers_before >= 2, "got {markers_before} markers");

    // Walk the whole stack back. Every press must either undo something or
    // consume a superseded marker — never refuse, and never stand still.
    for press in 1..=(markers_before + 2) {
        let before = booted.inner.engine.text_epoch_count().await;
        let outcome =
            booted.inner.engine.undo().await.unwrap_or_else(|e| {
                panic!("cmd-z #{press} refused instead of making progress: {e:#}")
            });
        let after = booted.inner.engine.text_epoch_count().await;
        if matches!(outcome, holon_api::UndoOutcome::Empty) {
            break;
        }
        assert!(
            after < before || before == 0,
            "cmd-z #{press} returned {outcome:?} but the marker count stayed at {before}; the \
             stack is wedged"
        );
    }
}

/// The two caps must keep markers and manager groups equal.
///
/// The journal bounds OPERATION entries; the manager bounds its own groups and
/// evicts oldest-first. If operation pressure could evict a marker, the
/// top-up would push a replacement and evict another operation entry, for
/// ever.
#[tokio::test(flavor = "multi_thread")]
async fn many_typing_groups_keep_markers_and_manager_in_step() {
    let booted = boot_with_undo().await;

    // More groups than the cap, each its own group (past the merge interval is
    // too slow at this count, so drive the manager directly through the cell
    // and let the journal top up from its depth).
    for i in 0..(holon_core::TEXT_UNDO_MAX_GROUPS + 50) {
        keystroke(&booted.inner, 0, "x").await;
        if i % 25 == 0 {
            booted.inner.engine.sync_text_epochs().await.unwrap();
        }
    }
    booted.inner.engine.sync_text_epochs().await.unwrap();

    let groups = booted.undo.undo_count().unwrap();
    let markers = booted.inner.engine.text_epoch_count().await;
    assert!(
        groups <= holon_core::TEXT_UNDO_MAX_GROUPS,
        "the manager is uncapped: {groups} groups"
    );
    assert_eq!(
        markers, groups,
        "the journal holds {markers} markers for {groups} manager groups after eviction"
    );
}

/// The one-stack invariant, across BOTH mechanisms.
///
/// The design rests on there being a single user-facing order: text groups the
/// CRDT undoes and operations the journal replays occupy one stack, in the
/// order the user worked. Nothing pinned that until now. This interleaves
/// typing, a journalled operation and a file rewrite, then walks the whole
/// stack back, asserting after every press that the gesture made progress and
/// never refused.
#[tokio::test(flavor = "multi_thread")]
async fn interleaved_typing_and_operations_walk_back_in_one_order() {
    let booted = boot_with_undo().await;

    async fn user_set_field(booted: &harness::Booted, value: &str) {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(format!("block:{PROBE_CHILD}")));
        params.insert("field".into(), Value::String("content".to_string()));
        params.insert("value".into(), Value::String(value.to_string()));
        booted
            .engine
            .execute_operation(
                &holon_api::EntityName::from("block"),
                "set_field",
                params,
                OpOrigin::User,
            )
            .await
            .expect("a user set_field through the production dispatcher");
    }

    let pause =
        std::time::Duration::from_millis((holon_loro::text_undo::MERGE_INTERVAL_MS as u64) + 200);

    keystroke(&booted.inner, 0, "AAA").await;
    tokio::time::sleep(pause).await;
    user_set_field(&booted.inner, "set by the user").await;
    ingest_rewrite(&booted.inner, "rewritten by the file on disk").await;
    tokio::time::sleep(pause).await;
    keystroke(&booted.inner, 0, "BBB").await;
    tokio::time::sleep(pause).await;
    user_set_field(&booted.inner, "set by the user again").await;

    booted.inner.engine.sync_text_epochs().await.unwrap();
    let markers = booted.inner.engine.text_epoch_count().await;
    assert!(
        markers >= 1,
        "no typing reached the stack; the walk is vacuous"
    );

    // Walk it all back. Every press must return an outcome — never an error —
    // and the stack must reach Empty in a bounded number of presses.
    let mut outcomes = Vec::new();
    let mut reached_empty = false;
    for press in 1..=20 {
        let outcome = booted.inner.engine.undo().await.unwrap_or_else(|e| {
            panic!(
                "cmd-z #{press} refused instead of making progress: {e:#}\nouctomes so far: \
                 {outcomes:?}"
            )
        });
        let empty = matches!(outcome, holon_api::UndoOutcome::Empty);
        outcomes.push(format!("{outcome:?}"));
        if empty {
            reached_empty = true;
            break;
        }
    }
    assert!(
        reached_empty,
        "20 presses did not drain the stack, so it is not making progress: {outcomes:?}"
    );
    assert_eq!(
        booted.inner.engine.text_epoch_count().await,
        0,
        "markers survived a full walk back: {outcomes:?}"
    );
    // A stale-drop is legitimate here — the ingest rewrite genuinely moves the
    // content out from under the second journalled entry. What may never
    // happen again is a drop because the reader could not see the field at all.
    for outcome in &outcomes {
        assert!(
            !outcome.contains("found None"),
            "a press dropped because the precondition reader found nothing, not because the \
             state diverged: {outcomes:?}"
        );
    }
    eprintln!("[ONE-STACK] outcomes in order: {outcomes:?}");
}
