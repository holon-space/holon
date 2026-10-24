//! File-adapter file-state fragment of the PBT reference model (ADR 0004 Phase
//! 5).
//!
//! `FileAdapterState` holds the doc_uri → filename mapping owned by a **file
//! adapter** (org or markdown — peers, neither privileged; ADR 0004). Per ADR
//! 0004 this is an adapter concern, not domain: filenames are how a file
//! backend persists a document on disk, distinct from the document's domain
//! identity (its `EntityUri`, which is simply the keyset of this map).
//! Isolating it here lets a wiring without any file adapter drop the fragment.
//!
//! H10 note: `documents` historically tangled three concerns — doc identity
//! (the `EntityUri` keys), the filename mapping (the values), and
//! persists-across-delete-for-undo. Identity is the keyset (no separate
//! structure — a single source of truth); the undo-persistence concern is
//! moot while the undo subsystem is dormant (see `push_undo_snapshot`). The
//! `Document.filename` semantics themselves are pending ADR 0009.
//!
//! @pbt kind ref
//! @pbt covers file-adapter-state — `doc_uri → filename` map (doc identity =
//!   its keyset, single source of truth) plus pre-boot file/VCS flags. An
//!   adapter concern, not domain identity.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;

/// File-adapter file-state extracted from `ReferenceState` (ADR 0004 Phase 5).
///
/// Also holds the pre-startup file/VCS boot flags (RefStateSplit Inc 3): the
/// directories, org files, and git/jj initialization a `StartApp` observes.
/// These are file-adapter boot concerns (what the on-disk workspace looked like
/// before the app booted), so they live with the filename mapping rather than
/// loose on `ReferenceState`.
#[derive(Debug, Clone, Default)]
pub struct FileAdapterState {
    /// Created documents (doc_uri -> file_name).
    /// `BTreeMap` for deterministic iteration (see `BlockState::blocks`).
    pub documents: BTreeMap<EntityUri, String>,

    /// Blocks that exist because a FILE was ingested — the doc root page plus
    /// every block the watcher parsed out of it. Prod's undo stack holds only
    /// User-origin ops (`operation_engine.rs:1220`), so `engine.undo()` never
    /// removes these; the oracle's whole-`block_state` snapshot would, which is
    /// why this set lives OUTSIDE the snapshot next to `documents` (see
    /// `ReferenceState::rematerialize_file_ingested`).
    pub ingest_origin_blocks: BTreeSet<EntityUri>,

    /// Pre-startup directories created (relative paths).
    pub pre_startup_directories: Vec<String>,

    /// Whether git has been initialized.
    pub git_initialized: bool,

    /// Whether jj has been initialized.
    pub jj_initialized: bool,

    /// Number of pre-startup org files created (for weighting StartApp).
    pub pre_startup_file_count: usize,

    /// Blocks an external editor pasted into a second file while the file
    /// that owns them still holds them (D229.b), by the pasted heading.
    pub copies: BTreeMap<EntityUri, PastedCopy>,

    /// Blocks deleted in Holon that a file holding a copy brought back, and
    /// that file's document.
    pub kept_after_delete: BTreeMap<EntityUri, EntityUri>,

    /// Blocks whose undone deletion an edit ended in this process, and the
    /// name of the file the deletion was undone in.
    pub deletions_ended_by_edit: BTreeMap<EntityUri, String>,

    /// Documents whose file some process keeps rewriting with its own bytes.
    /// Churn only delays a write-back; it never changes the expected end
    /// state. A deleted document's churn ends with its file.
    pub write_churn: BTreeSet<EntityUri>,

    /// A churn was armed in this run.
    pub write_churn_ran: bool,

    /// Documents whose file an editor emptied, which Holon refuses to
    /// ingest until the user accepts the removal or rewrites the file from
    /// the store, with the document's blocks at the refusal.
    pub refused: BTreeMap<EntityUri, Vec<Block>>,

    /// Each document's file as the last landed write-back or ingest of it
    /// recorded it: the base the next boot merges the file against.
    pub merge_base: BTreeMap<EntityUri, MergeBase>,

