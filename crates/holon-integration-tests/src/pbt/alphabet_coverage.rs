//! Did every transition the drawn wiring PROMISES actually reach the generated
//! alphabet?
//!
//! @pbt kind coverage
//!
//! `aggregate_transitions` admits a variant into a wiring's alphabet when its
//! `required_wiring` + `required_caps` are satisfied — that admission is the
//! PROMISE the wiring makes. The variant only becomes drawable on top of that
//! if its `weighted_generator` finds a candidate in the state. When it never
//! does, the kind leaves the alphabet with no signal at all and the keystone
//! stays green over a smaller sample: `ToggleState` vanished exactly this way
//! on 2026-07-05, when the reference's seed profile was missing and the
//! generator saw zero `state_toggle` widgets
//! (`transitions/start_app.rs::load_seed_profile_into_ref`).
//!
//! The judged claim is OFFERED-FOR-AN-ALPHABET-REASON, not DRAWN. A shipped
//! keystone run draws a few hundred transitions over ~100 alphabet kinds, so a
//! weight-1 kind is absent from the drawn multiset by sampling accident alone
//! with double-digit probability — asserting on draws would be a coin-flip red.
//! Offering is not a sample: a kind the generator can serve is offered in every
//! state that admits it, so "promised many times, offered never" is a property
//! of the alphabet. Actual draws are reported beside it as an observe-only
//! dashboard (the same split `harness::teardown` uses for invariant
//! engagement).
//!
//! Offering alone is still not enough to judge on: most kinds a wiring promises
//! decline because the STATE is no fit yet (`Redo` with nothing to redo,
//! `MergeFromPeer` with no peer added, `StartApp` once the harness has booted),
//! and at 4 cases that is 51 of ~100 kinds. So the verdict reads the decline's
//! [`ReasonClass`]: a kind reds only when every decline it ever gave says the
//! surface is missing everywhere ([`ReasonClass::AlphabetGone`]). The classes
//! live on `Reason` itself as an exhaustive match, so a new reason does not
//! compile until it is classified.
//!
//! A third population declines by CONSTRUCTION: kinds whose
//! `weighted_generator(_: &R)` never binds the state and fails every time
//! ([`ReasonClass::NotDrawnHere`]). Their alphabet is not gone — they are drawn
//! by a hand-authored regression step or another medium's PBT — so they are
//! named in `EXPECTED_ABSENT` with the suite that covers them, and that list is
//! pinned against the generators by a test. Six of them decline for a second
//! reason — the `HOLON_PBT_WRITEBACK_FAULTS` gate is off — so they live in
//! `EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS` and count as accepted holes only
//! while that gate stays off.
//!
//! The counters are thread-local, like the rejection histogram in
//! `holon_pbt_core::validation`: proptest runs a test's cases on the test's own
//! thread, so one test's alphabet is never mixed into another's.

use std::cell::RefCell;
use std::collections::BTreeMap;

use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::ReasonClass;
use nonempty_collections::NEVec;

/// Gate admissions a kind needs before "never offered" is a claim about the
/// alphabet rather than about a run too short to have reached the kind's state.
const PROMISE_FLOOR: u64 = 20;

/// Kinds whose `weighted_generator` declines by construction — it never binds
/// the state — each with WHERE the kind is drawn instead. The list is pinned
/// against the generators themselves by
/// `the_by_construction_declines_are_exactly_the_listed_kinds`, so a kind that
/// joins or leaves it is a test diff, never silent. An entry whose second field
/// names no other suite is an uncovered kind, i.e. a finding.
const EXPECTED_ABSENT: &[(&str, &str)] = &[
    (
        "CrashRestart",
        "hand-authored-regressions/keystone.jsonl (5 steps)",
    ),
    (
        "DelayAdmissions",
        "hand-authored-regressions/keystone.jsonl (3 steps)",
    ),
    (
        "DeliverBlockContent",
        "NOWHERE: the GPUI layout PBT that drew it is gone (no \
         `arb_deferred_live_block_scenario` in the tree) and `layout_bridge` reports no deferred \
         blocks, so the deferred-live-block axis is uncovered",
    ),
    (
        "FailNextDispatch",
        "hand-authored-regressions/keystone.jsonl (2 steps)",
    ),
    (
        "HoldDispatch",
        "hand-authored-regressions/keystone.jsonl (10 steps)",
    ),
    (
        "RebootWithBacklogHeld",
        "hand-authored-regressions/keystone.jsonl (1 step)",
    ),
    (
        "ReleaseBacklog",
        "hand-authored-regressions/keystone.jsonl (1 step)",
    ),
    (
        "ReleaseHeld",
        "hand-authored-regressions/keystone.jsonl (10 steps)",
    ),
    (
        "StructuralKeyBurst",
        "crates/holon-integration-tests/tests/structural_burst_under_lag.rs",
    ),
];

