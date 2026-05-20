//! `inv-focus-roots` — the `focus_roots` matview's per-region root set matches
//! the reference's expected focus roots.
//!
//! Strict with a CDC-lag → `Skipped` downgrade (same shape as
//! `inv-blocks-match-ref/matview`): the body first compares the CDC-driven
//! `LiveData<FocusRoot>` mirror (`SutBackend::live_focus_root_rows`) to the
//! reference; on a per-region mismatch it consults the `focus_roots` matview
//! (`SutSqlProjection::focus_roots_rows`, the convergent truth). If the matview
//! matches the reference the mirror merely lagged → the region is skipped. If
//! the matview ALSO disagrees, the body reads the BASE `navigation_history`
//! table (`SutSqlProjection::nav_history_open_rows`) the matview projects from
//! and classifies the `Fail`: base==matview means a holon close-path /
//! reference-model bug (the matview is correct); base≠matview is a genuine Turso
//! IVM drift. (Historically this was hardcoded as "real Turso IVM bug" — an
//! unjustified leap, since a faithful matview over a wrong base looks identical
//! to drift unless you check the base.)
//! Expected roots come from the focus component
//! (`RefFocus::expected_focus_root_rows`, string-keyed and already resolved).

use std::collections::{BTreeSet, HashMap};

use holon_pbt_core::capabilities::{RefFocus, SutBackend, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvFocusRoots;

impl InvFocusRoots {
    pub const ID: InvariantId = InvariantId("inv-focus-roots");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFocusRoots
where
    R: RefFocus,
    S: SutBackend + SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // CDC-driven mirror, grouped region -> root-id set.
        let mut mirror: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (region, root_id) in sut.live_focus_root_rows().await {
            mirror.entry(region).or_default().insert(root_id);
        }

        // Matview truth, fetched lazily on the first mismatch (the common case
        // is the mirror matching, needing no extra query).
        let mut matview: Option<HashMap<String, BTreeSet<String>>> = None;
        let mut lagged_region: Option<String> = None;

        for (region, expected_vec) in ref_.expected_focus_root_rows() {
            let expected: BTreeSet<String> = expected_vec.into_iter().collect();
            let actual = mirror.get(&region).cloned().unwrap_or_default();
            if actual == expected {
                continue;
            }

            if matview.is_none() {
                let mut mv: HashMap<String, BTreeSet<String>> = HashMap::new();
                for (r, root_id) in sut.focus_roots_rows().await {
                    mv.entry(r).or_default().insert(root_id);
                }
                matview = Some(mv);
            }
            let truth = matview
                .as_ref()
                .unwrap()
                .get(&region)
                .cloned()
                .unwrap_or_default();

            if truth == expected {
                // Mirror lagged the matview — CDC delivery race, not a bug.
                lagged_region = Some(region.clone());
                continue;
            }

            // The matview disagrees with the reference. Before blaming the IVM,
            // read the BASE `navigation_history` table: the `focus_roots`
            // matview is a pure filtered projection of it
            // (`closed_at IS NULL AND block_id IS NOT NULL`). If the base set
            // already matches the matview, the matview is faithfully reflecting
            // the base — the bug is a holon close-path / reference-model
            // mismatch, NOT Turso. Only a base≠matview split is a real IVM drift.
            let mut base: HashMap<String, BTreeSet<String>> = HashMap::new();
            for (r, block_id) in sut.nav_history_open_rows().await {
                base.entry(r).or_default().insert(block_id);
            }
            let base_truth = base.get(&region).cloned().unwrap_or_default();

            if base_truth == truth {
                return InvariantResult::Fail(format!(
                    "[inv-focus-roots] region '{region}' focus_roots mismatch — the matview \
                     faithfully reflects the BASE navigation_history table, which itself disagrees \
                     with the reference. This is a holon close-path / reference-model bug, NOT a \
                     Turso IVM drift.\n  expected:        {expected:?}\n  mirror:          {actual:?}\
                     \n  matview:         {truth:?}\n  base(nav_history): {base_truth:?}"
                ));
            }

            return InvariantResult::Fail(format!(
                "[inv-focus-roots] region '{region}' focus_roots mismatch — the matview disagrees \
                 with the BASE navigation_history table it projects from (genuine Turso IVM \
                 drift).\n  expected:        {expected:?}\n  mirror:          {actual:?}\n  \
                 matview:         {truth:?}\n  base(nav_history): {base_truth:?}"
            ));
        }

        // Reverse direction: mirror regions with a non-empty root set must be
        // ⊆ ref regions — roots for a region the reference never opened are
        // ghosts (previously passed silently). Consult the matview before
        // failing: a ghost only in the mirror is CDC lag → Skipped via the
        // existing path; a ghost the matview confirms is a real divergence.
        let ref_regions: BTreeSet<String> = ref_
            .expected_focus_root_rows()
            .into_iter()
            .map(|(region, _)| region)
            .collect();
        for (region, roots) in &mirror {
            if roots.is_empty() || ref_regions.contains(region) {
                continue;
            }
            if matview.is_none() {
                let mut mv: HashMap<String, BTreeSet<String>> = HashMap::new();
                for (r, root_id) in sut.focus_roots_rows().await {
                    mv.entry(r).or_default().insert(root_id);
                }
                matview = Some(mv);
            }
            let truth = matview
                .as_ref()
                .unwrap()
                .get(region)
                .cloned()
                .unwrap_or_default();
            if truth.is_empty() {
                // Mirror-only ghost: stale CDC mirror entry the matview has
                // already dropped — same lag semantics as the forward path.
                lagged_region = Some(region.clone());
                continue;
            }
            return InvariantResult::Fail(format!(
                "[inv-focus-roots] ghost region '{region}': focus_roots has roots {truth:?} \
                 (mirror: {roots:?}) but the reference expects no focus roots for that region"
            ));
        }

        match lagged_region {
            Some(r) => InvariantResult::Skipped(format!(
                "[inv-focus-roots] LiveData<FocusRoot> mirror lagged the focus_roots matview on \
                 region '{r}' (Turso IVM CDC delivery race); matview matched the reference"
            )),
            None => InvariantResult::Ok,
        }
    }
}
