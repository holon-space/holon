//! SQL projection semantics across the migration window (senior-review
//! amendment — this is part of Inc 4, not an afterthought).
//!
//! A crossing is delete+create across two container docs, but the SQL
//! projection (blocks table, matviews, consolidator) is **global**. This module
//! is the DESIGN OF RECORD for how the two reconcile.
//!
//! ## Which side projects during the (journaled, resumable) migration window
//!
//! The **target** container owns the block's SQL projection once the crossing
//! commits. Concretely, keyed off the [`crate::journal::MigrationJournal`]:
//! - `CreateInTarget` **projects** the block's rows under the target container.
//! - `DeleteFromSource` **retracts** the block's rows from the source
//!   container.
//!
//! There is never a moment where BOTH sides project the same block under
//! *different* containers — the two ops touch disjoint (container, block) rows,
//! and the leak-direction ordering (see [`crate::journal`]) fixes which side is
//! momentarily authoritative:
//! - **widen**: create-in-target first ⇒ during the window the block is
//!   projected under BOTH containers (union = the wider, intended target). No
//!   leak: the extra visibility is exactly the audience the widen grants.
//! - **narrow**: delete-from-source first ⇒ during the window the block is
//!   projected under NEITHER container = **disclosed transient invisibility**
//!   (Inc 2 narrowing contract). It is NOT a silent orphan: the row is
//!   *absent*, not present-with-a-dangling-container.
//!
//! ## Crash between the two projection steps
//!
//! The journal is the unit of atomicity. A crash between `DeleteFromSource` and
//! `CreateInTarget` (the narrow case) leaves the block transiently invisible
//! and the journal step `Pending`; resume completes the projection. This
//! transient invisibility is disclosed (surfaced as a degraded-mode signal),
//! never a silent orphan row. See CLAUDE.md "Fail Loud, Never Fake" priority
//! order: this is level 2 (falls back *visibly*), never level 4.
//!
//! ## Orphan-row tripwire
//!
//! The failure mode the window must never leave behind is a *present* block row
//! whose container is not a registered container (a silently orphaned row —
//! e.g. create-in-target projected under a container the registry never knew,
//! or a delete-from-source that half-applied). [`assert_no_orphan_rows`] is the
//! projection-side assertion; its WIRING into the consolidator lands with Inc 3
//! (the container registry), but the helper ships now so Inc 3 only calls it.

use std::collections::BTreeSet;

use crate::types::BlockId;
use crate::types::ContainerId;

/// A projected block row whose container is not registered — a silent orphan.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "orphan block row: block `{}` is projected under container `{}`, which is \
     not a registered container (ADR 0028 Inc 4 projection invariant — every \
     block row's container MUST be registered; a crossing left this behind)",
    block.0,
    container.0
)]
pub struct OrphanRowError {
    pub block: BlockId,
    pub container: ContainerId,
}

/// Tripwire: every *present* projected block row's container must be a
/// registered container. Transient invisibility during a migration window is
/// row ABSENCE and does not trip this (there is no row to point at an
/// unregistered container).
///
/// Fails loudly on the FIRST orphan (parse-don't-validate: the projection is
/// either wholly consistent or the migration is broken — no partial "warn and
/// continue"). Wiring lands with Inc 3.
pub fn assert_no_orphan_rows<'a, I>(
    projected: I,
    registered: &BTreeSet<ContainerId>,
) -> Result<(), OrphanRowError>
where
    I: IntoIterator<Item = (&'a BlockId, &'a ContainerId)>,
{
    for (block, container) in projected {
        if !registered.contains(container) {
            return Err(OrphanRowError {
                block: block.clone(),
                container: container.clone(),
            });
        }
    }
    Ok(())
}
