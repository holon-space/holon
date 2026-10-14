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
use holon_api::user_transition::ANSWER_ID_KEY;
use holon_api::user_transition::ANSWER_INPUT_KEY;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_net::exec::Accepted;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Completion;
use holon_net::exec::Condition;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::FireError;
use holon_net::exec::HASH_FIELD;
use holon_net::exec::Holder;
use holon_net::exec::MAX_ROUNDS;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Quiesced;
use holon_net::exec::REASON_FIELD;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Reversal;
use holon_net::exec::ReversalOutcome;
use holon_net::exec::Route;
use holon_net::exec::Routed;
use holon_net::exec::SEEN_FIELD;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use holon_net::exec::UndoError;
use holon_net::exec::Undone;
use holon_net::exec::WhyNot;
use holon_net::exec::answer_transition;
use holon_net::exec::answered_place;
use holon_net::exec::asks_place;
use holon_net::exec::expire_reason_text;
use holon_net::exec::expired_place;
use holon_net::exec::hash_from_text;
use holon_net::exec::hash_text;
use holon_net::exec::work_occurrence;

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
    /// Answer id → (work, subject).
    answers: BTreeMap<String, (String, String)>,
    undone: BTreeSet<String>,
    conditions: Vec<Condition>,
    /// Per work occurrence: the content before its commit and the version
    /// after it.
    deltas: BTreeMap<OccurrenceId, BTreeMap<Key, (Option<Content>, u64)>>,
}

pub struct Reader<'a> {
    model: &'a Model,
    trace: &'a mut Trace,
}

