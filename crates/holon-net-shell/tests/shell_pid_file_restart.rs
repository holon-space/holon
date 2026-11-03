//! With no write held, any sequence of starts, child ends, abandons, dropped
//! runs and crash-restarts leaves a pid file that a restart reads as exactly
//! the groups the runner still runs: every register and forget write lands,
//! and a dropped run's group is killed and leaves the listing.

mod pid_file_support;

use std::collections::BTreeSet;
use std::time::Duration;

use holon_net_shell::effect::Group;
use pid_file_support::Answer;
use pid_file_support::BOUND;
use pid_file_support::Caller;
use pid_file_support::ChildPlan;
use pid_file_support::Flavor;
use pid_file_support::Lab;
use pid_file_support::Run;
use pid_file_support::SETTLE;
use pid_file_support::abandoned;
use pid_file_support::alive;
use pid_file_support::caller;
use pid_file_support::child_plan;
use pid_file_support::dropped_group_dies;
use pid_file_support::ended;
use pid_file_support::eventually;
use pid_file_support::flavor;
use pid_file_support::held_plan;
use pid_file_support::read_back;
use pid_file_support::record;
use pid_file_support::records;
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::Config;

const MAX_RUNS: usize = 6;
/// Attempt ids come from a small range, so two runs sometimes share one.
const ATTEMPTS: u64 = 12;
/// Two in three starts draw from this smaller range, so two running runs
/// often share an attempt when one of them is abandoned.
const CROWDED: u64 = 1;
/// Delays before a just-started run is dropped, so the drop lands before the
/// run's log exists, while it is being created, or after the group spawned.
const DROP_DELAYS: [Duration; 5] = [
    Duration::ZERO,
    Duration::from_millis(1),
    Duration::from_millis(5),
    Duration::from_millis(20),
    Duration::from_millis(100),
];

#[derive(Debug, Clone)]
enum Target {
    Run(Index),
    Unknown(u64),
}

#[derive(Debug, Clone)]
enum Op {
    /// Starts runs back to back, so their register writes overlap.
    Start(Vec<(u64, ChildPlan)>),
    /// Lets held children end together, so their forget writes overlap.
    Tell(Vec<Index>),
    Abandon(Caller, Target),
    /// Drops a running run's future once its group is listed.
    Drop(Index),
    /// Starts a held run and drops its future after one of `DROP_DELAYS`,
    /// wherever it waits then.
    DropAtStart(u64, ChildPlan, usize),
    Await(Index),
    InFlight(Caller),
    Restart,
}

fn start_attempt() -> impl Strategy<Value = u64> {
    prop_oneof![2 => 0..CROWDED, 1 => 0..ATTEMPTS]
}

fn op() -> impl Strategy<Value = Op> {
    let target = prop_oneof![
        any::<Index>().prop_map(Target::Run),
        any::<u64>()
            .prop_filter("an attempt no run has", |a| *a >= ATTEMPTS)
            .prop_map(Target::Unknown),
    ];
    prop_oneof![
        3 => prop::collection::vec((start_attempt(), child_plan()), 1..=3).prop_map(Op::Start),
        2 => prop::collection::vec(any::<Index>(), 1..=4).prop_map(Op::Tell),
        2 => (caller(), target).prop_map(|(c, t)| Op::Abandon(c, t)),
        1 => any::<Index>().prop_map(Op::Drop),
        1 => (0..ATTEMPTS, held_plan(), 0..DROP_DELAYS.len())
            .prop_map(|(a, plan, delay)| Op::DropAtStart(a, plan, delay)),
        1 => any::<Index>().prop_map(Op::Await),
        1 => caller().prop_map(Op::InFlight),
        1 => Just(Op::Restart),
    ]
}

#[derive(Debug, Clone)]
struct Scenario {
    flavor: Flavor,
    ops: Vec<Op>,
}

fn scenario() -> impl Strategy<Value = Scenario> {
    (flavor(), prop::collection::vec(op(), 1..=14))
        .prop_map(|(flavor, ops)| Scenario { flavor, ops })
}

