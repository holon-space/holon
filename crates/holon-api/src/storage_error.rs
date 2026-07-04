//! Shared storage/projection domain errors.
//!
//! Two deliberately *distinct* types so a consumer can tell apart:
//! - [`ParentNotFound`]: invalid input rejected at the *write boundary* — a
//!   block references a parent that does not exist / cannot be resolved. Both
//!   the Turso (SQL) and Loro backends unify onto this one error instead of
//!   their prior ad-hoc messages.
//! - [`ProjectionInvariantViolated`]: a *projection* (org render, matview)
//!   observed a should-never-happen state. Not caused by bad input — it signals
//!   a broken internal invariant.

use crate::entity_uri::EntityUri;

/// A block references a parent that does not exist or cannot be resolved at the
/// storage write boundary. Shared by the Turso and Loro backends.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("parent block not found: {parent_id} (creating child {child_id})")]
pub struct ParentNotFound {
    /// The offending, unresolvable parent identifier.
    pub parent_id: EntityUri,
    /// The child block whose creation triggered the lookup.
    pub child_id: EntityUri,
}

/// A projection (org render, matview build, …) saw an impossible state. Used by
/// cheap should-never-happen assertions in projection code — distinct from
/// [`ParentNotFound`] so "impossible internal state" is never confused with
/// "bad input rejected at the boundary".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("projection invariant violated: {detail}")]
pub struct ProjectionInvariantViolated {
    pub detail: String,
}
