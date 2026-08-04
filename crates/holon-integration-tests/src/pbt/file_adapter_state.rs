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
}

impl FileAdapterState {
    pub fn new() -> Self {
        Self::default()
    }
}