/// Kinds the writeback-fault gate keeps out of the generated alphabet: their
/// `weighted_generator` declines for every state while
/// `HOLON_PBT_WRITEBACK_FAULTS` is unset
/// (`transitions::arm_write_churn::writeback_faults_enabled`), and nothing in
/// this repo sets it, so today they are absent from every run. Same contract as
/// [`EXPECTED_ABSENT`]: the second field names where the kind IS covered, and
/// "NOWHERE" is a finding.
const EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS: &[(&str, &str)] = &[
    (
        "ArmWriteChurn",
        "hand-authored-regressions/keystone.jsonl (7 steps)",
    ),
    (
        "ArmWriteFailure",
        "hand-authored-regressions/keystone.jsonl (2 steps)",
    ),
    (
        "DisarmWriteChurn",
        "hand-authored-regressions/keystone.jsonl (3 steps)",
    ),
    (
        "DropNextWatcherEvent",
        "hand-authored-regressions/keystone.jsonl (1 step)",
    ),
    (
        "ExternalLineEdit",
        "hand-authored-regressions/keystone.jsonl (7 steps)",
    ),
    (
        "TouchFile",
        "NOWHERE: no hand-authored step draws it and the only generator is behind \
         `HOLON_PBT_WRITEBACK_FAULTS`, which nothing sets — the no-op-save axis is uncovered",
    ),
];

/// The accepted holes for THIS process: the always-absent kinds, plus the
/// writeback-fault kinds while that gate is off.
fn expected_absent() -> Vec<(&'static str, &'static str)> {
    let mut all: Vec<(&'static str, &'static str)> = EXPECTED_ABSENT.to_vec();
    if !crate::pbt::transitions::arm_write_churn::writeback_faults_enabled() {
        all.extend_from_slice(EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS);
    }
    all
}

/// One kind's run tallies.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Counts {
    /// States whose wiring + caps admitted the kind into the alphabet.
    pub promised: u64,
    /// States whose `weighted_generator` produced an arm for it.
    pub offered: u64,
    /// Transitions of this kind applied to the reference.
    pub drawn: u64,
    /// Declines whose reason says the kind cannot be served while the
    /// composition renders what it renders.
    pub alphabet_gone: u64,
    /// Declines whose reason says THIS state is no fit, and another could be.
    pub precondition_unmet: u64,
    /// Declines whose reason says this medium never generates the kind; it is
    /// drawn by the suite `EXPECTED_ABSENT` names.
    pub not_drawn_here: u64,
}

thread_local! {
    static COUNTERS: RefCell<BTreeMap<&'static str, Counts>> = const { RefCell::new(BTreeMap::new()) };
}

fn bump(name: &'static str, f: impl FnOnce(&mut Counts)) {
    COUNTERS.with(|c| f(c.borrow_mut().entry(name).or_default()));
}

/// The wiring + caps gate admitted this kind in one state.
pub fn note_promised(name: &'static str) {
    bump(name, |c| c.promised += 1);
}

/// The kind's generator produced an arm in one state — it is drawable there.
pub fn note_offered(name: &'static str) {
    bump(name, |c| c.offered += 1);
}

/// A transition of this kind was applied to the reference.
pub fn note_drawn(name: &'static str) {
    bump(name, |c| c.drawn += 1);
}

/// The kind's generator declined one state; its reasons say whether the state
/// or the alphabet is what the decline describes.
pub fn note_declined(name: &'static str, reasons: &NEVec<Reason>) {
    bump(name, |c| {
        for reason in reasons {
            match reason.class() {
                ReasonClass::AlphabetGone => c.alphabet_gone += 1,
                ReasonClass::PreconditionUnmet => c.precondition_unmet += 1,
                ReasonClass::NotDrawnHere => c.not_drawn_here += 1,
            }
        }
    });
}

