//! `SutLoroUiRows` over a live Loro tree: each block goes through the
//! production no-Turso read path (`LoroBlockQuerySource` snapshot →
//! `loro_ui_watcher::block_to_row` → the Turso-free profile resolver), the
//! same rows a `LoroUiWatcher` session renders.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_profile::ProfileResolving;
use holon_api::lifecycle::SessionShutdown;
use holon_core::storage::BlockQuerySource;
use holon_loro::LoroBackend;
use holon_loro_wiring::loro_block_query_source::LoroBlockQuerySource;
use holon_loro_wiring::loro_ui_watcher::block_to_row;
use holon_loro_wiring::loro_ui_watcher::build_turso_free_profile_resolver;
use holon_loro_wiring::loro_ui_watcher::sibling_index;
use holon_pbt_core::capabilities::LoroUiRow;
use holon_pbt_core::capabilities::SutLoroUiRows;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;

pub struct LoroUiRowsComponent {
    source: Arc<dyn BlockQuerySource>,
    resolver: Arc<dyn ProfileResolving>,
    /// Owns the resolver's entity-refresh task, which ends once the resolver
    /// drops with this component.
    _shutdown: SessionShutdown,
}

impl LoroUiRowsComponent {
    /// Must run inside the harness's runtime: the resolver spawns its
    /// entity-refresh task here, at boot, well before the first read.
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        let source: Arc<dyn BlockQuerySource> = Arc::new(LoroBlockQuerySource::new(backend));
        let shutdown = SessionShutdown::new();
        let resolver = build_turso_free_profile_resolver(source.clone(), &shutdown);
        Self {
            source,
            resolver,
            _shutdown: shutdown,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroUiRows for LoroUiRowsComponent {
    async fn loro_ui_rows(&self) -> Vec<LoroUiRow> {
        let resolver = &self.resolver;
        let snapshot = self
            .source
            .snapshot()
            .await
            .expect("LoroBlockQuerySource::snapshot");
        snapshot
            .iter_blocks()
            .map(|block| {
                let row = block_to_row(block, sibling_index(&snapshot, block));
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
