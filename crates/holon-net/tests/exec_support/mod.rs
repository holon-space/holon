//! Shared driver of the executor PBTs: it plays the effect runner and the
//! clock, runs every operation on the executor and on the reference model,
//! and fails on the first divergence. It picks the naive twin named by
//! `HOLON_NET_TWIN`.

#![allow(dead_code)]

pub mod model;

mod twin_names;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::time::Duration;

use holon_api::Value;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::UftError;
use holon_net::exec::Accepted;
use holon_net::exec::AskMeta;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Capacity;
use holon_net::exec::Class;
use holon_net::exec::Completion;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::Executor;
use holon_net::exec::FireError;
use holon_net::exec::NetDef;
use holon_net::exec::OUTCOME_UNKNOWN;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Quiesced;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Retry;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::exec::UndoError;
use holon_net::exec::Undone;
use holon_net::exec::WhyNot;
use model::Model;

/// The marking as a guard, a binding or a derived place reads it: the
/// executor's [`Reader`] or the model's.
pub trait Reads {
    fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<Value>;
    fn present(&mut self, place: &str, subject: &str) -> bool;

    /// A field as a string; `None` when absent or not a string.
    fn text(&mut self, place: &str, subject: &str, field: &str) -> Option<String> {
        match self.field(place, subject, field)? {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

impl Reads for Reader<'_> {
    fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<Value> {
        Reader::field(self, place, subject, field)
    }

    fn present(&mut self, place: &str, subject: &str) -> bool {
        Reader::present(self, place, subject)
    }
}

/// One binding, compiled once against the executor's reader and once against
/// the model's. Build it with [`bind!`].
#[derive(Clone)]
pub struct Bind {
    pub sut: holon_net::exec::Read,
    pub model: model::Read,
}

/// `bind!(|r, s| expr)` or `bind!(path)`: `r` is a `&mut dyn Reads`, `s` a
/// `&Subject`.
macro_rules! bind {
    (|$r:ident, $s:ident| $body:expr) => {
        crate::exec_support::Bind {
            sut: ::std::sync::Arc::new(
                |$r: &mut ::holon_net::exec::Reader<'_>,
                 $s: &::holon_net::exec::Subject|
                 -> Option<::holon_net::exec::Content> {
                    let $r: &mut dyn crate::exec_support::Reads = $r;
                    $body
                },
            ),
            model: |$r: &mut crate::exec_support::model::Reader<'_>,
                    $s: &::holon_net::exec::Subject|
             -> Option<::holon_net::exec::Content> {
                let $r: &mut dyn crate::exec_support::Reads = $r;
                $body
            },
        }
    };
    ($f:path) => {
        bind!(|r, s| $f(r, s))
    };
}
#[allow(unused_imports)]
pub(crate) use bind;

#[derive(Clone)]
pub enum PlaceSpec {
    Stored {
        class: Class,
        candidate: Option<&'static str>,
        capacity: Capacity,
    },
    Derived(Bind),
}

/// A runtime net as the tests write it: it builds the executor's [`NetDef`]
/// and the model reads it directly.
#[derive(Clone, Default)]
pub struct Spec {
    pub places: BTreeMap<&'static str, PlaceSpec>,
    pub transitions: Vec<Transition>,
    pub asks: BTreeMap<String, AskMeta>,
}

impl Spec {
    pub fn place(mut self, name: &'static str, class: Class) -> Self {
        self.places.insert(
            name,
            PlaceSpec::Stored {
                class,
                candidate: None,
                capacity: Capacity::Single,
            },
        );
        self
    }

    /// A single-token place whose fact put replaces the held token
    /// (322/E3.b).
    pub fn register(mut self, name: &'static str, class: Class) -> Self {
        self.places.insert(
            name,
            PlaceSpec::Stored {
                class,
                candidate: None,
                capacity: Capacity::Register,
            },
        );
        self
    }

    /// A place whose row holds a FIFO of tokens (REV2-cand.c).
    pub fn fifo(mut self, name: &'static str, class: Class) -> Self {
        self.places.insert(
            name,
            PlaceSpec::Stored {
                class,
                candidate: None,
                capacity: Capacity::Fifo,
            },
        );
        self
    }

    pub fn place_with_candidate(mut self, name: &'static str, candidate: &'static str) -> Self {
        self.places.insert(
            name,
            PlaceSpec::Stored {
                class: Class::Process,
                candidate: Some(candidate),
                capacity: Capacity::Single,
            },
        );
        self
    }

    pub fn derived(mut self, name: &'static str, derive: Bind) -> Self {
        self.places.insert(name, PlaceSpec::Derived(derive));
        self
    }

    pub fn transition(mut self, t: Transition) -> Self {
        self.transitions.push(t);
        self
    }

    pub fn ask(mut self, work: &str, meta: AskMeta) -> Self {
        self.asks.insert(work.to_string(), meta);
        self
    }

    pub fn netdef(&self) -> NetDef {
        let net = self
            .places
            .iter()
            .fold(NetDef::default(), |net, (name, def)| match def {
                PlaceSpec::Stored {
                    class,
                    candidate: None,
                    capacity: Capacity::Single,
                } => net.place(name, *class),
                PlaceSpec::Stored {
                    class,
                    candidate: None,
                    capacity: Capacity::Register,
                } => net.register(name, *class),
                PlaceSpec::Stored {
                    class,
                    candidate: None,
                    capacity: Capacity::Fifo,
                } => net.fifo(name, *class),
                PlaceSpec::Stored {
                    class,
                    candidate: Some(c),
                    ..
                } => {
                    assert_eq!(*class, Class::Process, "a candidate on a {class:?} place");
                    net.place_with_candidate(name, c)
                }
                PlaceSpec::Derived(b) => net.derived(name, b.sut.clone()),
            });
        let net = self
            .asks
            .iter()
            .fold(net, |net, (work, meta)| net.ask(work, meta.clone()));
        self.transitions.iter().fold(net, |net, t| {
            let mut def =
                TransitionDef::new(&t.name, t.trigger, t.bind.sut.clone(), t.body.clone());
            for p in &t.consumes {
                def = def.consumes(p);
            }
            for p in &t.reads {
                def = def.reads(p);
            }
            for p in &t.inhibits {
                def = def.inhibits(p);
            }
            for p in &t.facts {
                def = def.fact(p);
            }
            if let Some(p) = &t.on_failure {
                def = def.on_failure(p);
            }
            if let Some(p) = &t.on_timeout {
                def = def.on_timeout(p);
            }
            if let Some(r) = t.retry {
                def = def.retry(r.backoff, r.max_attempts);
            }
            if let Some(d) = t.run_timer {
                def = def.run_timer(d);
            }
            if let Some(d) = t.abandon {
                def = def.abandon(d);
            }
            net.transition(def)
        })
    }
}

#[derive(Clone)]
pub struct Transition {
    pub name: String,
    pub trigger: Trigger,
    pub consumes: Vec<String>,
    pub reads: Vec<String>,
    pub inhibits: Vec<String>,
    pub bind: Bind,
    pub body: Body,
    pub facts: Vec<String>,
    pub on_failure: Option<String>,
    pub on_timeout: Option<String>,
    pub retry: Option<Retry>,
    pub run_timer: Option<Duration>,
    pub abandon: Option<Duration>,
}

impl Transition {
    pub fn new(name: &str, trigger: Trigger, bind: Bind, body: Body) -> Self {
        Self {
            name: name.to_string(),
            trigger,
            consumes: Vec::new(),
            reads: Vec::new(),
            inhibits: Vec::new(),
            bind,
            body,
            facts: Vec::new(),
            on_failure: None,
            on_timeout: None,
            retry: None,
            run_timer: None,
            abandon: None,
        }
    }

    pub fn consumes(mut self, place: &str) -> Self {
        self.consumes.push(place.to_string());
        self
    }

    pub fn reads(mut self, place: &str) -> Self {
        self.reads.push(place.to_string());
        self
    }

    pub fn inhibits(mut self, place: &str) -> Self {
        self.inhibits.push(place.to_string());
        self
    }

    pub fn fact(mut self, place: &str) -> Self {
        self.facts.push(place.to_string());
        self
    }

    pub fn on_failure(mut self, place: &str) -> Self {
        self.on_failure = Some(place.to_string());
        self
    }

    pub fn on_timeout(mut self, place: &str) -> Self {
        self.on_timeout = Some(place.to_string());
        self
    }

    pub fn retry(mut self, backoff: Duration, max_attempts: u32) -> Self {
        self.retry = Some(Retry {
            backoff,
            max_attempts,
        });
        self
    }

    pub fn run_timer(mut self, after: Duration) -> Self {
        self.run_timer = Some(after);
        self
    }

    pub fn abandon(mut self, after: Duration) -> Self {
        self.abandon = Some(after);
        self
    }
}

/// The twin under test, or `None` for the real executor. `inhibitor-net` is a
/// net variant, not an executor twin.
pub fn twin_name() -> Option<String> {
    std::env::var("HOLON_NET_TWIN").ok()
}

pub fn executor(net: NetDef) -> Executor {
    let exec = Executor::new(net).expect("the test net loads");
    match twin_name().as_deref() {
        None | Some("inhibitor-net") => exec,
        Some(name) => with_twin(exec, name),
    }
}

#[cfg(feature = "naive-twins")]
fn with_twin(exec: Executor, name: &str) -> Executor {
    let twin = twin_names::executor_twin(name)
        .unwrap_or_else(|| panic!("HOLON_NET_TWIN={name} names no twin"));
    exec.with_twin(twin)
}

#[cfg(not(feature = "naive-twins"))]
fn with_twin(_: Executor, name: &str) -> Executor {
    panic!("HOLON_NET_TWIN={name} needs --features naive-twins")
}

/// Runs every operation on the executor and on the reference model, and
/// compares their answers and their full observable state after each one.
pub struct Driver {
    exec: Executor,
    model: Model,
    spec: Spec,
    /// False for [`Driver::unchecked`]: no comparison with the model.
    compare: bool,
    pub now: Duration,
    /// Effect requests returned and not yet completed, oldest first.
    pub pending: VecDeque<EffectRequest>,
    /// Every effect request ever returned.
    pub requests: Vec<EffectRequest>,
}

fn diverge<T: Debug>(op: &str, what: &str, sut: T, model: T) -> ! {
    panic!("model divergence after {op}: {what}\n  SUT:   {sut:?}\n  model: {model:?}")
}

fn agree<T: Debug + PartialEq>(op: &str, what: &str, sut: T, model: T) -> T {
    if sut != model {
        diverge(op, what, sut, model);
    }
    sut
}

impl Driver {
    pub fn new(spec: Spec) -> Self {
        Self::build(spec, true)
    }

    /// A driver that compares nothing with the model, for a test whose own
    /// check must be the one that fails.
    pub fn unchecked(spec: Spec) -> Self {
        Self::build(spec, false)
    }

    fn build(spec: Spec, compare: bool) -> Self {
        let mut d = Self {
            exec: executor(spec.netdef()),
            model: Model::new(spec.clone()),
            spec,
            compare,
            now: Duration::ZERO,
            pending: VecDeque::new(),
            requests: Vec::new(),
        };
        d.step().expect("the initial marking quiesces");
        d
    }

    /// The executor, read-only: every change goes through the driver.
    pub fn exec(&self) -> &Executor {
        &self.exec
    }

    /// Compares the answers of one external call, then queues the effect
    /// requests it returned.
    fn quiesced<T: Debug + PartialEq, E: Debug + PartialEq>(
        &mut self,
        op: &str,
        sut: Result<Quiesced<T>, E>,
        model: Result<Quiesced<T>, E>,
    ) -> Result<T, E> {
        let r = if self.compare {
            self.properties(op);
            let r = agree(op, "answer", sut, model);
            self.check(op);
            r
        } else {
            sut
        };
        r.map(|q| {
            self.requests.extend(q.requests.iter().cloned());
            self.pending.extend(q.requests);
            q.value
        })
    }

    /// An outside commit.
    pub fn write(&mut self, place: &str, subject: &str, content: Content) -> Result<(), ExecError> {
        let op = format!("write {place}/{subject} {content:?}");
        let sut = self.exec.write(place, subject, content.clone(), self.now);
        let model = self.model.write(place, subject, content, self.now);
        self.quiesced(&op, sut, model)
    }

    /// Fires the timers due at `now`, then quiesces.
    pub fn step(&mut self) -> Result<(), ExecError> {
        let op = format!("step at {:?}", self.now);
        let sut = self.exec.step(self.now);
        let model = self.model.step(self.now);
        let new = if self.compare {
            self.properties(&op);
            let new = agree(&op, "effect requests", sut, model);
            self.check(&op);
            new
        } else {
            sut
        };
        let new = new?;
        self.requests.extend(new.iter().cloned());
        self.pending.extend(new);
        Ok(())
    }

    pub fn tick(&mut self, secs: u64) -> Result<(), ExecError> {
        self.now += Duration::from_secs(secs);
        self.step()
    }

    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
    ) -> Result<Accepted, FireError> {
        let op = format!("fire {transition}/{subject} as {occurrence}");
        let sut = self.exec.fire(
            transition,
            subject,
            occurrence.clone(),
            Content::new(),
            self.now,
        );
        let model = self
            .model
            .fire(transition, subject, occurrence, Content::new(), self.now);
        self.quiesced(&op, sut, model)
    }

    pub fn answer(&mut self, call: AnswerCall) -> Result<AnswerId, UftError> {
        let op = format!("answer {call:?}");
        let sut = self.exec.answer(call.clone(), self.now);
        let model = self.model.answer(call, self.now);
        self.quiesced(&op, sut, model)
    }

    pub fn undo(&mut self, occurrence: &OccurrenceId) -> Result<Undone, UndoError> {
        let op = format!("undo {occurrence}");
        let sut = self.exec.undo(occurrence, self.now);
        let model = self.model.undo(occurrence, self.now);
        self.quiesced(&op, sut, model)
    }

    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult,
    ) -> Result<Completion, ExecError> {
        let op = format!("complete {attempt:?} with {result:?}");
        let sut = self.exec.complete(attempt, result.clone(), self.now);
        let model = self.model.complete(attempt, result, self.now);
        self.quiesced(&op, sut, model)
    }

