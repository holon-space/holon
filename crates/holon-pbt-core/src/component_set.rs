//! `ComponentSet` — the single source of truth for a PBT run (ADR 0009).
//!
//! @pbt kind cap-plumbing
//! @pbt covers all-slices — the subsystem lattice (Wiring + projections) the
//!   bisection shrinker walks; membership only, providers live in the CapMap.
//!
//! A `ComponentSet` is a [`Wiring`] (ADR 0007: storage/sync adapters + actors)
//! extended with the set of **observable projections** that are checked against
//! the reference oracle and can be toggled as bisection nodes. From it we
//! derive *everything*: the transition alphabet (via `Wiring`), the invariant
//! `Subsystem` selection (`subsystems()` mapping, in the consumer crate that
//! owns `Subsystem`), the reference fragments, and the runner
//! (`needs_real_window`).
//!
//! ## Why a typed member, not a bag of strings
//!
//! Parse-don't-validate: a `ComponentSet` is parsed once into `{wiring,
//! projections}` and never re-validated as a loose tuple downstream. The
//! ergonomic [`ComponentSet::of`] takes a closed [`Component`] enum so a
//! literal list can't smuggle in an ill-typed member.

use std::collections::BTreeSet;

use crate::wiring::Actor;
use crate::wiring::StorageAdapter;
use crate::wiring::SyncAdapter;
use crate::wiring::Wiring;
use crate::wiring::WiringError;

/// An observable projection of the domain that a run can check and bisection
/// can toggle. Distinct from a `Wiring` adapter/actor: a projection is a *view*
/// of state (the oracle compares it to the reference), not a mutation source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Projection {
    /// The `ReactiveEngine` ViewModel tree — drives `Subsystem::{ViewModel,
    /// Renderer}`. Always the oracle target when present.
    ViewModel,
    /// `InputState` + active-editor mirror — drives `Subsystem::EditorState`.
    EditorState,
}

/// A single member of a [`ComponentSet`]. The closed alternative to a mixed
/// `[Loro, ViewModel, UI]` literal (three different types): every element is
/// one `Component`, so `ComponentSet::of` can lower a homogeneous array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Component {
    Storage(StorageAdapter),
    Sync(SyncAdapter),
    Actor(Actor),
    Project(Projection),
}

impl From<StorageAdapter> for Component {
    fn from(a: StorageAdapter) -> Self {
        Component::Storage(a)
    }
}
impl From<SyncAdapter> for Component {
    fn from(a: SyncAdapter) -> Self {
        Component::Sync(a)
    }
}
impl From<Actor> for Component {
    fn from(a: Actor) -> Self {
        Component::Actor(a)
    }
}
impl From<Projection> for Component {
    fn from(p: Projection) -> Self {
        Component::Project(p)
    }
}

/// `Wiring` + the observable projections that are checked and bisectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSet {
    pub wiring: Wiring,
    pub projections: BTreeSet<Projection>,
}

/// Why a [`ComponentSet`] is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentSetError {
    /// The underlying `Wiring` is itself invalid.
    Wiring(WiringError),
    /// `Actor::UI` is present but the `ViewModel` projection is not — a real
    /// window with nothing to render against the oracle (ADR 0009
    /// §"validate(): UI ⟹ ≥1 storage + ViewModel"; the storage half is already
    /// enforced by `Wiring::validate`).
    UiWithoutViewModel,
}

impl std::fmt::Display for ComponentSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentSetError::Wiring(e) => write!(f, "invalid wiring: {e}"),
            ComponentSetError::UiWithoutViewModel => {
                write!(f, "Actor::UI requires the ViewModel projection")
            }
        }
    }
}

impl std::error::Error for ComponentSetError {}

impl ComponentSet {
    // ── Construction ────────────────────────────────────────────────────

    /// Build from an explicit wiring + projection set.
    pub fn new(wiring: Wiring, projections: impl IntoIterator<Item = Projection>) -> Self {
        ComponentSet {
            wiring,
            projections: projections.into_iter().collect(),
        }
    }

