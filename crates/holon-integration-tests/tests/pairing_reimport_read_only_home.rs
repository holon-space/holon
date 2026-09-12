//! The THIRD write-tier seam: whole-store pairing re-import.
//!
//! Two seams already ask the one authority before turning a remote fact into a
//! local row — the operation dispatcher's gate, and the share backend's
//! projection legs. The pairing re-import is the third, and it asked nothing:
//! `DevicePairing::reimport` writes through
//! `BlockOrdering::create_in_tree_batch` straight into the Loro tree, so a
//! block the archive hung under a block of a read-only-format file landed FULLY
//! EDITABLE. No writer can ever put it into the authoritative file, which is
//! the exact hole the share backend's adoption closed for the other import
//! route.
//!
//! Driven through the production wiring, not a fixture: the vault holds a real
//! `.cook` file, the boot's file-sync controller records its home, and the
//! `DevicePairing` this test calls is the one the DI container built.
//!
//! Entry `2026-09-12-a-pairing-reimport-under-a-read-only-document-stays-editable`.
//!
//! @pbt kind harness
//! @pbt covers pairing-reimport-write-tier — a re-imported block whose parent
//! is homed in a read-only file inherits that home's refusal
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone has no pairing
//! transition, so it cannot reach this seam at all

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_loro::LoroDocumentStore;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_backend::NewBlockWithProperties;
use holon_loro::pairing_swap::PairingMarker;

const SYNC_TIMEOUT: Duration = Duration::from_secs(15);

/// The vault's read-only-format document. Cooklang ships no writer, so the
/// file is authoritative input and every block it declares is uneditable.
const RECIPE_FILE: &str = "Pancakes.cook";
const RECIPE: &str = "\
---
title: Fluffy Pancakes
servings: 4
---
Crack the @eggs{2} into a bowl.
";

/// `CookFormatAdapter` ids a step `block:<vault-relative path>::b::<seq>`, so
/// this is derived from the filename rather than minted.
const RECIPE_STEP: &str = "block:Pancakes.cook::b::0";

/// The block the archive hung under the recipe step — this device's own note,
/// captured before the pair swapped the owner's store in.
const OWED_NOTE: &str = "pair-owed-note";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build a runtime"),
    )
}

fn new_block(parent: EntityUri, id: &str, content: &str) -> NewBlockWithProperties {
    NewBlockWithProperties {
        parent_id: parent,
        id: EntityUri::block(id),
        content: BlockContent::text(content),
        properties: HashMap::new(),
        edges: BlockEdges::default(),
    }
}

/// Write the pre-pair document the swap moved aside: the recipe step as the
/// device knew it, with this device's note under it. The step itself is
/// already in the adopted store, so the plan skips it and hangs the note under
/// the store's own node — which is the whole point.
async fn seed_archive(archive: &std::path::Path) {
    std::fs::create_dir_all(archive).expect("the archive directory");
    let store = LoroDocumentStore::new(archive.to_path_buf());
    let doc = store
        .get_doc(holon_loro::DocScope::Global)
        .await
        .expect("the archived global doc");
    LoroBackend::from_document(doc)
        .create_blocks_with_properties(vec![
            new_block(
                EntityUri::no_parent(),
                RECIPE_STEP.trim_start_matches("block:"),
                "Crack the eggs into a bowl.",
            ),
            new_block(
                EntityUri::parse(RECIPE_STEP).expect("a literal step id"),
                OWED_NOTE,
                "use duck eggs next time",
            ),
        ])
        .await
        .expect("seeding the archive");
    store.save_all().await.expect("the archive persists");
}

#[test]
fn a_reimported_block_under_a_read_only_document_inherits_its_refusal() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_vault_file(RECIPE_FILE, RECIPE)
            .build(rt.clone())
            .await
            .expect("a vault holding a `.cook` file must boot");

        assert!(
            env.wait_for_block(RECIPE_STEP, SYNC_TIMEOUT).await,
            "{RECIPE_STEP} never ingested — the fixture, not the seam, is what this run measured"
        );

        let injector = env.injector().expect("the booted injector").clone();
        let authority = injector
            .resolve_async::<dyn holon_core::WriteTierAuthority>()
            .await;

        // The premise: production already refuses writes to the recipe's own
        // step. Without this the assertion below could pass for the wrong
        // reason (nothing is read-only-homed, so nothing is judged).
        assert!(
            authority
                .refusal_for(RECIPE_STEP)
                .await
                .expect("the authority answers")
                .is_some(),
            "the boot did not record {RECIPE_STEP} as read-only-homed, so this run never armed \
             the seam under test"
        );

        let store_dir = {
            let store = env.doc_store().expect("the Loro document store");
            let guard = store.read().await;
            guard.storage_dir().to_path_buf()
        };
        let archive = store_dir.join("archive").join("20260912T101500Z");
        seed_archive(&archive).await;
        let marker = PairingMarker {
            archive: archive.clone(),
            staging: store_dir.join("staging-20260912T101500Z"),
            owner: "owner-endpoint".to_string(),
            started_at: "2026-09-12T10:15:00Z".to_string(),
        };
        holon_loro::pairing_swap::write_marker(&store_dir, &marker).expect("the pairing marker");

        let pairing = injector
            .resolve_async::<Arc<holon_loro::device_pairing_op::DevicePairing>>()
            .await;
        pairing
            .complete_interrupted_pairing(&marker)
            .await
            .expect("the re-import must finish: the note's parent is in the adopted store");

        let owed = format!("block:{OWED_NOTE}");
        let rows = env
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{owed}'"))
            .await
            .expect("query block_raw");
        assert_eq!(
            rows.len(),
            1,
            "the re-import did not write {owed}, so the tier assertion below would be vacuous"
        );

        let refusal = authority
            .refusal_for(&owed)
            .await
            .expect("the authority answers");
        assert!(
            refusal.is_some(),
            "{owed} was re-imported under {RECIPE_STEP}, a block of the read-only file \
             {RECIPE_FILE}, yet the production WriteTierAuthority still lets it be edited. The \
             store would accept text no writer can ever put into that file."
        );
    });
}
