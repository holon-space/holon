//! Measurement: how long can a keystroke wait for the document write lock?
//!
//! `LoroTextCellBacking` takes the doc lock so a commit carries exactly one
//! writer's ops. On the desktop frontend that acquire happens on the GPUI main
//! thread inside the keystroke handler, so the wait is UI freeze. The
//! interaction budget is the p95 interaction-to-visible SLO of 200 ms, and the
//! longest concurrent holds on a vault document are the snapshot saves:
//! `LoroDocument::export_snapshot` holds the READ guard across a full-document
//! export, and `export_compact_snapshot` — every 64th `save_all` — holds the
//! WRITE guard across a shallow-snapshot export.
//!
//! Run it (it is excluded from the gates because it is a soak):
//! `cargo nextest run -p holon-loro keystroke_lock_wait --run-ignored all`

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use holon_core::cell::TextCellBacking;
use holon_core::cell::TextOp;
use loro::LoroDoc;

use crate::LoroDocument;
use crate::loro_text_cell_backing::LoroTextCellBacking;

/// Blocks in the soak document. The dogfood vault is ~3000 blocks.
const BLOCKS: usize = 3200;
/// Keystrokes timed against a save loop.
const KEYSTROKES: usize = 200;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn report(label: &str, mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let max = *samples.last().unwrap();
    println!(
        "{label}: n={} p50={:.2}ms p95={:.2}ms max={:.2}ms",
        samples.len(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0
    );
    max
}

/// A document of `BLOCKS` tree nodes, each carrying a text container, plus one
/// extra node whose text the keystrokes type into.
fn soak_doc() -> (LoroDocument, Arc<LoroDoc>, loro::LoroText) {
    let doc = Arc::new(LoroDoc::new());
    doc.set_peer_id(1).unwrap();
    let document = LoroDocument::from_existing(doc.clone(), "soak");
    let typed = document
        .with_write(
            crate::write_origin::WriteOrigin::Probe("soak_seed"),
            |txn| {
                let tree = txn.get_tree("blocks");
                tree.enable_fractional_index(0);
                let mut typed = None;
                for i in 0..=BLOCKS {
                    let node = tree.create(None)?;
                    let text = tree.get_meta(node)?.ensure_mergeable_text("content_raw")?;
                    text.insert(
                        0,
                        &format!("block {i} with a realistic amount of title text"),
                    )?;
                    if i == BLOCKS {
                        typed = Some(text);
                    }
                }
                Ok(typed.unwrap())
            },
        )
        .unwrap();
    (document, doc, typed)
}

#[test]
#[ignore = "soak measurement; run with --run-ignored all"]
fn keystroke_lock_wait_against_a_concurrent_snapshot_save() {
    let (document, doc, typed) = soak_doc();

    let full: Vec<Duration> = (0..20)
        .map(|_| {
            let t = Instant::now();
            document.export_snapshot().unwrap();
            t.elapsed()
        })
        .collect();
    let worst_read_hold = report("export_snapshot (READ guard held)", full);

    let compact: Vec<Duration> = (0..20)
        .map(|_| {
            let t = Instant::now();
            document.export_compact_snapshot().unwrap();
            t.elapsed()
        })
        .collect();
    let worst_write_hold = report("export_compact_snapshot (WRITE guard held)", compact);

    let backing = LoroTextCellBacking::new(doc, typed).unwrap();

    // Control: the same keystroke with nothing else on the document. What the
    // contended run costs ABOVE this is the lock wait; the rest is the commit
    // itself, which the keystroke paid before the lock existed too.
    let quiet: Vec<Duration> = (0..KEYSTROKES)
        .map(|_| {
            let t = Instant::now();
            backing
                .apply_text_op(TextOp::Insert {
                    pos_codepoint: 0,
                    text: "q".to_string(),
                })
                .unwrap();
            t.elapsed()
        })
        .collect();
    report("keystroke apply_text_op (uncontended control)", quiet);

    let stop = Arc::new(AtomicBool::new(false));
    let saver_stop = stop.clone();
    let saving = document;
    let saver = std::thread::spawn(move || {
        let mut n = 0u64;
        while !saver_stop.load(Ordering::Relaxed) {
            // Mirrors `LoroDocumentStore::save_all`: every 64th save compacts.
            if n.is_multiple_of(64) {
                saving.export_compact_snapshot().unwrap();
            } else {
                saving.export_snapshot().unwrap();
            }
            n += 1;
        }
    });

    let mut waits = Vec::with_capacity(KEYSTROKES);
    for i in 0..KEYSTROKES {
        let t = Instant::now();
        backing
            .apply_text_op(TextOp::Insert {
                pos_codepoint: 0,
                text: "x".to_string(),
            })
            .unwrap();
        waits.push(t.elapsed());
        if i % 20 == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    stop.store(true, Ordering::Relaxed);
    saver.join().unwrap();

    let worst_keystroke = report("keystroke apply_text_op (lock wait + commit)", waits);

    println!(
        "SLO 200ms · worst read hold {:.2}ms · worst write hold {:.2}ms · worst keystroke {:.2}ms",
        worst_read_hold.as_secs_f64() * 1000.0,
        worst_write_hold.as_secs_f64() * 1000.0,
        worst_keystroke.as_secs_f64() * 1000.0
    );
}
