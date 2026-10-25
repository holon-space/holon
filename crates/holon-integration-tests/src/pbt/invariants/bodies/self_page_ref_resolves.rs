//! `inv-self-page-ref-resolves`.
//!
//! @pbt oracle correspondence
//! @pbt covers self-page-reference — a query block that names its own page
//!   yields exactly that page: the nearest page ancestor of the block that
//!   holds the query, also after the page is renamed.
//! @pbt slips-if-removed a self-page reference that binds the outermost page,
//!   the document, or a stale page name instead of the nearest page ancestor.
//!
//! Vacuous unless a case authors such a query block.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvSelfPageRefResolves;

/// The reference spelling a query source uses for its own page.
const SELF_PAGE_REF: &str = "$context_page_id";

impl InvSelfPageRefResolves {
    pub const ID: InvariantId = InvariantId("inv-self-page-ref-resolves");
}

fn names_own_page<R: RefBlockTree>(ref_: &R, holder: &EntityUri) -> bool {
    ref_.owns_query_source(holder)
        && ref_.sorted_children(holder).iter().any(|c| {
            ref_.is_source_block(c)
                && ref_
                    .block_content(c)
                    .is_some_and(|s| s.contains(SELF_PAGE_REF))
        })
}

fn nearest_page<R: RefBlockTree>(ref_: &R, start: &EntityUri) -> EntityUri {
    let mut at = start.clone();
    while !ref_.is_page_block(&at) {
        at = ref_
            .parent_of(&at)
            .unwrap_or_else(|| panic!("{start} has no page ancestor in the reference"));
    }
    at
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvSelfPageRefResolves
where
    R: RefBlockTree,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let mut failures = Vec::new();
        for holder in ref_.all_non_seed_block_ids() {
            if !names_own_page(ref_, &holder) {
                continue;
            }
            let expected = BTreeSet::from([nearest_page(ref_, &holder)]);
            let rendered = sut.widget_tree_for(&holder).await;
            let rows = sut.collection_row_ids(&holder).await;
            if rows.as_ref() != Some(&expected) {
                failures.push(format!(
                    "{holder}: rows {rows:?}, expected its nearest page {expected:?} \
                     (widget tree: {rendered:?})"
                ));
            }
        }
        if failures.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "inv-self-page-ref-resolves: a query naming {SELF_PAGE_REF} did not yield its \
                 nearest page: {failures:#?}"
            ))
        }
    }
}
