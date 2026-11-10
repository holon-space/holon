//! While one pid-file write of a runner is held, `in_flight`, `abandon` and the
//! runner's tokio runtime answer within the bound, and they answer what the
//! pid file holds: a group is listed only once its record is written. After
//! the release every run ends as its child and the abandons say, a dropped
//! run's group is killed and never listed, and a restart reads the groups
//! that still run.

mod pid_file_support;

use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Group;
use holon_net_shell::effect::ShellError;
use pid_file_support::Answer;
use pid_file_support::BOUND;
use pid_file_support::Caller;
use pid_file_support::ChildPlan;
use pid_file_support::Flavor;
use pid_file_support::Hold;
use pid_file_support::Lab;
use pid_file_support::Pending;
use pid_file_support::Release;
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
use pid_file_support::release;
use proptest::prelude::*;
use proptest::test_runner::Config;

const BEFORE: u64 = 1;
const MAX_BEFORE: usize = 3;
const STALLED: u64 = 100;
const DURING: u64 = 200;
const MAX_DURING: usize = 3;

/// Which write the hold catches.
#[derive(Debug, Clone)]
enum Held {
    /// The register write of a new run with this held child.
    Register(ChildPlan),
    /// The forget write of the run from before the hold with this index,
    /// whose held child the test tells to end.
    Forget(usize),
}

#[derive(Debug, Clone)]
enum Target {
    /// The run whose write is held.
    Stalled,
    /// Attempt `BEFORE + i`, which runs only when that many ran before.
    Before(usize),
    /// Attempt `DURING + i`, which runs only when that many started yet.
    During(usize),
    /// An attempt no run of the scenario has.
    Unknown(u64),
}

impl Target {
    fn attempt(&self, stalled: u64) -> u64 {
        match self {
            Target::Stalled => stalled,
            Target::Before(i) => BEFORE + *i as u64,
            Target::During(i) => DURING + *i as u64,
            Target::Unknown(a) => *a,
        }
    }
}

#[derive(Debug, Clone)]
enum Ask {
    InFlight,
    Abandon(Target),
    /// A timer on the runner's runtime.
    Tick,
}

#[derive(Debug, Clone)]
enum Step {
    /// A new run, under a fresh attempt or under the attempt of the run from
    /// before with this index, which a caller can pass as well.
    StartDuring(ChildPlan, Option<usize>),
    /// Tells the held child of the run from before with this index to end.
    Tell(usize),
    Ask(Caller, Ask),
    DropStalled,
}

#[derive(Debug, Clone)]
struct Scenario {
    flavor: Flavor,
    before: Vec<ChildPlan>,
    held: Held,
    steps: Vec<Step>,
    release: Release,
    /// Abandons once the held write is released, when a run that started
    /// during the hold may share its attempt with a run from before.
    after: Vec<Target>,
}

fn used(attempt: u64) -> bool {
    (BEFORE..BEFORE + MAX_BEFORE as u64).contains(&attempt)
        || attempt == STALLED
        || (DURING..DURING + MAX_DURING as u64).contains(&attempt)
}

fn target(release: Release) -> BoxedStrategy<Target> {
    let unknown = any::<u64>()
        .prop_filter("an attempt no run has", |a| !used(*a))
        .prop_map(Target::Unknown);
    match release {
        Release::Write => prop_oneof![
            Just(Target::Stalled),
            (0..MAX_BEFORE).prop_map(Target::Before),
            (0..MAX_DURING).prop_map(Target::During),
            unknown,
        ]
        .boxed(),
        // An abandoned run from before writes its forget record while the pid
        // file path is the blocking directory, and the end a run reports when
        // its forget write fails is unspecified.
        Release::FailRename => prop_oneof![
            Just(Target::Stalled),
            (0..MAX_DURING).prop_map(Target::During),
            unknown,
        ]
        .boxed(),
    }
}

fn step(release: Release) -> BoxedStrategy<Step> {
    let ask = prop_oneof![
        Just(Ask::InFlight),
        target(release).prop_map(Ask::Abandon),
        Just(Ask::Tick),
    ];
    let reuse = prop::option::weighted(0.75, 0..MAX_BEFORE);
    let common = prop_oneof![
        3 => (caller(), ask).prop_map(|(c, a)| Step::Ask(c, a)),
        // A held child keeps a reused attempt shared until an abandon.
        2 => (prop_oneof![2 => held_plan(), 1 => child_plan()], reuse)
            .prop_map(|(p, r)| Step::StartDuring(p, r)),
        1 => Just(Step::DropStalled),
    ];
    match release {
        Release::Write => {
            prop_oneof![5 => common, 1 => (0..MAX_BEFORE).prop_map(Step::Tell)].boxed()
        }
        // A told child's forget write would fail the same way.
        Release::FailRename => common.boxed(),
    }
}