    /// Build from a flat list of [`Component`]s (ADR 0009 §0).
    /// Storage/sync/actor members lower into the `Wiring`; projection
    /// members into `projections`. Accepts anything that converts into
    /// `Component`, so callers can write
    /// `ComponentSet::of([Component::from(Loro), Component::from(ViewModel)])`.
    pub fn of(components: impl IntoIterator<Item = Component>) -> Self {
        let (mut storage, mut sync, mut actors, mut projections) =
            (Vec::new(), Vec::new(), Vec::new(), BTreeSet::new());
        for c in components {
            match c {
                Component::Storage(a) => storage.push(a),
                Component::Sync(a) => sync.push(a),
                Component::Actor(a) => actors.push(a),
                Component::Project(p) => {
                    projections.insert(p);
                }
            }
        }
        ComponentSet {
            wiring: Wiring::custom(storage, sync, actors),
            projections,
        }
    }

    // ── Blessed sets (ADR 0009 §2 — named one-liners CI can own) ─────────

    /// Wide + real UI window — the GPUI runner. `Wiring::full()` (which
    /// includes `Actor::UI`) + both projections. Subsystems = all 9.
    pub fn full_gpui() -> Self {
        Self::new(
            Wiring::full(),
            [Projection::ViewModel, Projection::EditorState],
        )
    }

    /// Wide, cheap, headless — the CI-default. `Wiring::full()` minus
    /// `Actor::UI`
    /// + both projections. Subsystems = headless_wide (all but FrontendBounds).
    pub fn full_headless() -> Self {
        let mut wiring = Wiring::full();
        wiring.actors.remove(&Actor::UI);
        Self::new(wiring, [Projection::ViewModel, Projection::EditorState])
    }

    /// Fast inner loop — `Wiring::loro_backend()` + `ViewModel` only. The
    /// scoped set goal 1 names: smallest interesting set for Loro↔VM work.
    pub fn loro_vm_fast() -> Self {
        Self::new(Wiring::loro_backend(), [Projection::ViewModel])
    }

    /// Turso-only (no Loro) wide slice — `Wiring::sql_only()` + both
    /// projections. Carries both projections so `subsystems()` reproduces
    /// the pre-ADR `headless_wide` selection (the `*_sql_only` blessed
    /// slices). The `Actor::UI` in `sql_only`'s manifest is normalised away
    /// headless by the runner (it derives UI from a real window, not the
    /// manifest), so this is the one-line form for the headless Turso-only
    /// slices.
    pub fn sql_only() -> Self {
        Self::new(
            Wiring::sql_only(),
            [Projection::ViewModel, Projection::EditorState],
        )
    }

    /// ComponentSet equivalents of the existing blessed PBT slices, with the
    /// projections that make `subsystems()` reproduce today's headless
    /// selection (ADR 0009 migration step 1a). Used as the
    /// behaviour-preservation anchors.
    pub fn blessed_sets() -> Vec<ComponentSet> {
        vec![
            Self::full_gpui(),
            Self::full_headless(),
            // loro_backend / org_create / sql_only carry both projections so the
            // running-invariant set matches the pre-ADR `headless_wide` slices.
            Self::new(
                Wiring::loro_backend(),
                [Projection::ViewModel, Projection::EditorState],
            ),
            Self::new(
                Wiring::org_create_ordering(),
                [Projection::ViewModel, Projection::EditorState],
            ),
            Self::sql_only(),
            Self::loro_vm_fast(),
        ]
    }

    // ── Queries ─────────────────────────────────────────────────────────

    pub fn has_storage(&self, a: StorageAdapter) -> bool {
        self.wiring.has_storage(a)
    }
    pub fn has_actor(&self, a: Actor) -> bool {
        self.wiring.has_actor(a)
    }
    pub fn has_projection(&self, p: Projection) -> bool {
        self.projections.contains(&p)
    }