#[derive(Debug, Clone)]
struct RunModel {
    attempt: u64,
    plan: ChildPlan,
    told: bool,
    abandoned: bool,
    dropped: bool,
    checked: bool,
}

impl RunModel {
    fn listed(&self) -> bool {
        self.plan.held && !self.told && !self.abandoned && !self.dropped
    }
}

#[derive(Default)]
struct Model {
    runs: Vec<RunModel>,
}

impl Model {
    /// Which of two runs that share an attempt the runner keeps under it is
    /// not this test's question; their records are left out of the
    /// comparison.
    fn shared(&self, attempt: u64) -> bool {
        self.runs.iter().filter(|r| r.attempt == attempt).count() > 1
    }

    fn listed_attempt(&self, attempt: u64) -> bool {
        self.runs.iter().any(|r| r.attempt == attempt && r.listed())
    }

    /// `groups` names the runs the model lists; unless `exact`, also any run
    /// that is ending and whose result was not taken yet, since its forget
    /// write may still be ahead.
    fn check(&self, groups: &[Group], exact: bool) -> Result<(), String> {
        let got: BTreeSet<_> = records(groups)
            .into_iter()
            .filter(|(a, _, _)| !self.shared(*a))
            .collect();
        let of = |keep: &dyn Fn(&RunModel) -> bool| -> BTreeSet<_> {
            self.runs
                .iter()
                .filter(|r| !self.shared(r.attempt) && keep(r))
                .map(|r| record(r.attempt, &r.plan))
                .collect()
        };
        let want = of(&|r| r.listed());
        let ending = if exact {
            BTreeSet::new()
        } else {
            of(&|r| !r.listed() && !r.checked)
        };
        if want.is_subset(&got) && got.difference(&want).all(|g| ending.contains(g)) {
            Ok(())
        } else {
            Err(format!(
                "names {got:?}, expected {want:?}, and maybe of the ending runs {ending:?}"
            ))
        }
    }
}

/// Every group a listing names that still runs is in the pid file.
///
/// A shared attempt is left out: a later run's register write of that
/// attempt replaces the listed group's record between the listing and the read.
fn listed_are_recorded(lab: &Lab, model: &Model, groups: &[Group]) -> Result<(), String> {
    let Some(left) = lab.restart_read()? else {
        return Ok(());
    };
    for g in groups {
        if !read_back(&left, g) && alive(g.pgid) {
            // A replaced or forgotten group leaves the listing for good, so a
            // group still listed after the read was listed and unrecorded at once.
            if model.shared(g.attempt.0) {
                let Answer::InTime(now) = lab.in_flight(Caller::Thread).within(BOUND) else {
                    continue;
                };
                if !now.contains(g) {
                    continue;
                }
            }
            return Err(format!(
                "in_flight lists {g:?}, which a restart does not read from the pid file ({left:?})"
            ));
        }
    }
    Ok(())
}

/// `in_flight` comes to name the runs the model lists, and every answer names
/// only recorded groups.
fn settle(lab: &Lab, model: &Model, when: &str) -> Result<(), String> {
    eventually(|| {
        let Answer::InTime(groups) = lab.in_flight(Caller::Thread).within(BOUND) else {
            return Err(format!("in_flight {when} did not answer within {BOUND:?}"));
        };
        lab.note_seen(&groups);
        listed_are_recorded(lab, model, &groups).map_err(|e| format!("in_flight {when}: {e}"))?;
        model
            .check(&groups, true)
            .map_err(|e| format!("in_flight {when} {e}"))
    })
}

