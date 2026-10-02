//! [`SutLoroDurability`] over a persisted Loro store and its live projection.

use std::sync::Arc;

use holon_loro::LoroSyncControllerHandle;
use holon_pbt_core::capabilities::SutLoroDurability;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

/// Reads the global document from disk (its `.loro` snapshot plus update log)
/// and compares it with the frontier the projection has written to SQL.
pub struct LoroSnapshotDurability {
    store: holon_loro::LoroDocumentStore,
    sync: Arc<LoroSyncControllerHandle>,
}

impl LoroSnapshotDurability {
    pub fn new(store: &holon_loro::LoroDocumentStore, sync: Arc<LoroSyncControllerHandle>) -> Self {
        Self {
            store: store.clone(),
            sync,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroDurability for LoroSnapshotDurability {
    async fn loro_snapshot_lag(&self) -> Option<String> {
        let synced = self.sync.last_synced_frontiers();
        if synced.is_empty() {
            return None;
        }
        let path = self
            .store
            .storage_dir()
            .join(holon_loro::GLOBAL_SNAPSHOT_NAME);
        let path = path.display();
        let disk = match self
            .store
            .read_persisted(holon_loro::DocScope::Global)
            .await
        {
            Ok((disk, _)) => disk,
            Err(e) => {
                return Some(format!(
                    "the document at {path} does not read back ({e:#}) while SQL reflects \
                     frontiers {synced:?}"
                ));
            }
        };
        let saved = disk.oplog_vv();
        let missing: Vec<loro::ID> = synced.iter().filter(|id| !saved.includes_id(*id)).collect();
        (!missing.is_empty()).then(|| {
            format!(
                "SQL is projected up to {synced:?}, the snapshot at {path} stops at {saved:?}; \
                 missing {missing:?}"
            )
        })
    }
}

impl CapProvider for LoroSnapshotDurability {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutLoroDurability>);
    }
}
