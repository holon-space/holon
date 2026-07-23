//! Point-at-vault fidelity spot-check (env-gated, skipped when unset).
//!
//! `HOLON_PBT_LOCAL_VAULT=<path>` boots the headless SUT on a COPY of a real
//! vault — every `.org` file's CONTENT is re-created inside the harness's own
//! in-memory filesystem/temp dir, so the original vault is only ever READ,
//! never written. It then asserts a small fidelity subset that must hold on
//! real data. Unset ⇒ the test returns immediately, so CI stays green.
//!
//! Run against a real vault locally:
//!   HOLON_PBT_LOCAL_VAULT=/path/to/vault cargo test \
//!     -p holon-integration-tests --features pbt \
//!     --test local_vault_fidelity -- --nocapture

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_api::QueryLanguage;
use holon_integration_tests::TestEnvironmentBuilder;
use walkdir::WalkDir;

#[test]
fn local_vault_fidelity_spot_check() {
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
        eprintln!("[local-vault] booting SUT on a copy of {file_count} files…");

        let env = builder
            .build(rt2)
            .await
            .unwrap_or_else(|e| panic!("SUT boot on real vault failed: {e:#}"));

        // ─── Fidelity subset (must hold on real data) ────────────────────

        // (1) Boot clean: no swallowed startup errors while ingesting a real
        //     vault (a real-data-only failure the synthetic keystone can miss).
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

        // (2) Non-empty projection: a real vault must project blocks.
        let rows = env
            .query("from block | select {id}", QueryLanguage::HolonPrql)
            .await
            .unwrap_or_else(|e| panic!("projection query failed: {e:#}"));
        assert!(!rows.is_empty(), "real vault projected zero blocks");

        // (3) No duplicate block IDs across the whole projection.
        let mut seen = HashSet::new();
        let mut dupes = 0usize;
        for row in &rows {
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

        eprintln!(
            "[local-vault] PASS — {file_count} files, {} blocks projected, 0 dupes, boot clean.",
            rows.len(),
        );
    });
}