impl Reads for Reader<'_> {
    fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<Value> {
        let model = self.model;
        match model.place(place) {
            PlaceSpec::Derived(_) => model.derived(self, place, subject)?.remove(field),
            PlaceSpec::Stored { .. } if field == ANSWER_INPUT_KEY && model.is_answered(place) => {
                None
            }
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
        if let Some(t) = place.strip_suffix(".Running")
            && model.spec.transitions.iter().any(|d| d.name == t)
        {
            return model.running().contains_key(&key(t, subject));
        }
        match model.place(place) {
            PlaceSpec::Derived(_) => model.derived(self, place, subject).is_some(),
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
            answers: BTreeMap::new(),
            undone: BTreeSet::new(),
            conditions: Vec::new(),
            deltas: BTreeMap::new(),
        }
    }

    fn is_answered(&self, place: &str) -> bool {
        self.spec.asks.keys().any(|w| answered_place(w) == place)
    }

    fn answered_of(&self, transition: &str) -> Option<String> {
        self.spec
            .asks
            .contains_key(transition)
            .then(|| answered_place(transition))
    }

    fn answered_field(&self, place: &str, subject: &str, field: &str) -> Value {
        self.content(place, subject)
            .expect("decide checked every consumed token")[field]
            .clone()
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

    pub fn write(
        &mut self,
        place: &str,
        subject: &str,
        content: Content,
        now: Duration,
    ) -> Result<Quiesced<()>, ExecError> {
        self.advance(now);
        self.put(place, subject, content);
        self.quiesced((), now)
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

    /// `<T>.RetryPending`: the due time of each pending retry backoff.
    pub fn retry_pending(&self) -> BTreeMap<Key, Duration> {
        self.runs
            .iter()
            .filter_map(|(k, r)| Some((k.clone(), r.retry_due?)))
            .collect()
    }

    /// `<T>.OutcomeUnknown` of a subject holds while an occurrence of `T` for
    /// it is parked; any of those occurrences may be the one it names.
    pub fn unknown_places(&self) -> BTreeMap<Key, BTreeSet<String>> {
        let mut unknown: BTreeMap<Key, BTreeSet<String>> = BTreeMap::new();
        for (id, o) in self.occs.iter().filter(|(_, o)| o.parked.is_some()) {
            unknown
                .entry(key(&o.transition, &o.subject))
                .or_default()
                .insert(id.to_string());
        }
        unknown
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

    // --- answers ---

    pub fn answer_ids(&self) -> Vec<String> {
        self.answers.keys().cloned().collect()
    }

    pub fn undone(&self) -> &BTreeSet<String> {
        &self.undone
    }

    pub fn asks(&self) -> &BTreeMap<String, holon_net::exec::AskMeta> {
        &self.spec.asks
    }

    pub fn settled(
        &self,
        occurrence: &OccurrenceId,
    ) -> Result<Option<Settled>, holon_net::exec::UnknownOccurrence> {
        let own = self
            .occs
            .get(occurrence)
            .map(|o| o.settled.clone())
            .ok_or_else(|| holon_net::exec::UnknownOccurrence(occurrence.clone()))?;
        let OccurrenceId::Caller(id) = occurrence else {
            return Ok(own);
        };
        let Some((work, _)) = self.answers.get(id.as_ref()) else {
            return Ok(own);
        };
        assert_eq!(own, Some(Settled::Done), "an Answer firing is pure");
        if let Some(o) = self.occs.get(&work_occurrence(work, id)) {
            return Ok(o.settled.clone());
        }
        if self.content(&expired_place(work), id).is_some() {
            return Ok(Some(Settled::Failed(Cause::definite(
                UftError::NoLongerApplies.to_string(),
            ))));
        }
        if self.undone.contains(id.as_ref()) {
            return Ok(Some(Settled::Failed(Cause::definite(
                "undone before the work ran",
            ))));
        }
        Ok(None)
    }

    pub fn answer(
        &mut self,
        call: AnswerCall,
        now: Duration,
    ) -> Result<Quiesced<AnswerId>, UftError> {
        let work = call.transition.0.clone();
        if !self.spec.asks.contains_key(&work) {
            return Err(UftError::UnknownTransition(call.transition));
        }
        let id = call.answer.0.clone();
        let value = Content::from([
            (ANSWER_ID_KEY.to_string(), Value::String(id.clone())),
            (SEEN_FIELD.to_string(), Value::String(hash_text(&call.seen))),
            (
                ANSWER_INPUT_KEY.to_string(),
                call.value.unwrap_or(Value::Null),
            ),
        ]);
        match self.fire(
            &answer_transition(&work),
            &call.subject.0,
            OccurrenceId::caller(&id),
            value,
            now,
        ) {
            Ok(fired) => Ok(Quiesced {
                value: call.answer,
                requests: fired.requests,
            }),
            Err(
                FireError::DuplicateOccurrence(_)
                | FireError::NotEnabled {
                    why: WhyNot::Inhibited(_),
                    ..
                },
            ) => Err(UftError::AlreadyAnswered),
            Err(FireError::NotEnabled {
                why: WhyNot::Guard | WhyNot::Needs { .. },
                ..
            }) => Err(UftError::NoLongerApplies),
            Err(FireError::NoQuiescence(steps)) => {
                self.conditions.push(Condition::NoQuiescence {
                    work: Arc::from(work.as_str()),
                    subject: Subject::new(&call.subject.0),
                    steps,
                });
                Err(UftError::NoQuiescence {
                    transition: call.transition,
                    subject: call.subject,
                    steps,
                })
            }
            Err(
                e @ (FireError::NotManual(_)
                | FireError::NotEnabled {
                    why:
                        WhyNot::InFlight | WhyNot::RetryPending { .. } | WhyNot::FailedOnThisInput(_),
                    ..
                }),
            ) => unreachable!("'{work}' has a manual pure Answer: {e}"),
        }
    }

    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }

    pub fn answer_settlement(&self, answer: &AnswerId) -> Result<AnswerSettlement, UftError> {
        let id = answer.0.as_str();
        let Some((work, _)) = self.answers.get(id) else {
            return Err(UftError::UnknownAnswer(answer.clone()));
        };
        if self.undone.contains(id) {
            return Ok(AnswerSettlement::Undone);
        }
        let occurrence = work_occurrence(work, id);
        Ok(match self.occs.get(&occurrence).map(|o| &o.settled) {
            Some(None | Some(Settled::OutcomeUnknown(_))) => {
                AnswerSettlement::Working { work: occurrence }
            }
            Some(Some(Settled::Done)) => AnswerSettlement::Done { work: occurrence },
            Some(Some(Settled::Failed(cause) | Settled::OverChangedInputs { cause, .. })) => {
                AnswerSettlement::Failed {
                    cause: cause.text.clone(),
                    work: occurrence,
                }
            }
            None => match self.content(&expired_place(work), id) {
                Some(c) => {
                    let Some(Value::String(reason)) = c.get(REASON_FIELD) else {
                        panic!("the expired token of '{id}' has no reason");
                    };
                    AnswerSettlement::Expired {
                        reason: [ExpireReason::AskGone, ExpireReason::SituationChanged]
                            .into_iter()
                            .find(|r| expire_reason_text(*r) == reason)
                            .unwrap_or_else(|| panic!("'{reason}' is no expire reason")),
                    }
                }
                None => AnswerSettlement::Waiting,
            },
        })
    }

    /// The `Asks.T` rows, in work and subject order.
    pub fn ask_rows(&self) -> Vec<AskRow> {
        let mut rows = Vec::new();
        for (work, meta) in &self.spec.asks {
            for (subject, row) in self.derive_all(&asks_place(work)) {
                let Some(Value::String(hash)) = row.get(HASH_FIELD) else {
                    panic!("the Asks row of {subject} has no hash");
                };
                rows.push(AskRow {
                    transition: TransitionName(work.clone()),
                    subject: SubjectName(subject.to_string()),
                    hash: hash_from_text(hash),
                    input: meta.input.clone(),
                    irreversible: meta.irreversible,
                    has_preview: meta.has_preview,
                });
            }
        }
        rows
    }

    pub fn derive(&self, place: &str, subject: &str) -> Option<Content> {
        let mut trace = Trace::default();
        let mut reader = Reader {
            model: self,
            trace: &mut trace,
        };
        self.derived(&mut reader, place, subject)
    }

    fn derived(&self, reader: &mut Reader<'_>, place: &str, s: &str) -> Option<Content> {
        let PlaceSpec::Derived(b) = self.place(place) else {
            panic!("'{place}' is no derived place");
        };
        (b.model)(reader, &Subject::new(s))
    }

    /// Every subject a stored place holds or a running attempt names.
    pub fn derive_all(&self, place: &str) -> Vec<(Subject, Content)> {
        let subjects: BTreeSet<String> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.content.is_some())
            .map(|((_, s), _)| s.clone())
            .chain(
                self.in_flight
                    .values()
                    .map(|l| self.occs[&l.occ].subject.clone()),
            )
            .collect();
        subjects
            .into_iter()
            .filter_map(|s| self.derive(place, &s).map(|c| (Subject::new(&s), c)))
            .collect()
    }

    pub fn undo(
        &mut self,
        answer: &OccurrenceId,
        now: Duration,
    ) -> Result<Quiesced<Undone>, UndoError> {
        self.advance(now);
        if !self.occs.contains_key(answer) {
            return Err(UndoError::UnknownOccurrence(answer.clone()));
        }
        let OccurrenceId::Caller(id) = answer else {
            return Err(UndoError::NotAnAnswer(answer.clone()));
        };
        let Some((work, subject)) = self.answers.get(id.as_ref()).cloned() else {
            return Err(UndoError::NotAnAnswer(answer.clone()));
        };
        if self.undone.contains(id.as_ref()) {
            return Err(UndoError::AlreadyUndone(answer.clone()));
        }
        let occurrence = work_occurrence(&work, id);
        if let Some(None | Some(Settled::OutcomeUnknown(_))) =
            self.occs.get(&occurrence).map(|o| &o.settled)
        {
            return Err(UndoError::WorkUnsettled(occurrence));
        }
        let answered = answered_place(&work);
        let withdrawn = self
            .content(&answered, &subject)
            .is_some_and(|c| c.get(ANSWER_ID_KEY) == Some(&Value::String(id.to_string())));
        if withdrawn {
            self.remove(&answered, &subject);
        }
        let mut reversed = Vec::new();
        for ((place, s), (before, version)) in
            self.deltas.get(&occurrence).cloned().unwrap_or_default()
        {
            let process = matches!(
                self.place(&place),
                PlaceSpec::Stored {
                    class: Class::Process,
                    ..
                }
            );
            let outcome = if !process {
                ReversalOutcome::NotProcessClass
            } else if self.version(&place, &s) != version {
                ReversalOutcome::Changed
            } else if let Some(holder) = self.holder(&place, &s) {
                ReversalOutcome::Held(holder)
            } else if let Some(content) = before {
                self.put(&place, &s, content);
                ReversalOutcome::Restored
            } else {
                self.remove(&place, &s);
                ReversalOutcome::Removed
            };
            reversed.push(Reversal {
                place: Place::new(&place),
                subject: Subject::new(&s),
                outcome,
            });
        }
        self.undone.insert(id.to_string());
        let requests = self.quiesce(now).map_err(UndoError::NoQuiescence)?;
        Ok(Quiesced {
            value: Undone {
                withdrawn,
                reversed,
            },
            requests,
        })
    }

    fn remove(&mut self, place: &str, subject: &str) {
        let slot = self.slot_mut(place, subject);
        assert!(
            slot.content.take().is_some(),
            "a removal of the empty {place} of {subject}"
        );
        slot.version += 1;
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
        let answered = self.answered_of(&t.name);
        if let Some(p) = &answered {
            let answer = self.answered_field(p, s, ANSWER_ID_KEY);
            self.row(&mut trace, p, s)
                .fields
                .insert(ANSWER_ID_KEY.to_string(), Some(answer));
        }
        let mut reader = Reader {
            model: self,
            trace: &mut trace,
        };
        let mut input = (t.bind.model)(&mut reader, &Subject::new(s)).ok_or(WhyNot::Guard)?;
        if t.trigger == Trigger::Auto
            && let Some((seen, cause)) = self.marks.get(&key(&t.name, s))
            && *seen == trace.seen()
        {
            return Err(WhyNot::FailedOnThisInput(cause.clone()));
        }
        if let Some(p) = &answered {
            let value = self.answered_field(p, s, ANSWER_INPUT_KEY);
            assert!(input.insert(ANSWER_INPUT_KEY.to_string(), value).is_none());
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
        self.quiesce(now).map_err(ExecError::NoQuiescence)
    }

    fn quiesced<T>(&mut self, value: T, now: Duration) -> Result<Quiesced<T>, ExecError> {
        let requests = self.quiesce(now).map_err(ExecError::NoQuiescence)?;
        Ok(Quiesced { value, requests })
    }

    /// The auto transitions to quiescence, no timer.
    fn quiesce(&mut self, now: Duration) -> Result<Vec<EffectRequest>, usize> {
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
                        let occ = match self.answered_of(&t.name) {
                            Some(p) => match self.answered_field(&p, &s, ANSWER_ID_KEY) {
                                Value::String(answer) => work_occurrence(&t.name, &answer),
                                other => panic!("the answer id is {other:?}"),
                            },
                            None => {
                                self.next_minted += 1;
                                OccurrenceId::Minted {
                                    transition: Arc::from(t.name.as_str()),
                                    n: self.next_minted,
                                }
                            }
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
        Err(MAX_ROUNDS)
    }

    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
        value: Content,
        now: Duration,
    ) -> Result<Quiesced<Accepted>, FireError> {
        self.advance(now);
        let t = self.transition(transition);
        if t.trigger != Trigger::Manual {
            return Err(FireError::NotManual(transition.to_string()));
        }
        if self.occs.contains_key(&occurrence) {
            return Err(FireError::DuplicateOccurrence(occurrence));
        }
        let (trace, mut input) = self
            .decide(&t, subject)
            .map_err(|why| FireError::NotEnabled {
                transition: transition.to_string(),
                subject: Subject::new(subject),
                why,
            })?;
        if let Some(work) = self
            .spec
            .asks
            .keys()
            .find(|w| answer_transition(w) == transition)
        {
            let OccurrenceId::Caller(id) = &occurrence else {
                panic!("an answer with the occurrence {occurrence}");
            };
            self.answers
                .insert(id.to_string(), (work.clone(), subject.to_string()));
        }
        for (k, v) in value {
            assert!(input.insert(k, v).is_none());
        }
        let id = self.start(&t, subject, occurrence.clone(), trace, input, now);
        self.run_body(id, now);
        let requests = self.quiesce(now).map_err(FireError::NoQuiescence)?;
        Ok(Quiesced {
            value: Accepted(occurrence),
            requests,
        })
    }

    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult,
        now: Duration,
    ) -> Result<Quiesced<Completion>, ExecError> {
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
        let completion = self.observe(&occ, attempt, answer, live, result, now);
        self.quiesced(completion, now)
    }

    pub fn resolve(
        &mut self,
        occurrence: &OccurrenceId,
        resolution: Resolution,
        now: Duration,
    ) -> Result<Quiesced<()>, ExecError> {
        self.advance(now);
        if self.occs.get(occurrence).is_none_or(|o| o.parked.is_none()) {
            return Err(ExecError::NotParked(occurrence.clone()));
        }
        let o = &self.occs[occurrence];
        if matches!(resolution, Resolution::Retry)
            && self.running().contains_key(&key(&o.transition, &o.subject))
        {
            return Err(ExecError::RetryWhileRunning {
                transition: o.transition.clone(),
                subject: Subject::new(&o.subject),
            });
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
        self.quiesced((), now)
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
        let attempts = self
            .occs
            .get(&occ)
            .map_or_else(BTreeMap::new, |o| o.attempts.clone());
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
                attempts,
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
        match &t.body {
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
            Some(Settled::Done | Settled::OverChangedInputs { .. } | Settled::Failed(_))
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
        let Body::Effectful(receiver) = &t.body else {
            panic!("a pure firing of '{}' has an unknown outcome", t.name);
        };
        match *receiver {
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
            let run_key = self.run_key(occ);
            self.marks.remove(&run_key);
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
        let (mut kept, mut routes) = (Vec::new(), Vec::new());
        for out in &rows {
            let route = match self.place(&out.place) {
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
                    Route::Candidate(Place::new(c))
                }
                PlaceSpec::Stored {
                    class: Class::Data, ..
                } => {
                    kept.push(out.clone());
                    Route::Applied
                }
                _ => Route::Withheld,
            };
            routes.push(Routed {
                place: out.place.clone(),
                subject: out.subject.clone(),
                route,
            });
        }
        let (run_key, seen) = (self.run_key(occ), self.occs[occ].trace.seen());
        if self.marks.get(&run_key).is_some_and(|(s, _)| *s != seen) {
            self.marks.remove(&run_key);
        }
        self.commit(occ, kept);
        self.occ_mut(occ).settled = Some(Settled::OverChangedInputs { cause, routes });
    }

    fn commit(&mut self, occ: &OccurrenceId, rows: Vec<Output>) {
        let mut before: BTreeMap<Key, Option<Content>> = BTreeMap::new();
        if let OccurrenceId::Work { .. } = occ {
            let o = &self.occs[occ];
            let answered = self.answered_of(&o.transition);
            for (p, c) in &o.taken {
                if self.content(p, &o.subject).is_none() && Some(p) != answered.as_ref() {
                    before.insert(key(p, &o.subject), Some(c.clone()));
                }
            }
            for out in &rows {
                before
                    .entry(key(&out.place, &out.subject))
                    .or_insert_with(|| self.content(&out.place, &out.subject).cloned());
            }
        }
        self.commit_consumption(occ);
        for out in rows {
            self.put(&out.place, &out.subject, out.content);
        }
        if !before.is_empty() {
            let delta = before
                .into_iter()
                .map(|(k, c)| {
                    let version = self.version(&k.0, &k.1);
                    (k, (c, version))
                })
                .collect();
            self.deltas.insert(occ.clone(), delta);
        }
        let run_key = self.run_key(occ);
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
