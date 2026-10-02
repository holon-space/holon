//! What `save_all` writes reaches the device before it returns: a snapshot
//! before it replaces the previous one and the rename after, a log record
//! before the save returns.

use holon_filesystem::fs_port::DurabilityEvent;
use holon_filesystem::fs_port::DurabilityRecorder;
use holon_filesystem::fs_port::atomic_temp_target;
use holon_loro::DocScope;
use holon_loro::GLOBAL_SNAPSHOT_NAME;
use holon_loro::LoroDocumentStore;

#[tokio::test(flavor = "current_thread")]
async fn a_snapshot_is_synced_before_its_rename_and_its_directory_after() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    doc.insert_text("probe", 0, "hello").unwrap();

    let recorder = DurabilityRecorder::start();
    store.save_all().await.unwrap();
    let events = recorder.finish();

    let snapshot = dir.path().join(GLOBAL_SNAPSHOT_NAME);
    let synced = events.iter().position(|e| {
        matches!(e, DurabilityEvent::FileSynced(temp)
            if atomic_temp_target(temp).as_deref() == Some(snapshot.as_path()))
    });
    let replaced = events
        .iter()
        .position(|e| *e == DurabilityEvent::Replaced(snapshot.clone()));
    let dir_synced = events
        .iter()
        .rposition(|e| *e == DurabilityEvent::DirSynced(dir.path().to_path_buf()));
    assert!(
        matches!((synced, replaced, dir_synced), (Some(s), Some(r), Some(d)) if s < r && r < d),
        "saving {} must sync the temp, rename it, then sync the directory; saw {events:?}",
        snapshot.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_appended_record_is_synced() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    doc.insert_text("probe", 0, &"seed ".repeat(1_000)).unwrap();
    store.save_all().await.unwrap();
    doc.insert_text("probe", 0, "more").unwrap();

    let recorder = DurabilityRecorder::start();
    store.save_all().await.unwrap();
    let events = recorder.finish();

    let log = holon_loro::update_log::log_path(&dir.path().join(GLOBAL_SNAPSHOT_NAME));
    assert_eq!(events, vec![DurabilityEvent::FileSynced(log)]);
}
