//! Boot behaviour when the re-import a pair owes can never be satisfied.
//!
//! `LoroModule` calls `complete_interrupted_pairing` on every boot that finds a
//! pairing marker, so anything it returns as an error stops the app before a
//! window exists. A re-import whose blocks have no parent in the adopted store
//! is permanently unsatisfiable — retrying it at the next boot changes nothing
//! — which makes stopping there a store the user can never open again.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_core::DownstreamProjection;
use holon_core::ProjectionPass;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::degraded_signal_bus::DegradedSignalBus;
use holon_loro::degraded_signal_bus::ShareDegraded;
use holon_loro::device_pairing_op::DevicePairing;
use holon_loro::device_pairing_op::DevicePairingOperations;
use holon_loro::device_pairing_op::PairingCompletion;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_backend::NewBlockWithProperties;
use holon_loro::pairing_swap::PairingMarker;

/// The archived phone content: a journal day the owner does not have, under the
/// journals block the owner does not have either. `own_content` keeps a day
/// block that has children, so both blocks are owed to the re-import and both
/// are unplaceable until `block:journals` exists in the adopted store.
const PHONE_DAY: &str = "2026-09-05";
const PHONE_NOTE: &str = "phone-note";

/// Archived under `block:owner-root` — an id both stores hold, so this block
/// has a home in the adopted store from the first boot.
const PHONE_TODO: &str = "phone-todo";

/// A page this device wrote before the pair, and a note under it. The page is
/// top-level, so its stored parent is the root sentinel — the commonest thing
/// a solo device writes.
const PHONE_PAGE: &str = "phone-page";
const PHONE_PAGE_NOTE: &str = "phone-page-note";

fn new_block(parent: EntityUri, id: &str, content: &str) -> NewBlockWithProperties {
    NewBlockWithProperties {
        parent_id: parent,
        id: EntityUri::block(id),
        content: BlockContent::text(content),
        properties: HashMap::new(),
        edges: BlockEdges::default(),
    }
}

async fn write_into(store: &LoroDocumentStore, blocks: Vec<NewBlockWithProperties>) -> Result<()> {
    let backend = LoroBackend::from_document(store.get_doc(DocScope::Global).await?);
    backend
        .create_blocks_with_properties(blocks)
        .await
        .map_err(|e| anyhow::anyhow!("seeding the global doc: {e:?}"))?;
    Ok(())
}

/// The re-import's write leg, into the same live document the pairing op reads,
/// so the plan's idempotence and the one-node-per-id postcondition are judged
/// against real tree state.
struct LiveOrdering(LoroDocumentStore);

