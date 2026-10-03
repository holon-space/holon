//! `inv-engine-views-match-reference`.
//!
//! @pbt oracle differential — every view the engine released equals the same
//!   view recomputed by the batch backend from the SUT's Loro docs, once
//!   everything settles
//! @pbt covers engine-views-drift — a feed the projection never posted, a
//!   feed whose rows and cover disagree, and an engine that stopped
//! @pbt slips-if-removed the engine's views: no other invariant reads them,
//!   and the loro_* invariants tie the Loro docs, not the engine, to the
//!   reference
//!
//! Bounded wait: the engine releases a version only after the projection fed
//! it, so a single-shot difference is the ordinary in-flight window.

use std::time::Duration;
use std::time::Instant;

use holon_pbt_core::capabilities::EngineViewsObservation;
use holon_pbt_core::capabilities::SutEngineViews;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvEngineViewsMatchReference;

impl InvEngineViewsMatchReference {
    pub const ID: InvariantId = InvariantId("inv-engine-views-match-reference");

    /// `None` when every view agrees; else per differing view a sample of
    /// the rows only one side holds.
    pub fn divergence(obs: &EngineViewsObservation) -> Option<String> {
        let engine = match &obs.engine {
            Ok(engine) => engine,
            Err(stopped) => return Some(format!("the view engine stopped: {stopped}")),
        };
        if engine == &obs.authority {
            return None;
        }
        let report = engine
            .iter()
            .zip(&obs.authority)
            .filter(|(e, a)| e != a)
            .map(|((view, engine), (_, authority))| {
                format!(
                    "{view}: engine ({} rows) only: {:?}\n  authority ({} rows) only: {:?}",
                    engine.len(),
                    only_in(engine, authority),
                    authority.len(),
                    only_in(authority, engine),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(report)
    }
}

fn only_in<'a>(a: &'a [String], b: &[String]) -> Vec<&'a String> {
    a.iter().filter(|row| !b.contains(row)).take(5).collect()
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvEngineViewsMatchReference
where
    S: SutEngineViews,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let budget = Duration::from_secs(5);
        let stable_for = Duration::from_millis(200);
        let deadline = Instant::now() + budget;
        let mut stable_since: Option<Instant> = None;
        let mut last_fail = String::new();
        loop {
            match Self::divergence(&sut.engine_views().await) {
                None => match stable_since {
                    None if last_fail.is_empty() => return InvariantResult::Ok,
                    Some(since) if since.elapsed() >= stable_for => return InvariantResult::Ok,
                    Some(_) => {}
                    None => stable_since = Some(Instant::now()),
                },
                Some(report) => {
                    stable_since = None;
                    last_fail = report;
                }
            }
            if Instant::now() >= deadline {
                return InvariantResult::Fail(format!(
                    "[{}] the engine's views differed from the views recomputed from the Loro \
                     docs for {budget:?}, past the in-flight window.\n{last_fail}",
                    Self::ID.0
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
