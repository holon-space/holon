//! The write-tier seam the engine's operation dispatch consults before any
//! provider runs.
//!
//! A [`WriteTier::ReadOnly`](crate::file_format::WriteTier) format's blocks are
//! a projection of a file Holon cannot write back. An edit the store accepts
//! but the disk can never take is a silent divergence, so the decision is made
//! at the ONE writer — the dispatcher — and returned as a typed refusal a UI
//! can disclose.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;

use async_trait::async_trait;
use holon_api::EntityUri;

/// A write the dispatcher refused, as DATA a caller can act on rather than a
/// message it can only print.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditRefused {
    #[error(
        "{format} is a read-only format: {} is authoritative input and Holon ships no writer for \
         it, so this edit would live only in the store. Edit the file on disk to change it.",
        path.display()
    )]
    ReadOnlyFormat { format: String, path: PathBuf },
}

/// Where a read-only-format document lives, and which format refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyHome {
    /// The refusing adapter's own
    /// [`format_name`](crate::file_format::FileFormatAdapter::format_name).
    pub format: String,
    pub path: PathBuf,
}

/// The documents whose backing file's format refuses write-back.
///
/// Filled by the file-sync controller as it records each document's home —
/// the one place that knows both the document and its file — and read above
/// it by the dispatcher's write-tier gate. A registry rather than a per-op
/// resolution because [`is_empty`](Self::is_empty) makes the org-only vault,
/// which is nearly every vault, pay nothing.
#[derive(Debug, Default)]
pub struct ReadOnlyDocuments {
    homes: RwLock<HashMap<EntityUri, ReadOnlyHome>>,
}

impl ReadOnlyDocuments {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no document in the vault is homed in a read-only format.
    pub fn is_empty(&self) -> bool {
        self.homes
            .read()
            .expect("ReadOnlyDocuments lock")
            .is_empty()
    }

    pub fn record(&self, doc_id: &EntityUri, format: &str, path: &Path) {
        self.homes.write().expect("ReadOnlyDocuments lock").insert(
            doc_id.clone(),
            ReadOnlyHome {
                format: format.to_string(),
                path: path.to_path_buf(),
            },
        );
    }

    /// Drop `doc_id`'s entry — its home moved to a writable format, or the
    /// file is gone.
    pub fn forget(&self, doc_id: &EntityUri) {
        self.homes
            .write()
            .expect("ReadOnlyDocuments lock")
            .remove(doc_id);
    }

    pub fn home(&self, doc_id: &EntityUri) -> Option<ReadOnlyHome> {
        self.homes
            .read()
            .expect("ReadOnlyDocuments lock")
            .get(doc_id)
            .cloned()
    }

    /// The refusal a write to a block of `doc_id` earns, or `None` when the
    /// document's file may be written back.
    pub fn refusal(&self, doc_id: &EntityUri) -> Option<EditRefused> {
        self.home(doc_id).map(|h| EditRefused::ReadOnlyFormat {
            format: h.format,
            path: h.path,
        })
    }
}

/// Answers "may a write name this block at all", for every writer.
///
/// The implementation resolves the block's owning document; the registry above
/// answers the tier. Split from [`ReadOnlyDocuments`] because block→document
/// resolution needs a store reader, which this crate has no business holding.
///
/// Model.md invariant 4 asks that the decision be made ONCE. The dispatcher is
/// one caller; the editor's text cell
/// ([`ReadOnlyTextCellBacking`](crate::cell::ReadOnlyTextCellBacking)) is the
/// other, because it writes the block's `LoroText` container directly. Both
/// hold this same authority rather than each carrying its own rule.
#[async_trait]
pub trait WriteTierAuthority: Send + Sync {
    /// Whether ANY document in the vault is homed in a read-only format.
    ///
    /// Synchronous, so a writer that cannot await — the text cell — skips the
    /// whole decision in an org-only vault, which is nearly every vault.
    fn any_read_only_documents(&self) -> bool;

    /// Decide, without disclosing. The caller discloses when it actually
    /// refuses a user's edit, so a writer may ask ahead of one.
    async fn refusal_for(&self, block_id: &str) -> crate::Result<Option<EditRefused>>;

    /// Raise `refusal` where the window can show it.
    fn disclose(&self, refusal: &EditRefused);
}
