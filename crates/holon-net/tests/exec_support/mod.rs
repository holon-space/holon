//! Shared driver of the executor PBTs: it plays the effect runner and the
//! clock, runs every operation on the executor and on the reference model,
//! and fails on the first divergence. It picks the naive twin named by
//! `HOLON_NET_TWIN`.

#![allow(dead_code)]

pub mod model;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Accepted;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
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
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Resolution;
use holon_net::exec::Retry;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
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
#[derive(Clone, Copy)]
pub struct Bind {
    pub sut: holon_net::exec::Read,
    pub model: model::Read,
}

/// `bind!(|r, s| expr)` or `bind!(path)`: `r` is a `&mut dyn Reads`, `s` a
/// `&Subject`.
macro_rules! bind {
    (|$r:ident, $s:ident| $body:expr) => {
        crate::exec_support::Bind {
            sut: |$r: &mut ::holon_net::exec::Reader<'_>,
                  $s: &::holon_net::exec::Subject|
             -> Option<::holon_net::exec::Content> {
                let $r: &mut dyn crate::exec_support::Reads = $r;
                $body
            },
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
pub(crate) use bind;

#[derive(Clone, Copy)]
pub enum PlaceSpec {
    Stored {
        class: Class,
        candidate: Option<&'static str>,
    },
    Derived(Bind),
}

/// A runtime net as the tests write it: it builds the executor's [`NetDef`]
/// and the model reads it directly.
#[derive(Clone, Default)]
pub struct Spec {
    pub places: BTreeMap<&'static str, PlaceSpec>,
    pub transitions: Vec<Transition>,
}

impl Spec {
    pub fn place(mut self, name: &'static str, class: Class) -> Self {
        self.places.insert(
            name,
            PlaceSpec::Stored {
                class,
                candidate: None,
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

    pub fn netdef(&self) -> NetDef {
        let net = self
            .places
            .iter()
            .fold(NetDef::default(), |net, (name, def)| match *def {
                PlaceSpec::Stored {
                    class,
                    candidate: None,
                } => net.place(name, class),
                PlaceSpec::Stored {
                    class,
                    candidate: Some(c),
                } => {
                    assert_eq!(class, Class::Process, "a candidate on a {class:?} place");
                    net.place_with_candidate(name, c)
                }
                PlaceSpec::Derived(b) => net.derived(name, b.sut),
            });
        self.transitions.iter().fold(net, |net, t| {
            let mut def = TransitionDef::new(&t.name, t.trigger, t.bind.sut, t.body);
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
    use holon_net::exec::Twin;
    let twin = match name {
        "hidden-reservation" => Twin::HiddenReservation,
        "mark-by-version" => Twin::MarkByVersion,
        "no-mark" => Twin::NoMark,
        "no-cas" => Twin::NoCas,
        "no-count-bound" => Twin::NoCountBound,
        "unknown-is-failure" => Twin::UnknownIsFailure,
        "unknown-settles-failed" => Twin::UnknownSettlesFailed,
        "stale-done-returns-input" => Twin::StaleDoneReturnsInput,
        "drop-late-done" => Twin::DropLateDone,
        "drop-late-failed" => Twin::DropLateFailed,
        "failed-settles-alone" => Twin::FailedSettlesAlone,
        other => panic!("HOLON_NET_TWIN={other} names no twin"),
    };
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
    pub now: Duration,
    /// Effects returned by `step` and not yet completed, oldest first.
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
        let mut d = Self {
            exec: executor(spec.netdef()),
            model: Model::new(spec.clone()),
            spec,
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

    /// An outside commit, without a step.
    pub fn write(&mut self, place: &str, subject: &str, content: Content) {
        self.exec.write(place, subject, content.clone());
        self.model.write(place, subject, content.clone());
        self.check(&format!("write {place}/{subject} {content:?}"));
    }

    pub fn step(&mut self) -> Result<(), ExecError> {
        let op = format!("step at {:?}", self.now);
        let new = agree(
            &op,
            "effect requests",
            self.exec.step(self.now),
            self.model.step(self.now),
        );
        self.check(&op);
        let new = new?;
        self.requests.extend(new.iter().cloned());
        self.pending.extend(new);
        Ok(())
    }

    pub fn tick(&mut self, secs: u64) -> Result<(), ExecError> {
        self.now += Duration::from_secs(secs);
        self.step()
    }

    /// A manual fire, without a step.
    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
    ) -> Result<Accepted, FireError> {
        let op = format!("fire {transition}/{subject} as {occurrence}");
        let r = agree(
            &op,
            "answer",
            self.exec
                .fire(transition, subject, occurrence.clone(), self.now),
            self.model.fire(transition, subject, occurrence, self.now),
        );
        self.check(&op);
        r
    }

    /// Hands back one result, without a step.
    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult,
    ) -> Result<Completion, ExecError> {
        let op = format!("complete {attempt:?} with {result:?}");
        let r = agree(
            &op,
            "answer",
            self.exec.complete(attempt, result.clone(), self.now),
            self.model.complete(attempt, result, self.now),
        );
        self.check(&op);
        r
    }

    /// Resolves a parked occurrence, without a step.
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
        let r = agree(
            &op,
            "answer",
            self.exec.resolve(occurrence, resolution, self.now),
            self.model.resolve(occurrence, copy, self.now),
        );
        self.check(&op);
        r
    }

    /// Completes the oldest pending effect with `result(request)`, then
    /// steps; no-op when none is pending.
    pub fn complete_oldest(
        &mut self,
        result: impl FnOnce(&EffectRequest) -> EffectResult,
    ) -> Result<(), ExecError> {
        let Some(req) = self.pending.pop_front() else {
            return Ok(());
        };
        let r = result(&req);
        self.complete(req.attempt, r)?;
        self.step()
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
    fn check(&self, op: &str) {
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
            let sut = (
                tokens.version(p, s),
                tokens.token(p, s).map(|t| t.content.clone()),
            );
            agree(
                op,
                &format!("{p} of {s} (version, content)"),
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
