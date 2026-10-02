//! What a `LoroDocumentStore` reloads after a crash: everything `save_all`
//! acknowledged, a log tail cut short by the crash dropped and disclosed,
//! a damaged record anywhere refused.

use std::path::Path;
use std::sync::Arc;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_loro::DocScope;
use holon_loro::GLOBAL_SNAPSHOT_NAME;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::update_log;
use loro::LoroValue;
use loro::VersionVector;
use proptest::prelude::*;

fn state(doc: &LoroDocument) -> (LoroValue, VersionVector) {
    doc.with_read(|d| Ok((d.get_deep_value(), d.oplog_vv())))
        .unwrap()
}

fn log_of(dir: &Path) -> std::path::PathBuf {
    update_log::log_path(&dir.join(GLOBAL_SNAPSHOT_NAME))
}

fn log_len(dir: &Path) -> u64 {
    std::fs::metadata(log_of(dir)).unwrap().len()
}

/// A store whose snapshot is large enough that the next saves append.
async fn seeded(dir: &Path) -> (LoroDocumentStore, Arc<LoroDocument>) {
    let store = LoroDocumentStore::new(dir.to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    doc.insert_text("body", 0, &"seed ".repeat(2_000)).unwrap();
    store.save_all().await.unwrap();
    (store, doc)
}

#[tokio::test]
async fn a_crash_after_a_save_reloads_everything_the_save_acknowledged() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    doc.insert_text("body", 0, "acknowledged ").unwrap();
    store.save_all().await.unwrap();
    assert!(
        update_log::holds_records(&log_of(dir.path())).unwrap(),
        "premise: the second save appends to the log"
    );
    let (value, vv) = state(&doc);
    drop((store, doc));

    let reloaded = LoroDocumentStore::new(dir.path().to_path_buf())
        .get_doc(DocScope::Global)
        .await
        .unwrap();
    assert_eq!(state(&reloaded), (value, vv));
}

#[derive(Clone, Copy, Debug)]
enum Tear {
    HeaderCutByEof,
    PayloadCutByEof,
}

async fn reload_after(tear: Tear) {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    doc.insert_text("body", 0, "first ").unwrap();
    store.save_all().await.unwrap();
    let expected = state(&doc);
    let first_end = log_len(dir.path());
    doc.insert_text("body", 0, "second ").unwrap();
    store.save_all().await.unwrap();
    let end = log_len(dir.path());
    assert!(end > first_end, "premise: the second save appends a record");
    drop((store, doc));

    let log = log_of(dir.path());
    let mut bytes = std::fs::read(&log).unwrap();
    match tear {
        Tear::HeaderCutByEof => {
            bytes.truncate(first_end as usize + update_log::RECORD_HEADER as usize - 1)
        }
        Tear::PayloadCutByEof => {
            bytes.truncate(first_end as usize + update_log::RECORD_HEADER as usize + 10)
        }
    }
    std::fs::write(&log, &bytes).unwrap();

    let bus = Arc::new(ConditionBus::new());
    let reloaded = LoroDocumentStore::new(dir.path().to_path_buf())
        .with_condition_bus(bus.clone())
        .get_doc(DocScope::Global)
        .await
        .unwrap_or_else(|e| panic!("a torn tail ({tear:?}) must not fail the load: {e:#}"));
    assert_eq!(
        state(&reloaded),
        expected,
        "{tear:?}: the doc reloads as of the last whole record"
    );
    let disclosed = bus.current();
    assert!(
        disclosed
            .iter()
            .any(|c| c.subject == log.display().to_string()
                && matches!(c.reason, ConditionKind::LoroUpdateLogTailDropped { .. })),
        "{tear:?}: the dropped tail must be disclosed; conditions: {disclosed:?}"
    );
}

#[tokio::test]
async fn a_header_cut_by_eof_is_a_torn_tail_dropped_and_disclosed() {
    reload_after(Tear::HeaderCutByEof).await;
}

#[tokio::test]
async fn a_payload_cut_by_eof_is_a_torn_tail_dropped_and_disclosed() {
    reload_after(Tear::PayloadCutByEof).await;
}

/// Three appended records; returns the log bytes and the offset of each.
async fn three_records(dir: &Path) -> (Vec<u8>, [usize; 3]) {
    let (store, doc) = seeded(dir).await;
    let mut starts = [0; 3];
    for (start, word) in starts.iter_mut().zip(["a ", "b ", "c "]) {
        *start = log_len(dir) as usize;
        doc.insert_text("body", 0, word).unwrap();
        store.save_all().await.unwrap();
    }
    drop((store, doc));
    (std::fs::read(log_of(dir)).unwrap(), starts)
}

