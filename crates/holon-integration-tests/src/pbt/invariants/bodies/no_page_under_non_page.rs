//! `inv-no-page-under-non-page` — the REF-BOUNDARY tripwire for the
//! no-pages-under-non-pages ruling
//! (`docs/Proposals/PageHierarchy-2026-07-13.md`, interim ruling 2026-07-13;
//! enforced Fork B B1). A page's structural ancestors, walking `parent_id` to a
//! root, must ALL be pages — else the ancestor is the root itself. Pages under
//! non-pages are prohibited.
//!
//! @pbt oracle internal-consistency — ref-side structural tripwire on the
//!   generator/seed guarantee (walks the ref parent chain; reads no SUT)
//! @pbt covers page-hierarchy — a page nested under a non-page block
//! @pbt slips-if-removed a generator/seed change reparents a page under a
//!   non-page; DocumentManager::name_chain bails deep in writeback instead of
//!   a clean keystone RED at the ref boundary
//!
//! ## Why a ref-only oracle (no SUT cap)
//!
//! This is the *generator guarantee's* tripwire, not a SUT⇄ref differential.
//! The primary guarantee is that the composed generator never PRODUCES a page
//! under a non-page (verified 2026-07-13: pages are created only at seed time,
//! always at `EntityUri::no_parent()`, and no generated transition reparents a
//! page — see `docs/Plans/ForkB-B1-2026-07-13.md` §3.1 "Generator/oracle
//! obligation" R8). This oracle is the second half of "do both": if a future
//! generator/seed change reintroduces the prohibited topology, the keystone
//! catches it HERE, at the ref boundary, as a clean RED — instead of
//! `DocumentManager::name_chain` (`sync_ports.rs`) `bail!`-ing deep inside
//! writeback with a harder-to-localize failure. It reads ONLY `RefBlockTree`,
//! so it selects on every composed draw (the ref always provides that cap) and
//! adds no SUT dependency.
//!
//! ## Scope: non-seed pages
//!
//! The page set is `all_non_seed_block_ids` filtered by `is_page_block` — the
//! same authority `inv-every-page-has-its-own-file` and the companion twin use.
//! Seed pages (`block_documents` `no_parent`/sentinel — root shells) are
//! excluded: they are root-rooted by construction and own no ancestor chain to
//! check. A non-seed page (e.g. a journal date page nested under the `journals`
//! folder-page) is exactly what the ruling governs, and every ancestor up to
//! the root must be a page. Vacuously Ok on a ref modeling no non-seed page
//! (the default keystone).
//!
//! `Needs RefBlockTree` only.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvNoPageUnderNonPage;

