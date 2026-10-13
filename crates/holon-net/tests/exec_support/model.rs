//! The reference marking model of ADR 0032 §9: the executor's observable
//! behaviour as a plain state machine over the marking.
//!
//! It is occurrence-centric. An occurrence holds what its firing took and
//! read; its attempts hold what the runner answered; `<T>.Running` and
//! `OutcomeUnknown` are derived from them, never stored. A failure mark is
//! keyed by the read trace content itself, not by a hash of it. Data-class
//! places are the same in-memory slots as process places: whole-content
//! replace, no claims (#168).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Accepted;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Completion;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::FireError;
use holon_net::exec::Holder;
use holon_net::exec::MAX_ROUNDS;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use holon_net::exec::WhyNot;

use super::PlaceSpec;
use super::Reads;
use super::Spec;
use super::Transition;

pub type Read = fn(&mut Reader<'_>, &Subject) -> Option<Content>;

/// (place or transition, subject).
type Key = (String, String);

fn key(a: &str, s: &str) -> Key {
    (a.to_string(), s.to_string())
}

#[derive(Clone, Default)]
struct Slot {
    version: u64,
    content: Option<Content>,
}

#[derive(Clone, Debug)]
struct Row {
    version: u64,
    present: bool,
    fields: BTreeMap<String, Option<Value>>,
}

#[derive(Clone, Debug, Default)]
struct Trace {
    rows: BTreeMap<Key, Row>,
}

/// What a read trace saw, without its versions: the key of a failure mark.
type Seen = BTreeMap<Key, (bool, BTreeMap<String, Option<Value>>)>;

impl Trace {
    fn seen(&self) -> Seen {
        self.rows
            .iter()
            .map(|(k, r)| (k.clone(), (r.present, r.fields.clone())))
            .collect()
    }

    fn as_value(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|((p, s), r)| {
                    Value::Object(HashMap::from([
                        ("place".to_string(), Value::String(p.clone())),
                        ("subject".to_string(), Value::String(s.clone())),
                        ("version".to_string(), Value::Integer(r.version as i64)),
                        ("fields".to_string(), fields_value(&r.fields)),
                    ]))
                })
                .collect(),
        )
    }
}

fn fields_value(fields: &BTreeMap<String, Option<Value>>) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone().unwrap_or(Value::Null)))
            .collect(),
    )
}

/// What the runner answered one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    Running,
    /// Abandoned, or a late `Unknown`: a result may still come.
    Open,
    Unknown,
    Done,
    Failed,
}

#[derive(Clone)]
struct Occ {
    transition: String,
    subject: String,
    taken: Vec<(String, Content)>,
    trace: Trace,
    input: Content,
    settled: Option<Settled>,
    /// The cause it parked in `OutcomeUnknown` with.
    parked: Option<String>,
    attempts: BTreeMap<AttemptId, Answer>,
}

/// An attempt in flight.
#[derive(Clone)]
struct Live {
    occ: OccurrenceId,
    started: Duration,
    probe: bool,
}

/// A run spans Open → Settled; it outlives its attempts.
#[derive(Clone)]
struct Run {
    started: Duration,
    attempts: u32,
    retry_due: Option<Duration>,
}

#[derive(Clone)]
pub struct Model {
    spec: Spec,
    slots: BTreeMap<Key, Slot>,
    occs: BTreeMap<OccurrenceId, Occ>,
    in_flight: BTreeMap<AttemptId, Live>,
    runs: BTreeMap<Key, Run>,
    marks: BTreeMap<Key, (Seen, Cause)>,
    requests: Vec<EffectRequest>,
    now: Duration,
    next_attempt: u64,
    next_minted: u64,
}

pub struct Reader<'a> {
    model: &'a Model,
    trace: &'a mut Trace,
}

