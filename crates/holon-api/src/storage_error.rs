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

/// Stable marker embedded in every [`IdentityCollision`] message.
///
/// The concrete error type is erased by the string-enriching wrappers along the
/// dispatch chain (each layer keeps the *Display text* but re-boxes as a fresh
/// `format!`). Autonomous callers that must react to a refused deterministic-id
/// create — the journal auto-create rule (skip-and-log-once instead of
/// error-storming) and the page-identity PBT driver (model the refusal instead
/// of panicking) — recognise it by this marker in the rendered message. It is a
/// disclosed seam, checked, never parsed.
pub const IDENTITY_COLLISION_MARKER: &str = "holon-identity-collision";

/// A deterministically-derived entity id (e.g. a `PageId::for_path` page id) was
/// requested for a `create`, but that id is ALREADY held by a DIFFERENT entity
/// (its current canonical title differs from the requested one — the state a
/// page rename leaves behind: content changed, id preserved).
///
/// The interim identity policy (identity plan §5) refuses such a create
/// FAIL-LOUD rather than letting an `INSERT … ON CONFLICT(id) DO UPDATE`
/// silently clobber the existing holder. The end-state will instead mint a
/// distinct id and bind the NAME to it; until then the create is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "holon-identity-collision: id {id} is already held by a different entity (held title \
     {held_title:?}, requested {requested_title:?}); a deterministically-derived id must not \
     clobber its current holder (interim fail-loud identity policy §5)"
)]
pub struct IdentityCollision {
    /// The derived id whose current holder differs from the requested entity.
    pub id: EntityUri,
    /// The title the id's CURRENT holder carries.
    pub held_title: String,
    /// The title the refused create asked to place at this id.
    pub requested_title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_collision_message_carries_the_marker() {
        let e = IdentityCollision {
            id: EntityUri::block("a"),
            held_title: "Renamed".into(),
            requested_title: "pagea".into(),
        };
        assert!(
            e.to_string().contains(IDENTITY_COLLISION_MARKER),
            "IdentityCollision Display must embed IDENTITY_COLLISION_MARKER so the seam survives \
             error re-wrapping; got: {e}"
        );
    }
}
