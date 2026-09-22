//! The consolidator seam: existence, history and merge, asked of a store
//! without naming one.
//!
//! A consolidator is a store that carries HISTORY. Everything stated against
//! this trait — the existence rule, adoption, the single-adopter rule, the
//! duplicate-id error — is store-blind; Loro is one implementation and the
//! keystone model is another.
//! `docs/Plans/existence-authority-alternatives-2026-09-22.md` derives the
//! interface and the rulings it encodes.
//!
//! Two stores holding only CURRENT state cannot settle whether an id one of
//! them lacks was never created or was deleted. Only history answers that.

use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::capability::ConsolidatorId;

use crate::traits::Result;

/// An opaque point in a consolidator's history: a Loro frontier, a git commit.
///
/// Concrete rather than an associated type, so `dyn Consolidator` is usable —
/// the wiring holds trait objects. Implementations own the encoding and are the
/// only readers of it; callers compare through
/// [`Consolidator::is_ancestor`] and never parse it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version(String);

impl Version {
    pub fn new(encoded: impl Into<String>) -> Self {
        Self(encoded.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A replica registered for GC retention (invariant 9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReplicaId(String);

impl ReplicaId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a consolidator's history says about an entity id.
///
/// [`Never`](Seen::Never) and [`Deleted`](Seen::Deleted) are both "not live"
/// and mean opposite things: `Never` licenses adoption, `Deleted` forbids it
/// unless a replica's base proves the user re-added the entity. Collapsing them
/// into a boolean is what resurrects deleted blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// No entity with this id has ever existed here.
    Never,
    /// The entity exists now.
    Live,
    /// The entity existed and was deleted.
    ///
    /// The version is the consolidator's head at the time of asking, not the
    /// version that performed the delete: the Loro tree answers deletion as a
    /// current-state predicate over tombstones and carries no per-node delete
    /// version. A rule that needs the true delete point needs a history index
    /// first.
    Deleted(Version),
    /// This store keeps no history, so the question does not apply. Distinct
    /// from `Never`: it is "cannot say", and a caller must not read it as
    /// permission to adopt.
    NoHistory,
}

impl Seen {
    /// Does an entity with this id exist right now?
    pub fn is_live(&self) -> bool {
        matches!(self, Seen::Live)
    }

    /// May an entity with this id be adopted — created in this consolidator
    /// with its id preserved?
    ///
    /// Only `Never`. `Deleted` needs a replica base to overrule it and
    /// `NoHistory` cannot answer, so neither decides adoption alone.
    pub fn admits_adoption(&self) -> bool {
        matches!(self, Seen::Never)
    }

    /// The store holds no live entity for this id AND has a history saying so.
    ///
    /// `NoHistory` is excluded: "cannot say" is not "absent", and a caller that
    /// read it as absence would refuse a store it simply cannot ask.
    pub fn is_absent_from_history(&self) -> bool {
        matches!(self, Seen::Never | Seen::Deleted(_))
    }
}

/// One entity's change between two versions, restricted to the fields the
/// store carries.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityChange {
    Created { fields: HashMap<String, Value> },
    Updated { fields: HashMap<String, Value> },
    Deleted,
}

/// The entity-level difference between two versions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Delta {
    pub changes: Vec<(EntityUri, EntityChange)>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// A store that carries history and can therefore own existence.
///
/// Object-safe. An implementation that cannot answer a method returns `Err`.
#[async_trait]
pub trait Consolidator: Send + Sync {
    /// Is `a` in `b`'s past? A partial order is enough.
    async fn is_ancestor(&self, a: &Version, b: &Version) -> Result<bool>;

    /// The identity the consolidator-epoch marker persists (invariant 10).
    /// Bases are meaningful only within one epoch.
    fn epoch_id(&self) -> ConsolidatorId;

    /// The current head.
    async fn head(&self) -> Result<Version>;

    /// Per-entity changes between two versions.
    async fn diff(&self, from: &Version, to: &Version) -> Result<Delta>;

    /// What this store's history says about `id`.
    async fn ever_seen(&self, id: &EntityUri) -> Result<Seen>;

    /// Hold history back for `replica` from `at`, so a tombstone this replica
    /// has not yet seen cannot be collected out from under it.
    async fn register_base(&self, replica: &ReplicaId, at: &Version) -> Result<()>;

    /// Drop `replica`'s retention claim.
    async fn release_base(&self, replica: &ReplicaId) -> Result<()>;

    /// Every replica currently holding a retention claim.
    async fn registered_bases(&self) -> Result<Vec<ReplicaId>>;
}

/// A store diffed against a base to yield inbound intent (invariant 1).
///
/// Inbound intent is `diff(base, current)` — always against the base, never
/// against a cache.
#[async_trait]
pub trait Replica: Send + Sync {
    fn replica_id(&self) -> ReplicaId;

    /// The version this replica was last reconciled at.
    async fn base(&self) -> Result<Version>;

    /// Does this replica carry `field` for `kind`? A field it does not carry
    /// never travels through it, so its absence here is not evidence of a
    /// delete.
    fn carries(&self, kind: &str, field: &str) -> bool;
}