impl Reads for Reader<'_> {
    fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<Value> {
        let model = self.model;
        match *model.place(place) {
            PlaceSpec::Derived(b) => (b.model)(self, &Subject::new(subject))?.remove(field),
            PlaceSpec::Stored { .. } => {
                let value = model
                    .content(place, subject)
                    .and_then(|c| c.get(field).cloned());
                model
                    .row(self.trace, place, subject)
                    .fields
                    .insert(field.to_string(), value.clone());
                value
            }
        }
    }

    fn present(&mut self, place: &str, subject: &str) -> bool {
        let model = self.model;
        match *model.place(place) {
            PlaceSpec::Derived(b) => (b.model)(self, &Subject::new(subject)).is_some(),
            PlaceSpec::Stored { .. } => model.row(self.trace, place, subject).present,
        }
    }
}

fn cause_content(cause: &Cause) -> Content {
    Content::from([("cause".to_string(), Value::String(cause.text.clone()))])
}

impl Model {
    pub fn new(spec: Spec) -> Self {
        for t in &spec.transitions {
            for p in t.consumes.iter().chain(&t.reads).chain(&t.inhibits) {
                assert!(
                    spec.places.contains_key(p.as_str()),
                    "the model reads no executor place: '{}' has an arc on '{p}'",
                    t.name
                );
            }
        }
        Self {
            spec,
            slots: BTreeMap::new(),
            occs: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            runs: BTreeMap::new(),
            marks: BTreeMap::new(),
            requests: Vec::new(),
            now: Duration::ZERO,
            next_attempt: 0,
            next_minted: 0,
        }
    }

    fn place(&self, name: &str) -> &PlaceSpec {
        self.spec
            .places
            .get(name)
            .unwrap_or_else(|| panic!("the net reads place '{name}', which it does not declare"))
    }

    fn transition(&self, name: &str) -> Transition {
        self.spec
            .transitions
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no transition '{name}' in the net"))
            .clone()
    }

    fn content(&self, place: &str, subject: &str) -> Option<&Content> {
        self.slots
            .get(&key(place, subject))
            .and_then(|s| s.content.as_ref())
    }

    fn version(&self, place: &str, subject: &str) -> u64 {
        self.slots
            .get(&key(place, subject))
            .map_or(0, |s| s.version)
    }

