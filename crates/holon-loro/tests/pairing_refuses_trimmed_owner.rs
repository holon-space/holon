//! A device that pairs with an owner whose history is already trimmed keeps
//! its own first changes, which share no history with the owner: the owner
//! would refuse every one. The pair is refused before the live store changes,
//! with a condition that names the owner and the Loro error.

use std::sync::Arc;

use anyhow::Result;
use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_core::DownstreamProjection;
use holon_core::block_ordering::BlockOrdering;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::WriteOrigin;
use holon_loro::device_pairing_op::DEVICE_ENTITY;
use holon_loro::device_pairing_op::DevicePairing;
use holon_loro::device_pairing_op::DevicePairingOperations;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::peer_versions::Hello;
use tempfile::TempDir;

const EDITS: &str = "edits";
const JOINER_EDIT: &str = "joiner-before-pairing";

/// A pairing whose adoption steps must never run: each resolver panics, so a
/// pair that gets past the refusal fails the test loudly.
fn pairing(store: LoroDocumentStore, bus: Arc<ConditionBus>) -> DevicePairing {
    DevicePairing::new(
        store,
        Arc::new(IrohAdvertiser::new()),
        Arc::new(
            || -> futures::future::BoxFuture<'static, Arc<dyn BlockOrdering>> {
                panic!("the refused pair reached the re-import")
            },
        ),
        Arc::new(
            || -> futures::future::BoxFuture<'static, Arc<dyn DownstreamProjection>> {
                panic!("the refused pair reached the projection")
            },
        ),
        Arc::new(|| Box::pin(async { None })),
        bus,
    )
}

/// An owner store whose global document is shallow: one recorded peer holds
/// all of its history, and a new session's first save trimmed below it.
async fn trimmed_owner(dir: &TempDir) -> Result<LoroDocumentStore> {
    let store = LoroDocumentStore::new(dir.path().to_path_buf()).with_peer_id(Some(1));
    let doc = store.get_doc(DocScope::Global).await?;
    doc.with_write(WriteOrigin::BlockOps, |txn| {
        txn.get_map(EDITS).insert("owner", true)?;
        Ok(())
    })?;
    let vv = doc.with_read(|d| Ok(d.oplog_vv()))?;
    store.peer_versions()?.record(&Hello {
        device: Some(7),
        vv: vv.encode(),
        known: Vec::new(),
    })?;
    store.checkpoint().await?;

    let session = LoroDocumentStore::new(dir.path().to_path_buf()).with_peer_id(Some(1));
    session.get_doc(DocScope::Global).await?;
    session.checkpoint().await?;

    let owner = LoroDocumentStore::new(dir.path().to_path_buf()).with_peer_id(Some(1));
    let shallow = owner
        .get_doc(DocScope::Global)
        .await?
        .with_read(|d| Ok(d.is_shallow()))?;
    anyhow::ensure!(
        shallow,
        "the owner's document did not trim; the test proves nothing"
    );
    Ok(owner)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_with_a_trimmed_owner_fails_visibly_and_changes_nothing() -> Result<()> {
    let owner_dir = TempDir::new()?;
    let owner_store = trimmed_owner(&owner_dir).await?;
    let owner = pairing(owner_store, Arc::new(ConditionBus::new()));
    let offer = owner
        .pair_offer("write".to_string())
        .await
        .map_err(|e| anyhow::anyhow!("pair_offer: {e}"))?;
    let response = offer
        .response
        .and_then(|v| v.as_string().map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("pair_offer returned no response"))?;
    let invite = serde_json::from_str::<serde_json::Value>(&response)?["invite"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("pair_offer response holds no invite"))?
        .to_string();

    let joiner_dir = TempDir::new()?;
    let joiner_bus = Arc::new(ConditionBus::new());
    let joiner_store = LoroDocumentStore::new(joiner_dir.path().to_path_buf())
        .with_peer_id(Some(2))
        .with_condition_bus(joiner_bus.clone());
    let joiner_doc = joiner_store.get_doc(DocScope::Global).await?;
    joiner_doc.with_write(WriteOrigin::BlockOps, |txn| {
        txn.get_map(EDITS).insert(JOINER_EDIT, true)?;
        Ok(())
    })?;
    let before = joiner_doc.with_read(|d| Ok(d.oplog_vv()))?;
    let joiner = pairing(joiner_store.clone(), joiner_bus.clone());

    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        joiner.pair_with_owner(invite),
    )
    .await?;
    let error = match refused {
        Ok(_) => anyhow::bail!("the joiner paired with an owner that refuses its edits"),
        Err(e) => e.to_string(),
    };
    assert!(
        error.contains("compacted its history"),
        "the pair failed for another reason: {error}"
    );

    let conditions = joiner_bus.current();
    assert!(
        conditions.iter().any(|c| c.subject == DEVICE_ENTITY
            && matches!(
                &c.reason,
                ConditionKind::PeerUpdateRefused { error, .. }
                    if error.contains("shallow history")
            )),
        "the refused pair raises the refusal condition with the Loro error, got {conditions:?}"
    );
    assert!(
        holon_loro::pairing_swap::read_marker(joiner_dir.path())?.is_none()
            && holon_loro::pairing_swap::read_record(joiner_dir.path())?.is_none(),
        "the refused pair left a pairing marker or record"
    );
    let (holds, after) = joiner_store
        .get_doc(DocScope::Global)
        .await?
        .with_read(|d| Ok((d.get_map(EDITS).get(JOINER_EDIT).is_some(), d.oplog_vv())))?;
    assert!(
        holds && after == before,
        "the refused pair changed the joiner's document"
    );
    Ok(())
}
