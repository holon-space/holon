//! What one incremental projection pass costs in the doc read guard, and what
//! that guard costs a writer, at vault scale.
//!
//! `cargo run --release -p holon-loro --example projection_extract_bench --
//! 20000 100000`
//!
//! Per size it prints p50/p95/max of:
//! - `extract`: `incremental_block_changes` over one commit's facts, i.e. the
//!   time a pass holds the doc read guard (field edit, move, 100-block batch);
//! - `commit`: a one-op `with_write`, alone and while a reader loop runs
//!   `extract` passes back to back.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use holon_loro::CONTENT_RAW;
use holon_loro::CONTENT_TYPE;
use holon_loro::STABLE_ID;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::PendingChange;
use holon_loro::loro_backend::build_tid_index;
use holon_loro::loro_backend::extract_pending_changes;
use holon_loro::loro_backend::incremental_block_changes;
use holon_loro::loro_document::LoroDocument;
use loro::TreeID;

const ORIGIN: WriteOrigin = WriteOrigin::Probe("projection_extract_bench");
const SAMPLES: usize = 300;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn add_block(
    txn: &holon_loro::loro_document::WriteTxn,
    parent: Option<TreeID>,
    id: &str,
) -> anyhow::Result<TreeID> {
    let tree = txn.get_tree(TREE_NAME);
    let node = tree.create(parent)?;
    let meta = tree.get_meta(node)?;
    meta.insert(STABLE_ID, id)?;
    meta.insert(CONTENT_TYPE, "text")?;
    meta.ensure_mergeable_text(CONTENT_RAW)?
        .insert(0, &format!("block {id} with some words in it"))?;
    Ok(node)
}

/// `n` blocks: one page per 30 blocks, every other block under a page (40 %)
/// or under an earlier block.
fn generate(n: usize, rng: &mut Rng) -> anyhow::Result<(Arc<LoroDocument>, Vec<TreeID>)> {
    let doc = Arc::new(LoroDocument::new("bench".to_string())?);
    doc.with_write(ORIGIN, |txn| {
        txn.get_tree(TREE_NAME).enable_fractional_index(0);
        Ok(())
    })?;
    let pages = (n / 30).max(2);
    let mut nodes = Vec::with_capacity(n);
    let mut page_nodes = Vec::with_capacity(pages);
    for chunk in (0..n).collect::<Vec<_>>().chunks(5000) {
        doc.with_write(ORIGIN, |txn| {
            for &i in chunk {
                let parent = if i < pages {
                    None
                } else if i == pages || rng.below(10) < 4 {
                    Some(page_nodes[rng.below(page_nodes.len())])
                } else {
                    Some(nodes[pages + rng.below(nodes.len() - pages)])
                };
                let node = add_block(txn, parent, &format!("b{i}"))?;
                if i < pages {
                    page_nodes.push(node);
                }
                nodes.push(node);
            }
            Ok(())
        })?;
    }
    Ok((doc, nodes))
}

fn report(label: &str, mut d: Vec<Duration>) {
    d.sort();
    let at = |q: f64| d[((d.len() as f64 * q) as usize).min(d.len() - 1)];
    println!(
        "{label:<44} n={:<4} p50={:>9.1?} p95={:>9.1?} max={:>9.1?}",
        d.len(),
        at(0.5),
        at(0.95),
        d[d.len() - 1]
    );
}