    pub fn resolve(
        &mut self,
        occurrence: &OccurrenceId,
        resolution: Resolution,
    ) -> Result<(), ExecError> {
        let (op, copy) = match &resolution {
            Resolution::Verify(o) => ("Verify", Resolution::Verify(o.clone())),
            Resolution::Retry => ("Retry", Resolution::Retry),
            Resolution::Drop => ("Drop", Resolution::Drop),
        };
        let op = format!("resolve {occurrence} by {op}");
        let sut = self.exec.resolve(occurrence, resolution, self.now);
        let model = self.model.resolve(occurrence, copy, self.now);
        self.quiesced(&op, sut, model)
    }

    /// Completes the oldest pending effect with `result(request)`; no-op
    /// when none is pending.
    pub fn complete_oldest(
        &mut self,
        result: impl FnOnce(&EffectRequest) -> EffectResult,
    ) -> Result<(), ExecError> {
        let Some(req) = self.pending.pop_front() else {
            return Ok(());
        };
        let r = result(&req);
        self.complete(req.attempt, r).map(|_| ())
    }

    pub fn requests_of(&self, transition: &str) -> impl Iterator<Item = &EffectRequest> {
        self.requests
            .iter()
            .filter(move |r| &*r.transition == transition)
    }

    /// Compares the full observable state: every stored place with its
    /// versions, the Running and OutcomeUnknown places, the marks, the
    /// occurrences with their outcomes and disclosed conditions, the attempts
    /// in flight, and why each transition is not enabled for each subject.
    /// The answer properties, with their own messages: they run before the
    /// generic comparison, so a twin fails here and not as a divergence.
    fn properties(&self, op: &str) {
        for (_, _, _, occ) in self.exec.in_flight() {
            if let OccurrenceId::Work { answer, .. } = occ {
                assert!(
                    !self.model.undone().contains(answer.as_ref()),
                    "work fired again after undo: {occ} after {op}"
                );
            }
        }
        let subjects: BTreeSet<String> = self.model.slot_keys().map(|(_, s)| s).collect();
        for work in self.model.asks().keys() {
            for s in &subjects {
                let sut = self.exec.why_not(work, s);
                let model = self.model.why_not(work, s);
                assert!(
                    !matches!(sut, Some(WhyNot::FailedOnThisInput(_)))
                        || matches!(model, Some(WhyNot::FailedOnThisInput(_))),
                    "re-answer blocked by the old mark: '{work}' for {s} after {op}: {sut:?}, model {model:?}"
                );
            }
        }
        for id in self.model.answer_ids() {
            let occ = OccurrenceId::caller(&id);
            let sut = self.exec.settled(&occ);
            let model = self.model.settled(&occ);
            assert!(
                sut != Ok(Some(Settled::Done)) || model == Ok(Some(Settled::Done)),
                "settled Done before the work ran: answer {id} after {op}: model {model:?}"
            );
        }
        assert_eq!(
            self.exec.conditions(),
            self.model.conditions(),
            "disclosed conditions after {op}"
        );
    }

