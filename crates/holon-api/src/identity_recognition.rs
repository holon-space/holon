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
use crate::link_parser::PageId;
use crate::link_parser::PageTitleKey;
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
    /// The derived id is held by a row carrying NO title at all — a structural
    /// placeholder stood up for THIS id (a Loro placeholder root auto-created
    /// when a child's create reached its parent first), not a second entity.
    /// There is no name to clobber, so the caller COMPLETES it with the
    /// requested title; the create is an adoption, not a substitution.
    UnnamedPlaceholder,
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
/// from the active block-read authority (`None` = the id is unheld); only its
/// first line, the holder's title, is compared. The comparison is by
/// [`PageTitleKey`], which folds every spelling
/// [`PageId::for_path`](crate::link_parser::PageId::for_path) folds — so
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
    match holder_title.map(crate::block::title_of) {
        None => Recognition::Free,
        // Checked BEFORE the title comparison: a blank holder is not a rival
        // name, so it can never be the collision case. Same "an empty-content
        // Page is never legal" predicate the title-less doc-root heal uses.
        Some(held) if held.trim().is_empty() => Recognition::UnnamedPlaceholder,
        Some(held) if PageTitleKey::of(held) == PageTitleKey::of(requested_title) => {
            Recognition::AlreadySatisfied
        }
        Some(held) => Recognition::Collision(IdentityCollision {
            id: derived_id.clone(),
            held_title: held.to_string(),
            requested_title: requested_title.to_string(),
        }),
    }
}

/// Where a page titled `title` at page path `path` lives, decided by
/// [`page_slot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageSlot {
    /// Create the page at this id: nothing holds it, or an untitled
    /// placeholder that the create completes.
    Create(PageId),
    /// The page of this title under this parent already holds this id.
    Existing(PageId),
}

/// The row holding an id, as [`page_slot`] compares it: its stored title and
/// its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHolder {
    pub title: String,
    pub parent: EntityUri,
}

/// Bound on the [`PageId::for_path_beside`] chain: each step means one more
/// other page holds the previous id, i.e. one more rename or move away from
/// this path.
const MAX_PAGE_SLOT_STEPS: usize = 64;

/// The id for a page titled `title` under `parent` at `path` that the
/// caller's by-position lookup did not find: `PageId::for_path(path)`, or,
/// while another page holds that id (a renamed or moved page keeps its id),
/// the next id along the [`PageId::for_path_beside`] chain
/// (docs/Plans/PageIdentityDeterminism.md §5.3). A holder under `parent` is
/// this page when its [`PageTitleKey`] equals `title`'s, and an untitled one
/// is a placeholder the create completes; a holder under another parent is
/// always passed. `holder` reads the row holding an id (`None` = unheld) from
/// the caller's write authority. Every passed holder and every adopted page
/// is disclosed with a warning: either means the caller's lookup and the id
/// disagree.
pub async fn page_slot<F, Fut>(
    path: &str,
    parent: &EntityUri,
    title: &str,
    mut holder: F,
) -> anyhow::Result<PageSlot>
where
    F: FnMut(EntityUri) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<PageHolder>>>,
{
    let key = PageTitleKey::of(title);
    let untitled = PageTitleKey::of("");
    let mut id = PageId::for_path(path).map_err(anyhow::Error::msg)?;
    for _ in 0..MAX_PAGE_SLOT_STEPS {
        let Some(held) = holder(id.as_entity_uri().clone()).await? else {
            return Ok(PageSlot::Create(id));
        };
        if held.parent == *parent {
            let held_key = PageTitleKey::of(&held.title);
            if held_key == untitled {
                return Ok(PageSlot::Create(id));
            }
            if held_key == key {
                tracing::warn!(
                    path,
                    %parent,
                    id = %id.as_entity_uri(),
                    held_title = %held.title,
                    "the lookup missed page {title:?}, but the id its page path derives holds \
                     that page at that place; adopting it"
                );
                return Ok(PageSlot::Existing(id));
            }
        }
        let next = PageId::for_path_beside(path, id.as_entity_uri()).map_err(anyhow::Error::msg)?;
        tracing::warn!(
            path,
            %parent,
            held_id = %id.as_entity_uri(),
            held_title = %held.title,
            held_parent = %held.parent,
            new_id = %next.as_entity_uri(),
            "another page holds the id this page path derives; the new page {title:?} takes the \
             next id beside it"
        );
        id = next;
    }
    anyhow::bail!(
        "page_slot({path:?}): {MAX_PAGE_SLOT_STEPS} consecutive derived ids are held by other \
         pages; refusing to search further"
    )
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
mod tests;
