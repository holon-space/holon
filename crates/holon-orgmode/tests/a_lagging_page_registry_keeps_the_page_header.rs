//! A block change of a page the page registry does not list yet still writes
//! the page's header.
//!
//! Production's `DocumentManager` answers `get_by_id` from a matview of
//! `Page`-tagged blocks that its own CDC stream updates, apart from the block
//! feed that drives write-back. Sharing a page re-projects it, so a change of
//! that page can arrive while the registry briefly lacks it.

use std::sync::Arc;

use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::MapAliasRegistrar;
use org_vault_double::vault;

use crate::org_vault_double;

#[tokio::test]
async fn a_page_missing_from_the_registry_keeps_its_id_header_on_write_back() {
    let v = vault();
    let path = v.path.clone();
    let root = path.parent().unwrap().to_path_buf();
    let (controller, _store, _disclosures, _net) = org_vault_double::controller_at(root, &v.docs);
    let mut controller = controller.with_alias_registrar(Arc::new(MapAliasRegistrar::default()));

    let header = format!(":PROPERTIES:\n:ID: {DOC_ID}\n:END:\n#+TITLE: Notes\n");
    std::fs::write(&path, &header).unwrap();
    let _ = controller.on_file_changed(&path).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), header);

    let page_id = EntityUri::block(DOC_ID);
    let page = v
        .docs
        .by_id
        .lock()
        .unwrap()
        .remove(&page_id)
        .expect("the fixture registers its page");
    controller
        .on_block_changed(
            &page_id,
            &BlockDelta::Upsert {
                block: page,
                prev: None,
            },
        )
        .await
        .expect("a change of a page the registry lags on must not fail the sync loop");

    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        disk.contains(DOC_ID),
        "write-back stripped the page's id from {}: {disk:?}",
        path.display(),
    );
}
