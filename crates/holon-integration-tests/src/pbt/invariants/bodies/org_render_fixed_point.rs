//! `inv-org-render-fixed-point`.
//!
//! Re-renders every tracked org file from the current SQL state and asserts
//! the output equals the bytes already on disk — guards against the
//! echo-suppression loop spin where `render(SQL) != disk` would force
//! `re_render_all_tracked` to write a different file on the next tick,
//! firing FSEvent, reprocessing via `on_file_changed`, and looping.
//!
//! Catches the May-2026 shared-tree mount loop where a property-drawer key
//! round-trip differed between ingestion and render. The `inv-blocks-match-ref`
//! family does NOT cover this — the parser is forgiving of property
//! ordering / sibling reordering driven by `sort_key` drift, and never
//! sees disagreement at all when the bug only manifests in a file shape
//! the reference model never generates (e.g. `:share-role: mount`).
//!
//! Capability: `SutOrgRender::snapshot_org_render_pairs` returns
//! `(path, disk, rendered)` triples.

use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvOrgRenderFixedPoint;

impl InvOrgRenderFixedPoint {
    pub const ID: InvariantId = InvariantId("inv-org-render-fixed-point");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvOrgRenderFixedPoint
where
    S: SutOrgRender,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // Fast path: already at the fixed point (the settle converged). This is
        // the overwhelming common case, so it must add no latency.
        if Self::first_mismatch(&sut.snapshot_org_render_pairs().await).is_none() {
            return InvariantResult::Ok;
        }

        // A divergence here is NOT automatically an echo-loop. Prod's
        // `re_render_all_tracked` is CDC-gated and 50ms-debounced, so a transient
        // Loro->SQL projection LAG (a just-reordered sibling batch whose new
        // `sort_key` has not projected yet) self-heals in ONE render pass without
        // spinning. The echo-loop this invariant guards needs render != disk to
        // PERSIST. So model prod: bounded-wait for the render to reach a STABLE
        // fixed point (equal, and holding for a quiet floor so a momentarily-equal
        // OSCILLATION can't slip through). Fail only if the divergence persists.
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
                    "[inv-org-render-fixed-point] render != disk PERSISTED for {budget:?} — not a \
                     transient projection lag but a real echo-loop / oscillation: the next \
                     re_render_all_tracked would keep rewriting the file.\n{last_fail}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let pairs = sut.snapshot_org_render_pairs().await;
            match Self::first_mismatch(&pairs) {
                None => match stable_since {
                    Some(t) if t.elapsed() >= stable_for => return InvariantResult::Ok,
                    Some(_) => {}
                    None => stable_since = Some(Instant::now()),
                },
                Some((path, disk, rendered)) => {
                    // Still diverged (or oscillated back) — reset the stability window.
                    stable_since = None;
                    last_fail = format!(
                        "--- disk ({} bytes) [{path}] ---\n{disk}\n--- rendered from SQL ({} \
                         bytes) ---\n{rendered}",
                        disk.len(),
                        rendered.len(),
                    );
                }
            }
        }
    }
}

impl InvOrgRenderFixedPoint {
    /// First `(path, disk, rendered)` whose disk bytes differ from the render,
    /// if any.
    fn first_mismatch(pairs: &[(String, String, String)]) -> Option<(&str, &str, &str)> {
        pairs.iter().find_map(|(path, disk, rendered)| {
            (disk != rendered).then_some((path.as_str(), disk.as_str(), rendered.as_str()))
        })
    }
}
