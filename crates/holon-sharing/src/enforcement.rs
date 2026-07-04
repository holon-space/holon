//! The policy overlay as the engine sees it: the
//! [`holon_core::BoundaryEnforcer`] the operation dispatcher consults before
//! every op (ADR 0028 C3).
//!
//! This module is the ONE place that turns the committed [`PolicySet`] into the
//! `(source container, target container, shared?)` tuple
//! [`crate::boundary::check_boundary`] decides on. `PolicySet::commit` already
//! fixes `ContainerId == selector`, so container resolution is exactly
//! "which selector's subtree contains this block" — and A7 disjointness makes
//! that selector unique.
//!
//! ## Overlay semantics (ADR 0028 §2/§3, C1)
//!
//! Sharing is an **overlay**: structure is content, membership is policy. A
//! vault with no committed policy therefore has exactly ONE container, so no
//! op can cross a boundary and every op is allowed. That is the degenerate case
//! of the model, not a permissive default: it is decided by a single
//! slice-length test, so the single-user hot path (the SLO case) never touches
//! the policy machinery.

use holon_api::BoundaryBehavior;
use holon_core::BoundaryEnforcer;
use holon_core::BoundaryRejection;

use crate::boundary::BoundaryDecision;
use crate::boundary::check_boundary;
use crate::policy::PolicySet;
use crate::policy::SubtreeContainment;
use crate::types::BlockId;
use crate::types::ContainerId;

/// The owner's root container: everything no share policy selects. Mirrors
/// `holon_loro::container_registry::ROOT_CONTAINER_ID` (duplicated rather than
/// depended on so this stays a pure, synchronous decision — the registry's own
/// value is behind an `async` snapshot).
pub const ROOT_CONTAINER: &str = "holon_tree";

/// Classifications that can RELOCATE a block between containers. For these the
/// destination is load-bearing: judging one without knowing it would be
/// fail-open.
fn can_relocate(behavior: &BoundaryBehavior) -> bool {
    matches!(
        behavior,
        BoundaryBehavior::Crossing { .. }
            | BoundaryBehavior::ForbiddenAtPageBoundary
            | BoundaryBehavior::IdentityOp
    )
}

/// The committed policy overlay, wired into the engine's operation dispatch.
pub struct PolicyOverlayEnforcer {
    policies: PolicySet,
    /// The block→subtree relation container resolution needs. `None` is only
    /// legal alongside an EMPTY policy set (see
    /// [`PolicyOverlayEnforcer::inert`]); a populated overlay without a
    /// relation is a loud rejection, never a guess.
    containment: Option<Box<dyn SubtreeContainment + Send + Sync>>,
}

impl PolicyOverlayEnforcer {
    /// The overlay of a vault that shares nothing: no committed policies, so
    /// every block is in the root container and every op is allowed in O(1).
    ///
    /// This is prod's wiring today — nothing mints or persists share policies
    /// yet. It installs the seam so the check-path is LIVE (and the moment a
    /// policy is committed it enforces), without pretending a containment
    /// relation exists that no caller could have built.
    pub fn inert() -> Self {
        Self {
            policies: PolicySet::new(),
            containment: None,
        }
    }

    /// The populated overlay: committed policies plus the relation mapping a
    /// block to the selector subtree governing it —
    /// [`crate::registry_binding::RegistryContainment`] over the live container
    /// tree in prod, [`crate::policy::MapContainment`] in tests.
    pub fn new(
        policies: PolicySet,
        containment: Box<dyn SubtreeContainment + Send + Sync>,
    ) -> Self {
        Self {
            policies,
            containment: Some(containment),
        }
    }

    /// The container governing `block`: the selector whose subtree contains it
    /// (unique by A7), else the root container.
    fn container_of(&self, block: &BlockId, rel: &dyn SubtreeContainment) -> ContainerId {
        self.policies
            .policies()
            .iter()
            .map(|signed| &signed.policy.selector)
            .find(|selector| rel.contains(selector, block))
            .map(|selector| ContainerId(selector.0.clone()))
            .unwrap_or_else(|| ContainerId(ROOT_CONTAINER.to_string()))
    }
}

impl BoundaryEnforcer for PolicyOverlayEnforcer {
    fn check(
        &self,
        op_name: &str,
        behavior: &BoundaryBehavior,
        subject: &str,
        target_parent: Option<&str>,
    ) -> Result<(), BoundaryRejection> {
        // Overlay semantics + C1 fast path: one container ⇒ no boundary.
        if self.policies.policies().is_empty() {
            return Ok(());
        }

        let rel = self.containment.as_deref().ok_or_else(|| {
            BoundaryRejection(format!(
                "boundary check REJECTED op `{op_name}`: the policy overlay holds {} committed \
                 policies but no subtree-containment relation is bound, so the container of \
                 `{subject}` cannot be resolved",
                self.policies.policies().len()
            ))
        })?;

        let source = self.container_of(&BlockId(subject.to_string()), rel);
        let source_is_shared = source.0 != ROOT_CONTAINER;

        let target = match target_parent {
            Some(parent) => self.container_of(&BlockId(parent.to_string()), rel),
            None if source_is_shared && can_relocate(behavior) => {
                // The destination of these ops is derived from the tree by the
                // drag / document-lifecycle layer (see `boundary`'s module
                // docs), which owes this seam both endpoints. Judging the op
                // with target := source would silently pass an audience-widening
                // move, so refuse instead.
                return Err(BoundaryRejection(format!(
                    "boundary check REJECTED op `{op_name}`: it is classified {behavior:?} (it \
                     can relocate a block) and its subject `{subject}` lives in shared container \
                     `{}`, but the intent names no destination container — the container-resolving \
                     layer must call the boundary seam with both endpoints (ADR 0028 C3, \
                     fail-closed)",
                    source.0
                )));
            }
            // No destination named and none needed: the op stays where it is.
            None => source.clone(),
        };
        let target_is_shared = target.0 != ROOT_CONTAINER;

        match check_boundary(
            op_name,
            behavior,
            &source,
            source_is_shared,
            &target,
            target_is_shared,
        ) {
            BoundaryDecision::AllowPrivate | BoundaryDecision::AllowPolicyEdit => Ok(()),
            BoundaryDecision::AllowCrossing {
                requires_confirm: false,
                ..
            } => Ok(()),
            BoundaryDecision::AllowCrossing {
                widens_audience,
                requires_confirm: true,
            } => Err(BoundaryRejection(format!(
                "boundary check REJECTED op `{op_name}`: it would move `{subject}` from container \
                 `{}` to `{}` (widens_audience={widens_audience}). ADR 0028 D1 requires explicit \
                 confirmation before a crossing adds an audience, and no confirmation ceremony is \
                 wired at the operation-dispatch seam — refusing the crossing rather than \
                 performing it unconfirmed",
                source.0, target.0
            ))),
            BoundaryDecision::RejectLoud { reason } => Err(BoundaryRejection(reason)),
        }
    }
}
