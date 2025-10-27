//! Benchmark: Loro binary storage overhead for different image sizes.
//!
//! Measures memory overhead, export size, sync round-trip time, and operation
//! history growth when storing binary blobs (simulating pasted images) in Loro.

use std::time::Instant;

use loro::{ExportMode, LoroDoc, LoroValue};

fn random_bytes(size: usize) -> Vec<u8> {
    // Pseudo-random but deterministic — compressibility similar to PNG
    let mut buf = vec![0u8; size];
    let mut state: u64 = 0xdeadbeef;
    for byte in buf.iter_mut() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (state >> 33) as u8;
    }
    buf
}

struct BenchResult {
    label: &'static str,
    raw_bytes: usize,
    snapshot_bytes: usize,
    overhead_pct: f64,
    insert_us: u128,
    export_us: u128,
    import_us: u128,
    ops_after_insert: usize,
    update_export_bytes: usize,
    update_export_us: u128,
    ops_after_update: usize,
}

fn bench_single_binary(label: &'static str, size: usize) -> BenchResult {
    let data = random_bytes(size);

    // --- Insert binary into a LoroMap (simulating block metadata) ---
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    let t0 = Instant::now();
    let node = tree.create(None).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert("stable_id", LoroValue::from("test-block-001"))
        .unwrap();
    meta.insert("content_type", LoroValue::from("image"))
        .unwrap();
    meta.insert("image_data", LoroValue::from(data.clone()))
        .unwrap();
    doc.commit();
    let insert_us = t0.elapsed().as_micros();
    let ops_after_insert = doc.len_ops();

    // --- Full snapshot export ---
    let t1 = Instant::now();
    let snapshot = doc.export(ExportMode::Snapshot).unwrap();
    let export_us = t1.elapsed().as_micros();
    let snapshot_bytes = snapshot.len();
    let overhead_pct = ((snapshot_bytes as f64 / size as f64) - 1.0) * 100.0;

    // --- Import into a fresh doc (simulating sync receive) ---
    let doc2 = LoroDoc::new();
    let t2 = Instant::now();
    doc2.import(&snapshot).unwrap();
    let import_us = t2.elapsed().as_micros();

    // Verify round-trip
    let tree2 = doc2.get_tree("blocks");
    let nodes = tree2
        .children(None)
        .expect("children should exist after import");
    assert_eq!(nodes.len(), 1, "expected 1 node after import");
    let meta2 = tree2.get_meta(nodes[0]).unwrap();
    let rt_data = meta2.get("image_data").unwrap();
    match rt_data {
        loro::ValueOrContainer::Value(LoroValue::Binary(b)) => {
            assert_eq!(b.len(), size, "round-trip size mismatch")
        }
        other => panic!("expected Binary, got {:?}", other),
    }

    // --- Simulate updating the image (re-paste) ---
    let new_data = random_bytes(size);
    let vv_before = doc.oplog_vv();
    meta.insert("image_data", LoroValue::from(new_data))
        .unwrap();
    doc.commit();
    let ops_after_update = doc.len_ops();

    // Incremental update export (delta since before the update)
    let t3 = Instant::now();
    let update = doc.export(ExportMode::updates(&vv_before)).unwrap();
    let update_export_us = t3.elapsed().as_micros();
    let update_export_bytes = update.len();

    BenchResult {
        label,
        raw_bytes: size,
        snapshot_bytes,
        overhead_pct,
        insert_us,
        export_us,
        import_us,
        ops_after_insert,
        update_export_bytes,
        update_export_us,
        ops_after_update,
    }
}

fn bench_multiple_images_in_one_doc(count: usize, size_each: usize) -> (usize, usize, u128) {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");

    let t0 = Instant::now();
    for i in 0..count {
        let data = random_bytes(size_each);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert("stable_id", LoroValue::from(format!("img-{i}")))
            .unwrap();
        meta.insert("content_type", LoroValue::from("image"))
            .unwrap();
        meta.insert("image_data", LoroValue::from(data)).unwrap();
    }
    doc.commit();
    let insert_us = t0.elapsed().as_micros();

    let snapshot = doc.export(ExportMode::Snapshot).unwrap();
    (doc.len_ops(), snapshot.len(), insert_us)
}