    fn check(&self, op: &str) {
        self.properties(op);
        let tokens = self.exec.tokens();
        let stored: Vec<&str> = self
            .spec
            .places
            .iter()
            .filter(|(_, d)| matches!(d, PlaceSpec::Stored { .. }))
            .map(|(p, _)| *p)
            .collect();
        let mut keys: BTreeSet<(String, String)> = self.model.slot_keys().collect();
        for p in &stored {
            keys.extend(
                tokens
                    .subjects(p)
                    .into_iter()
                    .map(|s| (p.to_string(), s.to_string())),
            );
        }
        for (p, s) in &keys {
            let sut = (tokens.version(p, s), tokens.contents(p, s));
            agree(
                op,
                &format!("{p} of {s} (version, tokens oldest first)"),
                sut,
                self.model.slot(p, s),
            );
        }

        let model_running = self.model.running();
        let mut sut_running = BTreeMap::new();
        for t in &self.spec.transitions {
            let place = Place::running(&t.name);
            for s in tokens.subjects(&place) {
                let content = &tokens.token(&place, &s).expect("listed").content;
                let occ = match content.get("occurrence") {
                    Some(Value::String(o)) => o.clone(),
                    other => panic!("{place} of {s} names occurrence {other:?}"),
                };
                sut_running.insert((t.name.clone(), s.to_string()), occ);
            }
        }
        agree(
            op,
            "Running places (transition, subject)",
            sut_running.keys().collect::<Vec<_>>(),
            model_running.keys().collect::<Vec<_>>(),
        );
        for (key, occ) in &sut_running {
            if !model_running[key].contains(occ) {
                diverge(
                    op,
                    &format!("the occurrence in {}.Running of {}", key.0, key.1),
                    occ.clone(),
                    format!("one of {:?}", model_running[key]),
                );
            }
        }

        let mut sut_retry = BTreeMap::new();
        for t in &self.spec.transitions {
            let place = Place::retry_pending(&t.name);
            for s in tokens.subjects(&place) {
                let content = &tokens.token(&place, &s).expect("listed").content;
                let Some(Value::Integer(nanos)) = content.get("due") else {
                    panic!("{place} of {s} has no integer due: {content:?}");
                };
                let due = Duration::from_nanos(u64::try_from(*nanos).expect("not negative"));
                sut_retry.insert((t.name.clone(), s.to_string()), due);
            }
        }
        agree(
            op,
            "RetryPending places (transition, subject) → due",
            sut_retry,
            self.model.retry_pending(),
        );

        let mut sut_pending = BTreeMap::new();
        for t in &self.spec.transitions {
            let place = Place::undo_pending(&t.name);
            for s in tokens.subjects(&place) {
                let content = &tokens.token(&place, &s).expect("listed").content;
                let Some(Value::Array(rows)) = content.get("held") else {
                    panic!("{place} of {s} has no held rows: {content:?}");
                };
                let held: BTreeSet<String> = rows
                    .iter()
                    .map(|row| {
                        let Value::Object(row) = row else {
                            panic!("a held row of {place} of {s} is no object: {row:?}")
                        };
                        let text = |f: &str| match row.get(f) {
                            Some(Value::String(v)) => v.clone(),
                            other => panic!("the {f} of a held row is {other:?}"),
                        };
                        format!(
                            "{}/{} held by {}",
                            text("place"),
                            text("subject"),
                            text("holder")
                        )
                    })
                    .collect();
                sut_pending.insert((t.name.clone(), s.to_string()), held);
            }
        }
        agree(
            op,
            "UndoPending places (work, answer) → the rows the undo owes",
            sut_pending,
            self.model.undo_pending(),
        );

        let model_unknown = self.model.unknown_places();
        let mut sut_unknown_places = BTreeMap::new();
        for t in &self.spec.transitions {
            let place = Place::outcome_unknown(&t.name);
            for s in tokens.subjects(&place) {
                let content = &tokens.token(&place, &s).expect("listed").content;
                let occ = match content.get("occurrence") {
                    Some(Value::String(o)) => o.clone(),
                    other => panic!("{place} of {s} names occurrence {other:?}"),
                };
                sut_unknown_places.insert((t.name.clone(), s.to_string()), occ);
            }
        }
        agree(
            op,
            "OutcomeUnknown places (transition, subject)",
            sut_unknown_places.keys().collect::<Vec<_>>(),
            model_unknown.keys().collect::<Vec<_>>(),
        );
        for (key, occ) in &sut_unknown_places {
            if !model_unknown[key].contains(occ) {
                diverge(
                    op,
                    &format!("the occurrence in {}.OutcomeUnknown of {}", key.0, key.1),
                    occ.clone(),
                    format!("one of {:?}", model_unknown[key]),
                );
            }
        }

        let sut_unknown: BTreeMap<String, Content> = tokens
            .subjects(OUTCOME_UNKNOWN)
            .into_iter()
            .map(|s| {
                let c = tokens.token(OUTCOME_UNKNOWN, &s).expect("listed");
                (s.to_string(), c.content.clone())
            })
            .collect();
        agree(op, "OutcomeUnknown", sut_unknown, self.model.unknown());

        let sut_marks: BTreeMap<_, _> = self
            .exec
            .marks()
            .map(|(t, s, m)| ((t.to_string(), s.to_string()), m.cause.clone()))
            .collect();
        agree(op, "failure marks", sut_marks, self.model.marks());

        let sut_occs: Vec<_> = self
            .exec
            .occurrences()
            .map(|(o, s)| (o.clone(), s.cloned()))
            .collect();
        agree(op, "occurrences", sut_occs, self.model.occurrences());

        let sut_flight: Vec<_> = self
            .exec
            .in_flight()
            .map(|(a, t, s, o)| (a, t.to_string(), s.to_string(), o.clone()))
            .collect();
        agree(op, "attempts in flight", sut_flight, self.model.in_flight());

        for id in self.model.answer_ids() {
            let occ = OccurrenceId::caller(&id);
            agree(
                op,
                &format!("settled({occ})"),
                self.exec.settled(&occ),
                self.model.settled(&occ),
            )
            .expect("the answer occurrence is known");
            let answer = AnswerId(id.clone());
            agree(
                op,
                &format!("answer_settlement({id})"),
                self.exec.answer_settlement(&answer),
                self.model.answer_settlement(&answer),
            )
            .expect("the answer is known");
        }
        agree(op, "asks", self.exec.asks(), self.model.ask_rows());
        for (p, d) in &self.spec.places {
            if let PlaceSpec::Derived(_) = d {
                agree(
                    op,
                    &format!("derive_all({p})"),
                    self.exec.derive_all(p),
                    self.model.derive_all(p),
                );
            }
        }

        let subjects: BTreeSet<&str> = keys.iter().map(|(_, s)| s.as_str()).collect();
        for t in &self.spec.transitions {
            for s in &subjects {
                agree(
                    op,
                    &format!("why '{}' is not enabled for {s}", t.name),
                    self.exec.why_not(&t.name, s),
                    self.model.why_not(&t.name, s),
                );
            }
        }
    }
}