async fn assert_load_refused_naming_rebuild(dir: &Path, bytes: &[u8], what: &str) -> String {
    std::fs::write(log_of(dir), bytes).unwrap();
    let err = LoroDocumentStore::new(dir.to_path_buf())
        .get_doc(DocScope::Global)
        .await
        .err()
        .unwrap_or_else(|| panic!("{what} must fail the load"));
    assert!(
        format!("{err:#}").contains("to rebuild it from the org files"),
        "{what}: the error must name the recovery: {err:#}"
    );
    format!("{err:#}")
}

#[tokio::test]
async fn a_flipped_bit_in_a_middle_records_length_fails_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut bytes, starts) = three_records(dir.path()).await;
    // A high bit makes the length point past EOF: without a header checksum
    // that reads as a torn tail and drops the third record.
    bytes[starts[1] + 3] ^= 0x40;
    let message =
        assert_load_refused_naming_rebuild(dir.path(), &bytes, "a garbled middle length").await;
    assert!(
        message.contains(" fails its checksum, so the log is damaged, not cut short; "),
        "{message}"
    );
}

#[tokio::test]
async fn a_flipped_bit_in_a_middle_payload_fails_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut bytes, starts) = three_records(dir.path()).await;
    bytes[starts[1] + update_log::RECORD_HEADER as usize + 3] ^= 0x01;
    let message =
        assert_load_refused_naming_rebuild(dir.path(), &bytes, "a garbled middle payload").await;
    let log = log_of(dir.path());
    let at = starts[1];
    assert!(
        message.contains(&format!(
            "the record at byte {at} of {} fails its checksum although all ",
            log.display()
        )) && message.contains(" bytes are present, so the log is damaged, not cut short; "),
        "{message}"
    );
}

#[tokio::test]
async fn a_flipped_bit_in_the_last_payload_inside_the_file_fails_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut bytes, _) = three_records(dir.path()).await;
    *bytes.last_mut().unwrap() ^= 0x01;
    assert_load_refused_naming_rebuild(dir.path(), &bytes, "a garbled last payload").await;
}

#[tokio::test]
async fn a_log_in_the_previous_format_fails_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut bytes, _) = three_records(dir.path()).await;
    bytes[..8].copy_from_slice(b"HOLONUL1");
    assert_load_refused_naming_rebuild(dir.path(), &bytes, "an old-format log").await;
}

#[tokio::test]
async fn a_checkpoint_leaves_the_whole_document_in_the_snapshot() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    doc.insert_text("body", 0, "logged ").unwrap();
    store.save_all().await.unwrap();
    assert!(update_log::holds_records(&log_of(dir.path())).unwrap());

    store.checkpoint().await.unwrap();
    assert!(
        !update_log::holds_records(&log_of(dir.path())).unwrap(),
        "a checkpoint empties the log"
    );
    let (snapshot_only, _) = update_log::read_persisted(&dir.path().join(GLOBAL_SNAPSHOT_NAME))
        .map(|(d, r)| (d.get_deep_value(), r))
        .unwrap();
    assert_eq!(snapshot_only, state(&doc).0);
}

#[tokio::test]
async fn a_log_over_a_snapshot_it_does_not_extend_fails_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    doc.insert_text("body", 0, "logged ").unwrap();
    store.save_all().await.unwrap();
    drop((store, doc));

    let other = tempfile::TempDir::new().unwrap();
    let (store, _doc) = open(other.path()).await;
    store.checkpoint().await.unwrap();
    std::fs::copy(
        other.path().join(GLOBAL_SNAPSHOT_NAME),
        dir.path().join(GLOBAL_SNAPSHOT_NAME),
    )
    .unwrap();

    let err = LoroDocumentStore::new(dir.path().to_path_buf())
        .get_doc(DocScope::Global)
        .await
        .err()
        .expect("a record whose base the snapshot lacks must fail the load");
    assert!(
        format!("{err:#}").contains("to rebuild it from the org files"),
        "the error must name the recovery: {err:#}"
    );
}

