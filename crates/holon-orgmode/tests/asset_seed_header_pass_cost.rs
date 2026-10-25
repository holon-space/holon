//! The cost of the asset seed's `#+ID` pass on a large synthetic vault.
//!
//! The seed reads every org file's header (up to its first headline) before the
//! initial sync may start, so its time is a delay on the first projection.
//! The bar is its share of the controller's initial scan on the same vault.
//! Prints both; run with
//! `cargo test -p holon-orgmode --features di --test
//! asset_seed_header_pass_cost -- --ignored --nocapture`.

#![cfg(feature = "di")]

use std::time::Duration;
use std::time::Instant;

use holon_filesystem::FileSystem;
use holon_filesystem::RealFileSystem;
use holon_orgmode::di::OrgModeConfig;
use holon_orgmode::di::seed_default_org_assets;

mod org_vault_double;

const ASSET: &str = "#+TITLE: Journals\n#+ID: journals\n";

fn page(n: usize, declares_asset: bool) -> String {
    let id = if declares_asset {
        "journals".to_string()
    } else {
        format!("page-{n}")
    };
    let mut text = format!("#+TITLE: Page {n}\n#+ID: {id}\n\n");
    for h in 0..20 {
        text.push_str(&format!(
            "* Heading {h}\n:PROPERTIES:\n:ID: b-{n}-{h}\n:END:\n"
        ));
        text.push_str(&"Body text of a block in a synthetic page. ".repeat(4));
        text.push('\n');
    }
    text
}

fn vault(files: usize, with_declarer: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Flat: the store doubles hold one tree per file, not folder pages.
    for n in 0..files {
        std::fs::write(
            dir.path().join(format!("p{n}.org")),
            page(n, with_declarer && n == files / 2),
        )
        .unwrap();
    }
    dir
}

async fn seed(dir: &tempfile::TempDir) -> Duration {
    let mut config = OrgModeConfig::new(dir.path().to_path_buf());
    config.seed_assets = vec![("Journals.org".to_string(), ASSET.to_string())];
    let start = Instant::now();
    seed_default_org_assets(&RealFileSystem, &config)
        .await
        .unwrap();
    start.elapsed()
}

/// The controller's initial scan as the file-sync task runs it: initialize, the
/// whole-file read of every file, then one ingest per file.
async fn initial_scan(dir: &tempfile::TempDir) -> Duration {
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (mut controller, _store, _disclosures, _net) =
        org_vault_double::controller_at(root.clone(), &org_vault_double::PageStore::default());
    let start = Instant::now();
    controller.initialize().await.unwrap();
    let files = RealFileSystem.scan_directory(&root).await.unwrap().files;
    futures::future::try_join_all(files.iter().map(|p| RealFileSystem.read_to_string(p)))
        .await
        .unwrap();
    controller.begin_initial_scan();
    for path in &files {
        let outcome = controller
            .on_file_changed(path)
            .await
            .unwrap_or_else(|e| panic!("ingest {}: {e:#}", path.display()));
        assert!(
            matches!(outcome, holon_filesystem::IngestOutcome::Ingested),
            "{} was not ingested: {outcome:?}",
            path.display()
        );
    }
    controller.finish_initial_scan(30_000).await.unwrap();
    start.elapsed()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; prints the seed's share of the initial scan per vault size"]
async fn the_header_pass_share_of_the_initial_scan() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    // The scan's cost grows with the square of the file count, so a run picks its
    // sizes.
    let sizes = std::env::var("HEADER_PASS_FILES").unwrap_or_else(|_| "1000,5000,20000".into());
    for files in sizes
        .split(',')
        .map(|n| n.parse::<usize>().expect("HEADER_PASS_FILES lists counts"))
    {
        let declared = vault(files, true);
        let mut passes: Vec<Duration> = Vec::new();
        let mut scans: Vec<Duration> = Vec::new();
        for _ in 0..5 {
            passes.push(seed(&declared).await);
            scans.push(initial_scan(&declared).await);
        }
        assert!(
            !declared.path().join("Journals.org").exists(),
            "a vault file declares the asset id, so the seed must not write it"
        );
        let ms = |runs: &[Duration]| runs.iter().map(|d| d.as_millis()).collect::<Vec<_>>();
        println!(
            "header-pass profile={profile} files={files} raw_pass_ms={:?} raw_scan_ms={:?}",
            ms(&passes),
            ms(&scans)
        );
        passes.sort();
        scans.sort();
        let (pass, scan) = (passes[2], scans[2]);
        println!(
            "header-pass profile={profile} files={files} pass_median_ms={} scan_median_ms={} \
             share_pct={:.1}",
            pass.as_millis(),
            scan.as_millis(),
            100.0 * pass.as_secs_f64() / scan.as_secs_f64()
        );

        let undeclared = vault(files, false);
        seed(&undeclared).await;
        assert!(undeclared.path().join("Journals.org").exists());
    }
}
