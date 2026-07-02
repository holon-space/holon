//! `inv-navigation-focus` — the `current_focus` matview's per-region focus
//! matches the reference's navigation focus.
//!
//! Component-correct: the SUT side reads the `current_focus` matview via the
//! query engine (`SutSqlProjection::current_focus_rows`); the ref side is the
//! focus component (`RefFocus::navigation_focus_rows`, string-keyed for
//! LeftSidebar/RightSidebar granularity that `CapRegion` collapses). The ref
//! rows are already resolved into SUT id space by `with_resolved_doc_uris`.

use std::collections::HashMap;

use holon_pbt_core::capabilities::{RefFocus, SutFocusProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvNavigationFocus;

impl InvNavigationFocus {
    pub const ID: InvariantId = InvariantId("inv-navigation-focus");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNavigationFocus
where
    R: RefFocus,
    S: SutFocusProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // region -> block_id (None = NULL/home). Presence of the key = the
        // matview has a row for that region.
        let sut_map: HashMap<String, Option<String>> =
            sut.current_focus_rows().await.into_iter().collect();

        for (region, expected) in ref_.navigation_focus_rows() {
            let has_row = sut_map.contains_key(&region);
            let row_block_id = sut_map.get(&region).cloned().flatten();
            match (has_row, &expected) {
                (true, Some(exp)) => {
                    if row_block_id.as_deref() != Some(exp.as_str()) {
                        return InvariantResult::Fail(format!(
                            "[inv-navigation-focus] region '{region}': expected focus {exp:?}, \
                             matview has {row_block_id:?}"
                        ));
                    }
                }
                (true, None) => {
                    if row_block_id.is_some() {
                        return InvariantResult::Fail(format!(
                            "[inv-navigation-focus] region '{region}': expected home (no focus), \
                             matview has {row_block_id:?}"
                        ));
                    }
                }
                (false, None) => {}
                (false, Some(exp)) => {
                    return InvariantResult::Fail(format!(
                        "[inv-navigation-focus] region '{region}' should have focus on {exp:?} \
                         but has no row in the current_focus matview"
                    ));
                }
            }
        }

        // Reverse direction: matview regions must be ⊆ ref regions — a
        // focused row for a region the reference never navigated is a ghost
        // (previously passed silently; only the ref→SUT direction was
        // checked). NULL rows (focus cleared / home) for an untracked
        // region carry no focus and are tolerated.
        let ref_regions: std::collections::HashSet<String> = ref_
            .navigation_focus_rows()
            .into_iter()
            .map(|(region, _)| region)
            .collect();
        for (region, block_id) in &sut_map {
            if let Some(ghost) = block_id {
                if !ref_regions.contains(region) {
                    return InvariantResult::Fail(format!(
                        "[inv-navigation-focus] ghost row: current_focus matview has region \
                         '{region}' focused on {ghost:?}, but the reference has no navigation \
                         history for that region"
                    ));
                }
            }
        }
        InvariantResult::Ok
    }
}
