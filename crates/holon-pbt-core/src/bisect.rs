//! Component bisection — localize a failure to the smallest reproducing
//! `ComponentSet` (ADR 0009 §3).
//!
//! This is the *pure* lattice search: given a node-oracle `reproduces(&set)`
//! (in practice: "replay the captured sequence against a SUT wired for `set`
//! and see if it still fails"), walk the validity-pruned lattice and report the
//! smallest set that still reproduces. The oracle is a closure so the search is
//! testable without any SUT — the SUT-backed oracle (replay through
//! `stepper::run_sequence` with `ReplayMode::SkipGated`) is a thin adapter the
//! consumer crate supplies.

use crate::component_set::ComponentSet;

/// Where a failure localizes across the lattice (ADR 0009 §3). The direction
/// that held is reported honestly rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Localization {
    /// The starting `ceiling` set does not reproduce — nothing to localize.
    NotReproduced,
    /// Smallest reproducing set reachable by *removing* components: it
    /// reproduces, and no valid child does. The components it cannot shed
    /// localize a present-when-component-present bug.
    DownwardMinimal(ComponentSet),
    /// Smallest reproducing set reachable by *adding* components back toward
    /// the ceiling: it reproduces, and no valid parent (within the ceiling)
    /// does. A present-when-component-*absent* bug (a handler missing for
    /// the disabled case), which inverts the lattice direction.
    UpwardMinimal(ComponentSet),
}

/// Greedy downward delta-debug from `ceiling`: repeatedly drop one component
/// (via [`ComponentSet::valid_children`]) while the result still `reproduces`,
/// until no valid child reproduces. Returns the locally-minimal reproducing
/// set.
///
/// Assumes downward closure (if a superset reproduces, some child does too) —
/// true for present-when-present bugs. For the absent-component case use
/// [`bisect_upward`]; [`bisect`] tries downward first and falls through to
/// upward, labelling which held.
pub fn bisect_downward(
    ceiling: ComponentSet,
    mut reproduces: impl FnMut(&ComponentSet) -> bool,
) -> Localization {
    if !reproduces(&ceiling) {
        return Localization::NotReproduced;
    }
    let mut current = ceiling;
    loop {
        let next = current
            .valid_children()
            .into_iter()
            .find(|child| reproduces(child));
        match next {
            Some(child) => current = child,
            None => return Localization::DownwardMinimal(current),
        }
    }
}

/// Greedy upward delta-debug from `floor` toward `ceiling`: repeatedly add one
/// component back (via [`ComponentSet::valid_parents_within`]) while the result
/// still `reproduces`. Returns the locally-maximal-toward-ceiling reproducing
/// set whose every valid parent stops reproducing — the signature of an
/// absent-component bug. `floor` must itself reproduce (else `NotReproduced`).
pub fn bisect_upward(
    floor: ComponentSet,
    ceiling: &ComponentSet,
    mut reproduces: impl FnMut(&ComponentSet) -> bool,
) -> Localization {
    if !reproduces(&floor) {
        return Localization::NotReproduced;
    }
    let mut current = floor;
    loop {
        let next = current
            .valid_parents_within(ceiling)
            .into_iter()
            .find(|parent| reproduces(parent));
        match next {
            Some(parent) => current = parent,
            None => return Localization::UpwardMinimal(current),
        }
    }
}

/// Localize honestly: if the `ceiling` reproduces, minimize downward. Otherwise
/// the bug is non-monotone in the usual direction — try minimizing upward from
/// the empty-projection floor of the ceiling's wiring (an absent-component
/// bug). Returns [`Localization::NotReproduced`] if neither end reproduces.
pub fn bisect(
    ceiling: ComponentSet,
    floor: ComponentSet,
    mut reproduces: impl FnMut(&ComponentSet) -> bool,
) -> Localization {
    if reproduces(&ceiling) {
        return bisect_downward(ceiling, reproduces);
    }
    bisect_upward(floor, &ceiling, reproduces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_set::Projection;
    use crate::wiring::StorageAdapter;

    /// A present-when-present bug (needs both Loro storage and the ViewModel
    /// projection) localizes downward to exactly `{Loro} + {ViewModel}`.
    #[test]
    fn downward_localizes_a_combination_bug() {
        let oracle = |s: &ComponentSet| {
            s.has_storage(StorageAdapter::Loro) && s.has_projection(Projection::ViewModel)
        };
        match bisect_downward(ComponentSet::full_gpui(), oracle) {
            Localization::DownwardMinimal(set) => {
                assert!(set.has_storage(StorageAdapter::Loro));
                assert!(set.has_projection(Projection::ViewModel));
                // Truly minimal: dropping either factor stops reproduction, so
                // no valid child reproduces.
                assert!(!set.valid_children().iter().any(oracle));
                // Everything irrelevant has been shed.
                assert_eq!(set.wiring.storage_adapters.len(), 1);
                assert!(set.wiring.actors.is_empty());
                assert!(!set.has_projection(Projection::EditorState));
            }
            other => panic!("expected DownwardMinimal, got {other:?}"),
        }
    }

    /// A ceiling that never reproduces yields `NotReproduced` downward.
    #[test]
    fn downward_reports_not_reproduced() {
        let oracle = |s: &ComponentSet| s.has_storage(StorageAdapter::Turso);
        assert_eq!(
            bisect_downward(ComponentSet::loro_vm_fast(), oracle),
            Localization::NotReproduced
        );
    }

    /// An absent-component bug — reproduces only while Turso is NOT wired (a
    /// missing handler for the no-Turso case) — is found by the upward walk,
    /// not the downward one, and `bisect` routes to it because the ceiling
    /// (which has Turso) does not reproduce.
    #[test]
    fn upward_localizes_an_absent_component_bug() {
        let ceiling = ComponentSet::full_headless(); // has Turso
        let floor = ComponentSet::loro_vm_fast(); // no Turso
        // Bug present iff Loro present AND Turso absent.
        let oracle = |s: &ComponentSet| {
            s.has_storage(StorageAdapter::Loro) && !s.has_storage(StorageAdapter::Turso)
        };
        assert!(
            !oracle(&ceiling),
            "ceiling must not reproduce for this case"
        );
        match bisect(ceiling, floor, oracle) {
            Localization::UpwardMinimal(set) => {
                assert!(set.has_storage(StorageAdapter::Loro));
                assert!(!set.has_storage(StorageAdapter::Turso));
                // Adding any valid parent (toward ceiling) stops reproduction —
                // in particular adding Turso would.
                assert!(
                    set.valid_parents_within(&ComponentSet::full_headless())
                        .iter()
                        .all(|p| !oracle(p))
                );
            }
            other => panic!("expected UpwardMinimal, got {other:?}"),
        }
    }
}
