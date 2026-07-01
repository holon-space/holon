//! Per-component durable-state descriptors — the seam behind the
//! consolidator-epoch guard (Model.md invariant 10) and, later, the real
//! handover migration (spec 0008 Phase 4.1).
//!
//! Each block-handling component owns the knowledge of what it persists on
//! disk (Replication.md §2: components are capability profiles, not roles).
//! The epoch guard consumes only this trait: it never names Loro, Turso, or
//! their file layouts.

use std::path::PathBuf;

/// What a component persists on disk for one vault/session.
///
/// `durable_paths` returns every filesystem path the component writes durable
/// state under — an empty vec means "ephemeral, nothing to protect". The
/// interim consolidator handover (wipe-and-reseed) removes exactly these
/// paths; the Phase 4.1 state-preserving migration will extend this seam with
/// a reseed step.
pub trait DurableReplicaState {
    fn durable_paths(&self) -> Vec<PathBuf>;
}
