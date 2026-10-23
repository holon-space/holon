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
//! named in `EXPECTED_ABSENT` with the [`CoveredBy`] claim that accounts for
//! them, and that list is pinned against the generators by a test. Six of them
//! decline for a second reason — the `HOLON_PBT_WRITEBACK_FAULTS` gate is off —
//! so they live in `EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS` and count as
//! accepted holes only while that gate stays off.
//!
//! A coverage claim this process cannot MEASURE is not representable (ruling
//! 280/suite-owns-claim.b). [`CoveredBy::HandAuthored`] is resolved against the
//! corpus this process loads; every other claim is REPORTED in
//! [`ACCOUNTED_ELSEWHERE`] and resolved by nobody. Resolving a claim against
//! the TEXT of the file it pointed at was the earlier shape, and an
//! `attest_drew` line sitting in a comment satisfied it while nothing ran. So a
//! suite outside the generated alphabet vouches for ITSELF: it calls
//! [`attest_drew`], which reds in that suite's own process when its run drew
//! the kind 0 times.
//!
//! The counters are process-global, not thread-local: a vouching suite may
//! apply its transitions on a worker thread while [`attest_drew`] runs on the
//! test thread (holon-work#441), and nextest gives every test its own process,
//! so "this process" is "this test's run".

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::MutexGuard;

use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::ReasonClass;
use nonempty_collections::NEVec;

/// Gate admissions a kind needs before "never offered" is a claim about the
/// alphabet rather than about a run too short to have reached the kind's state.
const PROMISE_FLOOR: u64 = 20;

/// What accounts for a kind that declines by construction. A closed set of
/// claims this process can either MEASURE or must REPORT — never a site whose
/// file some check greps (ruling 280/suite-owns-claim.b), which is why no
/// variant carries a path. Pinned by `no_coverage_claim_carries_a_site_path`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoveredBy {
    /// The hand-authored regression corpus
    /// (`hand_authored::DEFAULT_SIDECAR`). WHICH cases draw the kind is derived
    /// from the corpus, so the entry cannot claim a step that is not there, and
    /// at least one of those cases must be one the `hand-authored` recipe runs
    /// by default — a kind whose every case sits in the recipe's quarantine
    /// list is drawn by nobody.
    HandAuthored,
    /// A suite outside the generated alphabet draws the kind and vouches for
    /// ITSELF: it calls [`attest_drew`], which reds in that suite's process
    /// when its run drew the kind 0 times. This process cannot see that run, so
    /// the entry is no claim this module resolves — [`ACCOUNTED_ELSEWHERE`]
    /// names the suite, for the reader.
    SelfAttestingSuite,
    /// Nothing draws the kind: an uncovered axis, reported via
    /// [`ACCOUNTED_ELSEWHERE`]. The string says why the surface is gone.
    Nowhere(&'static str),
}

/// Kinds whose `weighted_generator` declines by construction — it never binds
/// the state — each with WHERE the kind is drawn instead. The list is pinned
/// against the generators themselves by
/// `the_by_construction_declines_are_exactly_the_listed_kinds`, so a kind that
/// joins or leaves it is a test diff, never silent.
const EXPECTED_ABSENT: &[(&str, CoveredBy)] = &[
    ("CrashRestart", CoveredBy::HandAuthored),
    ("DelayAdmissions", CoveredBy::HandAuthored),
    (
        "DeliverBlockContent",
        CoveredBy::Nowhere(
            "the GPUI layout PBT that drew it is gone (no `arb_deferred_live_block_scenario` in \
             the tree) and `layout_bridge` reports no deferred blocks, so the deferred-live-block \
             axis is uncovered",
        ),
    ),
    (
        "FailNextDispatch",
        CoveredBy::Nowhere(
            "the only two corpus steps live in \
             `a-failed-create-cancels-its-keystrokes-with-one-disclosure` and its `-loro-dispatch` \
             twin, both quarantined by the `hand-authored` recipe's default \
             `HOLON_HAND_AUTHORED_SKIP` (registered known reds until admission Inc 2) — the \
             failed-dispatch axis is uncovered until those cases run again",
        ),
    ),
    ("HoldDispatch", CoveredBy::HandAuthored),
    ("RebootWhileDatabaseHeld", CoveredBy::HandAuthored),
    ("RebootWithBacklogHeld", CoveredBy::HandAuthored),
    ("ReleaseBacklog", CoveredBy::HandAuthored),
    ("ReleaseDatabaseAndRestart", CoveredBy::HandAuthored),
    ("ReleaseHeld", CoveredBy::HandAuthored),
    ("StructuralKeyBurst", CoveredBy::SelfAttestingSuite),
];

