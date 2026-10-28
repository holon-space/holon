//! The controller's initial scan is linear in the vault's file count.
//!
//! Each ingest writes one more subject into the write-back net, so a net step
//! whose cost grows with the subject count makes the scan quadratic. Run with
//! `cargo test --release -p holon-orgmode --features di --test
//! initial_scan_cost -- --ignored --nocapture`.

#![cfg(feature = "di")]

use std::time::Duration;

use crate::org_vault_double;

fn vault(files: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..files {
        let mut text = format!("#+TITLE: Page {n}\n#+ID: page-{n}\n\n");
        for h in 0..20 {
            text.push_str(&format!(
                "* Heading {h}\n:PROPERTIES:\n:ID: b-{n}-{h}\n:END:\nBody text of a block.\n"
            ));
        }
        std::fs::write(dir.path().join(format!("p{n}.org")), text).unwrap();
    }
    dir
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: five scans per vault size, release profile"]
async fn the_initial_scan_grows_linearly_with_the_file_count() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let mut medians = Vec::new();
    for files in [1000, 2000, 4000] {
        let dir = vault(files);
        let mut scans: Vec<Duration> = Vec::new();
        for _ in 0..5 {
            scans.push(org_vault_double::initial_scan(dir.path()).await);
        }
        let raw: Vec<_> = scans.iter().map(Duration::as_millis).collect();
        scans.sort();
        println!(
            "initial-scan profile={profile} files={files} raw_ms={raw:?} median_ms={}",
            scans[2].as_millis()
        );
        medians.push(scans[2].as_secs_f64());
    }
    let ratio = medians[2] / medians[0];
    println!("initial-scan profile={profile} ratio_4k_1k={ratio:.2}");
    assert!(
        ratio < 8.0,
        "4x the files took {ratio:.2}x the time: linear is near 4, quadratic near 16"
    );
}
