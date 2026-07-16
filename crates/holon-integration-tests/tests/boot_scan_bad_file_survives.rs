//! Regression for the dogfood 2026-07-10 boot-scan ship-blocker.
//!
//! Symptom: booting against a real vault, OrgMode's initial scan failed to
//! ingest ONE file and then a detached tokio worker PANICKED
//! (`crates/holon-app/src/wiring.rs`, "OrgMode startup failed: ..."). The
//! window kept running and looked healthy, but file sync was DEAD for EVERY
//! file — subsequent vault edits were silently ignored, with no user-visible
//! signal.
//!
//! Root cause (systemic): `holon-orgmode` `run_file_sync_controller` collected
//! per-file scan failures and then `return`ed BEFORE arming the notify watch
//! loop, so a single bad file left runtime sync dead for the whole vault; and
//! the wiring layer turned the aggregated error into a `panic!` on a detached
//! worker (invisible, non-fatal to the window).
//!
//! A file that reliably fails its initial-scan feed barrier: a block whose id
//! is OWNED by another document (a duplicate `:ID:`, or a `seed_default_layout`
//! id) presented as a child of a fresh `:ID:` parent — it is re-parented via
//! `update_in_tree` during the scan and trips the per-file feed catch-up. This
//! is the deterministic stand-in for the real vault's bad file.
//!
//! After the fix: boot must NOT panic, the OTHER files must still ingest, and
//! runtime sync (the armed watch loop) must survive so a post-boot edit lands.
//!
//! @pbt kind harness
//! @pbt covers boot-scan-bad-file — boot survives a malformed vault file
//! (dogfood 2026-07-10) @pbt overlaps general_e2e_composed_pbt — kept: no
//! boot-scan-of-bad-vault path in keystone

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

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

// A clean file that ingests without trouble.
const GOOD_FILE: &str = "\
* Good Root
:PROPERTIES:
:ID: good-root
:END:
** Good Child
:PROPERTIES:
:ID: good-child
:END:
";

// `aaa_owner.org` OWNS block:shared-child under its own root, so when
// `zzz_bad.org` (scanned later) presents the SAME id as a child of a fresh
// `:ID: <uuid>` parent, that child is re-parented across documents during the
// initial scan and reliably trips the per-file feed catch-up — a deterministic
// stand-in for the real vault's bad file.
const OWNER_FILE: &str = "\
* Other Root
:PROPERTIES:
:ID: other-root
:END:
** Shared
:PROPERTIES:
:ID: shared-child
:END:
";

const BAD_FILE: &str = "\
* Bad Top
:PROPERTIES:
:ID: bad-top
:END:
** Parent
:PROPERTIES:
:ID: 66666666-6666-6666-6666-666666666666
:END:
*** Shared
:PROPERTIES:
:ID: shared-child
:END:
";

/// Core regression: a vault containing one file that fails its initial scan
/// must still (a) boot WITHOUT panicking, (b) ingest the OTHER files, and
/// (c) leave the runtime watch loop armed so a post-boot edit still syncs.
#[test]
fn one_bad_file_does_not_kill_sync_for_others() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        // build() waits for the file watcher; before the fix this PANICKED
        // (the detached post_ready worker `panic!`) — build would unwind.
        let env = TestEnvironmentBuilder::new()
            .with_org_file("aaa_owner.org", OWNER_FILE)
            .with_org_file("mmm_good.org", GOOD_FILE)
            .with_org_file("zzz_bad.org", BAD_FILE)
            .build(rt.clone())
            .await
            .expect("boot must NOT panic when one vault file fails its initial scan");

        // (b) The clean file's blocks ingested despite the bad file.
        for id in ["good-root", "good-child", "other-root"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "clean-file block {id} did not sync — a bad file killed sync for others"
            );
        }

        // (c) Runtime sync survived: the watch loop is armed, so a NEW file
        // created after boot still ingests. Before the fix, di.rs returned
        // before arming the loop, so this write was silently ignored.
        env.write_org_file(
            "post_boot.org",
            "* Post Boot\n:PROPERTIES:\n:ID: post-boot-block\n:END:\n",
        )
        .await
        .expect("write post-boot file");
        assert!(
            env.wait_for_block("block:post-boot-block", SYNC_TIMEOUT)
                .await,
            "post-boot edit did not sync — the file-watch loop was not armed (sync is dead after \
             a bad file)"
        );
    });
}