/// Kinds the writeback-fault gate keeps out of the generated alphabet: their
/// `weighted_generator` declines for every state while
/// `HOLON_PBT_WRITEBACK_FAULTS` is unset
/// (`transitions::arm_write_churn::writeback_faults_enabled`), and nothing in
/// this repo sets it, so today they are absent from every run. Same contract as
/// [`EXPECTED_ABSENT`], and the same checks: splitting the data into a second
/// const does not split the coverage duty, which is read over
/// [`all_declared_absent`].
const EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS: &[(&str, CoveredBy)] = &[
    ("ArmWriteChurn", CoveredBy::HandAuthored),
    ("ArmWriteFailure", CoveredBy::HandAuthored),
    ("DisarmWriteChurn", CoveredBy::HandAuthored),
    ("DropNextWatcherEvent", CoveredBy::HandAuthored),
    ("ExternalLineEdit", CoveredBy::HandAuthored),
    (
        "TouchFile",
        CoveredBy::Nowhere(
            "no hand-authored step draws it and the only generator is behind \
             `HOLON_PBT_WRITEBACK_FAULTS`, which nothing sets — the no-op-save axis is uncovered",
        ),
    ),
];

/// Every accepted hole this process cannot measure, each with WHERE it is
/// accounted for: the card that carries an uncovered axis
/// ([`CoveredBy::Nowhere`], a finding per ruling 280/static-decline.c), or the
/// suite that vouches for itself ([`CoveredBy::SelfAttestingSuite`], whose own
/// run reds). These values are pointers for the reader, resolved by no check —
/// a claim a check could resolve belongs in [`CoveredBy`] instead.
/// `every_claim_this_process_cannot_measure_is_reported` pins the list against
/// those two variants in both directions.
const ACCOUNTED_ELSEWHERE: &[(&str, &str)] = &[
    ("DeliverBlockContent", "holon-work#280"),
    ("FailNextDispatch", "holon-work#280"),
    (
        "StructuralKeyBurst",
        "crates/holon-integration-tests/tests/structural_burst_under_lag.rs, whose own run calls \
         `attest_drew`",
    ),
    ("TouchFile", "holon-work#280"),
];

/// The accepted holes for THIS process: the always-absent kinds, plus the
/// writeback-fault kinds while that gate is off.
fn expected_absent() -> Vec<(&'static str, CoveredBy)> {
    let mut all: Vec<(&'static str, CoveredBy)> = EXPECTED_ABSENT.to_vec();
    if !crate::pbt::transitions::arm_write_churn::writeback_faults_enabled() {
        all.extend_from_slice(EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS);
    }
    all
}

/// Where an unmeasurable claim is accounted for. Every [`CoveredBy::Nowhere`]
/// and [`CoveredBy::SelfAttestingSuite`] entry has one;
/// `every_claim_this_process_cannot_measure_is_reported` pins that, so a miss
/// here is a broken invariant, not a missing annotation.
fn reported_at(kind: &str) -> &'static str {
    ACCOUNTED_ELSEWHERE
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, where_)| *where_)
        .unwrap_or_else(|| {
            panic!(
                "{kind} is an accepted hole this process cannot measure and it is accounted for \
                 nowhere — ACCOUNTED_ELSEWHERE must name the card or the self-attesting suite \
                 that carries it"
            )
        })
}

/// Every DECLARED accepted hole, both lists, whatever the writeback-fault gate
/// says: a hole's coverage duty is a property of the declaration, not of the
/// run that happens to merge it.
#[cfg(test)]
fn all_declared_absent() -> Vec<(&'static str, CoveredBy)> {
    EXPECTED_ABSENT
        .iter()
        .chain(EXPECTED_ABSENT_WITHOUT_WRITEBACK_FAULTS)
        .copied()
        .collect()
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
    /// Declines whose reason says this medium never generates the kind; what
    /// accounts for it instead is the `EXPECTED_ABSENT` claim.
    pub not_drawn_here: u64,
}

static COUNTERS: LazyLock<Mutex<BTreeMap<&'static str, Counts>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// The run's tally. Poisoned means a case panicked mid-bump, which is a defect
/// of its own and must not read as an empty alphabet.
fn counters() -> MutexGuard<'static, BTreeMap<&'static str, Counts>> {
    COUNTERS
        .lock()
        .expect("a panic poisoned the alphabet counters; the tally cannot be trusted")
}

