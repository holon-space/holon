//! Capability descriptor + consolidator selection (block-sync rework).
//!
//! Lives in `holon-api` (the shared low layer) so every block-handling
//! component — `holon` (Loro/SQL), `holon-orgmode`, `holon-core`'s
//! `BlockOrdering` — can speak the same capability vocabulary. This is the
//! type-level expression of `docs/Architecture/Replication.md` §2 ("components
//! are capability profiles, not roles").
//!
//! Today there are exactly **two** real runtime configurations, modelled by
//! [`CapabilityProfile`]:
//!
//! - [`CapabilityProfile::Projected`] — a separate upstream consolidator owns
//!   order/merge and is the text-merge home; SQL is a downstream projection
//!   (sink). (Loro is the only adapter that provides this today — but the
//!   profile names the *mechanism*, not the adapter; the concrete "is Loro
//!   present" fact appears only at the [`CapabilityProfile::detect`] boundary.)
//! - [`CapabilityProfile::Direct`] — degraded: the SQL store is itself the
//!   order owner (single-writer via `new_child_anchor`/parser keys), written
//!   directly with no separate downstream feed; text is transient.
//!
//! The descriptor is sealed (no `#[non_exhaustive]`, no constructor that admits
//! a third config) so the type system carries the "only two configs" decision.
//! Generalizing to the open capability lattice (the §2 axes table) is a
//! deliberate later step (D slice 2), not something a caller can do by
//! accident.
//!
//! **Consolidator is pinned at session start** (Risk #4, decided): who owns
//! order/merge is resolved once into a [`SessionCapabilities`] and never
//! changes for the life of the session. Adding or removing Loro mid-session
//! requires a full re-sync, not a live handoff — so [`SessionCapabilities`] is
//! immutable after construction.

/// Which storage substrate a DI container is assembled with (ADR 0004 Phase 9).
///
/// `Turso` is the full substrate (matviews, PRQL/GQL, CDC). `LoroMemory`
/// assembles a Turso-free container: no `TursoBackend` connection, no Turso
/// schema/matview registration, no `BackendEngine` — storage is Loro only, read
/// through a Loro `BlockQuerySource`. A `LoroMemory` container deliberately
/// gives up SQL/GQL/PRQL queries (there is no query substrate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageSelector {
    /// The full Turso substrate (current production default).
    #[default]
    Turso,
    /// Loro-only, in-memory — no Turso connection or schema.
    LoroMemory,
}

/// The two real runtime sync configurations. Sealed: not an open N-peer
/// lattice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityProfile {
    /// A separate upstream consolidator owns order/merge; SQL is a downstream
    /// projection of it. (Loro provides this today.)
    Projected,
    /// No upstream consolidator (degraded). The SQL store owns order and is
    /// written directly; text is transient.
    Direct,
}

impl CapabilityProfile {
    /// Resolve the profile from whether an upstream (CRDT) consolidator is
    /// present. This is the detect-from-capabilities boundary: the runtime
    /// passes the single concrete fact it knows — does a Loro doc exist for
    /// this session — and gets back the mechanism profile. This is the one
    /// place in this layer that names the concrete adapter.
    pub fn detect(loro_present: bool) -> Self {
        if loro_present {
            Self::Projected
        } else {
            Self::Direct
        }
    }

    /// Whether there is a separate downstream projection feed into the SQL
    /// sink. True only for [`Self::Projected`] — in [`Self::Direct`] the store
    /// is written directly.
    pub fn has_downstream_projection(self) -> bool {
        matches!(self, Self::Projected)
    }

    /// Who owns order/merge under this profile.
    pub fn consolidator(self) -> Consolidator {
        match self {
            Self::Projected => Consolidator::Upstream,
            Self::Direct => Consolidator::Store,
        }
    }
}

/// Who owns sibling order and merge for the session. Resolved from the
/// [`CapabilityProfile`] and pinned at session start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consolidator {
    /// A separate upstream consolidator owns order (fractional index) and merge
    /// (CRDT); the SQL sink is its derived single-writer projection. (Loro.)
    Upstream,
    /// The SQL store itself is the consolidator: owns order (single-writer
    /// `sort_key`), written directly, no CRDT merge.
    Store,
}

/// Opaque, persistable identity of a consolidator configuration — what the
/// epoch marker records across sessions (Model.md invariant 10). The guard that
/// consumes this never interprets the value; it only writes it, compares it for
/// equality, and prints it. Today the id names the *mechanism*
/// ([`CapabilityProfile`] variant), not a concrete adapter — consistent with
/// this module's rule that adapter names appear only at the `detect` boundary.
/// When D slice 2 opens the capability lattice, the derivation extends; the
/// opaque consumers don't change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidatorId(String);

impl ConsolidatorId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConsolidatorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The session's capability profile and its consolidator, pinned once at
/// startup. Immutable: changing the consolidator requires a full re-sync, not a
/// live handoff (Risk #4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionCapabilities {
    profile: CapabilityProfile,
    consolidator: Consolidator,
}

impl SessionCapabilities {
    /// Pin the session capabilities from a profile. Call once at session start.
    pub fn pin(profile: CapabilityProfile) -> Self {
        Self {
            profile,
            consolidator: profile.consolidator(),
        }
    }

    /// Detect the profile from Loro presence and pin in one step.
    pub fn detect_and_pin(loro_present: bool) -> Self {
        Self::pin(CapabilityProfile::detect(loro_present))
    }

    pub fn profile(self) -> CapabilityProfile {
        self.profile
    }

    pub fn consolidator(self) -> Consolidator {
        self.consolidator
    }

    /// The identity the consolidator-epoch marker persists across sessions
    /// (Model.md invariant 10). Derived from the pinned profile so there is
    /// exactly one derivation of "who consolidates" — the same pin the rest of
    /// the session uses.
    pub fn consolidator_id(self) -> ConsolidatorId {
        ConsolidatorId::new(match self.profile {
            CapabilityProfile::Projected => "projected",
            CapabilityProfile::Direct => "direct",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_maps_loro_presence_to_profile() {
        assert_eq!(
            CapabilityProfile::detect(true),
            CapabilityProfile::Projected
        );
        assert_eq!(CapabilityProfile::detect(false), CapabilityProfile::Direct);
    }

    #[test]
    fn projected_owns_upstream_and_has_downstream() {
        let p = CapabilityProfile::Projected;
        assert!(p.has_downstream_projection());
        assert_eq!(p.consolidator(), Consolidator::Upstream);
    }

    #[test]
    fn direct_owns_via_store_and_has_no_downstream() {
        let p = CapabilityProfile::Direct;
        assert!(!p.has_downstream_projection());
        assert_eq!(p.consolidator(), Consolidator::Store);
    }

    #[test]
    fn pinned_caps_carry_consolidator() {
        let caps = SessionCapabilities::detect_and_pin(true);
        assert_eq!(caps.profile(), CapabilityProfile::Projected);
        assert_eq!(caps.consolidator(), Consolidator::Upstream);

        let degraded = SessionCapabilities::detect_and_pin(false);
        assert_eq!(degraded.consolidator(), Consolidator::Store);
    }
}
