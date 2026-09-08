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

/// The blocks ONE read-only file declares, as the file itself accounts for
/// them.
///
/// The registry answers every write-tier decision out of this set, so a site
/// that could hand over a list it assembled itself would be deciding the tier
/// of blocks it never saw. The two constructors are the only two things that
/// have the file's own account: the parse that just read it, and the `file`
/// row a previous parse stamped. Neither can yield an empty set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyMembers(Vec<EntityUri>);

impl ReadOnlyMembers {
    /// The blocks the parse that just read the file produced — its document
    /// entity, the root the parse named, and every block under it.
    pub fn from_parse(
        document_uri: &EntityUri,
        parsed: &crate::file_format::FileFormatParseResult,
    ) -> Self {
        Self(
            std::iter::once(document_uri.clone())
                .chain(std::iter::once(parsed.document.id.clone()))
                .chain(parsed.blocks.iter().map(|b| b.id.clone()))
                .collect(),
        )
    }

    /// The same set as a previous parse stamped it into the `file` row, for a
    /// boot whose byte-identity skip never parses the file.
    ///
    /// Errs on an empty set: a read-only file declares at least its own root,
    /// so an empty column is a damaged row, and recording it would leave every
    /// block of an authoritative file editable.
    pub fn from_persisted_row(path: &Path, blocks: Vec<EntityUri>) -> crate::Result<Self> {
        if blocks.is_empty() {
            return Err(format!(
                "the `file` row for {} carries no `read_only_blocks`, so nothing here knows which \
                 blocks that authoritative file owns. Re-ingest the file — its parse is the only \
                 authority for the set.",
                path.display()
            )
            .into());
        }
        Ok(Self(blocks))
    }

    pub fn as_slice(&self) -> &[EntityUri] {
        &self.0
    }
}

/// Where a read-only-format document lives, and which format refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyHome {
    /// The refusing adapter's own
    /// [`format_name`](crate::file_format::FileFormatAdapter::format_name).
    pub format: String,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
struct Registry {
    /// Each read-only-format document's home.
    docs: HashMap<EntityUri, ReadOnlyHome>,
    /// Every block those documents hold — the root included — back to its
    /// document. The index that makes the tier decision a hash lookup instead
    /// of a parent walk.
    member_of: HashMap<EntityUri, EntityUri>,
    /// Blocks a peer's import placed UNDER one of those documents. Held apart
    /// from `member_of` because the file never declares them: a re-ingest
    /// re-derives `member_of` from the file and would drop these, leaving a
    /// block that can never be written to disk editable.
    adopted: HashMap<EntityUri, EntityUri>,
}

/// The documents whose backing file's format refuses write-back, and the
/// blocks they hold.
///
/// Filled by the file-sync controller as it records each document's home —
/// the one place that knows both the document and its file — and read above
/// it by the dispatcher's write-tier gate. A registry rather than a per-op
/// resolution because [`is_empty`](Self::is_empty) makes the org-only vault,
/// which is nearly every vault, pay nothing.
///
/// Membership is recorded once per ingest and never invalidated. It cannot go
/// stale: a block enters or leaves a read-only document only by a write that
/// names the document's root or one of its blocks as `id` or `parent_id`, and
/// [`refusal_for_block`](Self::refusal_for_block) refuses exactly those. The
/// file itself is the only thing that can change the set, and re-ingesting it
/// re-records it.
#[derive(Debug, Default)]
pub struct ReadOnlyDocuments {
    inner: RwLock<Registry>,
}

impl ReadOnlyDocuments {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no document in the vault is homed in a read-only format.
    pub fn is_empty(&self) -> bool {
        self.inner
            .read()
            .expect("ReadOnlyDocuments lock")
            .docs
            .is_empty()
    }

    /// Record `doc_id`'s home together with the blocks its file produced,
    /// replacing whatever it held before.
    pub fn record(&self, doc_id: &EntityUri, format: &str, path: &Path, members: &ReadOnlyMembers) {
        let mut inner = self.inner.write().expect("ReadOnlyDocuments lock");
        inner.member_of.retain(|_, owner| owner != doc_id);
        for member in members.as_slice().iter().chain(std::iter::once(doc_id)) {
            inner.member_of.insert(member.clone(), doc_id.clone());
        }
        inner.docs.insert(
            doc_id.clone(),
            ReadOnlyHome {
                format: format.to_string(),
                path: path.to_path_buf(),
            },
        );
    }