fn scenario() -> impl Strategy<Value = Scenario> {
    (
        flavor(),
        release(),
        prop::collection::vec(child_plan(), 0..=MAX_BEFORE),
    )
        .prop_flat_map(|(flavor, release, before)| {
            let held = match release {
                // The end a run reports when its forget write fails is
                // unspecified, so the held write is a register write.
                Release::FailRename => held_plan().prop_map(Held::Register).boxed(),
                Release::Write if before.is_empty() => held_plan().prop_map(Held::Register).boxed(),
                Release::Write => prop_oneof![
                    held_plan().prop_map(Held::Register),
                    (0..before.len()).prop_map(Held::Forget),
                ]
                .boxed(),
            };
            (
                Just(flavor),
                Just(before),
                held,
                prop::collection::vec(step(release), 1..=8),
                Just(release),
                // A run from before is the target that a run started during
                // the hold may share.
                prop::collection::vec(
                    prop_oneof![
                        3 => (0..MAX_BEFORE).prop_map(Target::Before),
                        1 => target(Release::Write),
                    ],
                    1..=3,
                ),
            )
        })
        .prop_map(|(flavor, mut before, held, steps, release, after)| {
            if let Held::Forget(i) = held {
                before[i].held = true;
            }
            Scenario {
                flavor,
                before,
                held,
                steps,
                release,
                after,
            }
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Before,
    Stalled,
    During,
}

#[derive(Debug, Clone)]
struct RunModel {
    attempt: u64,
    plan: ChildPlan,
    kind: Kind,
    told: bool,
    abandoned: bool,
    dropped: bool,
    /// Its register write meets the blocking directory.
    write_fails: bool,
    /// Its result was taken.
    checked: bool,
}

/// What the runs are, from the test's own steps.
#[derive(Default)]
struct Model {
    runs: Vec<RunModel>,
    holding: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Expect {
    End(Ended),
    PidFileError,
    /// Some end, `Ok` or `Err`: the run shares its attempt with another run.
    AnyEnd,
    /// No result: the run's future was dropped.
    Dropped,
}

impl Model {
    /// Two runs share the attempt. The later register replaces the earlier
    /// entry, so which group an abandon ends, and so which end each run
    /// reports, is unspecified.
    fn shared(&self, attempt: u64) -> bool {
        self.runs.iter().filter(|r| r.attempt == attempt).count() > 1
    }

    /// Whether `in_flight` lists the run: its child runs and its record is
    /// written. While the hold lasts, no register write completes.
    fn listed(run: &RunModel, holding: bool) -> bool {
        match run.kind {
            _ if run.dropped => false,
            Kind::Before => run.plan.held && !run.told && !run.abandoned,
            Kind::Stalled | Kind::During if holding => false,
            Kind::Stalled | Kind::During => {
                run.plan.held && !run.told && !run.abandoned && !run.write_fails
            }
        }
    }

    fn listed_attempt(&self, attempt: u64) -> bool {
        self.runs
            .iter()
            .any(|r| r.attempt == attempt && Self::listed(r, self.holding))
    }

    fn expect(&self, run: &RunModel) -> Expect {
        if run.dropped {
            Expect::Dropped
        } else if self.shared(run.attempt) && run.abandoned {
            Expect::AnyEnd
        } else if run.write_fails {
            Expect::PidFileError
        } else if run.abandoned {
            Expect::End(abandoned())
        } else {
            Expect::End(ended(run.plan.exit))
        }
    }

    /// Compares a listing with the model.
    fn check_listing(&self, groups: &[Group]) -> Result<(), String> {
        let got: BTreeSet<_> = groups
            .iter()
            .filter(|g| !self.shared(g.attempt.0))
            .map(|g| (g.attempt.0, g.transition.clone(), g.subject.clone()))
            .collect();
        let want: BTreeSet<_> = self
            .runs
            .iter()
            .filter(|r| !self.shared(r.attempt) && Self::listed(r, self.holding))
            .map(|r| (r.attempt, r.plan.transition.clone(), r.plan.subject.clone()))
            .collect();
        if got != want {
            return Err(format!("listed {got:?}, expected {want:?}"));
        }
        Ok(())
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

/// A call that did not answer within the bound while the write was held.
struct Late {
    what: String,
    pending: LatePending,
}

enum LatePending {
    InFlight(Pending<Vec<Group>>),
    Abandon(u64, Pending<bool>),
    Tick(Pending<()>),
}

enum Halt {
    Late(Late),
    Wrong(String),
}

/// The failure a late call leaves once the hold is gone: its answer time
/// is held to the caller budget, and no answer at all always fails.
fn late_answer(l: Late, lab: &Lab, model: &mut Model, when: &str) -> Option<String> {
    let answered = match l.pending {
        LatePending::InFlight(p) => p.late().map(|(groups, took)| {
            lab.note_seen(&groups);
            took
        }),
        LatePending::Abandon(a, p) => p.late().map(|(got, took)| {
            mark_abandoned(model, a, got);
            took
        }),
        LatePending::Tick(p) => p.late().map(|((), took)| took),
    };
    match answered {
        Some(took) => answer_budget(took).map(|e| format!("{} {when}: {e}", l.what)),
        None => Some(format!("{} never answered", l.what)),
    }
}

/// Every judged call's answer time, in time or late, goes through the caller
/// budget, so a clean run leaves its ledger lines too.
fn answer_budget(took: Duration) -> Option<String> {
    holon_api::duration_budget::DurationBudget::small("pid-file caller answer", BOUND)
        .check(took)
        .err()
        .map(|e| e.to_string())
}

/// Polls `in_flight` until it matches the model; every poll answers within
/// the bound and names only recorded groups.
fn settle(lab: &Lab, model: &Model, when: &str) -> Result<(), Halt> {
    let deadline = Instant::now() + SETTLE;
    loop {
        match lab.in_flight(Caller::Thread).timed_within(BOUND) {
            Err(pending) => {
                return Err(Halt::Late(Late {
                    what: format!("in_flight {when}"),
                    pending: LatePending::InFlight(pending),
                }));
            }
            Ok((groups, took)) => {
                if let Some(e) = answer_budget(took) {
                    return Err(Halt::Wrong(format!("in_flight {when}: {e}")));
                }
                lab.note_seen(&groups);
                listed_are_recorded(lab, model, &groups)
                    .map_err(|e| Halt::Wrong(format!("in_flight {when}: {e}")))?;
                match model.check_listing(&groups) {
                    Ok(()) => return Ok(()),
                    Err(e) if Instant::now() >= deadline => {
                        return Err(Halt::Wrong(format!(
                            "in_flight {when}: {e} (still after {SETTLE:?})"
                        )));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }
}

/// A crash-restart read names the groups the model lists.
fn durable(lab: &Lab, model: &Model, when: &str) -> Result<(), String> {
    eventually(|| {
        if let Answer::InTime(groups) = lab.in_flight(Caller::Thread).within(BOUND) {
            lab.note_seen(&groups);
        }
        let Some(left) = lab.restart_read()? else {
            return Err("the pid file is being replaced".to_owned());
        };
        for g in &left {
            if lab.pgid(g.attempt.0) != Some(g.pgid) && !model.shared(g.attempt.0) {
                return Err(format!(
                    "a restart reads {g:?}, which in_flight named with the group {:?}",
                    lab.pgid(g.attempt.0)
                ));
            }
        }
        model
            .check_listing(&left)
            .map_err(|e| format!("a restart {when}: {e}"))
    })
}

fn check_end(run: &Run, model: &mut Model, i: usize) -> Option<String> {
    let expect = model.expect(&model.runs[i]);
    model.runs[i].checked = true;
    let attempt = model.runs[i].attempt;
    // The abandon may have ended the other run of the attempt; this child then
    // runs until it is told.
    if expect == Expect::AnyEnd {
        run.tell();
        model.runs[i].told = true;
    }
    let result = run.result(SETTLE);
    match (&expect, result) {
        (Expect::Dropped, _) => None,
        (_, None) => Some(format!(
            "run {attempt} has no result after {SETTLE:?}: its task panicked or did not end"
        )),
        (Expect::AnyEnd, Some(_)) => None,
        (Expect::End(want), Some(Ok(finished))) if finished.ended == *want => None,
        (Expect::PidFileError, Some(Err(ShellError::PidFile { .. }))) => None,
        (want, Some(got)) => Some(format!("run {attempt} returned {got:?}, expected {want:?}")),
    }
}

fn new_run(attempt: u64, plan: &ChildPlan, kind: Kind, write_fails: bool) -> RunModel {
    RunModel {
        attempt,
        plan: plan.clone(),
        kind,
        told: false,
        abandoned: false,
        dropped: false,
        write_fails,
        checked: false,
    }
}

fn play(s: &Scenario) -> Vec<String> {
    let lab = Lab::new(s.flavor);
    let mut failures = Vec::new();
    let mut model = Model::default();
    let mut runs: Vec<Run> = Vec::new();

    // One write carries every group inserted before it, so a listing does not
    // show that the later writes are done. Each run starts once the writes of
    // the run before it are done, so the hold catches no write of theirs.
    for (i, plan) in s.before.iter().enumerate() {
        let a = BEFORE + i as u64;
        runs.push(lab.start(a, plan.clone()));
        model.runs.push(new_run(a, plan, Kind::Before, false));
        if plan.held {
            failures.extend(listed_once(&lab, a).err());
        } else {
            failures.extend(check_end(&runs[i], &mut model, i));
        }
    }
    match settle(&lab, &model, "with no write held") {
        Ok(()) => {}
        Err(Halt::Wrong(e)) => failures.push(e),
        Err(Halt::Late(late)) => {
            failures.extend(late_answer(late, &lab, &mut model, "with no write held"));
        }
    }
    if let Err(e) = durable(&lab, &model, "before the hold") {
        failures.push(e);
    }
    if !failures.is_empty() {
        end(&lab, &mut model, &runs, &mut failures);
        return failures;
    }

    let hold = Hold::new(&lab.pid_file);
    let blocked = (s.release == Release::FailRename).then(|| hold.block_rename(&lab));
    model.holding = true;
    let stalled = match &s.held {
        Held::Register(plan) => {
            runs.push(lab.start(STALLED, plan.clone()));
            model.runs.push(new_run(
                STALLED,
                plan,
                Kind::Stalled,
                s.release == Release::FailRename,
            ));
            runs.len() - 1
        }
        Held::Forget(i) => {
            runs[*i].tell();
            model.runs[*i].told = true;
            *i
        }
    };
    // The steps run while the held write is under way.
    let reached = match &s.held {
        Held::Register(_) => runs[stalled].wait_started(),
        Held::Forget(_) => forget_under_way(&lab, runs[stalled].attempt),
    };
    failures.extend(reached.err());
    let what_held = match s.held {
        Held::Register(_) => "a register write",
        Held::Forget(_) => "a forget write",
    };
    let mut late: Vec<Late> = Vec::new();
    let halt = |h: Halt, late: &mut Vec<Late>, failures: &mut Vec<String>| match h {
        Halt::Late(l) => late.push(l),
        Halt::Wrong(e) => failures.push(e),
    };
    if let Err(h) = settle(&lab, &model, &format!("while {what_held} is held")) {
        halt(h, &mut late, &mut failures);
    }
    let mut started_during = 0;
    for step in &s.steps {
        if !late.is_empty() {
            break;
        }
        match step {
            Step::StartDuring(plan, reuse) => {
                if started_during == MAX_DURING {
                    continue;
                }
                let a = match reuse {
                    Some(i) => BEFORE + *i as u64,
                    None => DURING + started_during as u64,
                };
                started_during += 1;
                runs.push(lab.start(a, plan.clone()));
                model.runs.push(new_run(
                    a,
                    plan,
                    Kind::During,
                    s.release == Release::FailRename,
                ));
            }
            Step::Tell(i) => {
                let Some(run) = model.runs.get_mut(*i) else {
                    continue;
                };
                if run.kind != Kind::Before
                    || !run.plan.held
                    || run.told
                    || run.abandoned
                    || run.dropped
                {
                    continue;
                }
                run.told = true;
                runs[*i].tell();
            }
            Step::DropStalled => {
                runs[stalled].drop_future();
                model.runs[stalled].dropped = true;
                if let Err(e) = dropped_group_dies(&runs[stalled]) {
                    failures.push(format!("the drop while {what_held} is held: {e}"));
                }
            }
            Step::Ask(caller, Ask::InFlight) => match lab.in_flight(*caller).timed_within(BOUND) {
                Ok((groups, took)) => {
                    lab.note_seen(&groups);
                    if let Err(e) = listed_are_recorded(&lab, &model, &groups)
                        .and_then(|()| model.check_listing(&groups))
                    {
                        failures.push(format!("in_flight while {what_held} is held: {e}"));
                    }
                    failures.extend(
                        answer_budget(took)
                            .map(|e| format!("in_flight while {what_held} is held: {e}")),
                    );
                }
                Err(pending) => late.push(Late {
                    what: format!("in_flight from a {caller:?}"),
                    pending: LatePending::InFlight(pending),
                }),
            },
            Step::Ask(caller, Ask::Abandon(t)) => {
                let a = t.attempt(model.runs[stalled].attempt);
                let want = model.listed_attempt(a);
                match lab.abandon(*caller, a).timed_within(BOUND) {
                    Ok((got, took)) => {
                        failures.extend(
                            answer_budget(took)
                                .map(|e| format!("abandon({a}) while {what_held} is held: {e}")),
                        );
                        if !model.shared(a) && got != want {
                            failures.push(format!(
                                "abandon({a}) while {what_held} is held answered {got}, expected {want}"
                            ));
                        }
                        mark_abandoned(&mut model, a, got);
                    }
                    Err(pending) => late.push(Late {
                        what: format!("abandon({a}) from a {caller:?}"),
                        pending: LatePending::Abandon(a, pending),
                    }),
                }
            }
            Step::Ask(_, Ask::Tick) => match lab.tick().timed_within(BOUND) {
                Ok(((), took)) => failures.extend(
                    answer_budget(took)
                        .map(|e| format!("a 1 ms timer while {what_held} is held: {e}")),
                ),
                Err(pending) => late.push(Late {
                    what: "a 1 ms timer on the runner's runtime".to_owned(),
                    pending: LatePending::Tick(pending),
                }),
            },
        }
        if late.is_empty()
            && let Err(h) = settle(&lab, &model, &format!("while {what_held} is held"))
        {
            halt(h, &mut late, &mut failures);
        }
    }

    let drained = match hold.release(s.release) {
        Ok(drained) => {
            if s.release == Release::Write {
                *lab.stand_in.lock().unwrap() = Some(drained.first.clone());
            }
            Some(drained)
        }
        Err(e) => {
            failures.push(e);
            None
        }
    };
    model.holding = false;
    for l in late {
        failures.extend(late_answer(
            l,
            &lab,
            &mut model,
            &format!("while {what_held} was held"),
        ));
    }

    if let Some(blocked) = blocked {
        for (i, run) in runs.iter().enumerate() {
            let m = &model.runs[i];
            if !m.write_fails {
                continue;
            }
            let a = m.attempt;
            if !m.dropped {
                failures.extend(check_end(run, &mut model, i));
            }
            // A run whose record failed leaves no group, dropped or not: no
            // restart could find it. A shared attempt names another run's
            // group too.
            if !model.shared(a)
                && let Some(pgid) = lab.pgid(a)
                && let Err(e) = eventually(|| {
                    if alive(pgid) {
                        Err(format!(
                            "the group of run {a}, whose record failed, is alive"
                        ))
                    } else {
                        Ok(())
                    }
                })
            {
                failures.push(e);
            }
        }
        if let Some(drained) = drained {
            drained.stop();
        }
        blocked.restore(&lab);
    }
    if let Err(Halt::Wrong(e)) = settle(&lab, &model, "after the release") {
        failures.push(e);
    }
    if let Err(e) = durable(&lab, &model, "after the release") {
        failures.push(e);
    }
    // A run that ends on its own is listed for a moment after its register
    // write; once its result is in, only an abandon changes a listing.
    for (i, run) in runs.iter().enumerate() {
        let m = &model.runs[i];
        if !m.checked && !m.dropped && !Model::listed(m, false) {
            failures.extend(check_end(run, &mut model, i));
        }
    }
    for t in &s.after {
        let a = t.attempt(model.runs[stalled].attempt);
        let want = model.listed_attempt(a);
        match lab.abandon(Caller::Thread, a).timed_within(BOUND) {
            Ok((got, took)) => {
                failures.extend(
                    answer_budget(took).map(|e| format!("abandon({a}) after the release: {e}")),
                );
                if !model.shared(a) && got != want {
                    failures.push(format!(
                        "abandon({a}) after the release answered {got}, expected {want}"
                    ));
                }
                mark_abandoned(&mut model, a, got);
            }
            Err(pending) => failures.extend(late_answer(
                Late {
                    what: format!("abandon({a})"),
                    pending: LatePending::Abandon(a, pending),
                },
                &lab,
                &mut model,
                "after the release",
            )),
        }
        if let Err(Halt::Wrong(e)) = settle(&lab, &model, &format!("after abandon({a})")) {
            failures.push(e);
        }
    }
    end(&lab, &mut model, &runs, &mut failures);
    failures
}

/// Waits until `in_flight` lists the run: when no other write is under way,
/// its register write is then done.
fn listed_once(lab: &Lab, attempt: u64) -> Result<(), String> {
    eventually(|| match lab.in_flight(Caller::Thread).within(BOUND) {
        Answer::InTime(groups) if groups.iter().any(|g| g.attempt.0 == attempt) => {
            lab.note_seen(&groups);
            Ok(())
        }
        _ => Err(format!("run {attempt} is not listed")),
    })
}

/// The run's forget write is under way once `in_flight` stops listing it, or
/// once `in_flight` waits for the write.
fn forget_under_way(lab: &Lab, attempt: u64) -> Result<(), String> {
    eventually(|| match lab.in_flight(Caller::Thread).within(BOUND) {
        Answer::InTime(groups) if groups.iter().any(|g| g.attempt.0 == attempt) => Err(format!(
            "run {attempt} is still listed after its child was told to end"
        )),
        Answer::InTime(_) | Answer::Late(_) => Ok(()),
    })
}

fn mark_abandoned(model: &mut Model, attempt: u64, got: bool) {
    if !got {
        return;
    }
    for run in model.runs.iter_mut().filter(|r| r.attempt == attempt) {
        run.abandoned = true;
    }
}

/// Tells every child that is not dropped to end, checks each run's end, that
/// every dropped run's group is dead, and that nothing is listed or recorded.
fn end(lab: &Lab, model: &mut Model, runs: &[Run], failures: &mut Vec<String>) {
    for (i, run) in runs.iter().enumerate() {
        if !model.runs[i].dropped {
            run.tell();
        }
    }
    for (i, run) in runs.iter().enumerate() {
        if !model.runs[i].checked {
            failures.extend(check_end(run, model, i));
        }
        model.runs[i].told = true;
        if model.runs[i].dropped {
            failures.extend(dropped_group_dies(run).err());
        }
    }
    if let Err(Halt::Wrong(e)) = settle(lab, model, "after every run ended") {
        failures.push(e);
    }
    if let Err(e) = durable(lab, model, "after every run ended") {
        failures.push(e);
    }
    for run in runs {
        run.tell();
    }
}

proptest! {
    #![proptest_config(Config {
        cases: 96,
        max_shrink_iters: 48,
        // A failing case waits out its eventual checks; nextest ends a test
        // at two minutes.
        max_shrink_time: 45_000,
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn in_flight_abandon_and_the_runtime_answer_within_the_bound_while_a_pid_file_write_is_held(
        s in scenario()
    ) {
        let failures = play(&s);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

proptest! {
    #![proptest_config(Config {
        cases: 8,
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn every_played_scenario_leaves_a_ledger_line_of_its_answer_budget(s in scenario()) {
        let ledger = std::env::temp_dir()
            .join(format!("held-write-ledger-{}.jsonl", std::process::id()));
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var(holon_api::timing_mode::LEDGER_ENV, &ledger) };
        let lines = || std::fs::read_to_string(&ledger).map_or(0, |t| t.lines().count());
        let before = lines();
        let failures = play(&s);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
        prop_assert!(
            lines() > before,
            "a played scenario judged its caller answers against BOUND but left no ledger line"
        );
    }
}

proptest! {
    #![proptest_config(Config {
        cases: 16,
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn every_in_time_answer_a_caller_takes_leaves_its_own_ledger_line(s in scenario()) {
        use std::sync::atomic::Ordering;
        let ledger = std::env::temp_dir()
            .join(format!("held-write-per-answer-ledger-{}.jsonl", std::process::id()));
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var(holon_api::timing_mode::LEDGER_ENV, &ledger) };
        let lines = || std::fs::read_to_string(&ledger).map_or(0, |t| t.lines().count());
        let before = lines();
        let taken = pid_file_support::TIMED_IN_TIME.load(Ordering::SeqCst);
        let failures = play(&s);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
        let judged = pid_file_support::TIMED_IN_TIME.load(Ordering::SeqCst) - taken;
        let written = lines() - before;
        prop_assert!(
            written >= judged,
            "{judged} in-time answers were taken with their elapsed time but only {written} ledger lines were written"
        );
    }
}
