//! `inv-shape-sim-matches-authority` — the shape gate's simulator predicts
//! the store.
//!
//! @pbt oracle differential — the simulated post-write state of every tagged
//!   block a write touched, against what the write authority then holds
//! @pbt covers shape-simulator-fidelity — every block write any transition
//!   makes near a tagged block, through any op the dispatcher routes
//! @pbt slips-if-removed the gate judges a state the store never reaches, so a
//!   write it admits can still leave a tagged block broken
//!
//! Skipped while no judged write touched a tagged block; the skip reason
//! carries the count, so the engagement summary is the reach.

use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvShapeSimMatchesAuthority;

impl InvShapeSimMatchesAuthority {
    pub const ID: InvariantId = InvariantId("inv-shape-sim-matches-authority");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvShapeSimMatchesAuthority
where
    S: SutShapeEdit,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let (compared, divergences) = sut.shape_audit().await;
        if !divergences.is_empty() {
            return InvariantResult::Fail(format!(
                "{} of {compared} judged write(s) stored something other than the shape gate \
                 simulated:\n  {}",
                divergences.len(),
                divergences.join("\n  ")
            ));
        }
        if compared == 0 {
            return InvariantResult::Skipped("no judged write touched a tagged block".into());
        }
        InvariantResult::Ok
    }
}

/// Cross-case reach of the two shape invariants, for the keystone's end-of-run
/// floor: tagged-subtree edits the gate ADMITTED, and judged writes the
/// simulator was compared on. A refused edit writes nothing, so it has nothing
/// to compare.
static ADMITTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static REFUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static COMPARED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DECISION_CASES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Count one booted case whose store carries a decision to edit.
pub fn record_decision_case() {
    DECISION_CASES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Count one dispatched tagged-subtree edit and the judged writes it added.
pub fn record_edit(admitted: bool, compared: usize) {
    let counter = if admitted { &ADMITTED } else { &REFUSED };
    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    COMPARED.fetch_add(compared, std::sync::atomic::Ordering::SeqCst);
}

/// Cases from which a run owes shape reach. A smoke run of 1 case may draw a
/// wiring with no decision at all.
pub const REACH_FLOOR_CASES: u32 = 8;

/// Print the run's shape reach and, for a run of at least
/// [`REACH_FLOOR_CASES`] cases of which one carried a decision, fail when the
/// shape gate admitted no tagged-subtree edit or its simulator was compared on
/// none. A failed run owes it too; the caller reports this panic next to the
/// run's own. `edits_off`: the run's weights draw no decision edit.
pub fn assert_reached(cases: u32, edits_off: bool) {
    let (admitted, refused, compared, decision_cases) = report_reach();
    let owed = !edits_off && cases >= REACH_FLOOR_CASES && decision_cases > 0;
    assert!(
        !owed || (admitted >= 1 && compared >= 1),
        "[shape reach] NOT REACHED: the shape gate admitted {admitted} tagged-subtree edit(s) \
         (refused {refused}) and its simulator was compared against the write authority \
         {compared} time(s); inv-shape-gate-refuses-illegal-writes and \
         inv-shape-sim-matches-authority need at least one of each per run"
    );
}

/// Print the run's shape reach and fail unless the shape gate admitted one
/// tagged-subtree edit, refused one, and its simulator was compared on one.
/// For a replay of fixed cases, whose reach is fixed too.
pub fn assert_fully_reached() {
    let (admitted, refused, compared, _) = report_reach();
    assert!(
        admitted >= 1 && refused >= 1 && compared >= 1,
        "[shape reach] NOT REACHED: admitted={admitted} refused={refused} \
         comparisons={compared}; inv-shape-gate-refuses-illegal-writes and \
         inv-shape-sim-matches-authority need an admitted and a refused edit and one comparison"
    );
}

/// Print and return (admitted, refused, compared, decision cases).
pub fn report_reach() -> (usize, usize, usize, usize) {
    let admitted = ADMITTED.load(std::sync::atomic::Ordering::SeqCst);
    let refused = REFUSED.load(std::sync::atomic::Ordering::SeqCst);
    let compared = COMPARED.load(std::sync::atomic::Ordering::SeqCst);
    let decision_cases = DECISION_CASES.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "[shape reach] tagged-subtree edits admitted={admitted} refused={refused} simulator \
         comparisons={compared} over {decision_cases} case(s) carrying a decision"
    );
    (admitted, refused, compared, decision_cases)
}