    /// Re-point `doc_id`'s home after its file moved, keeping its members.
    ///
    /// Errs when the document has no entry: a caller that only knows the new
    /// path cannot invent the membership, and a silently-empty entry would
    /// leave every block of an authoritative file editable.
    pub fn rehome(&self, doc_id: &EntityUri, format: &str, path: &Path) -> crate::Result<()> {
        let mut inner = self.inner.write().expect("ReadOnlyDocuments lock");
        let Some(home) = inner.docs.get_mut(doc_id) else {
            return Err(format!(
                "read-only home for `{doc_id}` moved to {} before its membership was ever \
                 recorded — the blocks of an authoritative file would be editable. Its ingest, \
                 or the persisted membership a boot-time skip loads, must run first.",
                path.display()
            )
            .into());
        };
        *home = ReadOnlyHome {
            format: format.to_string(),
            path: path.to_path_buf(),
        };
        Ok(())
    }

    /// Drop `doc_id`'s entry and its members — its home moved to a writable
    /// format, or the file is gone.
    pub fn forget(&self, doc_id: &EntityUri) {
        let mut inner = self.inner.write().expect("ReadOnlyDocuments lock");
        if inner.docs.remove(doc_id).is_some() {
            inner.member_of.retain(|_, owner| owner != doc_id);
            inner.adopted.retain(|_, owner| owner != doc_id);
        }
    }

    /// Bind `block_id` to the read-only document that owns `parent_id`, so a
    /// block a peer's import placed under an authoritative file is as
    /// uneditable as the file's own blocks. `false` when `parent_id` belongs to
    /// no such document, which is every ordinary import.
    ///
    /// An import is a merge that has already happened in the replica it came
    /// from, so it lands and inherits the tier; refusing it here would only
    /// make this store disagree with that peer while the file stays unwritable
    /// either way.
    pub fn adopt(&self, parent_id: &EntityUri, block_id: &EntityUri) -> bool {
        let mut inner = self.inner.write().expect("ReadOnlyDocuments lock");
        let Some(doc_id) = inner
            .member_of
            .get(parent_id)
            .or_else(|| inner.adopted.get(parent_id))
            .cloned()
        else {
            return false;
        };
        inner.adopted.insert(block_id.clone(), doc_id);
        true
    }

    pub fn home(&self, doc_id: &EntityUri) -> Option<ReadOnlyHome> {
        self.inner
            .read()
            .expect("ReadOnlyDocuments lock")
            .docs
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

    /// The refusal a write NAMING `block_id` earns — the whole tier decision,
    /// as two hash lookups.
    pub fn refusal_for_block(&self, block_id: &EntityUri) -> Option<EditRefused> {
        let inner = self.inner.read().expect("ReadOnlyDocuments lock");
        let doc_id = inner
            .member_of
            .get(block_id)
            .or_else(|| inner.adopted.get(block_id))?;
        let home = inner.docs.get(doc_id)?;
        Some(EditRefused::ReadOnlyFormat {
            format: home.format.clone(),
            path: home.path.clone(),
        })
    }
}

/// Answers "may a write name this block at all", for every writer.
///
/// The registry above answers; this trait is the seam that lets a writer ask
/// without linking the composition root that owns the registry and the
/// degraded bus.
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

    /// Bind a block a peer's import placed under a read-only document into
    /// that document, so the imported block earns the same refusal the file's
    /// own blocks do. `false` when the parent belongs to no such document.
    ///
    /// See [`ReadOnlyDocuments::adopt`] for why an import inherits the tier
    /// rather than being refused.
    async fn adopt_sync_import(&self, block_id: &str, parent_id: &str) -> crate::Result<bool>;

    /// Raise `refusal` where the window can show it.
    fn disclose(&self, refusal: &EditRefused);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> EntityUri {
        EntityUri::block("Pancakes.cook")
    }

    fn step(n: u32) -> EntityUri {
        EntityUri::block(&format!("Pancakes.cook::b::{n}"))
    }

    /// Stands for the `file` row a previous parse stamped — the only other
    /// thing than a parse that holds a file's own account of its blocks.
    fn declared(blocks: &[EntityUri]) -> ReadOnlyMembers {
        ReadOnlyMembers::from_persisted_row(Path::new("/vault/Pancakes.cook"), blocks.to_vec())
            .expect("a non-empty row")
    }

    fn recorded() -> ReadOnlyDocuments {
        let docs = ReadOnlyDocuments::new();
        docs.record(
            &doc(),
            "cooklang",
            Path::new("/vault/Pancakes.cook"),
            &declared(&[step(0), step(1)]),
        );
        docs
    }

