//! Regression + property pin for the dogfood 2026-07-10 doc-`#+ID` re-mint
//! data-loss bug.
//!
//! Symptom: booting against a real vault, org writeback re-minted a file's
//! document `#+ID:` — `Frontends.org` had `#+ID: 0776...` on disk and the app
//! rewrote line 1 to a DIFFERENT uuid, nothing else changed. Every reference to
//! the old doc id then breaks (identity mutation of user data).
//!
//! Root cause: a file `Frontends.org` alongside a subdirectory `Frontends/`
//! (dir + file share the base name). A sibling file scanned first under the
//! subdir (`Frontends/GPUI.org`) mints a name-chain *placeholder* page for the
//! shared `Frontends` segment with a RANDOM id. When `Frontends.org` is then
//! ingested, the document-manager `create` de-duped by `(parent, title)` and
//! handed back the placeholder's id, so writeback rendered the placeholder id
//! as the file's `#+ID` — re-minting it. Fix: the file's `#+ID` is
//! authoritative (`create_forcing_id`), and write-back resolves the document by
//! `#+ID` rather than the ambiguous name chain.
//!
//! This is a FULL-BOOT property: the pure parse↔render round-trip PBT
//! (`org_roundtrip_pbt`) structurally cannot reach the ingest / name-chain /
//! dedup layer where this bug lives, so the property lives here where a real
//! `FileSyncController` runs the ingest→writeback loop.
//!
//! @pbt kind harness
//! @pbt covers doc-id-writeback-stability — doc #+ID re-mint data-loss pin (dogfood 2026-07-10)

use std::sync::Arc;
use std::time::Duration;

use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

const DOC_ID: &str = "0776062a-e04c-4ae3-a1f5-9cc20ebeea67";

fn frontends_file() -> String {
    format!(
        "#+ID: {DOC_ID}\n#+TITLE: Frontends\n* Some Heading\n:PROPERTIES:\n:ID: \
         fe-child\n:END:\nbody text\n"
    )
}

fn sibling_file(id: &str, title: &str) -> String {
    format!(
        "#+ID: {id}\n#+TITLE: {title}\n* {title} Heading\n:PROPERTIES:\n:ID: \
         {title}-child\n:END:\n"
    )
}

fn doc_id_line(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.trim_start().starts_with("#+ID:"))
        .map(|l| l.trim().to_string())
}

/// Boot a vault shaped `Frontends.org` + `Frontends/<sibling>.org` for each
/// sibling, run the ingest→writeback loop (an edit to `Frontends.org`), and
/// return the `#+ID` line found on disk afterwards. `None` means the file lost
/// its `#+ID` entirely.
async fn boot_edit_and_read_doc_id(
    rt: Arc<tokio::runtime::Runtime>,
    siblings: &[(String, String)],
) -> Option<String> {
    let mut builder = TestEnvironmentBuilder::new()
        .without_loro()
        .with_org_file("Projects/Holon/Frontends.org", frontends_file());
    for (id, title) in siblings {
        builder = builder.with_org_file(
            format!("Projects/Holon/Frontends/{title}.org"),
            sibling_file(id, title),
        );
    }
    let env = builder.build(rt.clone()).await.expect("boot");

    assert!(
        env.wait_for_block("fe-child", SYNC_TIMEOUT).await,
        "Frontends.org content block must ingest"
    );

    env.update_block_content("fe-child", "Some Heading Edited")
        .await
        .expect("update");

    let path = env.org_file_path("Projects/Holon/Frontends.org");
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let content = env.org_fs.read_to_string(&path).await.expect("read");
        if content.contains("Some Heading Edited") {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("writeback did not land:\n{content}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let content = env.org_fs.read_to_string(&path).await.expect("read");
    doc_id_line(&content)
}

/// Named regression: the exact reproduced shape (one sibling under the
/// same-named subdir) must keep `Frontends.org`'s `#+ID` byte-stable.
#[test]
fn doc_id_stable_across_ingest_writeback() {
    let rt = runtime();
    rt.block_on(async {
        let id = boot_edit_and_read_doc_id(
            rt.clone(),
            &[(
                "11111111-1111-1111-1111-111111111111".to_string(),
                "GPUI".to_string(),
            )],
        )
        .await;
        assert_eq!(
            id.as_deref(),
            Some(format!("#+ID: {DOC_ID}").as_str()),
            "document #+ID on disk was re-minted (data loss)"
        );
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

    /// Property: no matter how many sibling `.org` files live under the
    /// same-named `Frontends/` subdirectory (each with its own `#+ID`), booting
    /// and writing back must leave `Frontends.org`'s explicit `#+ID` on disk
    /// byte-identical — the identity parsed at the boundary IS the identity.
    #[test]
    fn prop_doc_id_stable_under_same_named_subdir(
        titles in proptest::collection::vec("[A-Z][a-z]{2,7}", 0..4)
    ) {
        // Distinct titles (file names must not collide within the subdir).
        let mut seen = std::collections::HashSet::new();
        let siblings: Vec<(String, String)> = titles
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .enumerate()
            .map(|(i, t)| {
                (format!("{:08x}-0000-0000-0000-000000000000", i + 1), t)
            })
            .collect();

        let rt = runtime();
        let id = rt.block_on(boot_edit_and_read_doc_id(rt.clone(), &siblings));
        let expected = format!("#+ID: {DOC_ID}");
        prop_assert_eq!(
            id.as_deref(),
            Some(expected.as_str()),
            "document #+ID re-minted with siblings {:?}",
            siblings
        );
    }
}
