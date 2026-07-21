//! Boundary ENFORCEMENT seam — the first runtime consumer of
//! [`holon_api::BoundaryBehavior`] (ADR 0028 C3 → H2/D1).
//!
//! [`check_boundary`] is the single decision function: given an op's
//! classification and the source/target containers it would touch, it returns
//! whether the op is allowed (and, if so, whether it generates a crossing that
//! must enter the H2 log) or must be **rejected loudly**. This is where the
//! `Unclassified` fail-closed contract (Inc 1) becomes runtime behavior.
//!
//! ## Descriptor-less ops (create_document / delete_document / drag_drop_block)
//!
//! Three structural ops have **no `OperationDescriptor`** and so cannot carry a
//! `#[boundary_behavior(...)]` attr on the `operations_trait` macro. Their
//! classification is the responsibility of the **document-lifecycle layer**,
//! which constructs the [`holon_api::BoundaryBehavior`] explicitly and calls
//! [`check_boundary`]:
//!
//! - **`create_document`** mints a fresh, owner-private container. It does not
//!   cross an existing boundary — classified [`BoundaryBehavior::PrivateOnly`]
//!   at creation (a new container starts private; membership is set by a later
//!   `PolicyEdit`). Routed to the document-lifecycle layer.
//! - **`delete_document`** removes a container and revokes all its membership.
//!   Classified [`BoundaryBehavior::PolicyEdit`] — it mutates the overlay and
//!   its cascade enters the H2 log. Routed to the document-lifecycle layer.
//! - **`drag_drop_block`** is THE crossing generator: a reparent that may span
//!   containers. Classified [`BoundaryBehavior::Crossing`] with
//!   `widens_audience` computed by the drag layer from the source vs. target
//!   container audiences (conservative upper bound). Routed to the drag layer,
//!   which resolves source/target containers and calls [`check_boundary`].
//!
//! Implementation of these three call sites lands with the document-lifecycle /
//! drag layers (Inc 3+); this module fixes the classification contract they
//! must satisfy.

use holon_api::BoundaryBehavior;

use crate::types::ContainerId;

/// The verdict of the boundary check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryDecision {
    /// Stays within one container (or a within-container mutation of a shared
    /// container). No crossing generated.
    AllowPrivate,
    /// Generates a crossing that MUST enter the H2 log. `requires_confirm`
    /// mirrors the D1 explicit-confirm gate for audience-widening crossings.
    AllowCrossing {
        widens_audience: bool,
        requires_confirm: bool,
    },
    /// Mutates the policy overlay; enters the H2 log.
    AllowPolicyEdit,
    /// Rejected loudly. `reason` carries the op name (fail-closed).
    RejectLoud { reason: String },
}

/// Decide whether `op_name` (classified `behavior`) may run over `source` →
/// `target`, given whether each container is shared.
///
/// `crosses` = the op moves between two distinct containers. `touches_shared` =
/// either endpoint is a shared container. The `Unclassified` fail-closed
/// contract: works in purely-private contexts, but the moment it would cross or
/// touch a shared container it is REJECTED loudly with the op name.
pub fn check_boundary(
    op_name: &str,
    behavior: &BoundaryBehavior,
    source: &ContainerId,
    source_is_shared: bool,
    target: &ContainerId,
    target_is_shared: bool,
) -> BoundaryDecision {
    let crosses = source != target;
    let touches_shared = source_is_shared || target_is_shared;

    match behavior {
        BoundaryBehavior::PrivateOnly => {
            if crosses {
                reject(
                    op_name,
                    "is classified PrivateOnly (never crosses a container \
                     boundary) but was invoked across two containers",
                    source,
                    target,
                )
            } else {
                BoundaryDecision::AllowPrivate
            }
        }
        BoundaryBehavior::Crossing { widens_audience } => {
            if crosses {
                BoundaryDecision::AllowCrossing {
                    widens_audience: *widens_audience,
                    // D1: audience-widening crossings require explicit confirm.
                    requires_confirm: *widens_audience,
                }
            } else {
                // A crossing-capable op that did not actually change container
                // is an in-place edit — no crossing, no log entry.
                BoundaryDecision::AllowPrivate
            }
        }
        BoundaryBehavior::ForbiddenAtPageBoundary => {
            if crosses {
                reject(
                    op_name,
                    "is Forbidden at a page boundary (ADR 0028 D1) but was \
                     invoked across two containers",
                    source,
                    target,
                )
            } else {
                BoundaryDecision::AllowPrivate
            }
        }
        BoundaryBehavior::PolicyEdit => BoundaryDecision::AllowPolicyEdit,
        BoundaryBehavior::IdentityOp => {
            // Identity ops (rename/alias/merge) can silently move content across
            // audiences via selector rebinding (review C3/H6). When they cross,
            // they are crossing-shaped and enter the log; the widen bound is
            // conservative (an identity rebind can only broaden).
            if crosses {
                BoundaryDecision::AllowCrossing {
                    widens_audience: true,
                    requires_confirm: true,
                }
            } else {
                BoundaryDecision::AllowPrivate
            }
        }
        BoundaryBehavior::Unclassified => {
            if crosses || touches_shared {
                reject(
                    op_name,
                    "is Unclassified (fail-closed): any boundary interaction is \
                     rejected until it carries an explicit #[boundary_behavior]",
                    source,
                    target,
                )
            } else {
                // Works in purely private contexts.
                BoundaryDecision::AllowPrivate
            }
        }
    }
}

fn reject(
    op_name: &str,
    why: &str,
    source: &ContainerId,
    target: &ContainerId,
) -> BoundaryDecision {
    BoundaryDecision::RejectLoud {
        reason: format!(
            "boundary check REJECTED op `{op_name}`: it {why} (source=`{}`, \
             target=`{}`)",
            source.0, target.0
        ),
    }
}