    /// The runner is derived, not chosen (ADR 0009 §5): a set with `Actor::UI`
    /// needs the phased GPUI loop on a real window; otherwise the proptest
    /// headless runner.
    pub fn needs_real_window(&self) -> bool {
        self.has_actor(Actor::UI)
    }

    /// Subset relation over all four axes — the lattice order for bisection.
    pub fn is_subset_of(&self, other: &ComponentSet) -> bool {
        self.wiring
            .storage_adapters
            .is_subset(&other.wiring.storage_adapters)
            && self
                .wiring
                .sync_adapters
                .is_subset(&other.wiring.sync_adapters)
            && self.wiring.actors.is_subset(&other.wiring.actors)
            && self.projections.is_subset(&other.projections)
    }

    // ── Validity (ADR 0009 §"Migration" step 6) ─────────────────────────

    pub fn validate(&self) -> Result<(), ComponentSetError> {
        self.wiring.validate().map_err(ComponentSetError::Wiring)?;
        if self.has_actor(Actor::UI) && !self.has_projection(Projection::ViewModel) {
            return Err(ComponentSetError::UiWithoutViewModel);
        }
        Ok(())
    }

    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }

    // ── Lattice walk for bisection (ADR 0009 §3) ────────────────────────

    /// Every *valid* set reachable by removing exactly one component (a storage
    /// adapter, sync adapter, actor, or projection). The downward neighbours in
    /// the bisection lattice; pruned to valid nodes (`validate()`), so e.g.
    /// dropping `ViewModel` while `UI` is present is not offered.
    pub fn valid_children(&self) -> Vec<ComponentSet> {
        let mut out = Vec::new();
        for a in &self.wiring.storage_adapters {
            out.push(self.without(Component::Storage(*a)));
        }
        for a in &self.wiring.sync_adapters {
            out.push(self.without(Component::Sync(*a)));
        }
        for a in &self.wiring.actors {
            out.push(self.without(Component::Actor(*a)));
        }
        for p in &self.projections {
            out.push(self.without(Component::Project(*p)));
        }
        out.retain(ComponentSet::is_valid);
        out
    }

    /// A copy with one component removed (no-op if absent).
    pub fn without(&self, c: Component) -> ComponentSet {
        let mut next = self.clone();
        match c {
            Component::Storage(a) => {
                next.wiring.storage_adapters.remove(&a);
            }
            Component::Sync(a) => {
                next.wiring.sync_adapters.remove(&a);
            }
            Component::Actor(a) => {
                next.wiring.actors.remove(&a);
            }
            Component::Project(p) => {
                next.projections.remove(&p);
            }
        }
        next
    }

    /// Every *valid* set reachable by adding back exactly one component that is
    /// present in `ceiling` but absent here. The upward neighbours in the
    /// bisection lattice — needed for absent-component bugs (a handler missing
    /// for the disabled-component case), which manifest at the *small* end and
    /// invert the lattice direction (ADR 0009 §3 `UpwardMinimal`). Bounded by
    /// the observed maximal (`ceiling`) set so the walk stays within the
    /// lattice between a candidate and the full reproducing set, and pruned
    /// to valid nodes.
    pub fn valid_parents_within(&self, ceiling: &ComponentSet) -> Vec<ComponentSet> {
        let mut out = Vec::new();
        for a in ceiling
            .wiring
            .storage_adapters
            .difference(&self.wiring.storage_adapters)
        {
            out.push(self.with(Component::Storage(*a)));
        }
        for a in ceiling
            .wiring
            .sync_adapters
            .difference(&self.wiring.sync_adapters)
        {
            out.push(self.with(Component::Sync(*a)));
        }
        for a in ceiling.wiring.actors.difference(&self.wiring.actors) {
            out.push(self.with(Component::Actor(*a)));
        }
        for p in ceiling.projections.difference(&self.projections) {
            out.push(self.with(Component::Project(*p)));
        }
        out.retain(ComponentSet::is_valid);
        out
    }

    /// A copy with one component added (no-op if already present).
    pub fn with(&self, c: Component) -> ComponentSet {
        let mut next = self.clone();
        match c {
            Component::Storage(a) => {
                next.wiring.storage_adapters.insert(a);
            }
            Component::Sync(a) => {
                next.wiring.sync_adapters.insert(a);
            }
            Component::Actor(a) => {
                next.wiring.actors.insert(a);
            }
            Component::Project(p) => {
                next.projections.insert(p);
            }
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blessed_sets_are_all_valid() {
        for set in ComponentSet::blessed_sets() {
            assert!(set.validate().is_ok(), "blessed set invalid: {set:?}");
        }
    }

    #[test]
    fn ui_without_viewmodel_is_rejected() {
        // sql_only has Actor::UI; drop the ViewModel projection → invalid.
        let bad = ComponentSet::new(Wiring::sql_only(), [Projection::EditorState]);
        assert_eq!(bad.validate(), Err(ComponentSetError::UiWithoutViewModel));
    }

    #[test]
    fn ui_with_viewmodel_is_accepted() {
        let ok = ComponentSet::new(Wiring::sql_only(), [Projection::ViewModel]);
        assert!(ok.is_valid());
    }

    #[test]
    fn needs_real_window_iff_ui() {
        assert!(ComponentSet::full_gpui().needs_real_window());
        assert!(!ComponentSet::full_headless().needs_real_window());
        assert!(!ComponentSet::loro_vm_fast().needs_real_window());
    }

    #[test]
    fn of_lowers_components_into_wiring_and_projections() {
        let set = ComponentSet::of([
            Component::from(StorageAdapter::Loro),
            Component::from(Projection::ViewModel),
        ]);
        assert!(set.has_storage(StorageAdapter::Loro));
        assert!(set.has_projection(Projection::ViewModel));
        assert!(!set.has_projection(Projection::EditorState));
        assert_eq!(set, ComponentSet::loro_vm_fast());
    }

    #[test]
    fn headless_is_a_subset_of_gpui() {
        // full_gpui ⊋ full_headless by exactly Actor::UI + FrontendBounds.
        let headless = ComponentSet::full_headless();
        let gpui = ComponentSet::full_gpui();
        assert!(headless.is_subset_of(&gpui));
        assert!(!gpui.is_subset_of(&headless));
    }

    #[test]
    fn valid_parents_within_add_one_component_toward_ceiling() {
        // From the fast scoped set up toward full_headless: each parent is a
        // valid proper superset, still ⊆ ceiling, differing by one component.
        let scoped = ComponentSet::loro_vm_fast();
        let ceiling = ComponentSet::full_headless();
        let parents = scoped.valid_parents_within(&ceiling);
        assert!(!parents.is_empty());
        for parent in &parents {
            assert!(scoped.is_subset_of(parent));
            assert_ne!(parent, &scoped);
            assert!(parent.is_subset_of(&ceiling));
            assert!(parent.is_valid());
        }
        // Adding the EditorState projection is one valid parent within ceiling.
        let add_editor = scoped.with(Component::Project(Projection::EditorState));
        assert!(parents.contains(&add_editor));
    }

    #[test]
    fn with_and_without_are_inverse() {
        let set = ComponentSet::loro_vm_fast();
        let added = set.with(Component::Storage(StorageAdapter::Turso));
        assert!(added.has_storage(StorageAdapter::Turso));
        assert_eq!(
            added.without(Component::Storage(StorageAdapter::Turso)),
            set
        );
    }

    #[test]
    fn valid_children_are_valid_proper_subsets() {
        let set = ComponentSet::full_gpui();
        let children = set.valid_children();
        assert!(!children.is_empty());
        for child in &children {
            assert!(child.is_subset_of(&set));
            assert_ne!(child, &set);
            assert!(child.is_valid());
        }
        // Dropping ViewModel from full_gpui (UI present) would be invalid, so it
        // must NOT appear as a child.
        let drop_vm = set.without(Component::Project(Projection::ViewModel));
        assert!(!drop_vm.is_valid());
        assert!(!children.contains(&drop_vm));
    }
}
