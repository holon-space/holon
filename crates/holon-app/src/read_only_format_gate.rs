//! The dispatcher's write-tier authority: may a write name this block at all.
//!
//! It lives with the composition root because disclosing the refusal needs the
//! degraded bus, which the engine crate does not link.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_core::EditRefused;
use holon_core::ReadOnlyDocuments;
use holon_core::Result;
use holon_core::WriteTierAuthority;
use holon_loro::DegradedSignalBus;
use holon_loro::ShareDegraded;
use holon_loro::ShareDegradedReason;

/// Refuses writes to blocks of documents homed in a read-only format, and
/// raises the refusal on the degraded bus so the window shows it.
pub struct ReadOnlyFormatGate {
    documents: Arc<ReadOnlyDocuments>,
    bus: Arc<DegradedSignalBus>,
}

impl ReadOnlyFormatGate {
    pub fn new(documents: Arc<ReadOnlyDocuments>, bus: Arc<DegradedSignalBus>) -> Self {
        Self { documents, bus }
    }
}

#[async_trait]
impl WriteTierAuthority for ReadOnlyFormatGate {
    fn any_read_only_documents(&self) -> bool {
        !self.documents.is_empty()
    }

    /// One membership lookup, no store read.
    ///
    /// Every rendered editable row asks this on every draw
    /// (`BlockCellRegistry::editable_field_any`), so a per-block parent walk
    /// here made one `.cook` file in a vault cost a tree walk per row per
    /// frame. The registry carries the file's blocks instead, recorded at
    /// ingest.
    async fn refusal_for(&self, block_id: &str) -> Result<Option<EditRefused>> {
        if self.documents.is_empty() {
            return Ok(None);
        }
        Ok(self
            .documents
            .refusal_for_block(&EntityUri::parse(block_id)?))
    }

    async fn adopt_sync_import(&self, block_id: &str, parent_id: &str) -> Result<bool> {
        if self.documents.is_empty() {
            return Ok(false);
        }
        Ok(self
            .documents
            .adopt(&EntityUri::parse(parent_id)?, &EntityUri::parse(block_id)?))
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
