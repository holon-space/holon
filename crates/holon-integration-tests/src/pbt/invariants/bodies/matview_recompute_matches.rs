//! `inv-matview-consistent-with-recompute`.
//!
//! @pbt oracle differential — every IVM-maintained MATERIALIZED view equals its
//!   own defining SELECT re-executed now (a bounded-wait STABLE fixed point to
//!   reject transient CDC maintenance lag)
//! @pbt covers matview-dup / matview-drift — the "22-vs-20 matview DUP" class
//!   where IVM incremental maintenance PERSISTS a row-count divergence the
//!   recompute would not produce
//! @pbt slips-if-removed an IVM antijoin / consolidation bug that leaves a
//!   matview permanently out of sync with its query, invisible to every
//!   ref-comparison invariant (they compare against the reference model, not
//!   against the view's OWN defining query)
//!
//! Differential: `matview_rows` (what IVM maintains) vs `recompute_rows` (the
//! stored defining SELECT evaluated now), per materialized view. Both come from
//! [`SutMatviews::matview_recompute_snapshot`] as canonically-sorted multisets,
//! so this body is a pure comparison with a bounded-wait stability window.
//!
//! A single-shot divergence is NOT automatically a bug: the matview read and
//! the recompute are two separate queries, and a concurrent write between them
//! (or an in-flight CDC pass) yields a spurious one-tick diff that self-heals
//! in one maintenance pass. So (mirroring `inv-org-render-fixed-point`) we
//! bounded-wait for a STABLE match and `Fail` only if a divergence PERSISTS.

use holon_pbt_core::capabilities::SutMatviews;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

type Snapshot = Vec<(String, Vec<Vec<String>>, Vec<Vec<String>>)>;

pub struct InvMatviewRecomputeMatches;

impl InvMatviewRecomputeMatches {
    pub const ID: InvariantId = InvariantId("inv-matview-consistent-with-recompute");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvMatviewRecomputeMatches
where
    S: SutMatviews,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // Fast path: already consistent (convergence settled). The common case,
        // so it must add no latency.
        if Self::first_divergence(&sut.matview_recompute_snapshot().await).is_none() {
            return InvariantResult::Ok;
        }

        // A divergence here may be transient IVM maintenance lag (self-heals in
        // one CDC pass) rather than a persistent matview-vs-recompute bug. Model
        // that: bounded-wait for a STABLE match (equal, and holding for a quiet
        // floor so a momentarily-equal oscillation can't slip through). Fail only
        // if the divergence persists for the whole budget.
        use std::time::Duration;
        use std::time::Instant;
        let budget = Duration::from_secs(5);
        let stable_for = Duration::from_millis(200);
        let deadline = Instant::now() + budget;
        let mut stable_since: Option<Instant> = None;
        let mut last_fail = String::new();
        loop {
            if Instant::now() >= deadline {
                return InvariantResult::Fail(format!(
                    "[inv-matview-consistent-with-recompute] matview != recompute PERSISTED for \
                     {budget:?} — not transient IVM maintenance lag but a real matview drift: the \
                     IVM-maintained view disagrees with its own defining SELECT.\n{last_fail}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let snapshot = sut.matview_recompute_snapshot().await;
            match Self::first_divergence(&snapshot) {
                None => match stable_since {
                    Some(t) if t.elapsed() >= stable_for => return InvariantResult::Ok,
                    Some(_) => {}
                    None => stable_since = Some(Instant::now()),
                },
                Some((name, matview, recompute)) => {
                    // Still diverged (or oscillated back) — reset the window.
                    stable_since = None;
                    last_fail = Self::describe(name, matview, recompute);
                }
            }
        }
    }
}

impl InvMatviewRecomputeMatches {
    /// First view whose IVM-maintained rows differ from its recompute, if any.
    fn first_divergence(snapshot: &Snapshot) -> Option<(&str, &[Vec<String>], &[Vec<String>])> {
        snapshot.iter().find_map(|(name, matview, recompute)| {
            (matview != recompute).then_some((
                name.as_str(),
                matview.as_slice(),
                recompute.as_slice(),
            ))
        })
    }

    /// First-divergent-layer report: name the view, both row counts, and the
    /// first differing row on each side.
    fn describe(name: &str, matview: &[Vec<String>], recompute: &[Vec<String>]) -> String {
        let first_diff = matview
            .iter()
            .zip(recompute.iter())
            .find(|(m, r)| m != r)
            .map(|(m, r)| {
                format!("--- first differing row ---\nmatview:   {m:?}\nrecompute: {r:?}")
            })
            .unwrap_or_else(|| {
                // Same prefix, different length: the extra/missing tail row.
                let idx = matview.len().min(recompute.len());
                format!(
                    "--- length mismatch, first unmatched row (index {idx}) ---\nmatview:   \
                     {:?}\nrecompute: {:?}",
                    matview.get(idx),
                    recompute.get(idx),
                )
            });
        format!(
            "view `{name}`: matview has {} rows, recompute has {} rows\n{first_diff}",
            matview.len(),
            recompute.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `SutMatviews` stub returning a fixed snapshot — the differential
    /// twin's deterministic red/green harness (mirrors
    /// `DivergentOrgRender`). `matview` and `recompute` are supplied
    /// directly so a test can make them agree (green) or disagree (red)
    /// with no live engine.
    struct StubMatviews(Snapshot);

    #[async_trait::async_trait(?Send)]
    impl SutMatviews for StubMatviews {
        async fn matview_recompute_snapshot(&self) -> Snapshot {
            self.0.clone()
        }
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn matview_recompute_matching_stub_is_ok() {
        let rows = vec![row(&["id=1", "v=a"]), row(&["id=2", "v=b"])];
        let stub = StubMatviews(vec![("block".to_string(), rows.clone(), rows)]);
        let result = InvMatviewRecomputeMatches.check(&(), &stub).await;
        assert!(
            matches!(result, InvariantResult::Ok),
            "matching matview/recompute must be Ok; got {result:?}"
        );
    }

    #[tokio::test]
    async fn matview_recompute_divergent_stub_fails() {
        // Recompute drops one row the matview still carries — a persistent
        // matview-vs-recompute divergence. Deliberately burns the 5s budget.
        let matview = vec![row(&["id=1", "v=a"]), row(&["id=2", "v=b"])];
        let recompute = vec![row(&["id=1", "v=a"])];
        let stub = StubMatviews(vec![("block".to_string(), matview, recompute)]);
        let result = InvMatviewRecomputeMatches.check(&(), &stub).await;
        match result {
            InvariantResult::Fail(msg) => {
                assert!(msg.contains("block"), "fail msg must name the view: {msg}");
            }
            other => panic!("divergent matview/recompute must Fail; got {other:?}"),
        }
    }
}
