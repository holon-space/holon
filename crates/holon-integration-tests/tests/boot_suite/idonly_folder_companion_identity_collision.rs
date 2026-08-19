//! Regression for the dogfood 2026-07-28 boot identity-collision cluster.
//!
//! Symptom (cold start on the real vault): six ERRORs of the form
//! `holon-identity-collision: id block:<id> is already held by a different
//! entity (held title "", requested "Music")` quarantined three org files, so
//! write-back for them was dead.
//!
//! Shape that produces it: a folder-companion `#+ID:`-only org file (`X.org`
//! next to a directory `X/`). Such a file is the NORMAL render of a
//! child-less page whose title equals its file stem — the org renderer emits
//! `#+TITLE:` only for an explicit title property, so `#+ID: <uuid>` alone is
//! healthy output, not corruption.
//!
//! @pbt kind harness
//! @pbt covers idonly-folder-companion-identity-collision — boot must not
//! refuse a page create at an id an empty tagless placeholder already holds
//! (dogfood 2026-07-28)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone generator
//! alphabet has no `#+ID:`-only folder-companion file shape

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
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

const AREAS_ID: &str = "3092ec5e-dd31-497f-938e-4cf8b26a409f";
const MUSIC_ID: &str = "a9e683b4-1111-4222-8333-444444444444";
const AUDIO_ID: &str = "b7c1d2e3-5555-4666-8777-888888888888";

fn idonly(id: &str) -> String {
    format!("#+ID: {id}\n")
}

/// Every folder-companion page must land in the store carrying its
/// filename-derived title — never an empty-content row, and never refused by
/// the identity minter.
#[test]
fn idonly_folder_companions_ingest_with_their_filename_titles() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Areas.org", idonly(AREAS_ID))
            .with_org_file("Areas/Music.org", idonly(MUSIC_ID))
            .with_org_file("Areas/Music/Audio Processing.org", idonly(AUDIO_ID))
            .build(rt.clone())
            .await
            .expect("boot over an id-only folder-companion vault");

        let expected: HashSet<EntityUri> = [AREAS_ID, MUSIC_ID, AUDIO_ID]
            .iter()
            .map(|i| EntityUri::block(i))
            .collect();
        env.wait_for_blocks_synced(&expected, Duration::from_secs(20))
            .await;
        env.wait_for_org_files_stable(150, Duration::from_secs(20))
            .await;
        assert_titles(&env).await;

        // The org files must still sit where the user put them. A page left
        // stranded at the tree root renders its file at the vault root, which
        // RELOCATES `Areas/Music.org` to `Music.org`.
        let scanned = holon_filesystem::FileSystem::scan_directory(
            env.org_fs.as_ref(),
            &env.org_file_path(""),
        )
        .await
        .expect("scan vault");
        for name in [
            "Areas.org",
            "Areas/Music.org",
            "Areas/Music/Audio Processing.org",
        ] {
            assert!(
                scanned.files.contains(&env.org_file_path(name)),
                "{name} must still exist at its original path; vault holds {:#?}",
                scanned.files
            );
        }

        // A touch-rewrite of every file replays ingest against the healed
        // store. A page whose Loro node is still the content-less placeholder
        // would have its title wiped here — so this phase is what proves the
        // placeholder was completed, not merely papered over on the SQL side.
        env.simulate_restart(&expected)
            .await
            .expect("restart over the healed vault");
        env.wait_for_org_files_stable(150, Duration::from_secs(20))
            .await;
        assert_titles(&env).await;
    });
}

async fn assert_titles(env: &holon_integration_tests::TestEnvironment) {
    for (id, title) in [
        (AREAS_ID, "Areas"),
        (MUSIC_ID, "Music"),
        (AUDIO_ID, "Audio Processing"),
    ] {
        let rows = env
            .query_sql(&format!(
                "SELECT content FROM block_raw WHERE id = 'block:{id}'"
            ))
            .await
            .expect("query block_raw");
        assert_eq!(
            rows.len(),
            1,
            "page block:{id} ({title}) must exist exactly once"
        );
        let content = rows[0]
            .get("content")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .unwrap_or_default();
        assert_eq!(
            content, title,
            "page block:{id} must carry its filename-derived title, not an empty \
                 placeholder content"
        );
    }
}