/// The alphabet a kind-keyed tally is judged against: promised often, offered
/// never, every decline saying the alphabet (not the state) is why, and not a
/// known hole.
fn judge(
    counts: &BTreeMap<&'static str, Counts>,
    expected_absent: &[(&str, &str)],
) -> Result<(), String> {
    let vanished: Vec<String> = counts
        .iter()
        .filter(|(name, c)| {
            c.promised >= PROMISE_FLOOR
                && c.offered == 0
                && c.alphabet_gone > 0
                && c.precondition_unmet == 0
                && c.not_drawn_here == 0
                && !expected_absent.iter().any(|(n, _)| n == *name)
        })
        .map(|(name, c)| {
            format!(
                "{name} (promised in {} states, {} alphabet-gone declines)",
                c.promised, c.alphabet_gone
            )
        })
        .collect();
    if vanished.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[inv-alphabet-coverage] {} transition kind(s) the drawn wiring PROMISES never reached \
         the generated alphabet: {}. Their `required_wiring` + `required_caps` were satisfied, so \
         `aggregate_transitions` admitted them, yet `weighted_generator` declined on EVERY state \
         with a `ReasonClass::AlphabetGone` reason and NEVER with a state-dependent one — the \
         kind silently left the alphabet and this green run certifies nothing about it (the \
         `ToggleState` class, 2026-07-05: a reference that renders no `state_toggle` widget). \
         Either restore the surface the generator needs (the rejection histogram above names the \
         `Reason`), or add the kind to `EXPECTED_ABSENT` in \
         crates/holon-integration-tests/src/pbt/alphabet_coverage.rs with the reason it cannot be \
         offered.",
        vanished.len(),
        vanished.join(", "),
    ))
}

/// Print the per-kind tally for every promised kind the run never drew, and the
/// `EXPECTED_ABSENT` entries the run DID offer.
///
/// Observe-only, deliberately: both lines are sampling- and budget-dependent (a
/// 4-case CI run reaches far fewer states than the 16-case tier), so reding on
/// them would make the keystone's verdict depend on its case count. The judged
/// claim beside it is not: an `AlphabetGone` decline is state-independent.
fn disclose(counts: &BTreeMap<&'static str, Counts>, expected_absent: &[(&str, &str)]) {
    let undrawn: Vec<String> = counts
        .iter()
        .filter(|(_, c)| c.promised > 0 && c.drawn == 0)
        .map(|(name, c)| format!("{name}=promised:{},offered:{}", c.promised, c.offered))
        .collect();
    if !undrawn.is_empty() {
        eprintln!(
            "[alphabet coverage] promised but never drawn: {}",
            undrawn.join(" ")
        );
    }
    let stale: Vec<&str> = expected_absent
        .iter()
        .filter(|(name, _)| counts.get(name).is_some_and(|c| c.offered > 0))
        .map(|(name, _)| *name)
        .collect();
    if !stale.is_empty() {
        eprintln!(
            "[alphabet coverage] stale EXPECTED_ABSENT (this run DID offer them): {stale:?} — \
             drop them from the list"
        );
    }
}

