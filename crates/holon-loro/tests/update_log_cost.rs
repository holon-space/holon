//! Cost oracle of `LoroDocumentStore::save_all`: the bytes one save writes
//! for a one-field change do not grow with the number N of blocks in the doc.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::write_stable_id;

const SMALL: usize = 500;
const LARGE: usize = 8_000;
const SAVES: usize = 50;

struct FileState {
    ino: u64,
    bytes: Vec<u8>,
}

fn files(dir: &Path) -> HashMap<PathBuf, FileState> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let state = FileState {
                ino: std::fs::metadata(&path).unwrap().ino(),
                bytes: std::fs::read(&path).unwrap(),
            };
            (path, state)
        })
        .collect()
}

/// Bytes a save wrote: a file appended in place counts its growth, any other
/// new or replaced file counts its whole length.
fn written(before: &HashMap<PathBuf, FileState>, after: &HashMap<PathBuf, FileState>) -> u64 {
    after
        .iter()
        .map(|(path, now)| match before.get(path) {
            Some(was) if was.ino == now.ino && now.bytes.starts_with(&was.bytes) => {
                (now.bytes.len() - was.bytes.len()) as u64
            }
            Some(was) if was.ino == now.ino && was.bytes == now.bytes => 0,
            _ => now.bytes.len() as u64,
        })
        .sum()
}

async fn mean_bytes_per_save(n: usize) -> f64 {
    use std::time::Instant;

    let dir = tempfile::TempDir::new().unwrap();
    let store = LoroDocumentStore::new(dir.path().to_path_buf());
    let doc = store.get_doc(DocScope::Global).await.unwrap();
    let nodes = doc
        .with_write(WriteOrigin::BlockOps, |d| {
            let tree = d.get_tree(TREE_NAME);
            let root = tree.create(None)?;
            write_stable_id(d, root, "page")?;
            (0..n)
                .map(|k| {
                    let node = tree.create(root)?;
                    write_stable_id(d, node, &format!("b{k}"))?;
                    Ok(node)
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .unwrap();
    let start = Instant::now();
    store.save_all().await.unwrap();
    eprintln!("N={n}: the snapshot save took {:?}", start.elapsed());

    let mut total = 0;
    let mut times = Vec::new();
    for k in 0..SAVES {
        doc.with_write(WriteOrigin::BlockOps, |d| {
            let node = nodes[(k * 7919) % n];
            d.get_tree(TREE_NAME)
                .get_meta(node)?
                .insert("completed", k % 2 == 0)?;
            Ok(())
        })
        .unwrap();
        let before = files(dir.path());
        let start = Instant::now();
        store.save_all().await.unwrap();
        times.push(start.elapsed());
        total += written(&before, &files(dir.path()));
    }
    times.sort();
    eprintln!(
        "N={n}: a one-field save takes median {:?}, p95 {:?}, max {:?}",
        times[SAVES / 2],
        times[SAVES * 95 / 100],
        times[SAVES - 1]
    );
    total as f64 / SAVES as f64
}

#[tokio::test]
async fn a_one_field_save_writes_bytes_independent_of_the_doc_size() {
    let small = mean_bytes_per_save(SMALL).await;
    let large = mean_bytes_per_save(LARGE).await;
    eprintln!("mean bytes per save: N={SMALL} {small:.0}, N={LARGE} {large:.0}");
    assert!(
        large <= 3.0 * small,
        "a one-field save writes {large:.0} bytes at N={LARGE} against {small:.0} at \
         N={SMALL}: the save rewrites state proportional to the doc"
    );
}