#[tokio::test]
async fn records_a_new_snapshot_already_holds_reload_as_no_ops() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    for word in ["one ", "two ", "three "] {
        doc.insert_text("body", 0, word).unwrap();
        store.save_all().await.unwrap();
    }
    drop((store, doc));
    let log = log_of(dir.path());
    let stale_log = std::fs::read(&log).unwrap();

    // The reopened store's first save writes a history-trimmed snapshot that
    // holds every logged record; the crash lands before the log reset.
    let (store, doc) = open(dir.path()).await;
    doc.insert_text("body", 0, "four ").unwrap();
    store.save_all().await.unwrap();
    let expected = state(&doc);
    drop((store, doc));
    std::fs::write(&log, &stale_log).unwrap();

    let (_, reloaded) = open(dir.path()).await;
    assert_eq!(state(&reloaded), expected);
}

#[derive(Clone, Debug)]
enum Op {
    Type(usize, String),
    AddNode(i64),
    SetMeta(usize, i64),
    Save,
    Checkpoint,
    Reopen,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (any::<usize>(), "[a-z ]{1,12}").prop_map(|(at, s)| Op::Type(at, s)),
        2 => any::<i64>().prop_map(Op::AddNode),
        2 => (any::<usize>(), any::<i64>()).prop_map(|(k, v)| Op::SetMeta(k, v)),
        3 => Just(Op::Save),
        1 => Just(Op::Checkpoint),
        1 => Just(Op::Reopen),
    ]
}

fn apply(doc: &LoroDocument, op: &Op) {
    doc.with_write(WriteOrigin::BlockOps, |d| {
        let tree = d.get_tree(TREE_NAME);
        match op {
            Op::Type(at, s) => {
                let text = d.get_text("body");
                text.insert(at % (text.len_unicode() + 1), s)?;
            }
            Op::AddNode(v) => {
                let node = tree.create(None)?;
                tree.get_meta(node)?.insert("v", *v)?;
            }
            Op::SetMeta(k, v) => {
                let nodes = tree.nodes();
                if !nodes.is_empty() {
                    tree.get_meta(nodes[k % nodes.len()])?.insert("v", *v)?;
                }
            }
            _ => unreachable!("only edits are applied"),
        }
        Ok(())
    })
    .unwrap();
}

async fn open(dir: &Path) -> (LoroDocumentStore, Arc<LoroDocument>) {
    let store = LoroDocumentStore::new(dir.to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    (store, doc)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    #[test]
    fn a_reopened_store_holds_exactly_what_was_saved(ops in prop::collection::vec(op(), 1..40)) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let dir = tempfile::TempDir::new().unwrap();
            let (mut store, mut doc) = open(dir.path()).await;
            for op in &ops {
                match op {
                    Op::Save => {
                        store.save_all().await.unwrap();
                        let log = std::fs::metadata(log_of(dir.path())).unwrap().len();
                        let snapshot =
                            std::fs::metadata(dir.path().join(GLOBAL_SNAPSHOT_NAME)).unwrap().len();
                        assert!(
                            log - update_log::MAGIC.len() as u64 <= snapshot,
                            "the log ({log} bytes) outgrew its snapshot ({snapshot} bytes)"
                        );
                    }
                    Op::Checkpoint => store.checkpoint().await.unwrap(),
                    Op::Reopen => {
                        store.save_all().await.unwrap();
                        let before = state(&doc);
                        (store, doc) = open(dir.path()).await;
                        assert_eq!(state(&doc), before, "reopened after {ops:?}");
                    }
                    edit => apply(&doc, edit),
                }
            }
            store.save_all().await.unwrap();
            let expected = state(&doc);
            let (_, reopened) = open(dir.path()).await;
            assert_eq!(state(&reopened), expected, "reopened after {ops:?}");
        });
    }
}

async fn reload_disclosing(dir: &Path) -> (Arc<LoroDocument>, Vec<holon_api::Condition>) {
    let bus = Arc::new(ConditionBus::new());
    let doc = LoroDocumentStore::new(dir.to_path_buf())
        .with_condition_bus(bus.clone())
        .get_doc(DocScope::Global)
        .await
        .unwrap_or_else(|e| panic!("the load must succeed: {e:#}"));
    (doc, bus.current())
}

fn drops_a_tail(conditions: &[holon_api::Condition]) -> bool {
    conditions
        .iter()
        .any(|c| matches!(c.reason, ConditionKind::LoroUpdateLogTailDropped { .. }))
}

#[tokio::test]
async fn a_zero_filled_tail_is_a_torn_tail_dropped_and_disclosed() {
    for zeros in [1usize, 11, 12, 4096] {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, doc) = seeded(dir.path()).await;
        doc.insert_text("body", 0, "acknowledged ").unwrap();
        store.save_all().await.unwrap();
        let expected = state(&doc);
        drop((store, doc));
        let mut bytes = std::fs::read(log_of(dir.path())).unwrap();
        bytes.resize(bytes.len() + zeros, 0);
        std::fs::write(log_of(dir.path()), &bytes).unwrap();

        let (reloaded, conditions) = reload_disclosing(dir.path()).await;
        assert_eq!(state(&reloaded), expected, "{zeros} zero bytes at EOF");
        assert!(
            drops_a_tail(&conditions),
            "{zeros} zero bytes at EOF must be disclosed: {conditions:?}"
        );
    }
}