    /// The conflict copies a crash-restore left beside each file, by the
    /// file's final path component, each as the block texts it holds.
    pub crash_restore_copies: BTreeMap<String, Vec<Vec<String>>>,
}

/// A document's file as a landed write-back or ingest recorded it.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeBase {
    /// The document's blocks, timestamps cleared: the file holds none.
    pub blocks: Vec<Block>,
    /// An ingest of an id-less file leaves the ids to a write-back.
    pub ids_on_disk: bool,
}

impl FileAdapterState {
    pub fn churning_docs(&self) -> impl Iterator<Item = &EntityUri> {
        self.write_churn
            .iter()
            .filter(|doc| self.documents.contains_key(*doc))
    }

    pub fn refused_docs(&self) -> impl Iterator<Item = &EntityUri> {
        self.refused
            .keys()
            .filter(|doc| self.documents.contains_key(*doc))
    }

    pub fn write_churn_armed(&self) -> bool {
        self.churning_docs().next().is_some()
    }
}

/// A heading on disk in other files beside its owner's (D229.b).
#[derive(Debug, Clone)]
pub struct PastedCopy {
    /// The subtree as the editor first pasted it, heading first: the common
    /// ancestor of every copy and the store.
    pub pasted: Vec<Block>,
    /// Each document whose file holds a copy, and what that copy holds.
    pub files: BTreeMap<EntityUri, CopyOnDisk>,
    /// A copy and the store were edited apart; the user's next deletion of
    /// one of them decides.
    pub conflict: bool,
    /// The app holds `pasted` as the common ancestor: the paste happened in
    /// this session. A restart forgets it.
    pub base_known: bool,
    /// Members the user deleted from the owner's file, which Holon put back.
    pub undone: BTreeMap<EntityUri, UndoneMember>,
}

/// A member of a copied heading whose deletion from the owner's file Holon
/// undid.
#[derive(Debug, Clone)]
pub struct UndoneMember {
    /// The documents whose copy still holds it.
    pub holders: std::collections::BTreeSet<EntityUri>,
    /// The member as Holon put it back. The deletion removes only this
    /// version.
    pub put_back: Block,
}

/// One file's copy of a pasted heading.
#[derive(Debug, Clone)]
pub struct CopyOnDisk {
    /// The subtree as the editor pasted it into this file, heading first,
    /// less the members deleted from it since.
    pub blocks: Vec<Block>,
    /// The copy's task keyword as the editor last saved it.
    pub disk_task_state: Option<String>,
}

impl FileAdapterState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl crate::pbt::reference_state::ReferenceState {
    /// `doc`'s blocks as a landed write-back puts them into its file.
    pub fn file_blocks(&self, doc: &EntityUri) -> Vec<Block> {
        self.doc_rows(doc)
            .into_iter()
            .map(|mut block| {
                block.created_at = 0;
                block.updated_at = 0;
                block
            })
            .collect()
    }

    /// Every write-back no churn holds lands, and records its file as the
    /// merge base.
    pub fn record_landed_write_backs(&mut self) {
        let landed: Vec<EntityUri> = self
            .files
            .documents
            .keys()
            .filter(|doc| !self.files.write_churn.contains(*doc))
            .cloned()
            .collect();
        for doc in landed {
            let base = MergeBase {
                blocks: self.file_blocks(&doc),
                ids_on_disk: true,
            };
            self.files.merge_base.insert(doc, base);
        }
    }

    /// The ingest of `doc`'s id-less file while a churn holds its write-back:
    /// the base is the file without ids, and stamping them is owed.
    pub fn record_held_id_less_ingest(&mut self, doc: &EntityUri) {
        if !self.files.write_churn.contains(doc) {
            return;
        }
        let base = MergeBase {
            blocks: self.file_blocks(doc),
            ids_on_disk: false,
        };
        self.files.merge_base.insert(doc.clone(), base);
    }

    /// Whether `doc`'s file is behind the store: a write-back of it is owed.
    pub fn write_back_owed(&self, doc: &EntityUri) -> bool {
        let base = self
            .files
            .merge_base
            .get(doc)
            .unwrap_or_else(|| panic!("{doc} has no merge base in the model"));
        !base.ids_on_disk || base.blocks != self.file_blocks(doc)
    }
}
