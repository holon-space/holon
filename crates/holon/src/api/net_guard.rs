//! The third pre-provider gate: firing-time legality of the marking an
//! operation would produce (ADR 0032 §3).
//!
//! The dispatcher asks a yes/no question and never learns which policy answered
//! it. A refusal is an `Err` at the call site, so the operation never reaches
//! its provider.

use async_trait::async_trait;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

/// The param a caller sets to turn a refusal into a confirmed firing.
///
/// Named in every [`NetRefusal`] message so a UI can render the refusal as a
/// confirm dialog and re-dispatch the same operation with it set.
pub const CONFIRM_BREAK_PARAM: &str = "confirm_break";

/// Whether the caller already confirmed the consequence a refusal would name.
///
/// Parsed once, at the dispatcher boundary, from
/// [`CONFIRM_BREAK_PARAM`]; a policy reads this instead of the params bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Absent,
    BreakConfirmed,
}

impl Confirmation {
    /// # Errors
    /// A `confirm_break` that is not a Boolean — the param exists to be
    /// unambiguous, and a string that happens to read `"true"` is a caller
    /// whose intent was never parsed.
    pub fn parse(params: &StorageEntity) -> Result<Self> {
        match params.get(CONFIRM_BREAK_PARAM) {
            None | Some(holon_api::Value::Null) => Ok(Self::Absent),
            Some(holon_api::Value::Boolean(true)) => Ok(Self::BreakConfirmed),
            Some(holon_api::Value::Boolean(false)) => Ok(Self::Absent),
            Some(other) => {
                Err(format!("`{CONFIRM_BREAK_PARAM}` must be a Boolean, got {other:?}").into())
            }
        }
    }
}

/// One dispatched operation, as the net guard sees it.
pub struct NetGuardOp<'a> {
    pub entity_name: &'a str,
    pub op_name: &'a str,
    pub params: &'a StorageEntity,
    pub confirmation: Confirmation,
}

/// Why the resulting marking is illegal, in words a user can act on.
pub struct NetRefusal {
    pub reason: String,
}

pub enum NetVerdict {
    Confirm,
    Refuse(NetRefusal),
}

/// Answers whether the marking an operation would produce is legal.
///
/// # Unification with [`crate::api::guard_world::GuardWorld`]
/// Both seams answer enabledness, and they stay separate only while their
/// inputs differ: a declared guard is a subject-bound predicate over the
/// current world, this one reads the whole delta an operation would write.
/// They unify once the derived net projection exists AND lived experience
/// shows the declared-guard predicates are expressible as net arcs — by
/// generalizing `GuardWorld` to marking-aware whole-delta evaluation and
/// folding this trait into it. Until then a policy that could be written as a
/// `#[require]` predicate belongs there, not here.
#[async_trait]
pub trait NetGuard: Send + Sync {
    async fn check(&self, op: &NetGuardOp<'_>) -> Result<NetVerdict>;
}

/// Confirms every operation, for a composition site that hosts no placement
/// policy — a Loro-only session, which has neither capability profiles nor a
/// document home to resolve a destination against.
///
/// Installed explicitly so the gate is never merely absent: absence is what
/// [`crate::api::operation_dispatcher::OperationDispatcher::assert_net_guard_installed`]
/// crashes on.
pub struct InertNetGuard;

#[async_trait]
impl NetGuard for InertNetGuard {
    async fn check(&self, _: &NetGuardOp<'_>) -> Result<NetVerdict> {
        Ok(NetVerdict::Confirm)
    }
}
