//! Focused reproducer for the phantom-Loro-exists race that blocks the
//! Phase 3.7 gate flip.
//!
//! Background — `devlog/2026-05-11-phantom-loro-exists-investigation.md`:
//! when a BulkExternalAdd writes a file with N sibling blocks, the first
//! flows through `apply_create` normally but the remaining N-1 take the
//! early-bypass to `apply_update_with_backend` because Loro reports they
//! already exist. The typed positional plumbing (Phase 3.7) is correct;
//! the bug is that some other writer is racing the inbound CDC path into
//! the Loro tree.
//!
//! This test collapses the 25-min Full PBT to seconds for the
//! bulk-create case specifically. It uses the production
//! `TestEnvironmentBuilder` (which wires `OrgFileWatcher` +
//! `FileSyncController` + `LoroSyncController` end-to-end) and writes a
//! 5-block bulk add under one parent. The instrumentation added in the
//! handoff (search for `[PHANTOM-LORO-TRACE]`) fires:
//!
//!   - `LoroBlockOperations::create` — should NEVER fire in production.
//!   - `LoroDocumentStore::get_global_doc` snapshot exists check —
//!     should report `exists=false` for a fresh temp dir.
//!   - `find_tree_id_by_stable_id` matched node — fires for every
//!     `resolve_to_tree_id` slow-path hit; the matched TreeID's peer
//!     and counter identify the writer.
//!
//! Run with:
//!   RUST_LOG=error cargo test -p holon-integration-tests \
//!     --test phantom_loro_exists_repro -- --nocapture
//!
//! If `[PHANTOM-LORO-TRACE]` appears for `bulk-0-N` (N >= 1) BEFORE
//! the corresponding `apply_create` call in the same batch, the race is
//! reproduced. The matched `tree_id` in the trace identifies the writer.

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

/// Initialise a tracing subscriber that prints `error!` (and below if
/// RUST_LOG widens it) to stderr. Without this, the `[PHANTOM-LORO-TRACE]`
/// `tracing::error!` calls land in /dev/null. `try_init` is fine to call
/// multiple times — only the first wins.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Five sibling blocks under one parent doc, written in a single org file
/// commit — mirrors the PBT's BulkExternalAdd shape exactly.
const BULK_ORG_CONTENT: &str = "\
* Block 0
:PROPERTIES:
:ID: bulk-0-0
:END:
* Block 1
:PROPERTIES:
:ID: bulk-0-1
:END:
* Block 2
:PROPERTIES:
:ID: bulk-0-2
:END:
* Block 3
:PROPERTIES:
:ID: bulk-0-3
:END:
* Block 4
:PROPERTIES:
:ID: bulk-0-4
:END:
";

#[test]
fn bulk_add_five_siblings_under_one_parent_at_startup() {
    init_tracing();
    let rt = runtime();
    rt.block_on(async {
        // Pre-populate the org file BEFORE engine start. This hits the
        // OrgFileWatcher initial-scan path, which calls on_file_changed
        // before signal_ready — same code path BulkExternalAdd's mid-test
        // write hits after the watcher is armed.
        let env = TestEnvironmentBuilder::new()
            .with_org_file("bulk.org", BULK_ORG_CONTENT)
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        // Wait for every block to land in SQL.
        for i in 0..5 {
            let id = format!("bulk-0-{i}");
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "block {} did not sync from org file to backend within {:?}",
                id,
                SYNC_TIMEOUT
            );
        }

        // If `[PHANTOM-LORO-TRACE]` lines appeared on stderr during this
        // test, the writer is identified — see the matched TreeID's peer
        // + counter. If no traces fired, the race is more specific to the
        // PBT's multi-transition sequencing, and step-5 (full PBT replay)
        // is required.
    });
}

/// Same as above but TWO consecutive bulk batches. The PBT often produces
/// the failure on the SECOND BulkExternalAdd because the parent's child
/// set has already been canonicalised once. If the first batch lands
/// cleanly but the second triggers `[PHANTOM-LORO-TRACE]`, the bug
/// surface is "redo against existing siblings", not "first batch".
#[test]
fn two_consecutive_bulk_batches_under_one_parent() {
    init_tracing();
    let rt = runtime();
    rt.block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        // Batch 1: 3 siblings.
        env.write_org_file(
            "bulk.org",
            "\
* A
:PROPERTIES:
:ID: bulk-A
:END:
* B
:PROPERTIES:
:ID: bulk-B
:END:
* C
:PROPERTIES:
:ID: bulk-C
:END:
",
        )
        .await
        .expect("write batch 1");
        for id in ["bulk-A", "bulk-B", "bulk-C"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "batch 1: block {} did not sync",
                id
            );
        }

        // Batch 2: extend with 5 more siblings; existing 3 stay.
        env.write_org_file(
            "bulk.org",
            "\
* A
:PROPERTIES:
:ID: bulk-A
:END:
* B
:PROPERTIES:
:ID: bulk-B
:END:
* C
:PROPERTIES:
:ID: bulk-C
:END:
* D
:PROPERTIES:
:ID: bulk-D
:END:
* E
:PROPERTIES:
:ID: bulk-E
:END:
* F
:PROPERTIES:
:ID: bulk-F
:END:
* G
:PROPERTIES:
:ID: bulk-G
:END:
* H
:PROPERTIES:
:ID: bulk-H
:END:
",
        )
        .await
        .expect("write batch 2");
        for id in ["bulk-D", "bulk-E", "bulk-F", "bulk-G", "bulk-H"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "batch 2: block {} did not sync",
                id
            );
        }
    });
}