/// A crash-restart read names the groups the model lists, each with the
/// process group `in_flight` named for it.
fn durable(lab: &Lab, model: &Model, exact: bool, when: &str) -> Result<(), String> {
    eventually(|| {
        if let Answer::InTime(groups) = lab.in_flight(Caller::Thread).within(BOUND) {
            lab.note_seen(&groups);
        }
        let Some(left) = lab.restart_read()? else {
            return Err("the pid file is being replaced".to_owned());
        };
        for g in &left {
            if !model.shared(g.attempt.0) && lab.pgid(g.attempt.0) != Some(g.pgid) {
                return Err(format!(
                    "a restart {when} reads {g:?}, which in_flight named with the group {:?}",
                    lab.pgid(g.attempt.0)
                ));
            }
        }
        model
            .check(&left, exact)
            .map_err(|e| format!("a restart {when} {e}"))
    })
}

fn check_end(run: &Run, model: &mut Model, i: usize) -> Option<String> {
    let m = &model.runs[i];
    let (attempt, abandoned_shared) = (m.attempt, m.abandoned && model.shared(m.attempt));
    let want = if m.abandoned {
        abandoned()
    } else {
        ended(m.plan.exit)
    };
    model.runs[i].checked = true;
    // The later register of a shared attempt replaces the earlier entry, so
    // the abandon may have ended the other run; this child then runs until it
    // is told.
    if abandoned_shared {
        run.tell();
        model.runs[i].told = true;
    }
    let result = run.result(SETTLE);
    match result {
        None => Some(format!(
            "run {attempt} has no result after {SETTLE:?}: its task panicked or did not end"
        )),
        Some(_) if abandoned_shared => None,
        Some(Ok(finished)) if finished.ended == want => None,
        Some(got) => Some(format!("run {attempt} returned {got:?}, expected {want:?}")),
    }
}

