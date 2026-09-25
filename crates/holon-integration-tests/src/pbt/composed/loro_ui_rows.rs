//! `SutLoroUiRows` over a live Loro tree: each block goes through the
//! production no-Turso read path (`LoroBlockQuerySource` snapshot →
//! `loro_ui_watcher::block_to_row` → the Turso-free profile resolver), the
//! same rows a `LoroUiWatcher` session renders.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_profile::ProfileResolving;
use holon_api::lifecycle::DEFAULT_SHUTDOWN_TIMEOUT;
use holon_api::lifecycle::SessionShutdown;
use holon_core::storage::BlockQuerySource;
use holon_loro::LoroBackend;
use holon_loro_wiring::loro_block_query_source::LoroBlockQuerySource;
use holon_loro_wiring::loro_ui_watcher::block_to_row;
use holon_loro_wiring::loro_ui_watcher::build_turso_free_profile_resolver;
use holon_pbt_core::capabilities::LoroUiRow;
use holon_pbt_core::capabilities::SutLoroUiRows;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

pub struct LoroUiRowsComponent {
    source: Arc<dyn BlockQuerySource>,
    resolver: tokio::sync::OnceCell<Arc<dyn ProfileResolving>>,
}

impl LoroUiRowsComponent {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self {
            source: Arc::new(LoroBlockQuerySource::new(backend)),
            resolver: tokio::sync::OnceCell::new(),
        }
    }

    /// Built at the first read, which the harness makes at quiescence, and
    /// with its entity-refresh task stopped right away: that task walks the
    /// tree, and a walk racing a transition's writes fails. Tag-driven computed
    /// fields read no entity lookup.
    async fn resolver(&self) -> &Arc<dyn ProfileResolving> {
        self.resolver
            .get_or_init(|| async {
                let shutdown = SessionShutdown::new();
                let resolver = build_turso_free_profile_resolver(self.source.clone(), &shutdown);
                shutdown
                    .shutdown(DEFAULT_SHUTDOWN_TIMEOUT)
                    .await
                    .expect("the Turso-free resolver's refresh task stops on shutdown");
                resolver
            })
            .await
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroUiRows for LoroUiRowsComponent {
    async fn loro_ui_rows(&self) -> Vec<LoroUiRow> {
        let resolver = self.resolver().await;
        let snapshot = self
            .source
            .snapshot()
            .await
            .expect("LoroBlockQuerySource::snapshot must not fail on a quiescent doc");
        snapshot
            .iter_blocks()
            .map(|block| {
                let row = block_to_row(block);
                let keyed: HashMap<String, Value> = row
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect();
                let (_, computed) = resolver.resolve_with_computed(&keyed);
                LoroUiRow {
                    id: block.id.clone(),
                    parsed: Block::try_from(row).map_err(|e| format!("{e:#}")),
                    is_page_row: computed.get("is_page_row").cloned().unwrap_or_else(|| {
                        panic!("block profile resolved no `is_page_row` for {}", block.id)
                    }),
                }
            })
            .collect()
    }
}

impl CapProvider for LoroUiRowsComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self as Arc<dyn SutLoroUiRows>);
    }
}
