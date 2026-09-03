//! The dispatcher's write-tier authority: may a write name this block at all.
//!
//! It lives with the composition root because answering needs a block reader
//! (to find the block's owning document) and the degraded bus (to disclose the
//! refusal), neither of which the engine crate links.

use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use holon_api::EntityUri;
use holon_core::EditRefused;
use holon_core::ReadOnlyDocuments;
use holon_core::Result;
use holon_core::WriteTierAuthority;
use holon_filesystem::BlockReader;
use holon_filesystem::sync_ports::BlockRowMemo;
use holon_filesystem::sync_ports::nearest_page_ancestor;
use holon_loro::DegradedSignalBus;
use holon_loro::ShareDegraded;
use holon_loro::ShareDegradedReason;

/// Refuses writes to blocks of documents homed in a read-only format, and
/// raises the refusal on the degraded bus so the window shows it.
pub struct ReadOnlyFormatGate {
    /// Resolved lazily: this gate is consulted from inside the dispatcher, so
    /// resolving the dispatcher's own dependencies at wiring time is a cycle.
    injector: Injector,
    documents: Arc<ReadOnlyDocuments>,
    bus: Arc<DegradedSignalBus>,
}

impl ReadOnlyFormatGate {
    pub fn new(
        injector: Injector,
        documents: Arc<ReadOnlyDocuments>,
        bus: Arc<DegradedSignalBus>,
    ) -> Self {
        Self {
            injector,
            documents,
            bus,
        }
    }

    /// The document owning `id`: the nearest `Page` at or above it.
    async fn owning_document(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let reader = self.injector.resolve_async::<dyn BlockReader>().await;
        let mut rows = BlockRowMemo::new();
        Ok(nearest_page_ancestor(reader.as_ref(), id, &mut rows, None)
            .await
            .map_err(|e| format!("write-tier gate: locating the document owning `{id}`: {e}"))?
            .map(|page| page.id))
    }
}

#[async_trait]
impl WriteTierAuthority for ReadOnlyFormatGate {
    fn any_read_only_documents(&self) -> bool {
        !self.documents.is_empty()
    }

    async fn refusal_for(&self, block_id: &str) -> Result<Option<EditRefused>> {
        if self.documents.is_empty() {
            return Ok(None);
        }
        let id = EntityUri::parse(block_id)?;
        // The block may BE the document; the parent walk starts at itself.
        let Some(doc_id) = self.owning_document(&id).await? else {
            return Ok(None);
        };
        Ok(self.documents.refusal(&doc_id))
    }

    fn disclose(&self, refusal: &EditRefused) {
        let EditRefused::ReadOnlyFormat { format, path } = refusal;
        self.bus.emit(ShareDegraded {
            shared_tree_id: path.display().to_string(),
            reason: ShareDegradedReason::EditRefusedReadOnlyFormat {
                format: format.clone(),
            },
        });
    }
}