fn play(s: &Scenario) -> Vec<String> {
    let lab = Lab::new(s.flavor);
    let mut model = Model::default();
    let mut runs: Vec<Run> = Vec::new();
    let mut failures = Vec::new();

    for (n, op) in s.ops.iter().enumerate() {
        let when = format!("after op {n} {op:?}");
        match op {
            Op::Start(starts) => {
                for (attempt, plan) in starts {
                    if runs.len() == MAX_RUNS {
                        break;
                    }
                    runs.push(lab.start(*attempt, plan.clone()));
                    model.runs.push(RunModel {
                        attempt: *attempt,
                        plan: plan.clone(),
                        told: false,
                        abandoned: false,
                        dropped: false,
                        checked: false,
                    });
                }
                // An instant child ends at once; taking its result here keeps
                // "still running" out of the timing.
                for (i, run) in runs.iter().enumerate() {
                    if !model.runs[i].plan.held && !model.runs[i].checked {
                        failures.extend(check_end(run, &mut model, i));
                    }
                }
            }
            Op::Tell(picks) => {
                for pick in picks {
                    if runs.is_empty() {
                        break;
                    }
                    let i = pick.index(runs.len());
                    // A dropped run's group is left to the restart.
                    if model.runs[i].dropped {
                        continue;
                    }
                    model.runs[i].told = true;
                    runs[i].tell();
                }
            }
            Op::Abandon(caller, target) => {
                let attempt = match target {
                    Target::Run(pick) if !runs.is_empty() => {
                        model.runs[pick.index(runs.len())].attempt
                    }
                    Target::Run(_) => continue,
                    Target::Unknown(a) => *a,
                };
                let want = model.listed_attempt(attempt);
                match lab.abandon(*caller, attempt).within(SETTLE) {
                    Answer::InTime(got) => {
                        if !model.shared(attempt) && got != want {
                            failures.push(format!(
                                "abandon({attempt}) {when} answered {got}, expected {want}"
                            ));
                        }
                        if got {
                            for r in model.runs.iter_mut().filter(|r| r.attempt == attempt) {
                                r.abandoned = true;
                            }
                        }
                    }
                    Answer::Late(_) => {
                        failures.push(format!("abandon({attempt}) {when} never answered"))
                    }
                }
            }
            Op::Drop(pick) => {
                if runs.is_empty() {
                    continue;
                }
                let i = pick.index(runs.len());
                let m = &model.runs[i];
                if !m.listed() || m.dropped || model.shared(m.attempt) {
                    continue;
                }
                runs[i].drop_future();
                model.runs[i].dropped = true;
                model.runs[i].checked = true;
                if let Err(e) = dropped_group_dies(&runs[i]) {
                    failures.push(format!("{when}: {e}"));
                }
            }
            Op::DropAtStart(attempt, plan, delay) => {
                let taken = |a: u64| model.runs.iter().any(|r| r.attempt == a);
                let later = (0..ATTEMPTS).find(|a| a != attempt && !taken(*a));
                let Some(later) = later.filter(|_| !taken(*attempt) && runs.len() + 2 <= MAX_RUNS)
                else {
                    continue;
                };
                let run = lab.start(*attempt, plan.clone());
                std::thread::sleep(DROP_DELAYS[*delay]);
                run.drop_future();
                if let Err(e) = dropped_group_dies(&run) {
                    failures.push(format!("{when}: {e}"));
                    runs.push(run);
                    break;
                }
                // Once the dropped task has ended, the register and forget
                // writes of a later run carry every entry it made.
                let instant = ChildPlan {
                    held: false,
                    ..plan.clone()
                };
                runs.push(lab.start(later, instant.clone()));
                model.runs.push(RunModel {
                    attempt: later,
                    plan: instant,
                    told: false,
                    abandoned: false,
                    dropped: false,
                    checked: false,
                });
                let i = runs.len() - 1;
                failures.extend(check_end(&runs[i], &mut model, i));
                model.runs.push(RunModel {
                    attempt: *attempt,
                    plan: plan.clone(),
                    told: false,
                    abandoned: false,
                    dropped: true,
                    checked: true,
                });
                runs.push(run);
            }
            Op::Await(pick) => {
                if runs.is_empty() {
                    continue;
                }
                let i = pick.index(runs.len());
                let m = &model.runs[i];
                if m.checked || m.dropped || m.listed() {
                    continue;
                }
                failures.extend(check_end(&runs[i], &mut model, i));
            }
            Op::InFlight(caller) => match lab.in_flight(*caller).within(SETTLE) {
                Answer::InTime(groups) => {
                    lab.note_seen(&groups);
                    if let Err(e) = listed_are_recorded(&lab, &model, &groups).and_then(|()| {
                        model
                            .check(&groups, false)
                            .map_err(|e| format!("in_flight {e}"))
                    }) {
                        failures.push(format!("{when}: {e}"));
                    }
                }
                Answer::Late(_) => failures.push(format!("in_flight {when} never answered")),
            },
            Op::Restart => {
                if let Err(e) = durable(&lab, &model, false, &when) {
                    failures.push(e);
                }
            }
        }
        if let Err(e) = settle(&lab, &model, &when) {
            failures.push(e);
        }
        if !failures.is_empty() {
            break;
        }
    }

    if !failures.is_empty() {
        for run in &runs {
            run.tell();
        }
        return failures;
    }
    for (i, run) in runs.iter().enumerate() {
        if !model.runs[i].dropped {
            run.tell();
            model.runs[i].told = true;
        }
    }
    for (i, run) in runs.iter().enumerate() {
        if !model.runs[i].checked && !model.runs[i].dropped {
            failures.extend(check_end(run, &mut model, i));
        }
    }
    if let Err(e) = settle(&lab, &model, "after every run ended") {
        failures.push(e);
    }
    // A child that started only after its run's drop returned is dead by now too.
    for (i, run) in runs.iter().enumerate() {
        if model.runs[i].dropped {
            failures.extend(dropped_group_dies(run).err());
        }
    }
    if let Err(e) = durable(&lab, &model, true, "after every run ended") {
        failures.push(e);
    }
    for run in &runs {
        run.tell();
    }
    failures
}

proptest! {
    #![proptest_config(Config {
        cases: 64,
        max_shrink_iters: 64,
        // A failing case waits out its eventual checks; nextest ends a test
        // at two minutes.
        max_shrink_time: 45_000,
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn a_restart_reads_exactly_the_groups_that_still_run_after_any_sequence_of_runs(
        s in scenario()
    ) {
        let failures = play(&s);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