    /// The whole tier decision, for a member and for the document root — the
    /// dispatcher judges `id` and `parent_id`, so a move into the document is
    /// refused by its root's entry and a move out by the member's.
    #[test]
    fn every_recorded_block_and_the_document_root_earn_the_refusal() {
        let docs = recorded();
        for id in [doc(), step(0), step(1)] {
            let refusal = docs
                .refusal_for_block(&id)
                .unwrap_or_else(|| panic!("{id} is a block of a read-only file"));
            assert!(refusal.to_string().contains("Pancakes.cook"));
        }
        assert_eq!(
            docs.refusal_for_block(&EntityUri::block("notes-child")),
            None
        );
    }

    /// A re-ingest is a REPLACEMENT: a block the file no longer holds must
    /// stop earning the refusal, or a deleted recipe step stays uneditable
    /// under an id the store may reuse.
    #[test]
    fn re_recording_drops_the_blocks_the_file_no_longer_declares() {
        let docs = recorded();
        docs.record(
            &doc(),
            "cooklang",
            Path::new("/vault/Pancakes.cook"),
            &declared(&[step(0)]),
        );
        assert!(docs.refusal_for_block(&step(0)).is_some());
        assert_eq!(docs.refusal_for_block(&step(1)), None);
    }

    #[test]
    fn forgetting_a_document_forgets_its_blocks() {
        let docs = recorded();
        docs.forget(&doc());
        assert!(docs.is_empty());
        assert_eq!(docs.refusal_for_block(&step(0)), None);
    }

    /// A moved file keeps its blocks and refuses under its new path.
    #[test]
    fn rehoming_keeps_the_membership_and_names_the_new_path() {
        let docs = recorded();
        docs.rehome(
            &doc(),
            "cooklang",
            Path::new("/vault/Recipes/Pancakes.cook"),
        )
        .expect("a recorded document may be re-homed");
        let refusal = docs
            .refusal_for_block(&step(0))
            .expect("the blocks follow the file");
        assert!(refusal.to_string().contains("Recipes/Pancakes.cook"));
    }

    /// The false-green shape: a caller that only knows the new path cannot
    /// invent a membership, and an empty one would leave every block of an
    /// authoritative file editable.
    #[test]
    fn rehoming_an_unrecorded_document_is_an_error() {
        let docs = ReadOnlyDocuments::new();
        let err = docs
            .rehome(&doc(), "cooklang", Path::new("/vault/Pancakes.cook"))
            .expect_err("nothing recorded this document's blocks");
        assert!(
            err.to_string()
                .contains("before its membership was ever recorded")
        );
    }

    /// A damaged `file` row is not a membership. The only alternative — taking
    /// it as "this file has no read-only blocks" — is the silent
    /// edits-accepted hole itself.
    #[test]
    fn a_persisted_row_without_blocks_is_not_a_membership() {
        let err =
            ReadOnlyMembers::from_persisted_row(Path::new("/vault/Pancakes.cook"), Vec::new())
                .expect_err("an empty row declares nothing");
        assert!(err.to_string().contains("carries no `read_only_blocks`"));
    }

    /// A peer's import lands under an authoritative file's root. It can never
    /// be written to that file, so it must be as uneditable as the file's own
    /// blocks — otherwise pairing reopens the hole the refusal closed.
    #[test]
    fn a_block_imported_under_a_read_only_document_earns_the_refusal() {
        let docs = recorded();
        let imported = EntityUri::block("peer-added");
        assert!(docs.adopt(&step(0), &imported));
        assert!(docs.refusal_for_block(&imported).is_some());
        assert!(!docs.adopt(&EntityUri::block("notes-child"), &imported));
    }

    /// The file never declares an imported block, so a re-ingest — which
    /// re-derives the membership FROM the file — must not be what makes it
    /// editable again. Only the document's own disappearance ends the binding.
    #[test]
    fn an_imported_block_survives_a_re_ingest_and_ends_with_the_document() {
        let docs = recorded();
        let imported = EntityUri::block("peer-added");
        docs.adopt(&doc(), &imported);
        docs.record(
            &doc(),
            "cooklang",
            Path::new("/vault/Pancakes.cook"),
            &declared(&[step(0), step(1)]),
        );
        assert!(docs.refusal_for_block(&imported).is_some());
        docs.forget(&doc());
        assert_eq!(docs.refusal_for_block(&imported), None);
    }
}