fn bump(name: &'static str, f: impl FnOnce(&mut Counts)) {
    f(counters().entry(name).or_default());
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

/// A suite outside the generated alphabet attests that ITS run drew `kind`.
/// The counter is the only evidence, and it is read here, in the suite's own
/// process: nothing resolves a [`CoveredBy::SelfAttestingSuite`] entry from
/// outside, so this call is where the claim lives or dies.
pub fn attest_drew(kind: &str) {
    let drawn = counters().get(kind).map_or(0, |counts| counts.drawn);
    assert!(
        drawn > 0,
        "{kind}: this suite vouches for itself as the kind's coverage \
         (`CoveredBy::SelfAttestingSuite` in `EXPECTED_ABSENT`), but its run drew it 0 times — \
         draw the kind here, or move the claim to the suite that does"
    );
    eprintln!("[alphabet coverage] attested: this suite drew {kind} {drawn}x");
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
    expected_absent: &[(&str, CoveredBy)],
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
fn disclose(counts: &BTreeMap<&'static str, Counts>, expected_absent: &[(&str, CoveredBy)]) {
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
    let uncovered: Vec<String> = expected_absent
        .iter()
        .filter(|(_, site)| matches!(site, CoveredBy::Nowhere(_)))
        .map(|(name, _)| format!("{name} (reported: {})", reported_at(name)))
        .collect();
    if !uncovered.is_empty() {
        eprintln!(
            "[alphabet coverage] accepted holes NOTHING draws: {}",
            uncovered.join(" ")
        );
    }
    let attested_elsewhere: Vec<String> = expected_absent
        .iter()
        .filter(|(_, site)| matches!(site, CoveredBy::SelfAttestingSuite))
        .map(|(name, _)| format!("{name} (its own run reds: {})", reported_at(name)))
        .collect();
    if !attested_elsewhere.is_empty() {
        eprintln!(
            "[alphabet coverage] accepted holes a self-attesting suite covers, unmeasurable from \
             THIS run: {}",
            attested_elsewhere.join(" ")
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
    let counts = counters().clone();
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
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::pbt::hand_authored;
    use crate::pbt::reference_state::ReferenceState;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the crate lives two levels under the repo root")
            .to_path_buf()
    }

    /// The `hand-authored` recipe's DEFAULT quarantine list: the cases
    /// `just hand-authored` does NOT run unless the caller overrides
    /// `HOLON_HAND_AUTHORED_SKIP`. The justfile is the only place it exists, so
    /// a shape change there must fail here rather than silently read as "skips
    /// nothing".
    fn gate_skipped_cases() -> Vec<String> {
        let justfile = repo_root().join("justfile");
        let text = std::fs::read_to_string(&justfile)
            .unwrap_or_else(|e| panic!("{justfile:?} is unreadable: {e}"));
        let marker = format!(
            "export {env}=${{{env}-",
            env = hand_authored::SKIP_FILTER_ENV
        );
        let tail = text
            .split_once(&marker)
            .unwrap_or_else(|| {
                panic!(
                    "{justfile:?} holds no `{marker}` default — the `hand-authored` recipe's \
                     quarantine list moved, so this check can no longer tell a case the gate runs \
                     from one it skips"
                )
            })
            .1;
        let spec = tail
            .split_once('}')
            .expect("the shell default is brace-terminated")
            .0;
        spec.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Corpus case names whose transition list draws `kind`.
    fn corpus_cases_drawing(kind: &str) -> Vec<String> {
        hand_authored::load_cases()
            .into_iter()
            .filter(|case| case.transitions.iter().any(|t| t.variant_name() == kind))
            .map(|case| case.name)
            .collect()
    }

    /// Resolve one accepted hole's claim against what THIS process can measure.
    /// `Err` carries the kind and why its claim is empty. The two claims this
    /// process cannot measure are reported instead, by
    /// `every_claim_this_process_cannot_measure_is_reported`.
    fn resolve_coverage(kind: &str, site: CoveredBy) -> Result<(), String> {
        match site {
            CoveredBy::Nowhere(_) | CoveredBy::SelfAttestingSuite => Ok(()),
            CoveredBy::HandAuthored => {
                let drawing = corpus_cases_drawing(kind);
                if drawing.is_empty() {
                    return Err(format!(
                        "{kind}: no case of {} draws it",
                        hand_authored::DEFAULT_SIDECAR
                    ));
                }
                let skipped = gate_skipped_cases();
                if drawing.iter().all(|case| skipped.contains(case)) {
                    return Err(format!(
                        "{kind}: the {} case(s) that draw it are ALL in the `hand-authored` \
                         recipe's default {} list ({drawing:?}), so no runner draws the kind",
                        drawing.len(),
                        hand_authored::SKIP_FILTER_ENV,
                    ));
                }
                Ok(())
            }
        }
    }

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
             add the new kind with the claim that accounts for it (`HandAuthored` resolves here; \
             anything else is reported in ACCOUNTED_ELSEWHERE), or drop the kind whose generator \
             now serves the state"
        );
    }

    #[test]
    fn a_listed_kind_is_an_accepted_hole() {
        judge(
            &tally(&[("AdvanceDay", 400, 0, 0, 400, 0, 0)]),
            &[(
                "AdvanceDay",
                CoveredBy::Nowhere("its cap registers only under HOLON_PBT_ADVANCE_DAY"),
            )],
        )
        .expect("listed");
    }

    /// Ruling 280/static-decline.c: an accepted hole names where the kind IS
    /// covered. A claim no runner executes is no coverage — the kind is then
    /// absent from the generated alphabet AND from every other suite, which is
    /// exactly the silent hole `inv-alphabet-coverage` exists to refuse. Only
    /// `HandAuthored` is resolved here, against the corpus this process loads;
    /// the rest is reported, by the test below.
    #[test]
    fn every_accepted_hole_names_coverage_a_runner_draws() {
        let empty_claims: Vec<String> = all_declared_absent()
            .into_iter()
            .filter_map(|(kind, site)| resolve_coverage(kind, site).err())
            .collect();
        assert!(
            empty_claims.is_empty(),
            "{} accepted hole(s) claim coverage no runner draws: {}. Either make a runner draw \
             the kind, or annotate it `CoveredBy::Nowhere` and report it in \
             ACCOUNTED_ELSEWHERE.",
            empty_claims.len(),
            empty_claims.join("; "),
        );
    }

    /// The reporting half of the same ruling, widened by ruling
    /// 280/suite-owns-claim.b: a claim this process cannot measure is REPORTED,
    /// and one that reaches nobody is the defect. Pinned both ways, so a kind
    /// that loses its last runner cannot be parked without a report, and a kind
    /// this process measures again cannot stay on the list.
    #[test]
    fn every_claim_this_process_cannot_measure_is_reported() {
        let mut unmeasurable: Vec<&str> = all_declared_absent()
            .iter()
            .filter(|(_, site)| {
                matches!(site, CoveredBy::Nowhere(_) | CoveredBy::SelfAttestingSuite)
            })
            .map(|(name, _)| *name)
            .collect();
        unmeasurable.sort_unstable();
        let mut reported: Vec<&str> = ACCOUNTED_ELSEWHERE.iter().map(|(name, _)| *name).collect();
        reported.sort_unstable();
        assert_eq!(
            unmeasurable, reported,
            "the accepted holes this process cannot measure and the reported ones disagree — name \
             the new one in ACCOUNTED_ELSEWHERE (its card, or the suite whose own run reds), or \
             drop the kind whose coverage this process measures again"
        );
    }

    /// Ruling 280/suite-owns-claim.b: a claim resolved from the TEXT of a file
    /// needs that file's path in the data, so no variant of [`CoveredBy`] may
    /// carry one. The match is the pin — a new variant does not compile until
    /// it is classified here — and the assertion catches the cheaper
    /// regression, a path smuggled into a `Nowhere` reason. The paths in
    /// `ACCOUNTED_ELSEWHERE` are deliberate: no check resolves them.
    #[test]
    fn no_coverage_claim_carries_a_site_path() {
        for (kind, site) in all_declared_absent() {
            let prose = match site {
                CoveredBy::HandAuthored | CoveredBy::SelfAttestingSuite => continue,
                CoveredBy::Nowhere(why) => why,
            };
            assert!(
                !prose.contains(".rs"),
                "{kind}: its claim carries the path `{prose}` — a claim this process cannot \
                 measure is reported in ACCOUNTED_ELSEWHERE, never resolved against a file's text"
            );
        }
    }

    /// The exclusion side: a hole whose corpus cases the gate really runs
    /// resolves. `HoldDispatch` is drawn by 10 steps, none quarantined.
    #[test]
    fn a_hole_whose_hand_authored_cases_the_gate_runs_resolves() {
        resolve_coverage("HoldDispatch", CoveredBy::HandAuthored)
            .expect("the hand-authored recipe runs the cases that draw it");
    }

    /// The attestation's teeth: the suite's claim is the COUNTER, so vouching
    /// for a kind the run never drew reds in the suite's own process.
    #[test]
    #[should_panic(expected = "its run drew it 0 times")]
    fn a_suite_attesting_a_kind_it_never_drew_reds() {
        attest_drew("StructuralKeyBurst");
    }

    /// holon-work#441: the vouching suite applies its transitions on a worker
    /// thread, so the draw is counted THERE while `attest_drew` runs on the
    /// test thread. The counter is the suite's, not the thread's.
    #[test]
    fn a_suite_that_draws_on_a_worker_thread_still_attests() {
        std::thread::spawn(|| note_drawn("WorkerThreadDraw"))
            .join()
            .expect("the drawing thread must not panic");
        attest_drew("WorkerThreadDraw");
    }
}