#[test]
fn loro_binary_storage_benchmark() {
    println!("\n{}", "=".repeat(80));
    println!("  Loro Binary Storage Benchmark");
    println!("{}\n", "=".repeat(80));

    let sizes: Vec<(&str, usize)> = vec![
        ("10 KB", 10 * 1024),
        ("100 KB", 100 * 1024),
        ("500 KB", 500 * 1024),
        ("1 MB", 1024 * 1024),
        ("5 MB", 5 * 1024 * 1024),
    ];

    println!("--- Single image per document ---\n");
    println!(
        "{:<10} {:>10} {:>12} {:>9} {:>10} {:>10} {:>10} {:>6} {:>12} {:>6}",
        "Size",
        "Raw",
        "Snapshot",
        "Overhead",
        "Insert",
        "Export",
        "Import",
        "Ops",
        "Update Δ",
        "Ops'"
    );
    println!("{}", "-".repeat(106));

    let mut results = Vec::new();
    for (label, size) in &sizes {
        let r = bench_single_binary(label, *size);
        println!(
            "{:<10} {:>10} {:>12} {:>8.1}% {:>8}µs {:>8}µs {:>8}µs {:>6} {:>12} {:>6}",
            r.label,
            fmt_bytes(r.raw_bytes),
            fmt_bytes(r.snapshot_bytes),
            r.overhead_pct,
            r.insert_us,
            r.export_us,
            r.import_us,
            r.ops_after_insert,
            fmt_bytes(r.update_export_bytes),
            r.ops_after_update,
        );
        results.push(r);
    }

    println!("\n--- Multiple 100KB images in one document ---\n");
    println!(
        "{:<12} {:>10} {:>12} {:>10} {:>10}",
        "Count", "Total Raw", "Snapshot", "Overhead", "Insert"
    );
    println!("{}", "-".repeat(58));

    for count in [1, 5, 10, 20, 50] {
        let size_each = 100 * 1024;
        let (ops, snap_size, insert_us) = bench_multiple_images_in_one_doc(count, size_each);
        let total_raw = count * size_each;
        let overhead_pct = ((snap_size as f64 / total_raw as f64) - 1.0) * 100.0;
        println!(
            "{:<12} {:>10} {:>12} {:>8.1}%  {:>8}µs  (ops: {})",
            format!("{count}x 100KB"),
            fmt_bytes(total_raw),
            fmt_bytes(snap_size),
            overhead_pct,
            insert_us,
            ops,
        );
    }

    println!("\n--- Multiple 500KB images in one document ---\n");
    println!(
        "{:<12} {:>10} {:>12} {:>10} {:>10}",
        "Count", "Total Raw", "Snapshot", "Overhead", "Insert"
    );
    println!("{}", "-".repeat(58));

    for count in [1, 5, 10, 20] {
        let size_each = 500 * 1024;
        let (ops, snap_size, insert_us) = bench_multiple_images_in_one_doc(count, size_each);
        let total_raw = count * size_each;
        let overhead_pct = ((snap_size as f64 / total_raw as f64) - 1.0) * 100.0;
        println!(
            "{:<12} {:>10} {:>12} {:>8.1}%  {:>8}µs  (ops: {})",
            format!("{count}x 500KB"),
            fmt_bytes(total_raw),
            fmt_bytes(snap_size),
            overhead_pct,
            insert_us,
            ops,
        );
    }

    println!("\n--- Update overhead: replacing image N times ---\n");
    println!(
        "{:<12} {:>12} {:>14} {:>14} {:>8}",
        "Revisions", "Snapshot", "All Updates", "Last Update Δ", "Ops"
    );
    println!("{}", "-".repeat(64));

    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    let node = tree.create(None).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert("stable_id", LoroValue::from("test-img"))
        .unwrap();
    meta.insert("content_type", LoroValue::from("image"))
        .unwrap();

    for rev in [1, 2, 5, 10, 20] {
        let single_doc = LoroDoc::new();
        let t = single_doc.get_tree("blocks");
        let n = t.create(None).unwrap();
        let m = t.get_meta(n).unwrap();
        m.insert("stable_id", LoroValue::from("test-img")).unwrap();

        for i in 0..rev {
            let data = random_bytes(100 * 1024); // 100KB each revision
            let vv = single_doc.oplog_vv();
            m.insert("image_data", LoroValue::from(data)).unwrap();
            single_doc.commit();

            if i == rev - 1 {
                let snap = single_doc.export(ExportMode::Snapshot).unwrap();
                let all = single_doc.export(ExportMode::all_updates()).unwrap();
                let delta = single_doc.export(ExportMode::updates(&vv)).unwrap();
                println!(
                    "{:<12} {:>12} {:>14} {:>14} {:>8}",
                    format!("{rev}x 100KB"),
                    fmt_bytes(snap.len()),
                    fmt_bytes(all.len()),
                    fmt_bytes(delta.len()),
                    single_doc.len_ops(),
                );
            }
        }
    }

    println!();
}

fn fmt_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
