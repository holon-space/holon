//! A reader of the persisted document (snapshot plus log) is excluded while a
//! save or a file swap holds the store's save lock, so it never sees a
//! snapshot and a log that belong to different moments.

use std::time::Duration;

use holon_loro::DocScope;
use holon_loro::GLOBAL_SNAPSHOT_NAME;
use holon_loro::LoroDocumentStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_waits_out_a_swap_instead_of_seeing_its_middle() {
    let dir = tempfile::TempDir::new().unwrap();
    let snapshot = dir.path().join(GLOBAL_SNAPSHOT_NAME);
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    doc.insert_text("body", 0, "before ").unwrap();
    store.save_all().await.unwrap();
    let stale_snapshot = std::fs::read(&snapshot).unwrap();
    doc.insert_text("body", 0, "acknowledged ").unwrap();
    let expected = doc.with_read(|d| Ok(d.get_deep_value())).unwrap();

    let reader = {
        let store = store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            store.read_persisted(DocScope::Global).await.unwrap().0
        })
    };

    // The swap checkpoints first, then passes through a state with the
    // previous snapshot and an empty log, which lacks the acknowledged text.
    let swapped = store
        .replace_global_files(|_| {
            let current = std::fs::read(&snapshot).unwrap();
            std::fs::write(&snapshot, &stale_snapshot).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            std::fs::write(&snapshot, &current).unwrap();
            Ok(())
        })
        .await;
    swapped.unwrap();

    let read = reader.await.unwrap();
    assert_eq!(read.get_deep_value(), expected);
}
