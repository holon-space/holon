//! The per-crate PBT **contribution seam** — the interface by which one
//! subsystem (its companion `holon-X-testing` crate) contributes its slice of
//! the ONE composed keystone PBT.
//!
//! North-star (memory `north-star.md`): ONE env-selected composed PBT,
//! assembled from per-crate contributions; slices are scaffolding to delete.
//! This module is the assembly seam that makes "a crate changed → activate that
//! crate's PBT" a physical, enumerable signal.
//!
//! Two entry points per contributor:
//! - [`PbtContribution`] — the **live** payload folded into the composed run:
//!   boxed [`CapInvariant`]s, cap installers, and generator factories.
//! - [`PbtFootprint`] — the **static, boot-free** enumeration (ids only) the
//!   wiring-ladder's floor and offline map verification read. Kept honest
//!   against the live contribution by a per-crate
//!   `footprint_matches_contribution()` anti-rot test (parse-don't-validate).
//!
//! ## Ref-state independence (modularization-friendliness, plan §4)
//! A contribution NEVER names a concrete reference-state type. Invariants,
//! transitions, and generators read `Ref*` capabilities through the composed
//! [`CapMap`] only, so whether the reference model is today's central monolith
//! or a future per-subsystem `RefLoro` split lives entirely behind the cap
//! traits — this seam does not depend on it.

use crate::component_set::ComponentSet;
use crate::composition::CapInvariant;
use crate::composition::CapMap;

/// Stable identity of the crate that owns a contribution — the wiring-ladder's
/// `crate → Component` key. `Central` is the transitional Phase-0 holder that
/// carries every not-yet-co-located contribution; it dissolves as the subsystem
/// variants take ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrateId {
    /// Transitional holder for contributions still living centrally in
    /// `holon-integration-tests`. Shrinks to nothing as co-location completes.
    Central,
    Loro,
    Turso,
    Engine,
    Orgmode,
    Frontend,
    /// Cross-store seam residue (Turso↔Loro) with no single owning subsystem.
    Seams,
}

impl CrateId {
    /// The cargo crate name that owns this contribution — the reverse-dep key
    /// the ladder floor inverts (`holon-X changed → run holon-X-testing`).
    pub fn crate_name(self) -> &'static str {
        match self {
            CrateId::Central => "holon-integration-tests",
            CrateId::Loro => "holon-loro-testing",
            CrateId::Turso => "holon-turso-testing",
            CrateId::Engine => "holon-engine-testing",
            CrateId::Orgmode => "holon-orgmode-testing",
            CrateId::Frontend => "holon-frontend-testing",
            CrateId::Seams => "holon-integration-tests",
        }
    }
}

/// Installs a subsystem's SUT (and, where a slice hosts them, `Ref`) capability
/// providers into the composed [`CapMap`], gated by the active
/// [`ComponentSet`].
///
/// The central assembler still owns precedence/ordering (§8 risk 2: `CapMap` is
/// insert-only + fail-loud on duplicate); an installer only registers the caps
/// a present component supplies. Empty on the Phase-0 central contributor — the
/// existing central `compose_sut_*` still assembles caps until per-crate
/// installers land (Phase 1+).
pub type CapInstaller = Box<dyn Fn(&ComponentSet, &mut CapMap) + Send + Sync>;

/// A per-variant transition generator a subsystem contributes. The *relative
/// weights / cross-subsystem gating* stay a central policy (plan §3); a factory
/// only names and produces its own variant's strategy. Object-safe so the
/// central generator can fold factories from every crate.
///
/// Phase 0 defines only the identity method (`transition_kind`, matched against
/// [`PbtFootprint::transition_kinds`]); the strategy-producing method lands
/// when the transition enum co-locates (Phase 4), because the concrete
/// `Transition` type it must produce is not nameable in `holon-pbt-core`.
pub trait GeneratorFactory: Send + Sync {
    /// The transition variant kind this factory generates — one
    /// [`PbtFootprint::transition_kinds`] entry.
    fn transition_kind(&self) -> &'static str;
}

/// Everything one subsystem's companion crate contributes to the composed PBT.
pub struct PbtContribution {
    pub crate_id: CrateId,
    pub invariants: Vec<Box<dyn CapInvariant>>,
    pub cap_installers: Vec<CapInstaller>,
    pub generators: Vec<Box<dyn GeneratorFactory>>,
}

impl PbtContribution {
    /// The ids of the invariants this contribution carries, in order — the live
    /// counterpart the anti-rot test compares its static [`PbtFootprint`]
    /// against.
    pub fn invariant_ids(&self) -> Vec<&'static str> {
        self.invariants.iter().map(|inv| inv.id().0).collect()
    }
}

/// The static, boot-free enumeration of a crate's PBT footprint. Read by the
/// ladder floor and offline map verification; NOT the runtime alphabet. Kept in
/// lockstep with the live [`PbtContribution`] by an anti-rot test.
pub struct PbtFootprint {
    pub crate_id: CrateId,
    pub invariant_ids: Vec<&'static str>,
    pub transition_kinds: Vec<&'static str>,
}

/// Fold per-crate contributions into the single composed invariant catalog.
///
/// **Order-safe**: `run_selected` runs *every* selected invariant
/// (`composition::run_selected`), so contributor order never changes which
/// failure surfaces — folding is semantically identical to the hand-maintained
/// central `Vec` it replaces.
pub fn fold_catalog(
    contributions: impl IntoIterator<Item = PbtContribution>,
) -> Vec<Box<dyn CapInvariant>> {
    contributions
        .into_iter()
        .flat_map(|c| c.invariants)
        .collect()
}