#[async_trait]
impl BlockOrdering for LiveOrdering {
    async fn create_in_tree_batch(
        &self,
        requests: &[BlockCreateRequest],
    ) -> holon_core::Result<Vec<bool>> {
        let blocks: Vec<NewBlockWithProperties> = requests
            .iter()
            .map(|r| NewBlockWithProperties {
                parent_id: r.parent_id.clone(),
                id: r.id.clone(),
                content: r.content.clone(),
                properties: r.properties.clone(),
                edges: BlockEdges::default(),
            })
            .collect();
        let created = blocks.len();
        let doc = self
            .0
            .get_doc(DocScope::Global)
            .await
            .expect("the live global doc");
        LoroBackend::from_document(doc)
            .create_blocks_with_properties(blocks)
            .await
            .expect("re-importing into the live global doc");
        Ok(vec![true; created])
    }

    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> holon_core::Result<()> {
        unreachable!("the re-import places through create_in_tree_batch only")
    }
    async fn prev_sibling(&self, _: &EntityUri) -> holon_core::Result<Option<EntityUri>> {
        unreachable!("the re-import reads no siblings")
    }
    async fn next_sibling(&self, _: &EntityUri) -> holon_core::Result<Option<EntityUri>> {
        unreachable!("the re-import reads no siblings")
    }
    async fn first_child(&self, _: &EntityUri) -> holon_core::Result<Option<EntityUri>> {
        unreachable!("the re-import reads no children")
    }
    async fn last_child(&self, _: &EntityUri) -> holon_core::Result<Option<EntityUri>> {
        unreachable!("the re-import reads no children")
    }
    async fn children(&self, _: &EntityUri) -> holon_core::Result<Vec<EntityUri>> {
        unreachable!("the re-import reads no children")
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> holon_core::Result<()> {
        unreachable!("the re-import only creates")
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> holon_core::Result<()> {
        unreachable!("the re-import only creates")
    }
}

/// Counts its passes, so a test can say the projection was driven rather than
/// only that nothing failed.
struct CountingProjection(Mutex<usize>);

#[async_trait]
impl DownstreamProjection for CountingProjection {
    async fn flush(&self) -> holon_core::Result<ProjectionPass> {
        *self.0.lock().unwrap() += 1;
        Ok(ProjectionPass::Converged)
    }
}

struct Fixture {
    pairing: DevicePairing,
    bus: Arc<DegradedSignalBus>,
    store: LoroDocumentStore,
    marker: PairingMarker,
    _dir: tempfile::TempDir,
}

/// A device whose pair swapped in the owner's store and whose re-import has not
/// run: the marker is on disk, the archive holds the phone's journals subtree,
/// and the adopted store has no `block:journals` to hang it under.
async fn interrupted_pair_with_no_home_for_the_archive() -> Result<Fixture> {
    interrupted_pair(vec![]).await
}

/// The same device, with `also_archived` added to the archive on top of the
/// unplaceable journals subtree.
async fn interrupted_pair(also_archived: Vec<NewBlockWithProperties>) -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let store_dir = dir.path().to_path_buf();

    let store = LoroDocumentStore::new(store_dir.clone());
    write_into(
        &store,
        vec![new_block(
            EntityUri::no_parent(),
            "owner-root",
            "Owner root",
        )],
    )
    .await?;
    store.save_all().await?;

    let archive = store_dir.join("archive").join("20260905T101500Z");
    std::fs::create_dir_all(&archive)?;
    let archived = LoroDocumentStore::new(archive.clone());
    let mut archived_blocks = vec![
        new_block(EntityUri::no_parent(), "journals", "Journals"),
        new_block(EntityUri::block("journals"), PHONE_DAY, "Saturday"),
        new_block(EntityUri::block(PHONE_DAY), PHONE_NOTE, "bought milk"),
    ];
    archived_blocks.extend(also_archived);
    write_into(&archived, archived_blocks).await?;
    archived.save_all().await?;

    let marker = PairingMarker {
        archive: archive.clone(),
        staging: store_dir.join("staging-20260905T101500Z"),
        owner: "owner-endpoint".to_string(),
        started_at: "2026-09-05T10:15:00Z".to_string(),
    };
    holon_loro::pairing_swap::write_marker(&store_dir, &marker)?;

    let bus = Arc::new(DegradedSignalBus::new());
    let ordering_store = store.clone();
    let pairing = DevicePairing::new(
        store.clone(),
        Arc::new(IrohAdvertiser::new()),
        Arc::new(move || {
            let store = ordering_store.clone();
            Box::pin(async move { Arc::new(LiveOrdering(store)) as Arc<dyn BlockOrdering> })
        }),
        Arc::new(|| {
            Box::pin(async {
                Arc::new(CountingProjection(Mutex::new(0))) as Arc<dyn DownstreamProjection>
            })
        }),
        bus.clone(),
    );

    Ok(Fixture {
        pairing,
        bus,
        store,
        marker,
        _dir: dir,
    })
}

fn conditions(bus: &DegradedSignalBus) -> Vec<ShareDegraded> {
    bus.subscribe().current
}

async fn live_ids(store: &LoroDocumentStore) -> Result<Vec<String>> {
    Ok(store
        .get_doc(DocScope::Global)
        .await?
        .with_read(|d| Ok(holon_loro::build_tid_index(d)))?
        .into_values()
        .collect())
}

/// D94.a: a block whose parent the adopted store holds is re-imported at the
/// boot that defers the rest. Withholding it would keep content out of a store
/// that can hold it, and would leave the banner counting fewer blocks than are
/// actually missing.
#[tokio::test]
async fn a_mixed_archive_reimports_what_has_a_home_and_defers_only_the_orphans() -> Result<()> {
    let f = interrupted_pair(vec![
        new_block(EntityUri::no_parent(), "owner-root", "Owner root"),
        new_block(EntityUri::block("owner-root"), PHONE_TODO, "call mum"),
    ])
    .await?;

    let PairingCompletion::Deferred { orphans, .. } = f
        .pairing
        .complete_interrupted_pairing(&f.marker)
        .await
        .expect("an unplaceable subtree must not stop the app")
    else {
        panic!("the journals subtree still has no parent, so this is not a completion");
    };

    let ids = live_ids(&f.store).await?;
    assert!(
        ids.iter().any(|id| id == &format!("block:{PHONE_TODO}")),
        "block:{PHONE_TODO} hangs under block:owner-root, which the adopted store holds, so the \
         deferred boot must have re-imported it; the store holds: {ids:?}"
    );

    let absent: Vec<&str> = [PHONE_DAY, PHONE_NOTE, PHONE_TODO]
        .into_iter()
        .filter(|owed| !ids.iter().any(|id| id == &format!("block:{owed}")))
        .collect();
    assert_eq!(
        orphans.len(),
        absent.len(),
        "the deferred set must be exactly what is not in the store; deferred {orphans:?}, absent \
         {absent:?}"
    );

    let raised = conditions(&f.bus);
    assert!(
        raised.iter().any(|c| matches!(
            &c.reason,
            holon_loro::degraded_signal_bus::ShareDegradedReason::PairingReimportDeferred {
                orphans: count,
                archive,
            } if *count == absent.len()
                && archive == &f.marker.archive.display().to_string()
        )),
        "the banner counts the blocks the user is missing ({}) and names the archive; the bus \
         holds: {raised:?}",
        absent.len()
    );

    assert!(
        holon_loro::pairing_swap::read_marker(f.store.storage_dir())?.is_some(),
        "blocks are still owed, so the marker stays"
    );
    Ok(())
}

/// A top-level page's parent is the root sentinel, which every store has. The
/// page and its subtree are therefore re-imported at the deferred boot, and
/// only the blocks whose parent id exists nowhere stay owed.
#[tokio::test]
async fn a_pre_pair_top_level_page_is_reimported_and_only_the_parentless_defer() -> Result<()> {
    let f = interrupted_pair(vec![
        new_block(EntityUri::no_parent(), PHONE_PAGE, "Phone page"),
        new_block(EntityUri::block(PHONE_PAGE), PHONE_PAGE_NOTE, "bought milk"),
    ])
    .await?;

    let PairingCompletion::Deferred { orphans, .. } = f
        .pairing
        .complete_interrupted_pairing(&f.marker)
        .await
        .expect("an unplaceable subtree must not stop the app")
    else {
        panic!("the journals subtree still has no parent, so this is not a completion");
    };

    let ids = live_ids(&f.store).await?;
    for arrived in [PHONE_PAGE, PHONE_PAGE_NOTE] {
        assert!(
            ids.iter().any(|id| id == &format!("block:{arrived}")),
            "block:{arrived} hangs under the root sentinel, which the adopted store has, so the \
             deferred boot must have re-imported it; the store holds: {ids:?}"
        );
    }

    assert_eq!(
        orphans.len(),
        2,
        "only block:{PHONE_DAY} and block:{PHONE_NOTE} have a parent id no store holds; deferred: \
         {orphans:?}"
    );
    for owed in [PHONE_DAY, PHONE_NOTE] {
        assert!(
            orphans.iter().any(|o| o.contains(owed)),
            "the deferred set names block:{owed}: {orphans:?}"
        );
    }

    let raised = conditions(&f.bus);
    assert!(
        raised.iter().any(|c| matches!(
            &c.reason,
            holon_loro::degraded_signal_bus::ShareDegradedReason::PairingReimportDeferred {
                orphans: count,
                ..
            } if *count == 2
        )),
        "the banner counts only the 2 genuinely parentless blocks; the bus holds: {raised:?}"
    );
    Ok(())
}

/// D94.a: a re-import that can never be satisfied must not be the reason the
/// app has no window. The owed work is disclosed and kept, not abandoned and
/// not fatal.
#[tokio::test]
async fn a_reimport_with_no_home_boots_degraded_and_keeps_what_it_owes() -> Result<()> {
    let f = interrupted_pair_with_no_home_for_the_archive().await?;
    let store_dir = f.store.storage_dir().to_path_buf();

    let outcome = f.pairing.complete_interrupted_pairing(&f.marker).await;
    assert!(
        outcome.is_ok(),
        "a re-import with nowhere to put its blocks must let the app boot; it stopped it: {:?}",
        outcome.as_ref().err()
    );
    let PairingCompletion::Deferred { orphans, .. } = outcome.expect("a boot-safe outcome") else {
        panic!("a re-import with no parent for its blocks cannot report completion");
    };
    assert_eq!(
        orphans.len(),
        2,
        "both archived blocks are owed, and the outcome names them: {orphans:?}"
    );

    let raised = conditions(&f.bus);
    let archive = f.marker.archive.display().to_string();
    assert!(
        raised.iter().any(|c| {
            let detail = format!("{:?}", c.reason);
            detail.contains(&archive) && detail.contains('2')
        }),
        "the banner must name the archive {archive} and the 2 blocks not re-imported; the bus \
         holds: {raised:?}"
    );

    assert!(
        holon_loro::pairing_swap::read_marker(&store_dir)?.is_some(),
        "the marker is the next boot's retry; a degraded boot must keep it"
    );
    Ok(())
}

/// The all-clear the degraded condition names: once the adopted store gains the
/// parent, the SAME call finishes the pair.
#[tokio::test]
async fn the_reimport_completes_once_its_parent_appears_and_lifts_the_banner() -> Result<()> {
    let f = interrupted_pair_with_no_home_for_the_archive().await?;
    let store_dir = f.store.storage_dir().to_path_buf();
    let _ = f.pairing.complete_interrupted_pairing(&f.marker).await;

    write_into(
        &f.store,
        vec![new_block(EntityUri::no_parent(), "journals", "Journals")],
    )
    .await?;

    f.pairing
        .complete_interrupted_pairing(&f.marker)
        .await
        .expect("the re-import completes once its parent is in the store");

    assert!(
        holon_loro::pairing_swap::read_marker(&store_dir)?.is_none(),
        "a completed re-import owes nothing, so the marker is gone"
    );
    assert!(
        holon_loro::pairing_swap::read_record(&store_dir)?.is_some(),
        "a completed pair records the owner it belongs to"
    );

    let ids: Vec<String> = f
        .store
        .get_doc(DocScope::Global)
        .await?
        .with_read(|d| Ok(holon_loro::build_tid_index(d)))?
        .into_values()
        .collect();
    for owed in [PHONE_DAY, PHONE_NOTE] {
        assert!(
            ids.iter().any(|id| id == &format!("block:{owed}")),
            "block:{owed} was owed by the re-import and is not in the store: {ids:?}"
        );
    }

    let still_raised = conditions(&f.bus);
    assert!(
        !still_raised.iter().any(|c| {
            format!("{:?}", c.reason).contains(&f.marker.archive.display().to_string())
                && !matches!(
                    c.reason,
                    holon_loro::degraded_signal_bus::ShareDegradedReason::
                        PairingReimportedLocalContent { .. }
                )
        }),
        "the deferred-re-import banner must be lifted once the re-import ran: {still_raised:?}"
    );
    Ok(())
}

/// The banner's action: the same work the next boot would do, on demand, and
/// refusing by name while it still cannot be done.
#[tokio::test]
async fn the_retry_action_refuses_by_name_until_the_parent_is_back() -> Result<()> {
    let f = interrupted_pair_with_no_home_for_the_archive().await?;
    let _ = f.pairing.complete_interrupted_pairing(&f.marker).await;

    let refused = f
        .pairing
        .pair_retry_reimport()
        .await
        .expect_err("a retry that still has nowhere to put the blocks is not a success");
    let message = format!("{refused:#}");
    for named in [PHONE_DAY, PHONE_NOTE] {
        assert!(
            message.contains(named),
            "the refusal must name block:{named}; got: {message}"
        );
    }

    write_into(
        &f.store,
        vec![new_block(EntityUri::no_parent(), "journals", "Journals")],
    )
    .await?;
    f.pairing
        .pair_retry_reimport()
        .await
        .expect("the retry completes once the parent is in the store");

    assert!(
        holon_loro::pairing_swap::read_marker(f.store.storage_dir())?.is_none(),
        "a completed retry clears the marker"
    );
    let again = f
        .pairing
        .pair_retry_reimport()
        .await
        .expect_err("nothing is owed once the marker is gone");
    assert!(
        format!("{again:#}").contains("owes no pairing re-import"),
        "pressing retry twice must say there is nothing owed; got: {again:#}"
    );
    Ok(())
}
