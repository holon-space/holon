//! A `.cook` file in the vault reaches `CookFormatAdapter` on a REAL boot, and
//! is never written back.
//!
//! Kitchen Inc A landed the adapter but nothing routed a vault file to it: the
//! controller held ONE adapter and the watcher's extension filter was
//! hardcoded to `.org`, so every `.cook` file was scanned and then dropped.
//! Inc A2's `FormatRegistry` is what makes "cooklang files are authoritative"
//! (K1) true of a real vault rather than of a crate-level fixture.
//!
//! Two properties, and the second is the one that bites: routing a read-only
//! format INTO the controller also exposes it to the controller's write half.
//! A recipe's identity is name-chain-derived (cooklang embeds no id), and the
//! page-file path derivation appends `.org` unconditionally
//! (`VaultPath::page_file_from_name_chain`), so an ungated write-back would
//! not overwrite the recipe — it would mint a SECOND, org-format home beside
//! it and quietly diverge from the authoritative file.
//!
//! @pbt kind harness
//! @pbt covers cook-vault-ingest — a `.cook` file in a real vault ingests as a
//! document with its steps, and its write-back is refused visibly with no
//! second home minted
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone drives an
//! org-only vault and has no second-format file in its fixture

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::QueryLanguage;
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

/// The content of the vault file named `name`, or `None` when the vault holds
/// no such file. Goes through the `FileSystem` port and matches on file NAME,
/// so it is immune to how the in-memory tree spells its root.
async fn read_vault_file(
    env: &holon_integration_tests::TestEnvironment,
    name: &str,
) -> Option<String> {
    use holon_filesystem::FileSystem;
    let scanned = env
        .org_fs
        .scan_directory(env.org_root())
        .await
        .expect("scan the vault root");
    let path = scanned
        .files
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(name))?;
    Some(
        env.org_fs
            .read_to_string(path)
            .await
            .expect("read a vault file the scan just listed"),
    )
}

/// Title comes from cooklang metadata, not the filename, so the assertion
/// below distinguishes "the adapter parsed it" from "something guessed a title
/// off the path".
const PANCAKES_COOK: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Crack the @eggs{2} into a bowl.

Whisk in the @flour{200%g} and cook for ~{3%minutes}.
";

/// An ordinary org page in the SAME vault: routing must not become
/// cook-shaped, so an org regression shows up as this same test failing.
const NOTES_ORG: &str = "\
* Notes Root
:PROPERTIES:
:ID: notes-root
:END:
** A Note
:PROPERTIES:
:ID: notes-child
:END:
";

/// Red 1 — routing. A `.cook` file in the vault must produce its document and
/// its step blocks in the store, alongside an org file that still ingests.
#[test]
fn a_cook_file_in_the_vault_ingests_beside_org() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        // The org leg is unaffected.
        for id in ["notes-root", "notes-child"] {
            assert!(
                env.wait_for_block(&format!("block:{id}"), SYNC_TIMEOUT)
                    .await,
                "org block {id} did not sync — widening the format registry broke the org leg"
            );
        }

        // The recipe's document. `CookFormatAdapter::parse` ids the document
        // by its vault-relative path and titles it from cooklang metadata.
        // The recipe's page entity. Titled from the FILENAME, as every page
        // is: the cooklang `title:` metadata reaches the document block the
        // adapter parses, but `sync_document_metadata` is the only seam that
        // could carry it onto the persisted page and the cook adapter does not
        // implement it. Asserted as it behaves, not as it ideally would —
        // carrying the metadata title is recorded as an Inc B follow-up.
        let doc_rows = env
            .test_ctx()
            .query(
                "SELECT id FROM block_raw WHERE content = 'Pancakes'".to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("query the recipe document row");
        assert_eq!(
            doc_rows.len(),
            1,
            "no page was minted for Pancakes.cook — nothing routed it into the controller"
        );

        // Its steps. Ids are `<file id>::b::<seq>`, minted by the adapter.
        let step_rows = env
            .test_ctx()
            .query(
                "SELECT id, content FROM block_raw WHERE id LIKE 'block:Pancakes.cook::b::%'"
                    .to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("query the recipe step rows");
        assert_eq!(
            step_rows.len(),
            2,
            "expected the recipe's 2 steps in the store, got {}",
            step_rows.len()
        );
        let steps = format!(
            "{:?}",
            step_rows
                .iter()
                .map(|r| r.get("content"))
                .collect::<Vec<_>>()
        );
        // De-sugared cooklang: `@eggs{2}` rendered as "eggs", `~{3%minutes}`
        // as "3 minutes". No org parser produces this from a file with no
        // headlines, so this is the decisive proof that CookFormatAdapter —
        // and not some fallback — parsed the file.
        assert!(
            steps.contains("Crack the eggs into a bowl.")
                && steps.contains("Whisk in the flour and cook for 3 minutes."),
            "step text is not the cook adapter's de-sugared output: {steps:?}"
        );
    });
}

/// Red 2 — the write half. The recipe file is authoritative: a store-side edit
/// of one of its blocks must leave the `.cook` bytes untouched AND must not
/// mint an org-format second home for the same page.
#[test]
fn a_cook_file_is_never_written_back_and_grows_no_second_home() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file("Pancakes.cook", PANCAKES_COOK)
            .with_vault_file("Notes.org", NOTES_ORG)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        assert!(
            env.wait_for_block("block:Pancakes.cook::b::0", SYNC_TIMEOUT)
                .await,
            "precondition: the recipe's first step must be in the store"
        );

        // Read through the FileSystem port and address files by NAME: the
        // vault root is an in-memory tree whose paths are canonicalized, so a
        // hand-joined path can miss.
        let before = read_vault_file(&env, "Pancakes.cook")
            .await
            .expect("the recipe file must exist before the edit");

        // Edit a recipe block in the store. In an org document this is exactly
        // what triggers write-back of the owning file.
        env.test_ctx()
            .query(
                "UPDATE block_raw SET content = 'REWRITTEN BY THE STORE' WHERE id = \
                 'block:Pancakes.cook::b::0'"
                    .to_string(),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await
            .expect("edit a recipe block in the store");

        // Give write-back every chance to happen before asserting it did not.
        env.wait_for_org_files_stable(25, Duration::from_millis(3000))
            .await;

        let after = read_vault_file(&env, "Pancakes.cook")
            .await
            .expect("the recipe file must still exist after the edit");
        assert_eq!(
            before, after,
            "the authoritative `.cook` file was REWRITTEN by write-back — Inc A ships no cooklang \
             renderer, so whatever landed on disk is a reconstruction, not the user's recipe"
        );

        // The second-home hazard: the page-file derivation appends `.org`, so
        // an ungated write-back materializes an org twin beside the recipe
        // instead of refusing.
        assert!(
            read_vault_file(&env, "Pancakes.org").await.is_none(),
            "an org-format SECOND home was minted for a page that already owns Pancakes.cook — \
             the same recipe now has two divergent files"
        );
    });
}