/// `inv-alphabet-coverage`: disclose the run's alphabet tally, then judge it.
/// Called once at the end of the keystone run.
///
/// A run with `HOLON_PBT_WEIGHTS` set is disclosed and NOT judged: a `0`
/// multiplier removes an arm on purpose, which is indistinguishable here from
/// the hole this guard hunts.
pub fn assert_no_kind_left_the_alphabet() {
    let counts = COUNTERS.with(|c| c.borrow().clone());
    let expected_absent = expected_absent();
    disclose(&counts, &expected_absent);
    if let Some(spec) = std::env::var_os("HOLON_PBT_WEIGHTS") {
        eprintln!(
            "[alphabet coverage] NOT judged: HOLON_PBT_WEIGHTS={spec:?} may remove arms on purpose"
        );
        return;
    }
    if let Err(breach) = judge(&counts, &expected_absent) {
        panic!("{breach}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::pbt::reference_state::ReferenceState;

    /// `(kind, promised, offered, drawn, alphabet_gone, precondition_unmet,
    /// not_drawn_here)`
    fn tally(
        entries: &[(&'static str, u64, u64, u64, u64, u64, u64)],
    ) -> BTreeMap<&'static str, Counts> {
        entries
            .iter()
            .map(
                |(
                    name,
                    promised,
                    offered,
                    drawn,
                    alphabet_gone,
                    precondition_unmet,
                    not_drawn_here,
                )| {
                    (
                        *name,
                        Counts {
                            promised: *promised,
                            offered: *offered,
                            drawn: *drawn,
                            alphabet_gone: *alphabet_gone,
                            precondition_unmet: *precondition_unmet,
                            not_drawn_here: *not_drawn_here,
                        },
                    )
                },
            )
            .collect()
    }

    /// The `ToggleState` signature: the wiring promised it in every state and
    /// the generator never served it.
    #[test]
    fn a_promised_kind_the_generator_never_offered_is_a_breach() {
        let err =
            judge(&tally(&[("ToggleState", 400, 0, 0, 400, 0, 0)]), &[]).expect_err("must red");
        assert!(
            err.contains("ToggleState (promised in 400 states, 400 alphabet-gone declines)"),
            "{err}"
        );
    }

    /// Offered but never sampled is the accident this guard refuses to red on.
    #[test]
    fn a_kind_offered_but_never_drawn_passes() {
        judge(&tally(&[("ToggleState", 400, 400, 0, 0, 0, 0)]), &[]).expect("offered is the claim");
    }

    /// Below the floor the run, not the alphabet, is what the absence
    /// describes.
    #[test]
    fn a_kind_promised_fewer_times_than_the_floor_is_not_judged() {
        judge(
            &tally(&[(
                "ToggleState",
                PROMISE_FLOOR - 1,
                0,
                0,
                PROMISE_FLOOR - 1,
                0,
                0,
            )]),
            &[],
        )
        .expect("under the floor");
    }

    /// The population this guard must NOT red on: promised in every state and
    /// never offered, because the state never reached the shape the kind needs.
    #[test]
    fn a_kind_declined_only_for_an_unmet_precondition_is_not_judged() {
        judge(&tally(&[("Redo", 400, 0, 0, 0, 400, 0)]), &[]).expect("the state, not the alphabet");
    }

    /// A kind whose surface is gone in some states but present in others is
    /// the state's doing, not the alphabet's.
    #[test]
    fn a_kind_with_a_mixed_decline_history_is_not_judged() {
        judge(&tally(&[("ToggleState", 400, 0, 0, 399, 1, 0)]), &[]).expect("mixed declines");
    }

    /// A by-construction decliner: its generator ignores the state, so absence
    /// says nothing about the alphabet.
    #[test]
    fn a_kind_declining_by_construction_is_not_judged() {
        judge(&tally(&[("ReleaseHeld", 400, 0, 0, 0, 0, 400)]), &[])
            .expect("drawn by the hand-authored regressions, not by the generator");
    }

    /// The accepted holes are pinned against the generators themselves: the
    /// set of kinds that decline with nothing but `NotDrawnHere` must be
    /// exactly `expected_absent()` for this process. A kind that joins or
    /// leaves the set fails here instead of silently widening (or vacating)
    /// the accepted holes.
    #[test]
    fn the_by_construction_declines_are_exactly_the_listed_kinds() {
        let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
        let state = ReferenceState::new(holon_pbt_core::Wiring::full(), interp);
        let mut derived = crate::pbt::transitions::kinds_declining_by_construction(&state);
        derived.sort_unstable();
        let mut listed: Vec<&str> = expected_absent().iter().map(|(name, _)| *name).collect();
        listed.sort_unstable();
        assert_eq!(
            derived, listed,
            "the generators that decline by construction and the EXPECTED_ABSENT list disagree — \
             add the new kind with the suite that DOES draw it, or drop the kind whose generator \
             now serves the state"
        );
    }

    #[test]
    fn a_listed_kind_is_an_accepted_hole() {
        judge(
            &tally(&[("AdvanceDay", 400, 0, 0, 400, 0, 0)]),
            &[(
                "AdvanceDay",
                "its cap registers only under HOLON_PBT_ADVANCE_DAY",
            )],
        )
        .expect("listed");
    }
}