fn main() -> anyhow::Result<()> {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.parse().expect("sizes are block counts"))
        .collect();
    assert!(
        !sizes.is_empty(),
        "usage: projection_extract_bench <blocks>..."
    );
    for n in sizes {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ n as u64);
        let t = Instant::now();
        let (doc, nodes) = generate(n, &mut rng)?;
        println!("== {n} blocks (generated in {:.1?}, release)", t.elapsed());

        let facts: Arc<Mutex<Vec<PendingChange>>> = Arc::default();
        let sink = facts.clone();
        let _sub = doc.doc().subscribe_root(Arc::new(move |event| {
            sink.lock().unwrap().extend(extract_pending_changes(&event));
        }));
        // Loro decides whether to record events when a transaction opens, and
        // the open one predates the subscription: let it commit once.
        doc.with_write(ORIGIN, |txn| {
            txn.get_tree(TREE_NAME)
                .get_meta(nodes[0])?
                .insert("bench", "warm")?;
            Ok(())
        })?;
        facts.lock().unwrap().clear();
        let mut tid_index: HashMap<TreeID, String> = doc.with_read(|d| Ok(build_tid_index(d)))?;

        let extract = |doc: &LoroDocument,
                       tid_index: &mut HashMap<TreeID, String>|
         -> anyhow::Result<Duration> {
            let pending = std::mem::take(&mut *facts.lock().unwrap());
            assert!(!pending.is_empty(), "a commit produced no facts");
            let t = Instant::now();
            let (changed, settled) =
                doc.with_read(|d| incremental_block_changes(d, &pending, tid_index))?;
            let took = t.elapsed();
            assert!(settled && !changed.is_empty());
            Ok(took)
        };

        let mut edit = Vec::new();
        let mut moved = Vec::new();
        let mut batch = Vec::new();
        for s in 0..SAMPLES {
            let target = nodes[rng.below(nodes.len())];
            doc.with_write(ORIGIN, |txn| {
                let meta = txn.get_tree(TREE_NAME).get_meta(target)?;
                meta.ensure_mergeable_text(CONTENT_RAW)?.insert(0, "x")?;
                Ok(())
            })?;
            edit.push(extract(&doc, &mut tid_index)?);

            let (a, b) = (nodes[rng.below(nodes.len())], nodes[rng.below(nodes.len())]);
            let moved_ok =
                doc.with_write(ORIGIN, |txn| Ok(txn.get_tree(TREE_NAME).mov(a, b).is_ok()))?;
            // A move under the current parent commits no diff.
            if moved_ok && !facts.lock().unwrap().is_empty() {
                moved.push(extract(&doc, &mut tid_index)?);
            }

            if s % 10 == 0 {
                let parent = nodes[rng.below(nodes.len())];
                doc.with_write(ORIGIN, |txn| {
                    for k in 0..100 {
                        add_block(txn, Some(parent), &format!("x{n}-{s}-{k}"))?;
                    }
                    Ok(())
                })?;
                batch.push(extract(&doc, &mut tid_index)?);
            }
        }
        report("extract: field edit (1 fact)", edit);
        report("extract: move", moved);
        report("extract: 100-block batch", batch);

        let commit_once = |doc: &LoroDocument, rng: &mut Rng| -> anyhow::Result<Duration> {
            let target = nodes[rng.below(nodes.len())];
            let t = Instant::now();
            doc.with_write(ORIGIN, |txn| {
                let meta = txn.get_tree(TREE_NAME).get_meta(target)?;
                meta.ensure_mergeable_text(CONTENT_RAW)?.insert(0, "y")?;
                Ok(())
            })?;
            Ok(t.elapsed())
        };
        let mut alone = Vec::new();
        for _ in 0..SAMPLES {
            alone.push(commit_once(&doc, &mut rng)?);
            facts.lock().unwrap().clear();
        }
        report("commit: alone", alone);

        // A reader re-extracting a 100-block batch's facts back to back holds
        // the read guard as long as the heaviest incremental pass does.
        let heavy: Vec<PendingChange> = {
            let parent = nodes[rng.below(nodes.len())];
            doc.with_write(ORIGIN, |txn| {
                for k in 0..100 {
                    add_block(txn, Some(parent), &format!("h{n}-{k}"))?;
                }
                Ok(())
            })?;
            std::mem::take(&mut *facts.lock().unwrap())
        };
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (doc, stop) = (doc.clone(), stop.clone());
            let mut tid_index = tid_index.clone();
            std::thread::spawn(move || -> anyhow::Result<usize> {
                let mut passes = 0;
                while !stop.load(Ordering::Relaxed) {
                    doc.with_read(|d| incremental_block_changes(d, &heavy, &mut tid_index))?;
                    passes += 1;
                }
                Ok(passes)
            })
        };
        let mut contended = Vec::new();
        for _ in 0..SAMPLES {
            contended.push(commit_once(&doc, &mut rng)?);
            facts.lock().unwrap().clear();
        }
        stop.store(true, Ordering::Relaxed);
        let passes = reader.join().unwrap()?;
        report(
            &format!("commit: under a 100-fact reader loop ({passes} passes)"),
            contended,
        );
    }
    Ok(())
}
