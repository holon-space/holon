//! Point-at-vault fidelity oracle (env-gated, skipped when unset).
//!
//! `HOLON_PBT_LOCAL_VAULT=<path>` boots the headless SUT on a COPY of a real
//! vault — every `.org` file's CONTENT is re-created inside the harness's own
//! in-memory filesystem/temp dir, so the original vault is only ever READ,
//! never written. It waits for INGEST QUIESCENCE (a bounded poll-until-stable
//! projection count — not a fixed sleep), then asserts a fidelity subset that
//! must hold on real data, INCLUDING that the post-quiescence projected block
//! count lands within a sane tolerance of the count the shape-profile extractor
//! independently measured over the same vault. Unset ⇒ returns immediately, so
//! CI stays green.
//!
//! Run against a real vault locally:
//!   HOLON_PBT_LOCAL_VAULT=/path/to/vault cargo test \
//!     -p holon-integration-tests --features pbt \
//!     --test local_vault_fidelity -- --nocapture
//!
//! The extractor oracle is the checked-in profile; its `depth` histogram counts
//! every parsed block exactly once, so its total is the extractor's block
//! count.
//!
//! KNOWN RED (2026-07-23) — this oracle currently FAILS when pointed at the
//! real vault, and that failure is the deliverable, not a defect in the test.
//! Point-at-vault mode caught a silent ingest gap: the extractor parses 17,315
//! blocks, but the SUT boot projects only ~1,526 (8.8%). Localization
//! (throwaway diagnostic, since deleted): the shortfall is at INGEST/storage
//! (`block_raw` ~= the `block` matview, so NOT a matview bug); ALL ingested
//! rows are `depth=0` while the extractor sees 14,435/17,315 blocks at depth >=
//! 2 — i.e. the whole nested hierarchy is dropped — and only 86 of 985 files
//! yield a `Page` block, with `startup_error_count=0` (a SILENT drop, violating
//! fail-loud). All 985 files ARE registered in the `file` table, so discovery
//! works; block extraction/persistence for nested content is what fails at real
//! scale. Handed back for root-cause; do NOT soften this assertion to hide it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::QueryLanguage;
use holon_api::vault_shape::VaultShapeProfile;
use holon_integration_tests::TestEnvironmentBuilder;
use walkdir::WalkDir;

/// The profile the extractor produced for this vault (checked into the repo).
/// Its `depth.total()` is the extractor's independent block count — the oracle.
const EXTRACTOR_PROFILE_JSON: &str = include_str!("../profiles/martin-vault-2026-07-23.json");

/// Consecutive equal count readings that declare the projection quiescent.
const STABLE_POLLS_REQUIRED: u32 = 5;
/// Interval between count polls (a measurement cadence, NOT the settle
/// mechanism — quiescence is declared by count stability, not by elapsed time).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Generous overall ceiling on the poll loop.
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(240);
/// Post-quiescence projected count must be at least this fraction of the
/// extractor's block count. A real vault projects page roots ON TOP of the
/// extractor's headline blocks, so the SUT count is expected to MEET or exceed
/// the extractor count; 0.70 leaves generous room for legitimate non-block
/// content differences while still catching a "wildly short" ingest gap.
const MIN_FIDELITY_RATIO: f64 = 0.70;

