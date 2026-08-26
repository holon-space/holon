//! `inv-every-page-has-its-own-file` — every block the reference models as a
//! `Page` doc-root must own EXACTLY ONE file on disk after settle: a `.org`
//! file whose `#+ID:` header equals the page's bare id. Neither zero (a
//! FILELESS page — content lives only in the store and vanishes on
//! store-rebuild-from-disk: the core Fork B bug, BugFunnel row 137) nor more
//! than one (a DOUBLE-HOMED page — the same id `#+ID:`-rooting two files, a
//! duplicate/split-identity hazard) is allowed.
//!
//! @pbt oracle correspondence
//! @pbt covers page materialization — every ref `Page` doc-root owns exactly
//!   one `#+ID:`-rooted file (not fileless, not double-homed)
//! @pbt slips-if-removed a nested page stays store-only (fileless, row 137) and
//!   vanishes on store-rebuild-from-disk, or double-homes across two files and
//!   splits its identity
//!
//! This is the materialization-side twin of
//! `inv-companion-has-no-child-page-headings`: the companion oracle asserts the
//! *inlining* file de-inlines a child page; THIS oracle asserts the child page
//! then lands in its OWN file. Together they pin the whole
//! de-inline↔materialize convergence (Fork B B1 / 07-12 doc §5).
//!
//! ## Page authority (OQ1: `is_page()` is the sole predicate)
//!
//! The page set is taken from the reference via `is_page_block` over
//! `all_non_seed_block_ids` — the same authority the companion twin uses.
//! `all_non_seed_block_ids` already excludes sentinel/`no_parent`-doc'd seed
//! blocks (root shells inserted via direct SQL that own no materialized file by
//! construction), so the oracle checks exactly the pages that SHOULD have a
//! materialized file: rule-created / user-created nested pages (e.g. the
//! journal date pages under `journals`). It is inert (vacuously green) on a ref
//! that models no non-seed page, so it adds no false RED to the default
//! keystone.
//!
//! ## Detection (dependency-free line-scan, like the companion twin)
//!
//! `SutOrgRender::snapshot_org_render_pairs` hands `(path, disk, rendered)`.
//! Each file's own root id is its `#+ID:` header (bare, per ORG_SYNTAX); we
//! index files by that id and, for each ref page, count the files rooted at it.
//! `Needs SutOrgRender` (SUT disk) + `RefBlockTree` (page authority).

use std::collections::HashMap;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvEveryPageHasItsOwnFile;

impl InvEveryPageHasItsOwnFile {
    pub const ID: InvariantId = InvariantId("inv-every-page-has-its-own-file");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEveryPageHasItsOwnFile
where
    R: RefBlockTree,
    S: SutOrgRender,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Index every on-disk file by its own doc-root `#+ID:` (bare).
        let mut files_by_root: HashMap<String, Vec<String>> = HashMap::new();
        for (path, disk, _rendered) in sut.snapshot_org_render_pairs().await {
            if let Some(root) = file_root_id(&disk) {
                files_by_root.entry(root).or_default().push(path);
            }
        }

        let mut violations: Vec<String> = Vec::new();
        for page in ref_.all_non_seed_block_ids() {
            if !ref_.is_page_block(&page) {
                continue;
            }
            // Ids are bare on disk; `EntityUri::block(bare)` <-> `page` scheme.
            let bare = page.id();
            match files_by_root.get(bare).map(|v| v.as_slice()) {
                None | Some([]) => violations.push(format!(
                    "page `{bare}` owns NO file (fileless — content lives only in the store; \
                     writeback must MATERIALIZE it into `…/{bare}.org`)"
                )),
                Some([_one]) => {}
                Some(many) => violations.push(format!(
                    "page `{bare}` is DOUBLE-HOMED across {} files {:?} (exactly one `#+ID: \
                     {bare}` file must root it)",
                    many.len(),
                    many,
                )),
            }
        }

