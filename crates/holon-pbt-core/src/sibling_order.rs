//! Shared sibling-order comparison for the composed invariants.
//!
//! Lifted here (co-location Phase 1) from the central
//! `holon-integration-tests` `block_compare` so that a co-located subsystem
//! invariant (`holon-loro-testing`'s `inv-loro-children-match-ref`, and the
//! central SQL `inv-live-children-match-ref`) can share ONE order comparator
//! without either crate depending on the other. Generic and self-contained — no
//! block/ref types, so it belongs in the shared cap crate.

/// `Ok(())` when `ref_children == sut_children`. Order is compared EXACTLY:
/// the render-artifact exemption that once relaxed intra-`Source|Image`-group
/// reordering was removed once the reference model was taught to reproduce the
/// store's true post-round-trip sibling order (`parse_order_rank`:
/// `Source < Image < Text`; see `assign_reference_sequences_canonical` and the
/// renderer+parser round-trip regression `zz_spike_sibling_order_world_b`).
/// Membership/cardinality divergence still surfaces here as an order mismatch;
/// set-level checks live in the id-set invariants. Otherwise an `Err` naming
/// the divergence under `parent`.
pub fn compare_sibling_order<T: Ord + std::fmt::Debug>(
    label: &str,
    parent: &dyn std::fmt::Display,
    ref_children: &[T],
    sut_children: &[T],
) -> Result<(), String> {
    if ref_children == sut_children {
        return Ok(());
    }
    Err(format!(
        "[{label}] sibling order diverges under parent {parent}.\n  \
         ref order: {ref_children:?}\n  \
         sut order: {sut_children:?}"
    ))
}
