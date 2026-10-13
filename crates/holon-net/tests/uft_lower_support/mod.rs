//! The reference model of the lowered `AcceptRemoval` (#167): per subject the
//! situation, the waiting answer, the work in flight, the failure port, the
//! failure mark and the disclosed expiries. It knows nothing of hashes: an
//! ask is identified by the situation content it shows.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

pub const SUBJECTS: [&str; 2] = ["a", "b"];

/// (why, base, ondisk): the situation content the ask hash covers.
pub type Sit = (u32, u32, u32);

/// The answer values; `None` is an ask answered without a value.
pub const VALUES: [Option<&str>; 3] = [Some("Remove"), Some("Keep"), None];

#[derive(Clone, Debug, PartialEq)]
pub struct Ans {
    pub id: u32,
    pub value: Option<&'static str>,
    pub seen: Sit,
    /// Answered right after a failed work of this subject.
    pub after_failure: bool,
}

#[derive(Clone, Debug)]
pub struct Flight {
    pub ans: Ans,
    pub at: Sit,
    pub changed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Subj {
    pub refused: Option<u32>,
    pub base: u32,
    pub ondisk: u32,
    pub answered: Option<Ans>,
    pub flight: Option<Flight>,
    pub failed_port: bool,
    pub removed: bool,
    /// The failed attempt's situation and answer: the answer is in the key.
    pub mark: Option<(Sit, Ans)>,
    pub last_failed: bool,
    /// Every situation this subject showed as an ask, oldest first.
    pub history: Vec<Sit>,
}

impl Subj {
    /// The ask's situation: `Refused` holds and no work runs (a take empties
    /// `Refused`, so the flight hides the ask by itself).
    pub fn sit(&self) -> Option<Sit> {
        Some((self.refused?, self.base, self.ondisk))
    }

    /// The situation the ask shows: only while the subject is idle, so no
    /// offered ask has a dead `Answer` (D151.a).
    pub fn offered(&self) -> Option<Sit> {
        let idle = self.answered.is_none() && self.flight.is_none() && !self.failed_port;
        self.sit().filter(|_| idle)
    }

