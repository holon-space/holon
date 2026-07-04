//! `inv-companion-has-no-child-page-headings` — a folder-companion `.org` file
//! must NOT retain, on disk after settle, a heading for a block that is itself
//! a `Page` doc-root owning its own file. The writeback must DE-INLINE such
//! child-page headings (the companion becomes empty-of-child-pages, like
//! `Frontends.org`); it retains literally nothing of them (OQ3 ruling,
//! 2026-07-12 — a backlink, if wanted, is a user-authored `[[…]]` mark, never a
//! writeback artifact).
//!
//! @pbt oracle correspondence
//! @pbt covers writeback de-inline — a folder-companion `.org` retaining a
//!   heading for a block the ref models as a `Page` doc-root (is_page_block)
//! @pbt slips-if-removed companion writeback keeps a child page inlined (or a
//!   leftover backlink line); the page is double-represented and a
//!   store-rebuild-from-disk re-ingests it under the wrong parent
//!
//! This is the WRITEBACK-side twin of `inv-sidebar-page-tag-preserved` (the
//! ingest-side page-tag-preservation oracle, Fork A). Fork A guarantees the
//! inlined page keeps its `Page` tag; Fork B guarantees the companion's
//! writeback then de-inlines it. The two meet at exactly `is_page()` — the sole
//! child-page predicate (OQ1 ruling): a heading is a foreign child page iff the
//! reference models its id as a `Page` doc-root
//! (`RefBlockTree::is_page_block`).
//!
//! ## Why it's distinct from `inv-org-render-fixed-point`
//!
//! The fixed-point oracle only asserts disk == render(SQL) after settle — it
//! would go GREEN on ANY stable companion, including a wrong one that
//! stabilized while still retaining a de-inlined child's backlink line. This
//! oracle locks the *shape*: the companion holds no child-page heading at all.
//! Both are RED today (the per-file writeback guard `ensure_ingest_lossless`
//! refuses the de-inline as apparent block loss — B0 red-first; B1's
//! SurvivingProjection union makes it green).
//!
//! ## Detection (dependency-free)
//!
//! `SutOrgRender::snapshot_org_render_pairs` hands `(path, disk, rendered)`.
//! For each file we read its own doc-root id (the `#+ID:` line) and every
//! heading's `:ID:` drawer value. A drawer id that (a) differs from the file's
//! own root and (b) the reference marks as a `Page` doc-root is an inlined
//! foreign child page → violation. Ids are bare on disk (ORG_SYNTAX) and
//! wrapped `block:` before the ref lookup, matching the parser boundary.
//! Parsing via line-scan (not the org parser) keeps this body free of the
//! optional `holon-orgmode` dep.
//!
//! `Needs SutOrgRender` (SUT disk) + `RefBlockTree` (page authority).

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvCompanionHasNoChildPageHeadings;

impl InvCompanionHasNoChildPageHeadings {
    pub const ID: InvariantId = InvariantId("inv-companion-has-no-child-page-headings");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvCompanionHasNoChildPageHeadings
where
    R: RefBlockTree,
    S: SutOrgRender,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let mut violations: Vec<String> = Vec::new();

        for (path, disk, _rendered) in sut.snapshot_org_render_pairs().await {
            let root_id = file_root_id(&disk);
            for inlined in heading_drawer_ids(&disk) {
                // The file's OWN doc-root (a page owning this very file) is not an
                // inlined child heading — skip it.
                if root_id.as_deref() == Some(inlined.as_str()) {
                    continue;
                }
                if ref_.is_page_block(&EntityUri::block(&inlined)) {
                    violations.push(format!("{path}: inlines child page `{inlined}`"));
                }
            }
        }

        if violations.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-companion-has-no-child-page-headings] {} folder-companion heading(s) for a page \
             that owns its own file survived on disk — writeback must DE-INLINE them (companion \
             retains nothing of child pages; OQ3). Likely the per-file writeback guard refused \
             the de-inline as apparent block loss (B0 red-first; B1's union guard fixes it): {:?}",
            violations.len(),
            violations.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}

/// The file's own doc-root id: the bare value of its `#+ID:` header line, if
/// any.
fn file_root_id(disk: &str) -> Option<String> {
    disk.lines()
        .find_map(|l| l.trim().strip_prefix("#+ID:").map(|s| s.trim().to_string()))
}

/// Every heading's `:ID:` property-drawer value (bare), in file order.
fn heading_drawer_ids(disk: &str) -> Vec<String> {
    disk.lines()
        .filter_map(|l| l.trim().strip_prefix(":ID:").map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPANION_INLINING_A_PAGE: &str =
        "#+ID: journals\n* 2026-07-10\n:PROPERTIES:\n:ID: journal-2026-07-10\n:END:\n";

    #[test]
    fn root_id_is_the_header() {
        assert_eq!(
            file_root_id(COMPANION_INLINING_A_PAGE).as_deref(),
            Some("journals")
        );
        assert_eq!(file_root_id("no header here\n"), None);
    }

    #[test]
    fn drawer_ids_are_the_headings() {
        assert_eq!(
            heading_drawer_ids(COMPANION_INLINING_A_PAGE),
            vec!["journal-2026-07-10".to_string()]
        );
        // A page-file that is only its own bare doc-root has no heading drawers.
        assert!(heading_drawer_ids("#+ID: journal-2026-07-10\n").is_empty());
    }
}