        if violations.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-every-page-has-its-own-file] {} page(s) not homed to exactly one own file: {:?}",
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
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use holon_pbt_core::capabilities::CapRegion;

    use super::*;

    /// Minimal `RefBlockTree` modeling a fixed page set. Only the two methods
    /// this invariant calls (`all_non_seed_block_ids`, `is_page_block`) are
    /// meaningful; the rest are inert.
    struct RefStub {
        pages: BTreeSet<EntityUri>,
    }
    impl RefStub {
        fn with(pages: &[&str]) -> Self {
            Self {
                pages: pages.iter().map(|p| EntityUri::block(p)).collect(),
            }
        }
    }
    impl RefBlockTree for RefStub {
        fn block_content(&self, _: &EntityUri) -> Option<&str> {
            None
        }
        fn is_text_block(&self, _: &EntityUri) -> bool {
            false
        }
        fn main_editable_descendants(&self) -> Vec<EntityUri> {
            Vec::new()
        }
        fn focus_root_ids(&self, _: CapRegion) -> BTreeSet<EntityUri> {
            BTreeSet::new()
        }
        fn previous_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
            None
        }
        fn next_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
            None
        }
        fn parent_of(&self, _: &EntityUri) -> Option<EntityUri> {
            None
        }
        fn grandparent(&self, _: &EntityUri) -> Option<EntityUri> {
            None
        }
        fn sorted_children(&self, _: &EntityUri) -> Vec<EntityUri> {
            Vec::new()
        }
        fn is_descendant_of_any(&self, _: &EntityUri, _: &BTreeSet<EntityUri>) -> bool {
            false
        }
        fn main_panel_renders(&self, _: &EntityUri) -> bool {
            false
        }
        fn owns_query_source(&self, _: &EntityUri) -> bool {
            false
        }

        fn is_layout_block(&self, _: &EntityUri) -> bool {
            false
        }
        fn is_focusable(&self, _: &EntityUri) -> bool {
            false
        }
        fn is_no_content_update(&self, _: &EntityUri) -> bool {
            false
        }
        fn is_page_block(&self, id: &EntityUri) -> bool {
            self.pages.contains(id)
        }
        fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
            self.pages.clone()
        }
    }

    struct OrgRenderStub {
        pairs: Vec<(String, String, String)>,
    }
    impl OrgRenderStub {
        fn from(files: &[(&str, &str)]) -> Self {
            Self {
                pairs: files
                    .iter()
                    .map(|(p, d)| (p.to_string(), d.to_string(), String::new()))
                    .collect(),
            }
        }
    }
    #[async_trait::async_trait(?Send)]
    impl SutOrgRender for OrgRenderStub {
        async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)> {
            self.pairs.clone()
        }
    }

    async fn check(pages: &[&str], files: &[(&str, &str)]) -> InvariantResult {
        InvEveryPageHasItsOwnFile
            .check(&RefStub::with(pages), &OrgRenderStub::from(files))
            .await
    }

    #[tokio::test]
    async fn every_page_with_its_own_file_is_ok() {
        let r = check(
            &["journal-2026-07-10"],
            &[(
                "Journals/2026-07-10.org",
                "#+ID: journal-2026-07-10\nbody\n",
            )],
        )
        .await;
        assert!(matches!(r, InvariantResult::Ok), "got {r:?}");
    }

    #[tokio::test]
    async fn a_fileless_page_fails() {
        // The page exists in the ref but no file roots it (row 137 shape).
        let r = check(
            &["journal-2026-07-10"],
            &[(
                "Journals.org",
                "#+ID: journals\n* 2026-07-10\n:PROPERTIES:\n:ID: journal-2026-07-10\n:END:\n",
            )],
        )
        .await;
        match r {
            InvariantResult::Fail(m) => assert!(
                m.contains("journal-2026-07-10") && m.contains("NO file"),
                "message must name the fileless page: {m}"
            ),
            other => panic!("a fileless page must FAIL, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_double_homed_page_fails() {
        let r = check(
            &["dup"],
            &[("a/dup.org", "#+ID: dup\n"), ("b/dup.org", "#+ID: dup\n")],
        )
        .await;
        match r {
            InvariantResult::Fail(m) => assert!(m.contains("DOUBLE-HOMED"), "got {m}"),
            other => panic!("a double-homed page must FAIL, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_pages_is_vacuously_ok() {
        let r = check(&[], &[("index.org", "#+ID: index\n")]).await;
        assert!(matches!(r, InvariantResult::Ok), "got {r:?}");
    }
}