    fn seen_now(&mut self) {
        if let Some(sit) = self.offered()
            && self.history.last() != Some(&sit)
        {
            self.history.push(sit);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    AlreadyAnswered,
    NoLongerApplies,
}

#[derive(Clone, Debug)]
pub struct Model {
    /// The work reads `Refused` instead of taking it, so the situation stays
    /// while the work runs.
    pub read_only: bool,
    pub subj: Vec<Subj>,
    pub next: u32,
    /// answer id → the disclosed `why` of its expiry.
    pub expired: BTreeMap<u32, &'static str>,
}

#[derive(Clone, Copy, Debug)]
pub enum Seen {
    Current,
    /// An older situation of the subject's history.
    Stale(usize),
}

#[derive(Clone, Debug)]
pub enum Op {
    ChangeBase(usize),
    ChangeOnDisk(usize),
    /// The editor's save consumes `Refused`: the ask goes away.
    Saved(usize),
    Reappear(usize),
    Answer {
        s: usize,
        value: Option<&'static str>,
        seen: Seen,
    },
    Step,
    Complete {
        s: usize,
        ok: bool,
    },
    /// The work's outcome is unknown, then the user drops it.
    Lost(usize),
}

pub const EXPIRED_GONE: &str = "the question no longer applies";
pub const EXPIRED_CHANGED: &str = "the situation changed";

impl Model {
    /// The situation an answer claims to have seen; `None` when the subject
    /// never showed an ask.
    pub fn seen_sit(&self, s: usize, seen: Seen) -> Option<Sit> {
        let sj = &self.subj[s];
        match seen {
            Seen::Stale(i) if !sj.history.is_empty() => Some(sj.history[i % sj.history.len()]),
            _ => sj.offered(),
        }
    }

    /// The reply to an answer. The value never matters (UFT-adopt.a).
    pub fn answer_result(&self, s: usize) -> Result<(), Refusal> {
        let sj = &self.subj[s];
        if sj.answered.is_some() || sj.flight.is_some() {
            return Err(Refusal::AlreadyAnswered);
        }
        if sj.offered().is_none() {
            return Err(Refusal::NoLongerApplies);
        }
        Ok(())
    }

    /// Runs the automatic transitions to quiescence; returns the answer ids
    /// whose work started.
    pub fn step(&mut self) -> Vec<u32> {
        let mut started = Vec::new();
        for sj in &mut self.subj {
            let Some(ans) = sj.answered.clone() else {
                sj.failed_port = false;
                continue;
            };
            let sit = sj.sit();
            if let Some(cur) = sit
                && cur == ans.seen
                && sj.mark != Some((cur, ans.clone()))
            {
                sj.flight = Some(Flight {
                    ans: ans.clone(),
                    at: cur,
                    changed: false,
                });
                if !self.read_only {
                    sj.refused = None;
                }
                sj.answered = None;
                sj.last_failed = false;
                started.push(ans.id);
            } else if sj.failed_port {
                sj.answered = None;
                sj.failed_port = false;
            } else if sit.is_none() {
                sj.answered = None;
                self.expired.insert(ans.id, EXPIRED_GONE);
            } else if sit != Some(ans.seen) {
                sj.answered = None;
                self.expired.insert(ans.id, EXPIRED_CHANGED);
            } else {
                panic!("model: ask and Answered present but the work is not enabled");
            }
        }
        started
    }

    fn complete(&mut self, s: usize, ok: bool) {
        let sj = &mut self.subj[s];
        let f = sj.flight.take().expect("precondition: in flight");
        if ok {
            sj.mark = None;
            sj.removed |= !f.changed;
        } else {
            if !self.read_only {
                sj.refused = Some(f.at.0);
            }
            sj.mark = Some((f.at, f.ans.clone()));
            sj.answered = Some(f.ans);
            sj.failed_port = true;
            sj.last_failed = true;
        }
    }

    /// The outcome stayed unknown and the user dropped it: the take is
    /// committed and the failure port gets a token without an `Answered`.
    fn drop_lost(&mut self, s: usize) {
        let sj = &mut self.subj[s];
        let f = sj.flight.take().expect("precondition: in flight");
        sj.mark = Some((f.at, f.ans));
        sj.failed_port = true;
        sj.last_failed = true;
    }
}

pub struct Ref;

impl ReferenceStateMachine for Ref {
    type State = Model;
    type Transition = Op;

    fn init_state() -> BoxedStrategy<Model> {
        let mut subj = vec![Subj::default(); SUBJECTS.len()];
        for sj in &mut subj {
            sj.refused = Some(0);
            sj.seen_now();
        }
        any::<bool>()
            .prop_map(move |read_only| Model {
                read_only,
                subj: subj.clone(),
                next: 1,
                expired: BTreeMap::new(),
            })
            .boxed()
    }

    fn transitions(_: &Model) -> BoxedStrategy<Op> {
        let s = 0..SUBJECTS.len();
        let seen = prop_oneof![
            3 => Just(Seen::Current),
            1 => (0usize..4).prop_map(Seen::Stale),
        ];
        prop_oneof![
            2 => s.clone().prop_map(Op::ChangeBase),
            1 => s.clone().prop_map(Op::ChangeOnDisk),
            1 => s.clone().prop_map(Op::Saved),
            1 => s.clone().prop_map(Op::Reappear),
            5 => (s.clone(), prop::sample::select(VALUES.to_vec()), seen)
                .prop_map(|(s, value, seen)| Op::Answer { s, value, seen }),
            5 => Just(Op::Step),
            3 => (s.clone(), any::<bool>()).prop_map(|(s, ok)| Op::Complete { s, ok }),
            1 => s.prop_map(Op::Lost),
        ]
        .boxed()
    }

    fn preconditions(m: &Model, op: &Op) -> bool {
        match op {
            Op::Reappear(s) => m.subj[*s].refused.is_none() && m.subj[*s].flight.is_none(),
            Op::Complete { s, .. } | Op::Lost(s) => m.subj[*s].flight.is_some(),
            Op::Answer { s, seen, .. } => m.seen_sit(*s, *seen).is_some(),
            _ => true,
        }
    }

    fn apply(mut m: Model, op: &Op) -> Model {
        let tick = m.next;
        match *op {
            Op::ChangeBase(s) => {
                m.subj[s].base = tick;
                if let Some(f) = &mut m.subj[s].flight {
                    f.changed = true;
                }
            }
            Op::ChangeOnDisk(s) => {
                m.subj[s].ondisk = tick;
                if let Some(f) = &mut m.subj[s].flight {
                    f.changed = true;
                }
            }
            Op::Saved(s) => {
                let fired = m.subj[s].refused.take().is_some();
                if let Some(f) = m.subj[s].flight.as_mut().filter(|_| fired) {
                    f.changed = true;
                }
            }
            Op::Reappear(s) => m.subj[s].refused = Some(tick),
            Op::Answer { s, value, seen } => {
                if m.answer_result(s).is_ok() {
                    m.subj[s].answered = Some(Ans {
                        id: tick,
                        value,
                        seen: m.seen_sit(s, seen).expect("precondition"),
                        after_failure: m.subj[s].last_failed,
                    });
                }
            }
            Op::Step => {
                m.step();
            }
            Op::Complete { s, ok } => m.complete(s, ok),
            Op::Lost(s) => m.drop_lost(s),
        }
        m.next += 1;
        for sj in &mut m.subj {
            sj.seen_now();
        }
        m
    }
}