impl InvNoPageUnderNonPage {
    pub const ID: InvariantId = InvariantId("inv-no-page-under-non-page");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoPageUnderNonPage
where
    R: RefBlockTree,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, _: &S) -> InvariantResult {
        let mut violations: Vec<String> = Vec::new();

        for page in ref_.all_non_seed_block_ids() {
            if !ref_.is_page_block(&page) {
                continue;
            }
            // Walk `parent_of` to a root. `parent_of` returns `None` at a root
            // (no_parent / sentinel — see the `ReferenceState` cap impl), so the
            // loop terminates at a valid root without a special-case check.
            // Every ancestor between this page and its root must itself be a page.
            let mut seen: BTreeSet<EntityUri> = BTreeSet::new();
            let mut cursor = ref_.parent_of(&page);
            while let Some(ancestor) = cursor {
                if !seen.insert(ancestor.clone()) {
                    // A parent cycle in the ref — `inv-no-parent-cycles` owns the
                    // SUT side; here we fail loud rather than spin forever.
                    violations.push(format!(
                        "page `{}` has a CYCLIC ancestor chain (revisited `{}`)",
                        page.id(),
                        ancestor.id(),
                    ));
                    break;
                }
                if !ref_.is_page_block(&ancestor) {
                    violations.push(format!(
                        "page `{}` has NON-PAGE ancestor `{}` (a page's ancestors up to a root \
                         must all be pages — no-pages-under-non-pages ruling)",
                        page.id(),
                        ancestor.id(),
                    ));
                    break;
                }
                cursor = ref_.parent_of(&ancestor);
            }
        }

        if violations.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-no-page-under-non-page] {} page(s) violate the page-ancestor invariant: {:?}",
            violations.len(),
            violations.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use holon_pbt_core::capabilities::CapRegion;

    use super::*;

    /// Minimal `RefBlockTree` modeling a fixed (page-set, parent-map) topology.
    /// Only the three methods this invariant calls (`all_non_seed_block_ids`,
    /// `is_page_block`, `parent_of`) are meaningful; the rest are inert.
    struct RefStub {
        /// non-seed ids (what `all_non_seed_block_ids` returns)
        non_seed: BTreeSet<EntityUri>,
        /// which ids are pages
        pages: BTreeSet<EntityUri>,
        /// parent map; absence ⇒ root (`parent_of` → None)
        parents: BTreeMap<EntityUri, EntityUri>,
    }
    impl RefStub {
        fn new() -> Self {
            Self {
                non_seed: BTreeSet::new(),
                pages: BTreeSet::new(),
                parents: BTreeMap::new(),
            }
        }
        fn page(mut self, id: &str, parent: Option<&str>) -> Self {
            let uri = EntityUri::block(id);
            self.non_seed.insert(uri.clone());
            self.pages.insert(uri.clone());
            if let Some(p) = parent {
                self.parents.insert(uri, EntityUri::block(p));
            }
            self
        }
        /// A page that is NOT enumerated as non-seed (a root shell) but IS a
        /// page — so it counts as a valid page-ancestor for a nested
        /// page.
        fn page_ancestor(mut self, id: &str, parent: Option<&str>) -> Self {
            let uri = EntityUri::block(id);
            self.pages.insert(uri.clone());
            if let Some(p) = parent {
                self.parents.insert(uri, EntityUri::block(p));
            }
            self
        }
        fn non_page(mut self, id: &str, parent: Option<&str>) -> Self {
            let uri = EntityUri::block(id);
            if let Some(p) = parent {
                self.parents.insert(uri, EntityUri::block(p));
            }
            self
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
        fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
            self.parents.get(id).cloned()
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
            self.non_seed.clone()
        }
    }

    async fn check(stub: &RefStub) -> InvariantResult {
        // The `S` type param is unused by the body; any type works.
        Invariant::<RefStub, ()>::check(&InvNoPageUnderNonPage, stub, &()).await
    }

    #[tokio::test]
    async fn page_under_page_up_to_root_is_ok() {
        // date(page) -> journals(page) -> root. The row-137 subdir topology.
        let stub = RefStub::new()
            .page("journal-2026-07-10", Some("journals"))
            .page_ancestor("journals", None);
        assert!(
            matches!(check(&stub).await, InvariantResult::Ok),
            "{:?}",
            check(&stub).await
        );
    }

    #[tokio::test]
    async fn page_directly_at_root_is_ok() {
        let stub = RefStub::new().page("top", None);
        assert!(matches!(check(&stub).await, InvariantResult::Ok));
    }

    #[tokio::test]
    async fn page_under_non_page_fails_naming_both() {
        // page P -> b1(non-page) -> A(page) -> root. b1 is the offender.
        let stub = RefStub::new()
            .page("P", Some("b1"))
            .non_page("b1", Some("A"))
            .page_ancestor("A", None);
        match check(&stub).await {
            InvariantResult::Fail(m) => {
                assert!(
                    m.contains("`P`") && m.contains("`b1`"),
                    "must name page + ancestor: {m}"
                );
            }
            other => panic!("a page under a non-page must FAIL, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_seed_page_absent_from_non_seed_is_not_checked() {
        // Only `page_ancestor` (not enumerated) — nothing to check.
        let stub = RefStub::new().page_ancestor("journals", None);
        assert!(matches!(check(&stub).await, InvariantResult::Ok));
    }

    #[tokio::test]
    async fn no_pages_is_vacuously_ok() {
        let stub = RefStub::new().non_page("x", None);
        assert!(matches!(check(&stub).await, InvariantResult::Ok));
    }
}
