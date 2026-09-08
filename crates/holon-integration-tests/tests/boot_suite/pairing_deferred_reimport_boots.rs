//! A pair whose re-import can never be satisfied must not be the reason the app
//! has no store (D94.a).
//!
//! The decision lives in `LoroModule`'s boot arm: it reads the pairing marker,
//! calls `complete_interrupted_pairing`, and continues on `Deferred`. That arm
//! only exists on the real boot path, so this drives the whole production
//! wiring — `TestEnvironment::start_app` over a store directory seeded to look
//! like an interrupted pair — rather than calling the library function.
//!
//! @pbt kind harness
//! @pbt covers pairing(deferred-reimport) — the boot arm serves the adopted
//! store and raises the banner instead of stopping @pbt overlaps
//! general_e2e_composed_pbt — kept: the keystone has no pairing transition and
//! cannot produce an unsatisfiable archive

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::degraded_signal_bus::DegradedSignalBus;
use holon_loro::degraded_signal_bus::ShareDegradedReason;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_backend::NewBlockWithProperties;
use holon_loro::pairing_swap::PairingMarker;

/// Archived under the bundled layout root, which the re-import never carries
/// and this store does not hold, so neither the page nor the note has a home.
const OWED_PAGE: &str = "pair-owed-page";
const ABSENT_PARENT: &str = "root-layout";
const OWED_NOTE: &str = "pair-owed-note";
const OWED_BLOCKS: usize = 2;
/// The one block the adopted store holds — reading it back proves the boot
/// served the owner's document rather than a fresh empty one.
const ADOPTED_ROOT: &str = "block:owner-root";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
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

async fn seed_document(dir: &std::path::Path, blocks: Vec<NewBlockWithProperties>) {
    std::fs::create_dir_all(dir).expect("the document directory");
    let store = LoroDocumentStore::new(dir.to_path_buf());
    let doc = store
        .get_doc(DocScope::Global)
        .await
        .expect("the global doc");
    LoroBackend::from_document(doc)
        .create_blocks_with_properties(blocks)
        .await
        .expect("seeding a global doc");
    store.save_all().await.expect("the document persists");
}

/// Leave `store_dir` looking like a device whose pair swapped in the owner's
/// document and whose re-import never ran.
async fn seed_interrupted_pair(store_dir: &std::path::Path) -> PairingMarker {
    seed_document(
        store_dir,
        vec![new_block(
            EntityUri::no_parent(),
            "owner-root",
            "Owner root",
        )],
    )
    .await;

    let archive = store_dir.join("archive").join("20260905T101500Z");
    seed_document(
        &archive,
        vec![
            new_block(EntityUri::no_parent(), ABSENT_PARENT, "Layout"),
            new_block(EntityUri::block(ABSENT_PARENT), OWED_PAGE, "Phone page"),
            new_block(EntityUri::block(OWED_PAGE), OWED_NOTE, "bought milk"),
        ],
    )
    .await;

    let marker = PairingMarker {
        archive,
        staging: store_dir.join("staging-20260905T101500Z"),
        owner: "owner-endpoint".to_string(),
        started_at: "2026-09-05T10:15:00Z".to_string(),
    };
    holon_loro::pairing_swap::write_marker(store_dir, &marker).expect("the pairing marker");
    marker
}

#[test]
fn a_pair_whose_reimport_has_no_home_boots_and_raises_the_banner() {
    let rt = runtime();
    let env = TestEnvironment::new(rt.clone()).expect("a test environment");
    let store_dir = env.temp_dir.path().join(".loro");
    let marker = rt.block_on(async { seed_interrupted_pair(&store_dir).await });

    rt.block_on(async { env.start_app(true).await })
        .expect("an unsatisfiable re-import must not stop the boot");

    let injector = env.injector().expect("the booted injector");
    let bus = injector
        .try_resolve::<Arc<DegradedSignalBus>>()
        .expect("the degraded bus is registered on every Loro boot");
    let raised = bus.subscribe().current;
    let archive = marker.archive.display().to_string();
    assert!(
        raised.iter().any(|c| matches!(
            &c.reason,
            ShareDegradedReason::PairingReimportDeferred { orphans, archive: at }
                if *orphans == OWED_BLOCKS && at == &archive
        )),
        "the boot must tell the user that {OWED_BLOCKS} block(s) are still only in {archive}; \
         the bus holds: {raised:?}"
    );

    let rows = rt
        .block_on(async {
            env.query_sql(&format!(
                "SELECT id FROM block_raw WHERE id = '{ADOPTED_ROOT}'"
            ))
            .await
        })
        .expect("query block_raw");
    assert_eq!(
        rows.len(),
        1,
        "the boot must serve the store this device adopted; {ADOPTED_ROOT} is not in block_raw"
    );

    assert!(
        holon_loro::pairing_swap::read_marker(&store_dir)
            .expect("reading the marker")
            .is_some(),
        "the archive is still the only copy of the owed blocks, so the marker stays for the next \
         attempt"
    );
}
