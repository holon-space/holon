//! `inv-view-model-matches-store-at-quiescence`.
//!
//! @pbt oracle internal-consistency — the UI read model (published at the Loro
//! commit,   before the SQL write), the projection's private diff base `live`,
//! and the   SQL index must state the same block set once everything settles
//! @pbt covers read-model-drift — a delta the commit point published but never
//!   projected, a delta the projection wrote but never published, and a reseed
//!   that leaves the read model holding rows Loro retracted
//! @pbt slips-if-removed a read model that is a SECOND SOURCE OF TRUTH rather
//!   than a second consumer of one diff: no other invariant compares it
//!   against either the diff base or the index, so a producer that silently
//!   stopped publishing would leave every render green against a frozen
//!   mirror
//!
//! Three-way, not two-way, on purpose. Read-model-vs-SQL alone cannot tell a
//! publish bug from a projection bug; `live` sits between them (it advances
//! only after the sink write commits) and localizes the failure: read model ==
//! live != SQL is an index fault, read model != live == SQL is a publish
//! fault.
//!
//! Bounded-wait, mirroring `inv-matview-consistent-with-recompute`: the read
//! model leads the SQL write by construction, so a single-shot difference is
//! the ordinary in-flight window, not a bug. Only a divergence that PERSISTS
//! is a failure.

use holon_pbt_core::capabilities::ReadModelObservation;
use holon_pbt_core::capabilities::ReadModelRow;
use holon_pbt_core::capabilities::SutReadModel;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewModelMatchesStore;

/// Set the first time the oracle actually COMPARES a triple in this process.
///
/// The red-vector seam (`HOLON_PBT_VIEWMODEL_STALE`) can only break a read
/// model that exists; a run whose every draw lacked a Loro projection
/// deselects the invariant and goes green while proving nothing. That is
/// indistinguishable from a passing run unless someone asks afterwards
/// whether the oracle ran at all, which is what
/// [`assert_engaged_if_armed`] does.
static COMPARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Refuse an ARMED run in which the oracle never compared anything.
///
/// Called at the end of the keystone run, i.e. after every early return any
/// individual check could have taken. Unarmed runs are unaffected: a draw
/// without a Loro projection legitimately deselects.
pub fn assert_engaged_if_armed() {
    let Ok(arm) = std::env::var("HOLON_PBT_VIEWMODEL_STALE") else {
        return;
    };
    assert!(
        COMPARED.load(std::sync::atomic::Ordering::SeqCst),
        "HOLON_PBT_VIEWMODEL_STALE={arm:?} was armed for the whole run and \
         inv-view-model-matches-store-at-quiescence never compared a single triple — every draw \
         deselected it, so the run is GREEN having exercised nothing. Raise the share of draws \
         that boot a Loro projection, or unset the seam."
    );
}

impl InvViewModelMatchesStore {
    pub const ID: InvariantId = InvariantId("inv-view-model-matches-store-at-quiescence");

    /// `None` when the three agree. Otherwise the report naming WHICH pair
    /// disagrees and by which rows — the localization the three-way shape
    /// exists for.
    ///
    /// **Strict on both halves.** Read model ⇄ live is F1a's own contract:
    /// one diff, two consumers, so a row in one and not the other is a
    /// publish fault with no benign reading. Live ⇄ SQL is strict too, and
    /// twice now an exclusion has been tried there and withdrawn:
    ///
    /// * "the ids `live` owns" — strictly weaker, it excuses any row the
    ///   projection simply failed to publish;
    /// * "rows whose block has no live Loro node" — meant to name the declared
    ///   unseeded-vault class, but it cannot tell that class from a block Loro
    ///   DELETED whose index row survived (a withheld or lost delete, i.e. an
    ///   index fault), and no shipped case ever exercised it.
    ///
    /// So nothing is excluded. [`ReadModelObservation::sql_only_diagnostics`]
    /// says what the tree knows about each unmatched row instead, so a red
    /// arrives with its cause attached rather than being silently narrowed
    /// away.
    pub fn divergence(obs: &ReadModelObservation) -> Option<String> {
        let (model, live, sql) = (&obs.model, &obs.live, &obs.sql);
        if model == live && live == sql {
            return None;
        }
        let fault = if model == live {
            "the SQL INDEX disagrees with the diff base"
        } else if live == sql {
            "the READ MODEL disagrees with the diff base and the index, so the commit-point \
             publish dropped or invented a delta"
        } else {
            "all three disagree"
        };
        Some(format!(
            "{fault}\n  read model ({} rows) only: {:?}\n  live ({} rows) only: {:?}\n  sql \
             ({} rows) only: {:?}\n  sql-only rows, as the Loro tree sees them: {:?}",
            model.len(),
            only_in(model, live, sql),
            live.len(),
            only_in(live, model, sql),
            sql.len(),
            only_in(sql, model, live),
            obs.sql_only_diagnostics,
        ))
    }
}

/// Rows `a` holds that `b` or `c` is missing, capped so a wholesale divergence
/// reports a sample rather than the whole vault. A row absent from EITHER of
/// the other two is worth naming — requiring it to be absent from both would
/// hide the single-sided case, which is the common one.
fn only_in(a: &[ReadModelRow], b: &[ReadModelRow], c: &[ReadModelRow]) -> Vec<ReadModelRow> {
    a.iter()
        .filter(|r| !b.contains(r) || !c.contains(r))
        .take(5)
        .cloned()
        .collect()
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewModelMatchesStore
where
    S: SutReadModel,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let obs = sut.read_model_triple().await;
        COMPARED.store(true, std::sync::atomic::Ordering::SeqCst);
        if Self::divergence(&obs).is_none() {
            return InvariantResult::Ok;
        }

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
                    "[{}] read model / live / SQL disagreed for {budget:?} — past the in-flight \
                     window in which the read model legitimately leads the sink write, so this \
                     is real drift, not lag.\n{last_fail}",
                    Self::ID.0
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let obs = sut.read_model_triple().await;
            match Self::divergence(&obs) {
                None => match stable_since {
                    Some(since) if since.elapsed() >= stable_for => return InvariantResult::Ok,
                    Some(_) => {}
                    None => stable_since = Some(Instant::now()),
                },
                Some(report) => {
                    stable_since = None;
                    last_fail = report;
                }
            }
        }
    }
}
