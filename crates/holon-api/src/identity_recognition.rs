//! Resolve-before-mint recognition — a first-class operation of the minting
//! surface (ADR 0029: "recognition/resolve-before-mint is first-class").
//!
//! Before any boundary mints or creates a name-addressable entity at a
//! deterministically-derived id (e.g. a page's [`PageId::for_path`]), it must
//! RECOGNIZE whether that id is already held — and, if so, by the SAME
//! name-addressable entity (an idempotent re-observation) or by a DIFFERENT one
//! (a rename left the id in place while the title changed). Minting blind at a
//! held id is exactly the journal re-mint clobber: the derived id of a page
//! renamed away from its date is re-created with the date title and overwrites
//! the rename.
//!
//! This module owns that classification as a PURE function over holon-api
//! primitives. The CALLER performs the (mode-correct) read of the id's current
//! holder — from whatever block-read authority it already has, the projected
//! `block_raw` base table in both Turso and Loro authority modes — and passes
//! the holder's title in. So recognition is decided identically regardless of
//! which consolidator executes the eventual write: the mode selects the
//! executor, never the recognition verdict.

use crate::entity_uri::EntityUri;
use crate::link_parser::normalize_for_hash;
use crate::storage_error::IdentityCollision;

/// Outcome of recognizing a derived-id create against the id's current holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognition {
    /// No entity currently holds the derived id — the caller may mint/create.
    Free,
    /// The derived id is already held by an entity whose normalized title
    /// MATCHES the requested one: the same name-addressable entity. Re-creating
    /// is an idempotent upsert of unchanged content — already satisfied; the
    /// caller treats it as a benign no-op (this is the convergent re-fire /
    /// re-ingest case, not a fault).
    AlreadySatisfied,
    /// The derived id is held by an entity whose normalized title DIFFERS from
    /// the requested one — the state a rename leaves behind (content changed,
    /// id preserved). Minting here would clobber a DIFFERENT entity; the
    /// caller must refuse (interim fail-loud policy, ADR 0029 D1b). Carries
    /// the typed [`IdentityCollision`] so the caller can surface or skip on
    /// it (its `Display` embeds the stable collision marker).
    Collision(IdentityCollision),
}

/// Recognize a derived-id create against the id's CURRENT holder.
///
/// `holder_title` is the derived id's current holder `content`/title as read
/// from the active block-read authority (`None` = the id is unheld). The
/// comparison is under [`normalize_for_hash`] — the SAME normalization
/// [`PageId::for_path`](crate::link_parser::PageId::for_path) hashes — so
/// recognition keys on TITLE/PATH, never on `(content, parent)`: content drifts
/// over time, whereas the normalized title is precisely what the derived id is
/// a function of. A rename changes the title while preserving the id, so a
/// title mismatch at a held derived id is exactly — and only — the collision
/// case.
pub fn recognize_derived_id(
    derived_id: &EntityUri,
    holder_title: Option<&str>,
    requested_title: &str,
) -> Recognition {
    match holder_title {
        None => Recognition::Free,
        Some(held) if normalize_for_hash(held) == normalize_for_hash(requested_title) => {
            Recognition::AlreadySatisfied
        }
        Some(held) => Recognition::Collision(IdentityCollision {
            id: derived_id.clone(),
            held_title: held.to_string(),
            requested_title: requested_title.to_string(),
        }),
    }
}

/// Sanitize a raw block-content string into the canonical page TITLE it maps to
/// (parse-don't-validate). Trim, then strip any TRAILING `/` (the slash-menu
/// trigger still trailing at plan time, or a stray separator) — `trim_end`
/// after each strip. Interior `/` is namespace-meaningful and preserved. `None`
/// when nothing survives (empty content — the caller decides whether that is an
/// error).
///
/// A page's TITLE, its deterministic id
/// ([`PageId::for_path`](crate::link_parser::PageId::for_path)),
/// and its on-disk filename must all agree on THIS value — and so must the
/// [`recognize_derived_id`] step that compares a create's title against the
/// id's current holder. `normalize_for_hash` keeps `/`, so a raw trailing-slash
/// title recognized on one side and a sanitized one on the other would DIVERGE.
/// This is the single source the convert planner, the reference model, and the
/// recognition step all funnel through, so no such split can open.
pub fn sanitize_page_title(content: &str) -> Option<String> {
    let mut leaf = content.trim();
    while leaf.ends_with('/') {
        leaf = leaf[..leaf.len() - 1].trim_end();
    }
    if leaf.is_empty() {
        None
    } else {
        Some(leaf.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> EntityUri {
        EntityUri::block("61133fe7")
    }

    #[test]
    fn unheld_id_is_free() {
        assert_eq!(
            recognize_derived_id(&id(), None, "2026-01-15"),
            Recognition::Free
        );
    }

    #[test]
    fn same_normalized_title_is_already_satisfied() {
        // Case/space differences that `normalize_for_hash` folds must NOT read
        // as a collision — that is the convergent re-fire, not a clobber.
        assert_eq!(
            recognize_derived_id(&id(), Some("2026-01-15"), "2026-01-15"),
            Recognition::AlreadySatisfied
        );
    }

    #[test]
    fn renamed_holder_is_a_collision_carrying_the_marker() {
        let r = recognize_derived_id(&id(), Some("Renamed"), "2026-01-15");
        match r {
            Recognition::Collision(c) => {
                assert_eq!(c.id, id());
                assert_eq!(c.held_title, "Renamed");
                assert_eq!(c.requested_title, "2026-01-15");
                assert!(
                    c.to_string().contains(crate::IDENTITY_COLLISION_MARKER),
                    "collision must carry the stable marker so the seam survives re-wrapping"
                );
            }
            other => panic!("expected Collision, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_then_recognize_is_stable_for_a_trailing_slash_title() {
        // A holder stored under the SANITIZED title, re-recognized from the same
        // sanitized title, reads AlreadySatisfied — the idempotent convergent
        // re-fire, not a false Collision.
        let raw = "My Page/";
        let sanitized = sanitize_page_title(raw).expect("non-empty after sanitize");
        assert_eq!(sanitized, "My Page");
        assert_eq!(
            recognize_derived_id(&id(), Some(&sanitized), &sanitized),
            Recognition::AlreadySatisfied
        );
        // Recognizing the SAME holder with the RAW trailing-slash title would
        // DIVERGE (normalize_for_hash keeps '/'): it reads Collision, not
        // AlreadySatisfied. That is exactly the SUT/oracle split the single-source
        // sanitize closes — both sides MUST sanitize before recognizing.
        assert!(
            matches!(
                recognize_derived_id(&id(), Some(&sanitized), raw),
                Recognition::Collision(_)
            ),
            "raw trailing-slash title must NOT match the sanitized holder — proving \
             why planner and reference must both sanitize before recognition"
        );
    }
}