pub fn content(fields: &[(&str, Value)]) -> Content {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

pub fn text(v: &str) -> Value {
    Value::String(v.to_string())
}

pub fn int(content: &Content, field: &str) -> i64 {
    match content.get(field) {
        Some(Value::Integer(i)) => *i,
        other => panic!("field '{field}' is {other:?}, not an integer"),
    }
}

pub fn string(content: &Content, field: &str) -> String {
    match content.get(field) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("field '{field}' is {other:?}, not a string"),
    }
}

pub fn subject(s: &str) -> Subject {
    Subject::new(s)
}

/// An ask net for the work `Accept` on `Item.n`. `Situation.Accept` holds
/// while `n >= 0`; its hash covers the subject and `n`. `Asks.Accept` is the
/// same row, offered only while `Accept` is idle for the subject, so the work
/// and the expiry read the situation. The work writes the
/// process place `Accepted` and the data place `Out` from its effect.
/// `ExpireAnswer.Accept` expires an answer whose ask is gone or changed;
/// `ReleaseFailed.Accept` clears a failed answer, so the user can answer
/// again.
pub fn answer_spec() -> Spec {
    Spec::default()
        .place("Item", Class::Process)
        .place("Accepted", Class::Process)
        .place("Out", Class::Data)
        .place("Answered.Accept", Class::Process)
        .place("WorkFailed.Accept", Class::Process)
        .place("Expired.Accept", Class::Process)
        .derived("Situation.Accept", bind!(ask_row))
        .derived(
            "Asks.Accept",
            bind!(|r, s| {
                let busy = ["Answered.Accept", "Accept.Running", "WorkFailed.Accept"];
                if busy.iter().any(|p| r.present(p, s)) {
                    return None;
                }
                ask_row(r, s)
            }),
        )
        .ask(
            "Accept",
            AskMeta {
                input: None,
                irreversible: false,
                has_preview: false,
            },
        )
        .transition(
            Transition::new(
                "Answer.Accept",
                Trigger::Manual,
                bind!(|r, s| r.present("Asks.Accept", s).then(Content::new)),
                Body::pure(|s, input| Ok(vec![Output::new("Answered.Accept", s, input.clone())])),
            )
            .inhibits("Answered.Accept"),
        )
        .transition(
            Transition::new(
                "Accept",
                Trigger::Auto,
                bind!(|r, s| {
                    assert!(
                        r.field(
                            "Answered.Accept",
                            s,
                            holon_api::user_transition::ANSWER_INPUT_KEY
                        )
                        .is_none(),
                        "guard read the value"
                    );
                    let n = item_n(r, s)?;
                    let seen = r.text("Answered.Accept", s, "seen")?;
                    let hash = r.text("Situation.Accept", s, "hash")?;
                    (n >= 0 && seen == hash).then(|| content(&[("n", Value::Integer(n))]))
                }),
                Body::Effectful(Receiver::None),
            )
            .consumes("Answered.Accept")
            .consumes("Item")
            .on_failure("WorkFailed.Accept"),
        )
        .transition(
            Transition::new(
                "ExpireAnswer.Accept",
                Trigger::Auto,
                bind!(|r, s| {
                    let answer = r.text(
                        "Answered.Accept",
                        s,
                        holon_api::user_transition::ANSWER_ID_KEY,
                    )?;
                    let reason = match r.text("Situation.Accept", s, "hash") {
                        None => "ask_gone",
                        Some(hash) if r.text("Answered.Accept", s, "seen")? != hash => {
                            "situation_changed"
                        }
                        Some(_) => return None,
                    };
                    Some(content(&[
                        ("answer", text(&answer)),
                        ("reason", text(reason)),
                    ]))
                }),
                Body::pure(|_, input| {
                    Ok(vec![Output::new(
                        "Expired.Accept",
                        &Subject::new(&string(input, "answer")),
                        content(&[("reason", input["reason"].clone())]),
                    )])
                }),
            )
            .consumes("Answered.Accept")
            .inhibits("WorkFailed.Accept"),
        )
        .transition(
            Transition::new(
                "ReleaseFailed.Accept",
                Trigger::Auto,
                bind!(|_r, _s| Some(Content::new())),
                Body::pure(|_, _| Ok(Vec::new())),
            )
            .consumes("Answered.Accept")
            .consumes("WorkFailed.Accept"),
        )
}

