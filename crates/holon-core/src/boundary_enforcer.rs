//! The ADR 0028 boundary/authz seam the engine's operation dispatch consults
//! before any provider runs.
//!
//! Only the *port* lives here — the same shape as [`crate::SyncTokenStore`].
//! The decision logic and the policy overlay it reads live in `holon-sharing`
//! (`PolicyOverlayEnforcer`), so the engine crate never learns the sharing
//! domain and `holon-sharing` never learns the engine.

use holon_api::BoundaryBehavior;

use crate::traits::MaybeSendSync;

/// A loud boundary rejection (ADR 0028 D2 "reject loud"). Carrying it as an
/// error — never a silent drop — is the contract: the operation did NOT run,
/// and the message names the op, why it was refused, and the two containers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BoundaryRejection(pub String);

/// Decides whether one dispatched operation may run, given how it is
/// classified and which containers it would touch (ADR 0028 C3).
///
/// Consulted **once per dispatched operation, before execution**. `subject` is
/// the block the intent names (`id`); `target_parent` is the reparent
/// destination the intent carries (`parent_id`) and is absent for ops that name
/// no second endpoint.
///
/// `Ok(())` MUST be O(1) when the vault has no committed share policy: a
/// single-user vault is the overwhelming case and pays nothing for the seam.
pub trait BoundaryEnforcer: MaybeSendSync {
    fn check(
        &self,
        op_name: &str,
        behavior: &BoundaryBehavior,
        subject: &str,
        target_parent: Option<&str>,
    ) -> Result<(), BoundaryRejection>;
}