#[tokio::test]
async fn zeros_followed_by_a_record_fail_the_load_naming_the_rebuild() {
    let dir = tempfile::TempDir::new().unwrap();
    let (mut bytes, starts) = three_records(dir.path()).await;
    bytes.splice(starts[2]..starts[2], [0u8; 12]);
    assert_load_refused_naming_rebuild(dir.path(), &bytes, "zeros mid-file").await;
}

#[tokio::test]
async fn an_incomplete_magic_is_an_empty_log_and_is_rewritten() {
    for kept in [0usize, 3, 7] {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, doc) = seeded(dir.path()).await;
        let expected = state(&doc);
        drop((store, doc));
        std::fs::write(log_of(dir.path()), &update_log::MAGIC[..kept]).unwrap();

        let (reloaded, conditions) = reload_disclosing(dir.path()).await;
        assert_eq!(state(&reloaded), expected, "{kept} magic bytes");
        assert!(conditions.is_empty(), "nothing was lost: {conditions:?}");
        assert_eq!(
            std::fs::read(log_of(dir.path())).unwrap(),
            update_log::MAGIC,
            "{kept} magic bytes: the magic is rewritten"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_log_holding_exactly_the_magic_is_a_complete_empty_log_and_is_not_rewritten() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    let expected = state(&doc);
    drop((store, doc));
    let log = log_of(dir.path());
    std::fs::write(&log, update_log::MAGIC).unwrap();
    let inode = std::fs::metadata(&log).unwrap().ino();

    let replay = update_log::replay(&loro::LoroDoc::new(), &log).unwrap();
    assert!(!replay.magic_incomplete, "the full magic is not incomplete");

    let (reloaded, conditions) = reload_disclosing(dir.path()).await;
    assert_eq!(state(&reloaded), expected);
    assert!(conditions.is_empty(), "{conditions:?}");
    assert_eq!(
        std::fs::metadata(&log).unwrap().ino(),
        inode,
        "a complete empty log is not durably rewritten on load"
    );
}

#[tokio::test]
async fn a_failed_append_cuts_its_partial_record_off_and_later_saves_survive() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, doc) = seeded(dir.path()).await;
    doc.insert_text("body", 0, "first ").unwrap();
    store.save_all().await.unwrap();
    let acked = log_len(dir.path());

    let result = update_log::append_with(&log_of(dir.path()), &[7; 50], acked, |file, record| {
        use std::io::Write;
        file.write_all(&record[..record.len() / 2])?;
        Err(std::io::Error::other("no space left on device"))
    });
    assert!(result.is_err(), "the failed append reports its error");
    assert_eq!(log_len(dir.path()), acked, "the partial record is cut off");

    doc.insert_text("body", 0, "second ").unwrap();
    store.save_all().await.unwrap();
    let expected = state(&doc);
    drop((store, doc));
    let (reloaded, _) = reload_disclosing(dir.path()).await;
    assert_eq!(state(&reloaded), expected, "both acknowledged saves load");
}

#[tokio::test]
async fn a_stump_behind_the_acknowledged_log_is_cut_off_before_the_next_append() {
    let dir = tempfile::TempDir::new().unwrap();
    let bus = Arc::new(ConditionBus::new());
    let store = LoroDocumentStore::new(dir.path().to_path_buf()).with_condition_bus(bus.clone());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    doc.insert_text("body", 0, &"seed ".repeat(2_000)).unwrap();
    store.save_all().await.unwrap();
    doc.insert_text("body", 0, "first ").unwrap();
    store.save_all().await.unwrap();
    {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(log_of(dir.path()))
            .unwrap();
        log.write_all(&[9; 7]).unwrap();
    }
    doc.insert_text("body", 0, "second ").unwrap();
    store.save_all().await.unwrap();
    assert!(
        drops_a_tail(&bus.current()),
        "the cut stump is disclosed: {:?}",
        bus.current()
    );
    let expected = state(&doc);
    drop((store, doc));
    let (reloaded, _) = reload_disclosing(dir.path()).await;
    assert_eq!(state(&reloaded), expected, "both acknowledged saves load");
}
