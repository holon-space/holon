//! A page root retracted while the page registry lags keeps the page's header
//! on disk.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::vault;

use crate::org_vault_double;

#[derive(Default)]
struct MapAliasRegistrar(Mutex<HashMap<EntityUri, PathBuf>>);

#[async_trait]
impl holon_filesystem::sync_ports::AliasRegistrar for MapAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        self.0
            .lock()
            .unwrap()
            .insert(doc_id.clone(), path.to_path_buf());
    }
    async fn forget_aliases_to(&self, path: &Path) {
        self.0.lock().unwrap().retain(|_, p| p != path);
    }
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        self.0.lock().unwrap().get(doc_id).cloned()
    }
}

#[tokio::test]
async fn a_page_root_retracted_while_the_registry_lags_keeps_its_id_header() {
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
        .unwrap();
    controller
        .on_block_changed(&page_id, &BlockDelta::Remove(page_id.clone()))
        .await
        .unwrap();

    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        disk.contains(DOC_ID),
        "write-back stripped the page's id from {}: {disk:?}",
        path.display(),
    );
}