#[test]
fn local_vault_fidelity_oracle() {
    let Ok(vault) = std::env::var("HOLON_PBT_LOCAL_VAULT") else {
        eprintln!("[local-vault] HOLON_PBT_LOCAL_VAULT unset — skipped (CI-safe)");
        return;
    };
    let vault = Path::new(&vault);
    assert!(
        vault.is_dir(),
        "HOLON_PBT_LOCAL_VAULT is not a directory: {}",
        vault.display()
    );

    let profile: VaultShapeProfile =
        serde_json::from_str(EXTRACTOR_PROFILE_JSON).expect("checked-in profile parses");
    let extractor_blocks = profile.depth.total() as usize;

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    let rt2 = rt.clone();
    rt.block_on(async move {
        // Seed the SUT from a COPY of the vault: read each file's content and
        // hand it to the builder, which re-creates it in a fresh temp/in-memory
        // FS. The original vault is never opened for writing.
        let mut builder = TestEnvironmentBuilder::new();
        let mut file_count = 0usize;
        for entry in WalkDir::new(vault).sort_by_file_name() {
            let entry = entry.expect("walk vault");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("org") {
                continue;
            }
            let rel = path
                .strip_prefix(vault)
                .expect("path under vault")
                .to_string_lossy()
                .to_string();
            let content = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            builder = builder.with_org_file(rel, content);
            file_count += 1;
        }
        assert!(
            file_count > 0,
            "no .org files found under {}",
            vault.display()
        );
        eprintln!(
            "[local-vault] booting SUT on a copy of {file_count} files (extractor saw \
             {extractor_blocks} blocks)…"
        );

        let env = builder
            .build(rt2)
            .await
            .unwrap_or_else(|e| panic!("SUT boot on real vault failed: {e:#}"));

        // ─── Wait for ingest quiescence (bounded poll-until-stable) ──────
        // The org-scan idle signal caps its own budget at ~2s, which is far too
        // short for a 985-file cold vault, so it is NOT the mechanism here.
        // Instead poll the projected block count until it stops growing for
        // STABLE_POLLS_REQUIRED consecutive reads (or the generous timeout).
        let start = Instant::now();
        let mut last_count = usize::MAX;
        let mut stable = 0u32;
        let mut final_rows;
        loop {
            final_rows = env
                .query("from block | select {id}", QueryLanguage::HolonPrql)
                .await
                .unwrap_or_else(|e| panic!("projection query failed: {e:#}"));
            let n = final_rows.len();
            eprintln!(
                "[local-vault] poll t={:>5.1}s count={n} (stable {stable}/{STABLE_POLLS_REQUIRED})",
                start.elapsed().as_secs_f64()
            );
            if n == last_count {
                stable += 1;
            } else {
                stable = 0;
                last_count = n;
            }
            if stable >= STABLE_POLLS_REQUIRED {
                eprintln!(
                    "[local-vault] QUIESCED at count={n} after {:.1}s",
                    start.elapsed().as_secs_f64()
                );
                break;
            }
            if start.elapsed() >= QUIESCENCE_TIMEOUT {
                eprintln!(
                    "[local-vault] TIMEOUT at count={n} after {:.1}s (never stabilised)",
                    start.elapsed().as_secs_f64()
                );
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let converged = final_rows.len();

        // ─── Fidelity subset (must hold on real data) ────────────────────

        // (1) Boot clean: no swallowed startup errors ingesting a real vault.
        assert_eq!(
            env.startup_error_count(),
            0,
            "SUT reported {} startup error(s) ingesting the real vault",
            env.startup_error_count(),
        );
        assert!(
            !env.has_startup_errors(),
            "SUT has startup errors on real vault"
        );

        // (2) No duplicate block IDs across the whole projection.
        let mut seen = HashSet::new();
        let mut dupes = 0usize;
        for row in &final_rows {
            let id = row
                .get("id")
                .and_then(|v| v.as_string())
                .expect("block row has an id");
            if !seen.insert(id) {
                dupes += 1;
            }
        }
        assert_eq!(
            dupes, 0,
            "{dupes} duplicate block id(s) in the real-vault projection"
        );

        // (3) Fidelity oracle: post-quiescence count vs the extractor count.
        let ratio = converged as f64 / extractor_blocks as f64;
        eprintln!(
            "[local-vault] VERDICT — files={file_count} converged_blocks={converged} \
             extractor_blocks={extractor_blocks} ratio={ratio:.3} (floor {MIN_FIDELITY_RATIO}) \
             dupes=0 boot=clean"
        );
        assert!(
            ratio >= MIN_FIDELITY_RATIO,
            "real-vault projection converged at {converged} blocks — only {:.1}% of the \
             {extractor_blocks} blocks the extractor independently parsed. A wildly short \
             post-quiescence projection is a real ingest gap caught by point-at-vault mode.",
            ratio * 100.0
        );

        eprintln!("[local-vault] PASS — fidelity oracle within tolerance.");
    });
}