fn item_n(r: &mut dyn Reads, s: &Subject) -> Option<i64> {
    match r.field("Item", s, "n")? {
        Value::Integer(n) => Some(n),
        other => panic!("Item.n of {s} is {other:?}"),
    }
}

fn ask_row(r: &mut dyn Reads, s: &Subject) -> Option<Content> {
    let n = item_n(r, s)?;
    let hash = blake3::hash(format!("Accept\0{s}\0{n}").as_bytes()).to_hex();
    (n >= 0).then(|| content(&[("hash", text(&hash))]))
}

/// An answer of `Accept` for `s`; `stale` or a missing ask gives a seen
/// hash that matches no ask.
pub fn answer_call(d: &Driver, s: &str, id: &str, stale: bool, value: Option<&str>) -> AnswerCall {
    let hash = match d.exec().derive("Asks.Accept", &Subject::new(s)) {
        Some(row) if !stale => string(&row, "hash"),
        _ => blake3::hash(b"a stale ask").to_hex().to_string(),
    };
    AnswerCall {
        transition: holon_api::user_transition::TransitionName("Accept".to_string()),
        subject: holon_api::user_transition::SubjectName(s.to_string()),
        seen: holon_net::exec::hash_from_text(&hash),
        answer: AnswerId(id.to_string()),
        value: value.map(text),
    }
}