    fn row<'t>(&self, trace: &'t mut Trace, place: &str, subject: &str) -> &'t mut Row {
        trace
            .rows
            .entry(key(place, subject))
            .or_insert_with(|| Row {
                version: self.version(place, subject),
                present: self.content(place, subject).is_some(),
                fields: BTreeMap::new(),
            })
    }

    fn slot_mut(&mut self, place: &str, subject: &str) -> &mut Slot {
        self.slots.entry(key(place, subject)).or_default()
    }

    /// A commit: a new version exactly when the content changes.
    fn put(&mut self, place: &str, subject: &str, content: Content) {
        assert!(
            matches!(self.place(place), PlaceSpec::Stored { .. }),
            "a commit to the derived place '{place}'"
        );
        let slot = self.slot_mut(place, subject);
        if slot.content.as_ref() != Some(&content) {
            slot.content = Some(content);
            slot.version += 1;
        }
    }

    pub fn write(&mut self, place: &str, subject: &str, content: Content) {
        self.put(place, subject, content);
    }

    // --- observable state ---

    pub fn slot_keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.slots.keys().cloned()
    }

    pub fn slot(&self, place: &str, subject: &str) -> (u64, Option<Content>) {
        (
            self.version(place, subject),
            self.content(place, subject).cloned(),
        )
    }

    /// `<T>.Running` of a subject holds while an attempt of `T` for it is in
    /// flight; any of those occurrences may be the one it names.
    pub fn running(&self) -> BTreeMap<Key, BTreeSet<String>> {
        let mut running: BTreeMap<Key, BTreeSet<String>> = BTreeMap::new();
        for live in self.in_flight.values() {
            let o = &self.occs[&live.occ];
            running
                .entry(key(&o.transition, &o.subject))
                .or_default()
                .insert(live.occ.to_string());
        }
        running
    }

    pub fn unknown(&self) -> BTreeMap<String, Content> {
        self.occs
            .iter()
            .filter_map(|(id, o)| {
                let cause = o.parked.as_ref()?;
                Some((
                    id.to_string(),
                    Content::from([
                        (
                            "transition".to_string(),
                            Value::String(o.transition.clone()),
                        ),
                        ("subject".to_string(), Value::String(o.subject.clone())),
                        ("cause".to_string(), Value::String(cause.clone())),
                    ]),
                ))
            })
            .collect()
    }

    pub fn marks(&self) -> BTreeMap<Key, Cause> {
        self.marks
            .iter()
            .map(|(k, (_, c))| (k.clone(), c.clone()))
            .collect()
    }

    pub fn occurrences(&self) -> Vec<(OccurrenceId, Option<Settled>)> {
        self.occs
            .iter()
            .map(|(id, o)| (id.clone(), o.settled.clone()))
            .collect()
    }

    pub fn in_flight(&self) -> Vec<(AttemptId, String, String, OccurrenceId)> {
        self.in_flight
            .iter()
            .map(|(a, l)| {
                let o = &self.occs[&l.occ];
                (*a, o.transition.clone(), o.subject.clone(), l.occ.clone())
            })
            .collect()
    }

    pub fn why_not(&self, transition: &str, subject: &str) -> Option<WhyNot> {
        self.decide(&self.transition(transition), subject).err()
    }

    // --- enabledness ---

    fn holder(&self, place: &str, subject: &str) -> Option<Holder> {
        let holds = |o: &Occ| o.subject == subject && o.taken.iter().any(|(p, _)| p == place);
        if let Some(o) = self
            .in_flight
            .values()
            .map(|l| &self.occs[&l.occ])
            .find(|o| holds(o))
        {
            return Some(Holder::Running(Arc::from(o.transition.as_str())));
        }
        self.occs
            .iter()
            .find(|(_, o)| o.parked.is_some() && holds(o))
            .map(|(id, _)| Holder::OutcomeUnknown(id.clone()))
    }

    fn decide(&self, t: &Transition, s: &str) -> Result<(Trace, Content), WhyNot> {
        let in_flight = self.in_flight.values().any(|l| {
            let o = &self.occs[&l.occ];
            o.transition == t.name && o.subject == s
        });
        if in_flight {
            return Err(WhyNot::InFlight);
        }
        if let Some(due) = self.runs.get(&key(&t.name, s)).and_then(|r| r.retry_due) {
            return Err(WhyNot::RetryPending { due });
        }
        let present = |p: &String| self.content(p, s).is_some();
        if let Some(p) = t.consumes.iter().chain(&t.reads).find(|p| !present(p)) {
            return Err(WhyNot::Needs {
                place: Place::new(p),
                held_by: self.holder(p, s),
            });
        }
        if let Some(p) = t.inhibits.iter().find(|p| present(p)) {
            return Err(WhyNot::Inhibited(Place::new(p)));
        }
        let mut trace = Trace::default();
        for p in t.consumes.iter().chain(&t.reads).chain(&t.inhibits) {
            self.row(&mut trace, p, s);
        }
        let mut reader = Reader {
            model: self,
            trace: &mut trace,
        };
        let input = (t.bind.model)(&mut reader, &Subject::new(s)).ok_or(WhyNot::Guard)?;
        if t.trigger == Trigger::Auto
            && let Some((seen, cause)) = self.marks.get(&key(&t.name, s))
            && *seen == trace.seen()
        {
            return Err(WhyNot::FailedOnThisInput(cause.clone()));
        }
        Ok((trace, input))
    }

    // --- operations ---

    fn advance(&mut self, now: Duration) {
        assert!(now >= self.now, "the clock went back");
        self.now = now;
    }

    pub fn step(&mut self, now: Duration) -> Result<Vec<EffectRequest>, ExecError> {
        self.advance(now);
        self.timers(now);
        for _ in 0..MAX_ROUNDS {
            let mut started = Vec::new();
            for t in self.spec.transitions.clone() {
                if t.trigger != Trigger::Auto {
                    continue;
                }
                let subjects: BTreeSet<String> = self
                    .slots
                    .iter()
                    .filter(|((p, _), slot)| {
                        slot.content.is_some() && t.consumes.iter().chain(&t.reads).any(|a| a == p)
                    })
                    .map(|((_, s), _)| s.clone())
                    .collect();
                for s in subjects {
                    if let Ok((trace, input)) = self.decide(&t, &s) {
                        self.next_minted += 1;
                        let occ = OccurrenceId::Minted {
                            transition: Arc::from(t.name.as_str()),
                            n: self.next_minted,
                        };
                        started.push(self.start(&t, &s, occ, trace, input, now));
                    }
                }
            }
            if started.is_empty() {
                return Ok(std::mem::take(&mut self.requests));
            }
            for id in started {
                self.run_body(id, now);
            }
        }
        Err(ExecError::NoQuiescence(MAX_ROUNDS))
    }

    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
        now: Duration,
    ) -> Result<Accepted, FireError> {
        self.advance(now);
        let t = self.transition(transition);
        if t.trigger != Trigger::Manual {
            return Err(FireError::NotManual(transition.to_string()));
        }
        if self.occs.contains_key(&occurrence) {
            return Err(FireError::DuplicateOccurrence(occurrence));
        }
        let (trace, input) = self
            .decide(&t, subject)
            .map_err(|why| FireError::NotEnabled {
                transition: transition.to_string(),
                subject: Subject::new(subject),
                why,
            })?;
        let id = self.start(&t, subject, occurrence.clone(), trace, input, now);
        self.run_body(id, now);
        Ok(Accepted(occurrence))
    }

    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult,
        now: Duration,
    ) -> Result<Completion, ExecError> {
        self.advance(now);
        let live = self.in_flight.remove(&attempt);
        let occ = match &live {
            Some(l) => l.occ.clone(),
            None => self
                .occs
                .iter()
                .find(|(_, o)| o.attempts.get(&attempt) == Some(&Answer::Open))
                .map(|(id, _)| id.clone())
                .ok_or(ExecError::UnknownAttempt(attempt))?,
        };
        let answer = match (&result, &live) {
            (EffectResult::Done(_), _) => Answer::Done,
            (EffectResult::Failed(_), _) => Answer::Failed,
            (EffectResult::Unknown(_), Some(_)) => Answer::Unknown,
            (EffectResult::Unknown(_), None) => Answer::Open,
        };
        Ok(self.observe(&occ, attempt, answer, live, result, now))
    }

    pub fn resolve(
        &mut self,
        occurrence: &OccurrenceId,
        resolution: Resolution,
        now: Duration,
    ) -> Result<(), ExecError> {
        self.advance(now);
        if self.occs.get(occurrence).is_none_or(|o| o.parked.is_none()) {
            return Err(ExecError::NotParked(occurrence.clone()));
        }
        self.occ_mut(occurrence).parked = None;
        match resolution {
            Resolution::Verify(outputs) => self.record(occurrence, outputs),
            Resolution::Retry => {
                self.occ_mut(occurrence).settled = None;
                self.issue(occurrence, false, now);
            }
            Resolution::Drop => {
                self.commit_consumption(occurrence);
                let cause = Cause::definite("dropped by the user");
                self.occ_mut(occurrence).settled = Some(Settled::Failed(cause.clone()));
                self.run_failed(occurrence, &cause, now);
            }
        }
        Ok(())
    }

    // --- firing ---

    fn occ_mut(&mut self, id: &OccurrenceId) -> &mut Occ {
        self.occs.get_mut(id).expect("a known occurrence")
    }

    fn run_key(&self, id: &OccurrenceId) -> Key {
        let o = &self.occs[id];
        key(&o.transition, &o.subject)
    }

    /// The take: the consumed tokens leave their places with no new version.
    fn start(
        &mut self,
        t: &Transition,
        s: &str,
        occ: OccurrenceId,
        trace: Trace,
        input: Content,
        now: Duration,
    ) -> AttemptId {
        let taken = t
            .consumes
            .iter()
            .map(|p| {
                let c = self.slot_mut(p, s).content.take();
                (p.clone(), c.expect("decide checked every consumed token"))
            })
            .collect();
        self.runs
            .entry(key(&t.name, s))
            .or_insert(Run {
                started: now,
                attempts: 0,
                retry_due: None,
            })
            .attempts += 1;
        self.occs.insert(
            occ.clone(),
            Occ {
                transition: t.name.clone(),
                subject: s.to_string(),
                taken,
                trace,
                input,
                settled: None,
                parked: None,
                attempts: BTreeMap::new(),
            },
        );
        self.launch(occ, now, false)
    }

    fn launch(&mut self, occ: OccurrenceId, now: Duration, probe: bool) -> AttemptId {
        let id = AttemptId(self.next_attempt);
        self.next_attempt += 1;
        self.in_flight.insert(
            id,
            Live {
                occ,
                started: now,
                probe,
            },
        );
        id
    }

    fn run_body(&mut self, id: AttemptId, now: Duration) {
        let occ = self.in_flight[&id].occ.clone();
        let t = self.transition(&self.occs[&occ].transition);
        match t.body {
            Body::Pure(body) => {
                self.in_flight.remove(&id);
                let o = &self.occs[&occ];
                match body(&Subject::new(&o.subject), &o.input) {
                    Ok(outputs) => self.record(&occ, outputs),
                    Err(cause) => self.fail(&occ, cause, now),
                }
            }
            Body::Effectful(_) => self.request(id),
        }
    }

    fn request(&mut self, id: AttemptId) {
        let live = &self.in_flight[&id];
        let (occ, probe) = (live.occ.clone(), live.probe);
        let o = self.occ_mut(&occ);
        o.attempts.insert(id, Answer::Running);
        let req = EffectRequest {
            attempt: id,
            occurrence: occ.clone(),
            transition: Arc::from(o.transition.as_str()),
            subject: Subject::new(&o.subject),
            input: o.input.clone(),
            probe,
        };
        self.requests.push(req);
    }

    fn issue(&mut self, occ: &OccurrenceId, probe: bool, now: Duration) {
        let id = self.launch(occ.clone(), now, probe);
        self.request(id);
    }

    /// Every attempt counts: Done when any is Done, Failed only when every
    /// one is Failed (EX-attempts.a).
    fn outcome(&self, occ: &OccurrenceId) -> Answer {
        let answers: Vec<Answer> = self.occs[occ].attempts.values().copied().collect();
        if answers.contains(&Answer::Done) {
            Answer::Done
        } else if answers.contains(&Answer::Running) {
            Answer::Running
        } else if answers.iter().all(|a| *a == Answer::Failed) {
            Answer::Failed
        } else {
            Answer::Unknown
        }
    }

    fn observe(
        &mut self,
        occ: &OccurrenceId,
        id: AttemptId,
        answer: Answer,
        live: Option<Live>,
        result: EffectResult,
        now: Duration,
    ) -> Completion {
        let probe = live.as_ref().is_some_and(|l| l.probe);
        let o = self.occ_mut(occ);
        o.attempts.insert(id, answer);
        if answer == Answer::Failed && probe {
            for a in o.attempts.values_mut() {
                if *a == Answer::Unknown {
                    *a = Answer::Failed;
                }
            }
        }
        if matches!(
            o.settled,
            Some(
                Settled::Done
                    | Settled::DoneOverChangedInputs(_)
                    | Settled::AppliedOverChangedInputs(_)
                    | Settled::Failed(_)
            )
        ) {
            return Completion::DroppedLate;
        }
        if let (Some(l), EffectResult::Unknown(_)) = (&live, &result)
            && let Some(probe) = self.continuation(occ, l)
        {
            self.issue(occ, probe, now);
            return Completion::Recorded;
        }
        match self.outcome(occ) {
            Answer::Running => Completion::StillUnknown,
            Answer::Done => {
                let EffectResult::Done(outputs) = result else {
                    panic!("{occ} is Done by an earlier attempt but did not settle");
                };
                self.occ_mut(occ).parked = None;
                self.record(occ, outputs);
                Completion::Recorded
            }
            Answer::Failed => {
                let EffectResult::Failed(cause) = result else {
                    panic!("{occ} is Failed by earlier attempts but did not settle");
                };
                let cause = if probe {
                    Cause::definite(format!("the goal does not hold: {}", cause.text))
                } else {
                    cause
                };
                self.occ_mut(occ).parked = None;
                self.fail(occ, cause, now);
                Completion::Recorded
            }
            Answer::Unknown | Answer::Open => {
                if live.is_none() {
                    return Completion::StillUnknown;
                }
                let cause = match result {
                    EffectResult::Unknown(cause) => cause,
                    EffectResult::Failed(cause) => Cause::definite(format!(
                        "{}; an earlier attempt may have taken effect",
                        cause.text
                    )),
                    EffectResult::Done(_) => unreachable!("a Done settles Done"),
                };
                self.park(occ, cause);
                Completion::Recorded
            }
        }
    }

    /// The attempt a receiver runs after an unknown outcome: `Some(probe)`,
    /// or `None` when the outcome parks.
    fn continuation(&mut self, occ: &OccurrenceId, live: &Live) -> Option<bool> {
        let t = self.transition(&self.occs[occ].transition);
        let Body::Effectful(receiver) = t.body else {
            panic!("a pure firing of '{}' has an unknown outcome", t.name);
        };
        match receiver {
            Receiver::Keyed if !live.probe => {
                let run_key = self.run_key(occ);
                let run = self.runs.get_mut(&run_key)?;
                let retry = t.retry.expect("a Keyed receiver has a count bound");
                (run.attempts < retry.max_attempts).then(|| {
                    run.attempts += 1;
                    false
                })
            }
            Receiver::StateProbe if !live.probe => Some(true),
            _ => None,
        }
    }

    fn stale(&self, trace: &Trace) -> Option<Key> {
        trace
            .rows
            .iter()
            .find(|((p, s), r)| self.version(p, s) != r.version)
            .map(|(k, _)| k.clone())
    }

    /// Facts commit; every other output compares and sets against the read
    /// trace. A stale pure firing rolls back. A stale effect happened, so its
    /// consumption commits and each output goes to the place's candidate, or
    /// applies on a data-class place, or is withheld; the settle discloses it.
    fn record(&mut self, occ: &OccurrenceId, outputs: Vec<Output>) {
        let o = &self.occs[occ];
        let t = self.transition(&o.transition);
        let (facts, rows): (Vec<_>, Vec<_>) = outputs
            .into_iter()
            .partition(|out| t.facts.iter().any(|f| f == out.place.as_str()));
        for f in facts {
            self.put(&f.place, &f.subject, f.content);
        }
        let Some((place, subject)) = self.stale(&self.occs[occ].trace) else {
            return self.commit(occ, rows);
        };
        let cause = Cause::definite(format!(
            "stale inputs: {place} of {subject} changed since '{}' read it",
            t.name
        ));
        if matches!(t.body, Body::Pure(_)) {
            self.rollback(occ);
            let run_key = self.run_key(occ);
            if let Some(run) = self.runs.get_mut(&run_key) {
                run.attempts -= 1;
            }
            self.occ_mut(occ).settled = Some(Settled::Failed(cause));
            return;
        }
        let trace = &self.occs[occ].trace;
        let (mut kept, mut applied, mut withheld) = (Vec::new(), false, false);
        for out in &rows {
            match *self.place(&out.place) {
                PlaceSpec::Stored {
                    candidate: Some(c), ..
                } => {
                    let base = trace
                        .rows
                        .get(&key(&out.place, &out.subject))
                        .map_or(Value::Null, |r| fields_value(&r.fields));
                    kept.push(Output::new(
                        c,
                        &out.subject,
                        Content::from([
                            ("base".to_string(), base),
                            (
                                "output".to_string(),
                                Value::Object(out.content.clone().into_iter().collect()),
                            ),
                            ("trace".to_string(), trace.as_value()),
                        ]),
                    ));
                }
                PlaceSpec::Stored {
                    class: Class::Data, ..
                } => {
                    applied = true;
                    kept.push(out.clone());
                }
                _ => withheld = true,
            }
        }
        self.commit(occ, kept);
        let settled = if withheld || rows.is_empty() {
            Settled::DoneOverChangedInputs(cause)
        } else if applied {
            Settled::AppliedOverChangedInputs(cause)
        } else {
            Settled::Done
        };
        self.occ_mut(occ).settled = Some(settled);
    }

    fn commit(&mut self, occ: &OccurrenceId, rows: Vec<Output>) {
        self.commit_consumption(occ);
        for out in rows {
            self.put(&out.place, &out.subject, out.content);
        }
        let run_key = self.run_key(occ);
        self.marks.remove(&run_key);
        self.runs.remove(&run_key);
        self.occ_mut(occ).settled = Some(Settled::Done);
    }

    /// The taken tokens leave the net: their places get a new version, unless
    /// a newer token was written meanwhile.
    fn commit_consumption(&mut self, occ: &OccurrenceId) {
        let o = &self.occs[occ];
        let (places, s): (Vec<String>, String) = (
            o.taken.iter().map(|(p, _)| p.clone()).collect(),
            o.subject.clone(),
        );
        for p in places {
            let slot = self.slot_mut(&p, &s);
            if slot.content.is_none() {
                slot.version += 1;
            }
        }
    }

    /// The taken tokens go back with no new version, unless a newer token was
    /// written meanwhile.
    fn rollback(&mut self, occ: &OccurrenceId) {
        let o = &self.occs[occ];
        let (taken, s) = (o.taken.clone(), o.subject.clone());
        for (p, c) in taken {
            let slot = self.slot_mut(&p, &s);
            if slot.content.is_none() {
                slot.content = Some(c);
            }
        }
    }

    fn set_mark(&mut self, occ: &OccurrenceId, cause: &Cause) {
        let seen = self.occs[occ].trace.seen();
        self.marks.insert(self.run_key(occ), (seen, cause.clone()));
    }

    fn fail(&mut self, occ: &OccurrenceId, cause: Cause, now: Duration) {
        self.rollback(occ);
        self.set_mark(occ, &cause);
        self.occ_mut(occ).settled = Some(Settled::Failed(cause.clone()));
        self.run_failed(occ, &cause, now);
    }

    /// A retry while the count bound allows, else the run settles failed and
    /// the failure port gets a token.
    fn run_failed(&mut self, occ: &OccurrenceId, cause: &Cause, now: Duration) {
        let run_key = self.run_key(occ);
        let t = self.transition(&run_key.0);
        let Some(run) = self.runs.get_mut(&run_key) else {
            return;
        };
        if cause.retryable
            && let Some(retry) = t.retry
            && run.attempts < retry.max_attempts
        {
            run.retry_due = Some(now + retry.backoff);
            return;
        }
        self.runs.remove(&run_key);
        if let Some(port) = &t.on_failure {
            self.put(port, &run_key.1, cause_content(cause));
        }
    }

    /// An unknown outcome is no failure: the run stays open, the input stays
    /// taken, the occurrence is disclosed in `OutcomeUnknown`.
    fn park(&mut self, occ: &OccurrenceId, cause: Cause) {
        self.set_mark(occ, &cause);
        let o = self.occ_mut(occ);
        o.parked = Some(cause.text.clone());
        o.settled = Some(Settled::OutcomeUnknown(cause));
    }

    fn timers(&mut self, now: Duration) {
        let hung: Vec<AttemptId> = self
            .in_flight
            .iter()
            .filter(|(_, l)| {
                let t = self.transition(&self.occs[&l.occ].transition);
                t.abandon.is_some_and(|d| l.started + d <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in hung {
            let live = self.in_flight.remove(&id).expect("listed above");
            let cause = Cause::definite(format!("abandoned: no result since {:?}", live.started));
            let occ = live.occ.clone();
            self.observe(
                &occ,
                id,
                Answer::Open,
                Some(live),
                EffectResult::Unknown(cause),
                now,
            );
        }
        let keys: Vec<Key> = self.runs.keys().cloned().collect();
        for k in keys {
            let t = self.transition(&k.0);
            let run = &self.runs[&k];
            if t.run_timer.is_some_and(|d| run.started + d <= now) {
                self.runs.remove(&k);
                if let Some(port) = &t.on_timeout {
                    let cause = Cause::definite(format!("run timer of '{}' expired", k.0));
                    self.put(port, &k.1, cause_content(&cause));
                }
            } else if run.retry_due.is_some_and(|due| due <= now) {
                self.runs.get_mut(&k).expect("listed above").retry_due = None;
                self.marks.remove(&k);
            }
        }
    }
}
