//! The executor of runtime nets (ADR 0032 §9): an owned marking of
//! process-class and data-class tokens, enabledness, pure and effectful
//! firings, the record's compare-and-set, failure marks and timers.
//!
//! Sans-IO: every external call runs the auto transitions it caused to
//! quiescence and returns the effects to run in a [`Quiesced`]. The caller
//! runs them and hands each result back through [`Executor::complete`]. The
//! clock is injected: every call that can move time takes `now`, a monotonic
//! offset. Only [`Executor::step`] fires the timers due at `now`.
//!
//! The executor implements no special case for producer order, run state or
//! failure ports. Those are places and arcs of the net (§9 intro, NET-run.a).
//!
//! Answers (UFT-adopt.a): [`Executor::answer`] fires the manual `Answer.T`
//! with the user value. The value joins the input of the work `T` after its
//! bind, so no guard reads it. The answer id is in the read trace of `T`, so a
//! new answer is a new failure mark key. [`Executor::settled`] of an answer
//! follows its work occurrence ([`work_occurrence`]), and [`Executor::undo`]
//! withdraws the answer and reverses its work's process-class delta in one
//! step. [`Executor::derive`], [`Executor::asks`] and [`ask_hash`] read the
//! derived places outside a firing.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use holon_api::TypeHint;
use holon_api::user_transition::ANSWER_ID_KEY;
use holon_api::user_transition::ANSWER_INPUT_KEY;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::ExpireReason;
pub use holon_api::user_transition::OccurrenceId;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_pattern::Value;

/// A round fires every transition its marking enables. A net still enabled
/// after this many rounds of one call does not terminate.
pub const MAX_ROUNDS: usize = 1000;

/// The place where an effect with an unknown outcome parks its inputs
/// (`Receiver::None`). Its subject is the occurrence id.
pub const OUTCOME_UNKNOWN: &str = "OutcomeUnknown";

macro_rules! name {
    ($t:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $t(Arc<str>);

        impl $t {
            pub fn new(name: &str) -> Self {
                Self(Arc::from(name))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $t {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $t {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

name!(Place);
name!(Subject);

impl Place {
    /// `<T>.Running`: where a take moves the inputs of a firing of `T`.
    pub fn running(transition: &str) -> Self {
        Self::new(&format!("{transition}.Running"))
    }

    /// `<T>.RetryPending`: a token while a retry backoff of `T` is pending;
    /// its `due` field is the time the retry fires.
    pub fn retry_pending(transition: &str) -> Self {
        Self::new(&format!("{transition}.RetryPending"))
    }

    /// `<T>.OutcomeUnknown`: a token while an occurrence of `T` for the
    /// subject is parked with an unknown outcome; its `occurrence` field
    /// names one of them.
    pub fn outcome_unknown(transition: &str) -> Self {
        Self::new(&format!("{transition}.OutcomeUnknown"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttemptId(pub u64);

pub type Content = BTreeMap<String, Value>;

/// The token classes of NET-class.a. This executor holds process class and
/// an in-memory stand-in for data class: whole-content slots, no claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Process,
    Device,
    Data,
}

/// Reads the marking through a [`Reader`], so every read lands in the trace.
/// A closure (ruling 1.a), so a runtime IR lowers like Rust code.
pub type Read = Arc<dyn Fn(&mut Reader<'_>, &Subject) -> Option<Content> + Send + Sync>;

/// A pure firing: its outputs from its bound input, or a definite failure.
pub type PureBody = Arc<dyn Fn(&Subject, &Content) -> Result<Vec<Output>, Cause> + Send + Sync>;

/// The places and transitions of an ask for the work transition `T`
/// (UFT-adopt.a): the derived `Asks.T`, the manual `Answer.T` that puts the
/// `Answered.T` token, and the `Expired.T` token of an answer that no longer
/// applies, whose subject is the answer id.
pub fn asks_place(work: &str) -> String {
    format!("Asks.{work}")
}

pub fn answered_place(work: &str) -> String {
    format!("Answered.{work}")
}

pub fn answer_transition(work: &str) -> String {
    format!("Answer.{work}")
}

pub fn expired_place(work: &str) -> String {
    format!("Expired.{work}")
}

/// `Answered.T`: the [`hash_text`] of the ask the user saw. The token also
/// holds [`ANSWER_ID_KEY`], which keys the failure mark of `T` so a new
/// answer is a new mark key, and [`ANSWER_INPUT_KEY`], which no reader sees:
/// the executor adds it to the bound input of `T`.
pub const SEEN_FIELD: &str = "seen";
/// `Expired.T`: the [`expire_reason_text`].
pub const REASON_FIELD: &str = "reason";
/// `Asks.T`: the [`hash_text`] of the ask.
pub const HASH_FIELD: &str = "hash";

pub fn expire_reason_text(reason: ExpireReason) -> &'static str {
    match reason {
        ExpireReason::AskGone => "ask_gone",
        ExpireReason::SituationChanged => "situation_changed",
    }
}

fn expire_reason(text: &str) -> ExpireReason {
    [ExpireReason::AskGone, ExpireReason::SituationChanged]
        .into_iter()
        .find(|r| expire_reason_text(*r) == text)
        .unwrap_or_else(|| panic!("'{text}' is no expire reason"))
}

/// What the `Asks` row of an ask shows besides its hash.
#[derive(Clone, Debug, PartialEq)]
pub struct AskMeta {
    pub input: Option<TypeHint>,
    pub irreversible: bool,
    pub has_preview: bool,
}

pub enum PlaceKind {
    /// `candidate`: the place declares an apply transition, whose input this
    /// candidate place is (NET-cand.a).
    Stored { candidate: Option<Place> },
    /// No identity or version: a read of it traces its sources' content.
    Derived(Read),
}

pub struct PlaceDef {
    pub class: Class,
    pub kind: PlaceKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub place: Place,
    pub subject: Subject,
    pub content: Content,
}

impl Output {
    pub fn new(place: &str, subject: &Subject, content: Content) -> Self {
        Self {
            place: Place::new(place),
            subject: subject.clone(),
            content,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    pub text: String,
    /// A declared retry timer may fire the transition again on this input.
    pub retryable: bool,
}

impl Cause {
    pub fn definite(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retryable: false,
        }
    }

    pub fn retryable(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retryable: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Auto,
    Manual,
}

/// How an effect's unknown outcome is resolved (NET-fire.a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    /// The receiver dedups by occurrence id: the effect runs again under it.
    Keyed,
    /// A probe asks whether the goal holds and records that.
    StateProbe,
    /// The inputs park in [`OUTCOME_UNKNOWN`] until Verify, Retry or Drop.
    None,
}

#[derive(Clone)]
pub enum Body {
    Pure(PureBody),
    Effectful(Receiver),
}

impl Body {
    pub fn pure(
        body: impl Fn(&Subject, &Content) -> Result<Vec<Output>, Cause> + Send + Sync + 'static,
    ) -> Self {
        Self::Pure(Arc::new(body))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Retry {
    pub backoff: Duration,
    /// The count bound of a run: attempts, the first included.
    pub max_attempts: u32,
}

pub struct TransitionDef {
    pub name: Arc<str>,
    pub trigger: Trigger,
    pub consumes: Vec<Place>,
    pub reads: Vec<Place>,
    pub inhibits: Vec<Place>,
    /// The guard and the binding in one: `None` is a false guard, `Some` is
    /// the input the body sees. Its reads are the firing's read trace.
    pub bind: Read,
    pub body: Body,
    /// Outputs into these places are facts about the outside world: they
    /// commit even when the inputs are stale.
    pub facts: Vec<Place>,
    /// Gets a token when a run settles failed (failure port).
    pub on_failure: Option<Place>,
    /// Gets a token when the run timer ends an unsettled run (timeout port).
    pub on_timeout: Option<Place>,
    pub retry: Option<Retry>,
    pub run_timer: Option<Duration>,
    /// An attempt without a result this long after it started is orphaned.
    pub abandon: Option<Duration>,
}

impl TransitionDef {
    pub fn new(name: &str, trigger: Trigger, bind: Read, body: Body) -> Self {
        Self {
            name: Arc::from(name),
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
        self.consumes.push(Place::new(place));
        self
    }

    pub fn reads(mut self, place: &str) -> Self {
        self.reads.push(Place::new(place));
        self
    }

    pub fn inhibits(mut self, place: &str) -> Self {
        self.inhibits.push(Place::new(place));
        self
    }

    pub fn fact(mut self, place: &str) -> Self {
        self.facts.push(Place::new(place));
        self
    }

    pub fn on_failure(mut self, place: &str) -> Self {
        self.on_failure = Some(Place::new(place));
        self
    }

    pub fn on_timeout(mut self, place: &str) -> Self {
        self.on_timeout = Some(Place::new(place));
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

    fn running(&self) -> Place {
        Place::running(&self.name)
    }
}

/// A runtime net, built in Rust. [`Executor::new`] checks it.
#[derive(Default)]
pub struct NetDef {
    places: BTreeMap<Place, PlaceDef>,
    transitions: Vec<TransitionDef>,
    /// Keyed by the work transition.
    asks: BTreeMap<Arc<str>, AskMeta>,
}

impl NetDef {
    pub fn place(mut self, name: &str, class: Class) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class,
                kind: PlaceKind::Stored { candidate: None },
            },
        );
        self
    }

    /// A process place that declares an apply transition fed by `candidate`.
    pub fn place_with_candidate(mut self, name: &str, candidate: &str) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class: Class::Process,
                kind: PlaceKind::Stored {
                    candidate: Some(Place::new(candidate)),
                },
            },
        );
        self
    }

    pub fn derived(mut self, name: &str, derive: Read) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class: Class::Process,
                kind: PlaceKind::Derived(derive),
            },
        );
        self
    }

    pub fn transition(mut self, t: TransitionDef) -> Self {
        self.transitions.push(t);
        self
    }

    /// Declares `work` an ask: the net must hold its places and transitions
    /// ([`asks_place`] and the others).
    pub fn ask(mut self, work: &str, meta: AskMeta) -> Self {
        self.asks.insert(Arc::from(work), meta);
        self
    }

    /// `Answered.T` when `transition` is the work `T` of an ask.
    fn answered_of(&self, transition: &str) -> Option<Place> {
        self.asks
            .contains_key(transition)
            .then(|| Place::new(&answered_place(transition)))
    }

    fn is_answered(&self, place: &str) -> bool {
        self.asks.keys().any(|w| answered_place(w) == place)
    }

    /// The work `T` when `transition` is `Answer.T`.
    fn work_of_answer(&self, transition: &str) -> Option<Arc<str>> {
        self.asks
            .keys()
            .find(|w| answer_transition(w) == transition)
            .cloned()
    }

    fn place_def(&self, place: &str) -> &PlaceDef {
        self.places
            .get(place)
            .unwrap_or_else(|| panic!("the net reads place '{place}', which it does not declare"))
    }

    fn transition_def(&self, name: &str) -> &TransitionDef {
        self.transitions
            .iter()
            .find(|t| &*t.name == name)
            .unwrap_or_else(|| panic!("no transition '{name}' in the net"))
    }

    fn candidate_of(&self, place: &Place) -> Option<&Place> {
        match &self.place_def(place).kind {
            PlaceKind::Stored { candidate } => candidate.as_ref(),
            PlaceKind::Derived(_) => panic!("a firing writes the derived place '{place}'"),
        }
    }

    fn check(&mut self) -> Result<(), LoadError> {
        let mut names = BTreeSet::new();
        for t in &self.transitions {
            if !names.insert(t.name.clone()) {
                return Err(LoadError::DuplicateTransition(t.name.to_string()));
            }
        }
        for t in &self.transitions {
            for reserved in [
                t.running(),
                Place::retry_pending(&t.name),
                Place::outcome_unknown(&t.name),
            ] {
                if self.places.contains_key(&reserved) {
                    return Err(LoadError::ReservedPlace(reserved));
                }
                self.places.insert(
                    reserved,
                    PlaceDef {
                        class: Class::Process,
                        kind: PlaceKind::Stored { candidate: None },
                    },
                );
            }
        }
        let unknown = Place::new(OUTCOME_UNKNOWN);
        if self.places.contains_key(&unknown) {
            return Err(LoadError::ReservedPlace(unknown));
        }
        self.places.insert(
            unknown,
            PlaceDef {
                class: Class::Process,
                kind: PlaceKind::Stored { candidate: None },
            },
        );
        for (name, def) in &self.places {
            if def.class == Class::Device {
                return Err(LoadError::ClassNotSupported {
                    place: name.clone(),
                    class: def.class,
                });
            }
            if let PlaceKind::Stored { candidate: Some(c) } = &def.kind
                && !self.places.contains_key(c)
            {
                return Err(LoadError::UndeclaredPlace {
                    user: name.to_string(),
                    place: c.clone(),
                });
            }
        }
        for t in &self.transitions {
            let arcs = t.consumes.iter().chain(&t.reads).chain(&t.inhibits);
            let ports = t.facts.iter().chain(&t.on_failure).chain(&t.on_timeout);
            for p in arcs.clone().chain(ports) {
                if !self.places.contains_key(p) {
                    return Err(LoadError::UndeclaredPlace {
                        user: t.name.to_string(),
                        place: p.clone(),
                    });
                }
            }
            if let Some(p) = t
                .consumes
                .iter()
                .find(|p| self.places[*p].class == Class::Data)
            {
                return Err(LoadError::ConsumesData {
                    transition: t.name.to_string(),
                    place: p.clone(),
                });
            }
            for p in arcs {
                if matches!(self.places[p].kind, PlaceKind::Derived(_)) {
                    return Err(LoadError::ArcOnDerived {
                        transition: t.name.to_string(),
                        place: p.clone(),
                    });
                }
            }
            if t.trigger == Trigger::Auto && t.consumes.is_empty() && t.reads.is_empty() {
                return Err(LoadError::NoSubject(t.name.to_string()));
            }
            if matches!(t.body, Body::Effectful(Receiver::Keyed)) && t.retry.is_none() {
                return Err(LoadError::KeyedWithoutCountBound(t.name.to_string()));
            }
        }
        for work in self.asks.keys() {
            let place = |name: String, derived: bool| {
                self.places
                    .get(name.as_str())
                    .is_some_and(|d| matches!(d.kind, PlaceKind::Derived(_)) == derived)
                    .then_some(())
                    .ok_or(name)
            };
            let transition = |name: String, ok: &dyn Fn(&TransitionDef) -> bool| {
                self.transitions
                    .iter()
                    .any(|t| *t.name == *name && ok(t))
                    .then_some(())
                    .ok_or(name)
            };
            let answered = Place::new(&answered_place(work));
            place(asks_place(work), true)
                .and(place(answered_place(work), false))
                .and(place(expired_place(work), false))
                .and(transition(answer_transition(work), &|t| {
                    t.trigger == Trigger::Manual
                }))
                .and(transition(work.to_string(), &|t| {
                    t.consumes.contains(&answered)
                }))
                .map_err(|missing| LoadError::IncompleteAsk {
                    work: work.to_string(),
                    missing,
                })?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LoadError {
    #[error("transition '{0}' is declared twice")]
    DuplicateTransition(String),
    #[error("place '{0}' is reserved for the executor")]
    ReservedPlace(Place),
    #[error("'{user}' names place '{place}', which the net does not declare")]
    UndeclaredPlace { user: String, place: Place },
    #[error("place '{place}' is {class:?} class; this executor holds process and data class only")]
    ClassNotSupported { place: Place, class: Class },
    #[error(
        "transition '{transition}' consumes the data-class place '{place}'; data class has no claims yet (#168)"
    )]
    ConsumesData { transition: String, place: Place },
    #[error("transition '{transition}' has an arc on the derived place '{place}'")]
    ArcOnDerived { transition: String, place: Place },
    #[error("auto transition '{0}' has no consume or read arc to find its subjects by")]
    NoSubject(String),
    #[error("'{0}' has a Keyed receiver but no retry count bound")]
    KeyedWithoutCountBound(String),
    #[error(
        "the ask '{work}' lacks '{missing}': it needs a derived Asks, a stored Answered and Expired, a manual Answer, and a work that consumes Answered"
    )]
    IncompleteAsk { work: String, missing: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub id: u64,
    pub content: Content,
}

#[derive(Default)]
struct Slot {
    /// Changes only when a record or an outside write commits a change.
    version: u64,
    token: Option<Token>,
    /// Only the hidden-reservation twin sets it.
    reserved: bool,
}

/// The owned marking: at most one token per (place, subject), with a version
/// per (place, subject).
#[derive(Default)]
pub struct Tokens {
    slots: BTreeMap<Place, BTreeMap<Subject, Slot>>,
    next_id: u64,
}

impl Tokens {
    pub fn token(&self, place: &str, subject: &str) -> Option<&Token> {
        self.slot(place, subject).and_then(|s| s.token.as_ref())
    }

    pub fn version(&self, place: &str, subject: &str) -> u64 {
        self.slot(place, subject).map_or(0, |s| s.version)
    }

    /// The subjects holding a token in `place`.
    pub fn subjects(&self, place: &str) -> Vec<Subject> {
        self.slots.get(place).map_or_else(Vec::new, |m| {
            m.iter()
                .filter(|(_, s)| s.token.is_some())
                .map(|(k, _)| k.clone())
                .collect()
        })
    }

    fn slot(&self, place: &str, subject: &str) -> Option<&Slot> {
        self.slots.get(place).and_then(|m| m.get(subject))
    }

    fn slot_mut(&mut self, place: &Place, subject: &Subject) -> &mut Slot {
        self.slots
            .entry(place.clone())
            .or_default()
            .entry(subject.clone())
            .or_default()
    }

    /// Commits `content`; unchanged content is no change and no new version.
    fn put(&mut self, place: &Place, subject: &Subject, content: Content) {
        let id = self.next_id;
        let slot = self.slot_mut(place, subject);
        if slot.token.as_ref().is_some_and(|t| t.content == content) {
            return;
        }
        slot.token = Some(Token { id, content });
        slot.version += 1;
        self.next_id += 1;
    }

    /// Commits the removal of a token: a new version.
    fn remove(&mut self, place: &Place, subject: &Subject) {
        let slot = self.slot_mut(place, subject);
        assert!(
            slot.token.take().is_some(),
            "a removal of the empty {place} of {subject}"
        );
        slot.version += 1;
    }

    /// Places an executor-owned marker (Running, OutcomeUnknown): a move, not
    /// a commit, so no version changes.
    fn mark_slot(&mut self, place: &Place, subject: &Subject, content: Option<Content>) {
        let id = self.next_id;
        self.next_id += 1;
        self.slot_mut(place, subject).token = content.map(|content| Token { id, content });
    }
}

/// Everything a firing's enabledness check read, captured once.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    rows: BTreeMap<(Place, Subject), RowRead>,
}

#[derive(Clone, Debug, Default)]
struct RowRead {
    version: u64,
    present: bool,
    fields: BTreeMap<String, Option<Value>>,
}

impl Trace {
    fn row(&mut self, tokens: &Tokens, place: &str, subject: &str) -> &mut RowRead {
        self.rows
            .entry((Place::new(place), Subject::new(subject)))
            .or_insert_with(|| RowRead {
                version: tokens.version(place, subject),
                present: tokens.token(place, subject).is_some(),
                fields: BTreeMap::new(),
            })
    }

    fn as_value(&self) -> Value {
        Value::Array(
            self.rows
                .iter()
                .map(|((p, s), r)| {
                    Value::Object(HashMap::from([
                        ("place".to_string(), Value::String(p.to_string())),
                        ("subject".to_string(), Value::String(s.to_string())),
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

/// The view a guard, a binding or a derived place reads the marking through.
pub struct Reader<'a> {
    net: &'a NetDef,
    tokens: &'a Tokens,
    trace: &'a mut Trace,
    /// The [`ANSWER_INPUT_KEY`] of an `Answered` token reads as absent.
    hide_value: bool,
}

impl Reader<'_> {
    pub fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<Value> {
        let net = self.net;
        match &net.place_def(place).kind {
            PlaceKind::Derived(_) => self.derived(place, &Subject::new(subject))?.remove(field),
            PlaceKind::Stored { .. }
                if self.hide_value && field == ANSWER_INPUT_KEY && net.is_answered(place) =>
            {
                None
            }
            PlaceKind::Stored { .. } => {
                let value = self
                    .tokens
                    .token(place, subject)
                    .and_then(|t| t.content.get(field).cloned());
                self.trace
                    .row(self.tokens, place, subject)
                    .fields
                    .insert(field.to_string(), value.clone());
                value
            }
        }
    }

    pub fn present(&mut self, place: &str, subject: &str) -> bool {
        let net = self.net;
        match &net.place_def(place).kind {
            PlaceKind::Derived(_) => self.derived(place, &Subject::new(subject)).is_some(),
            PlaceKind::Stored { .. } => self.trace.row(self.tokens, place, subject).present,
        }
    }

    fn derived(&mut self, place: &str, s: &Subject) -> Option<Content> {
        let PlaceKind::Derived(derive) = &self.net.place_def(place).kind else {
            panic!("'{place}' is no derived place");
        };
        derive(self, s)
    }

    /// Runs `f` and returns, besides its result, the trace of what `f` read,
    /// e.g. for an [`ask_hash`] of a binding. The reads stay in this trace too.
    pub fn traced<R>(&mut self, f: impl FnOnce(&mut Reader<'_>) -> R) -> (R, Trace) {
        let mut sub = Trace::default();
        let r = f(&mut Reader {
            net: self.net,
            tokens: self.tokens,
            trace: &mut sub,
            hide_value: self.hide_value,
        });
        for (k, row) in &sub.rows {
            self.trace
                .rows
                .entry(k.clone())
                .or_insert_with(|| RowRead {
                    fields: BTreeMap::new(),
                    ..row.clone()
                })
                .fields
                .extend(row.fields.clone());
        }
        (r, sub)
    }

    /// A field as a string; `None` when absent or not a string.
    pub fn text(&mut self, place: &str, subject: &str, field: &str) -> Option<String> {
        match self.field(place, subject, field)? {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Why a transition is not enabled for a subject, read from the marking.
#[derive(Clone, Debug, PartialEq)]
pub enum WhyNot {
    /// `<T>.Running` holds a firing for this subject.
    InFlight,
    /// A pending backoff counts as running.
    RetryPending {
        due: Duration,
    },
    Needs {
        place: Place,
        held_by: Option<Holder>,
    },
    Inhibited(Place),
    Guard,
    /// A failure mark keyed by the current read trace (NET-mark.a).
    FailedOnThisInput(Cause),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Holder {
    Running(Arc<str>),
    OutcomeUnknown(OccurrenceId),
}

/// The failure mark of one (transition, subject), reason `Failed`.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// The content hash of the read trace of the failed attempt.
    pub key: blake3::Hash,
    pub cause: Cause,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Settled {
    Done,
    /// The effect happened over stale inputs: its input is consumed and each
    /// row output took the route listed with it.
    OverChangedInputs {
        cause: Cause,
        routes: Vec<Routed>,
    },
    Failed(Cause),
    OutcomeUnknown(Cause),
}

/// Where one row output of a stale effect went.
#[derive(Clone, Debug, PartialEq)]
pub struct Routed {
    pub place: Place,
    pub subject: Subject,
    pub route: Route,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// Written to a data-class place over the changed inputs.
    Applied,
    /// Kept in this candidate place for an apply transition.
    Candidate(Place),
    /// Not written: a process place with no candidate.
    Withheld,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectRequest {
    pub attempt: AttemptId,
    pub occurrence: OccurrenceId,
    pub transition: Arc<str>,
    pub subject: Subject,
    pub input: Content,
    /// A `StateProbe` check: answer `Done` when the goal holds.
    pub probe: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectResult {
    Done(Vec<Output>),
    Failed(Cause),
    Unknown(Cause),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Completion {
    Recorded,
    /// The occurrence is already settled: the result only closes its attempt.
    DroppedLate,
    /// A late result that settles nothing: the occurrence stays parked or
    /// running.
    StillUnknown,
}

pub enum Resolution {
    /// The effect happened: record these outputs.
    Verify(Vec<Output>),
    /// Run the effect again under the same occurrence id.
    Retry,
    /// Discard the inputs.
    Drop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Accepted(pub OccurrenceId);

/// The answer of an external call and the effect requests of the auto
/// transitions it caused, run to quiescence.
#[derive(Clone, Debug, PartialEq)]
#[must_use = "the effect requests must run"]
pub struct Quiesced<T> {
    pub value: T,
    pub requests: Vec<EffectRequest>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FireError {
    #[error("'{0}' is not a manual transition")]
    NotManual(String),
    #[error("occurrence '{0}' is already known")]
    DuplicateOccurrence(OccurrenceId),
    #[error("'{transition}' is not enabled for '{subject}': {why:?}")]
    NotEnabled {
        transition: String,
        subject: Subject,
        why: WhyNot,
    },
    #[error("the net is still enabled after {0} rounds of one call")]
    NoQuiescence(usize),
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ExecError {
    #[error("the net is still enabled after {0} rounds of one call")]
    NoQuiescence(usize),
    #[error("no attempt {0:?} is running")]
    UnknownAttempt(AttemptId),
    #[error("occurrence '{0}' is not parked in OutcomeUnknown")]
    NotParked(OccurrenceId),
    #[error("a newer firing of {transition} for {subject} is in flight")]
    RetryWhileRunning {
        transition: String,
        subject: Subject,
    },
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("no occurrence '{0}'")]
pub struct UnknownOccurrence(pub OccurrenceId);

/// What an undo of an answer did (XQ2.a), disclosed in full.
#[derive(Clone, Debug, PartialEq)]
pub struct Undone {
    /// The `Answered` token of the answer was still there and is gone.
    pub withdrawn: bool,
    /// One entry per (place, subject) the work's commit touched.
    pub reversed: Vec<Reversal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reversal {
    pub place: Place,
    pub subject: Subject,
    pub outcome: ReversalOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReversalOutcome {
    /// The content before the work is back.
    Restored,
    /// The work put the token; it is gone.
    Removed,
    /// A later commit changed the place: left as it is.
    Changed,
    /// A later firing holds the token: left as it is, like a commit.
    Held(Holder),
    /// Data class has no undo here (inc0): left as it is.
    NotProcessClass,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum UndoError {
    #[error("no occurrence '{0}'")]
    UnknownOccurrence(OccurrenceId),
    #[error("'{0}' is no answer")]
    NotAnAnswer(OccurrenceId),
    #[error("the work of '{0}' has not settled")]
    WorkUnsettled(OccurrenceId),
    #[error("'{0}' is already undone")]
    AlreadyUndone(OccurrenceId),
    #[error("the net is still enabled after {0} rounds of one call")]
    NoQuiescence(usize),
}

/// A variant of the executor that breaks exactly one rule, so a property
/// test of that rule shows red for its own reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Twin {
    /// A take flags the token instead of moving it into `<T>.Running`.
    HiddenReservation,
    MarkByVersion,
    NoMark,
    NoCas,
    NoCountBound,
    UnknownIsFailure,
    /// A parked unknown outcome settles the run failed and puts the failure
    /// port token, so the next producer fires while the effect may be done.
    UnknownSettlesFailed,
    /// A stale record of an effect that happened returns the input and marks
    /// it, as if the effect had failed.
    StaleDoneReturnsInput,
    /// A late `Done` of an abandoned, parked attempt is dropped, so the
    /// occurrence stays unknown although the effect happened.
    DropLateDone,
    /// A late `Failed` of an abandoned, parked attempt is dropped, so the
    /// input stays parked although the effect did not happen.
    DropLateFailed,
    /// A `Failed` settles the occurrence failed although another of its
    /// attempts may have taken effect.
    FailedSettlesAlone,
    /// A reader sees the value of an `Answered` token, so a guard can read it.
    ValueBeforeBind,
    /// The answer id stays out of the work's read trace, so a new answer to
    /// the same situation has the old failure mark key.
    OccurrenceNotInTrace,
    /// Undo puts the consumed `Answered` token back, so the work fires again.
    UndoRestoresAnswered,
    /// `settled(answer)` is the outcome of the `Answer` firing itself.
    SettledFromAnswer,
    /// An external call returns without firing the auto transitions it enabled.
    NoQuiescence,
    /// Quiescing skips the auto transitions that consume a failure port token.
    KeepWorkFailed,
}

/// What is known about one effect attempt of an occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    Running,
    /// Abandoned, or a late `Unknown`: a result may still come.
    Open,
    Unknown,
    Done,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Running,
    Done,
    Failed,
    Unknown,
}

/// The outcome of an occurrence, derived from all its attempts (NET-fire.a):
/// Done when any attempt is Done, Failed only when every one is Failed.
fn outcome(attempts: &BTreeMap<AttemptId, Seen>) -> Outcome {
    let seen = || attempts.values().copied();
    if seen().any(|s| s == Seen::Done) {
        Outcome::Done
    } else if seen().any(|s| s == Seen::Running) {
        Outcome::Running
    } else if seen().all(|s| s == Seen::Failed) {
        Outcome::Failed
    } else {
        Outcome::Unknown
    }
}

struct Run {
    started: Duration,
    attempts: u32,
}

/// A condition the net discloses for a subject (NET-rev1 termination):
/// blocked and visible, never a panic.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// The answer of `work` for `subject` applied, but the automatic
    /// transitions it caused were still enabled after `steps` rounds.
    NoQuiescence {
        work: Arc<str>,
        subject: Subject,
        steps: usize,
    },
}

#[derive(Clone)]
struct Attempt {
    occurrence: OccurrenceId,
    transition: Arc<str>,
    subject: Subject,
    taken: Vec<(Place, Token)>,
    trace: Trace,
    input: Content,
    started: Duration,
    probe: bool,
}

pub struct Executor {
    net: NetDef,
    tokens: Tokens,
    marks: BTreeMap<(Arc<str>, Subject), Mark>,
    runs: BTreeMap<(Arc<str>, Subject), Run>,
    in_flight: BTreeMap<AttemptId, Attempt>,
    parked: BTreeMap<OccurrenceId, Attempt>,
    attempts: BTreeMap<OccurrenceId, BTreeMap<AttemptId, Seen>>,
    // ponytail: settled occurrences are kept forever; add retention with the
    // first long-running user.
    occurrences: BTreeMap<OccurrenceId, Option<Settled>>,
    /// Answer id → (work transition, subject).
    answers: BTreeMap<Arc<str>, (Arc<str>, Subject)>,
    undone: BTreeSet<Arc<str>>,
    deltas: BTreeMap<OccurrenceId, Delta>,
    conditions: Vec<Condition>,
    requests: Vec<EffectRequest>,
    now: Duration,
    next_attempt: u64,
    next_minted: u64,
    #[cfg(feature = "naive-twins")]
    twin: Option<Twin>,
}

/// What the commit of a work occurrence of an ask changed: the content before
/// it and the version after it, per (place, subject).
type Delta = BTreeMap<(Place, Subject), (Option<Content>, u64)>;

/// The work occurrence of an answer (ruling 3.b): derived from the answer
/// id, so a Keyed receiver dedups it and the settled chain finds it.
pub fn work_occurrence(work: &str, answer: &str) -> OccurrenceId {
    OccurrenceId::Work {
        transition: Arc::from(work),
        answer: Arc::from(answer),
    }
}

/// The ask identity: blake3 over the transition, the subject and the
/// content of `trace` (never its versions), with the encoding of the
/// failure mark keys (EXEC-inc0 D.a).
pub fn ask_hash(transition: &str, subject: &Subject, trace: &Trace) -> AskHash {
    let mut h = blake3::Hasher::new();
    encode_str(&mut h, transition);
    encode_str(&mut h, subject);
    encode_rows(&mut h, trace, false);
    AskHash(*h.finalize().as_bytes())
}

/// The hex text of an ask hash, as the `Asks` row and `Answered` carry it.
pub fn hash_text(hash: &AskHash) -> String {
    blake3::Hash::from_bytes(hash.0).to_hex().to_string()
}

pub fn hash_from_text(text: &str) -> AskHash {
    let hash = blake3::Hash::from_hex(text)
        .unwrap_or_else(|e| panic!("'{text}' is no ask hash text: {e}"));
    AskHash(*hash.as_bytes())
}

impl Executor {
    pub fn new(mut net: NetDef) -> Result<Self, LoadError> {
        net.check()?;
        Ok(Self {
            net,
            tokens: Tokens::default(),
            marks: BTreeMap::new(),
            runs: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            parked: BTreeMap::new(),
            attempts: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            answers: BTreeMap::new(),
            undone: BTreeSet::new(),
            deltas: BTreeMap::new(),
            conditions: Vec::new(),
            requests: Vec::new(),
            now: Duration::ZERO,
            next_attempt: 0,
            next_minted: 0,
            #[cfg(feature = "naive-twins")]
            twin: None,
        })
    }

    #[cfg(feature = "naive-twins")]
    pub fn with_twin(mut self, twin: Twin) -> Self {
        self.twin = Some(twin);
        self
    }

    #[cfg(feature = "naive-twins")]
    fn twin(&self, twin: Twin) -> bool {
        self.twin == Some(twin)
    }

    #[cfg(not(feature = "naive-twins"))]
    fn twin(&self, _: Twin) -> bool {
        false
    }

    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }

    pub fn mark(&self, transition: &str, subject: &str) -> Option<&Mark> {
        self.marks
            .get(&(Arc::from(transition), Subject::new(subject)))
    }

    pub fn marks(&self) -> impl Iterator<Item = (&str, &Subject, &Mark)> {
        self.marks.iter().map(|((t, s), m)| (&**t, s, m))
    }

    /// The occurrences with an attempt running now.
    pub fn in_flight(&self) -> impl Iterator<Item = (AttemptId, &str, &Subject, &OccurrenceId)> {
        self.in_flight
            .iter()
            .map(|(id, a)| (*id, &*a.transition, &a.subject, &a.occurrence))
    }

    /// Every occurrence the executor started, with its outcome (`None` while
    /// it runs), oldest id order.
    pub fn occurrences(&self) -> impl Iterator<Item = (&OccurrenceId, Option<&Settled>)> {
        self.occurrences.iter().map(|(o, s)| (o, s.as_ref()))
    }

    /// `Ok(None)` while the occurrence runs.
    ///
    /// An answer settles through the settled chain (NET-manual.a): the
    /// outcome of its work occurrence, `Failed(NoLongerApplies)` when the
    /// answer expired, `Failed` when it was undone before the work ran, and
    /// `None` while it waits.
    pub fn settled(&self, occurrence: &OccurrenceId) -> Result<Option<Settled>, UnknownOccurrence> {
        let own = self
            .occurrences
            .get(occurrence)
            .cloned()
            .ok_or_else(|| UnknownOccurrence(occurrence.clone()))?;
        let OccurrenceId::Caller(id) = occurrence else {
            return Ok(own);
        };
        let Some((work, _)) = self.answers.get(id) else {
            return Ok(own);
        };
        if own != Some(Settled::Done) || self.twin(Twin::SettledFromAnswer) {
            return Ok(own);
        }
        if let Some(settled) = self.occurrences.get(&work_occurrence(work, id)) {
            return Ok(settled.clone());
        }
        if self.tokens.token(&expired_place(work), id).is_some() {
            return Ok(Some(Settled::Failed(Cause::definite(
                UftError::NoLongerApplies.to_string(),
            ))));
        }
        if self.undone.contains(id) {
            return Ok(Some(Settled::Failed(Cause::definite(
                "undone before the work ran",
            ))));
        }
        Ok(None)
    }

    /// Fires `Answer.T` with the user value; the answer id is the occurrence.
    /// A net that does not quiesce is a broken net and panics here.
    pub fn answer(
        &mut self,
        call: AnswerCall,
        now: Duration,
    ) -> Result<Quiesced<AnswerId>, UftError> {
        let work = call.transition.0.as_str();
        if !self.net.asks.contains_key(work) {
            return Err(UftError::UnknownTransition(call.transition));
        }
        let id = call.answer.0.as_str();
        let value = Content::from([
            (ANSWER_ID_KEY.to_string(), Value::String(id.to_string())),
            (SEEN_FIELD.to_string(), Value::String(hash_text(&call.seen))),
            (
                ANSWER_INPUT_KEY.to_string(),
                call.value.unwrap_or(Value::Null),
            ),
        ]);
        match self.fire(
            &answer_transition(work),
            &call.subject.0,
            OccurrenceId::caller(id),
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
                    work: Arc::from(work),
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
            ) => unreachable!("the load checks that '{work}' has a manual pure Answer: {e}"),
        }
    }

    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }

    /// The state of an answer for the UI and MCP (UFT-adopt.a).
    pub fn answer_settlement(&self, answer: &AnswerId) -> Result<AnswerSettlement, UftError> {
        let id = answer.0.as_str();
        let Some((work, _)) = self.answers.get(id) else {
            return Err(UftError::UnknownAnswer(answer.clone()));
        };
        if self.undone.contains(id) {
            return Ok(AnswerSettlement::Undone);
        }
        let occurrence = work_occurrence(work, id);
        Ok(match self.occurrences.get(&occurrence) {
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
            None => match self.tokens.token(&expired_place(work), id) {
                Some(t) => {
                    let Some(Value::String(reason)) = t.content.get(REASON_FIELD) else {
                        panic!("the {} token of '{id}' has no reason", expired_place(work));
                    };
                    AnswerSettlement::Expired {
                        reason: expire_reason(reason),
                    }
                }
                None => AnswerSettlement::Waiting,
            },
        })
    }

    /// The rows of every `Asks.T`, in work and subject order.
    pub fn asks(&self) -> Vec<AskRow> {
        let mut rows = Vec::new();
        for (work, meta) in &self.net.asks {
            for (subject, row) in self.derive_all(&asks_place(work)) {
                let Some(Value::String(hash)) = row.get(HASH_FIELD) else {
                    panic!(
                        "the {} row of {subject} has no '{HASH_FIELD}'",
                        asks_place(work)
                    );
                };
                rows.push(AskRow {
                    transition: TransitionName(work.to_string()),
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

    /// Evaluates the derived `place` for one subject, outside any firing.
    pub fn derive(&self, place: &str, subject: &Subject) -> Option<Content> {
        self.reader(&mut Trace::default()).derived(place, subject)
    }

    /// Evaluates the derived `place` for every subject a stored place holds.
    pub fn derive_all(&self, place: &str) -> Vec<(Subject, Content)> {
        let subjects: BTreeSet<Subject> = self
            .net
            .places
            .iter()
            .filter(|(p, d)| {
                matches!(d.kind, PlaceKind::Stored { .. }) && p.as_str() != OUTCOME_UNKNOWN
            })
            .flat_map(|(p, _)| self.tokens.subjects(p))
            .collect();
        subjects
            .into_iter()
            .filter_map(|s| self.derive(place, &s).map(|c| (s, c)))
            .collect()
    }

    /// Undoes an answer (XQ2.a) in one step: removes its `Answered` token
    /// when still there and reverses the process-class delta its work
    /// committed. It never puts `Answered` back, so the work never fires
    /// again from an undo. A row changed since the work, or held by a later
    /// firing, is left as it is.
    pub fn undo(
        &mut self,
        answer: &OccurrenceId,
        now: Duration,
    ) -> Result<Quiesced<Undone>, UndoError> {
        self.advance(now);
        if !self.occurrences.contains_key(answer) {
            return Err(UndoError::UnknownOccurrence(answer.clone()));
        }
        let OccurrenceId::Caller(id) = answer else {
            return Err(UndoError::NotAnAnswer(answer.clone()));
        };
        let Some((work, subject)) = self.answers.get(id).cloned() else {
            return Err(UndoError::NotAnAnswer(answer.clone()));
        };
        if self.undone.contains(id) {
            return Err(UndoError::AlreadyUndone(answer.clone()));
        }
        let occurrence = work_occurrence(&work, id);
        if let Some(None | Some(Settled::OutcomeUnknown(_))) = self.occurrences.get(&occurrence) {
            return Err(UndoError::WorkUnsettled(occurrence));
        }
        let answered = Place::new(&answered_place(&work));
        let withdrawn = self
            .tokens
            .token(&answered, &subject)
            .is_some_and(|t| t.content.get(ANSWER_ID_KEY) == Some(&Value::String(id.to_string())));
        if withdrawn {
            self.tokens.remove(&answered, &subject);
        }
        let mut reversed = Vec::new();
        for ((place, subject), (before, version)) in
            self.deltas.get(&occurrence).cloned().unwrap_or_default()
        {
            let outcome = if self.net.places[&place].class != Class::Process {
                ReversalOutcome::NotProcessClass
            } else if self.tokens.version(&place, &subject) != version {
                ReversalOutcome::Changed
            } else if let Some(holder) = self.holder(&place, &subject) {
                ReversalOutcome::Held(holder)
            } else if let Some(content) = before {
                self.tokens.put(&place, &subject, content);
                ReversalOutcome::Restored
            } else {
                self.tokens.remove(&place, &subject);
                ReversalOutcome::Removed
            };
            reversed.push(Reversal {
                place,
                subject,
                outcome,
            });
        }
        self.undone.insert(id.clone());
        let requests = self.quiesce(now).map_err(UndoError::NoQuiescence)?;
        Ok(Quiesced {
            value: Undone {
                withdrawn,
                reversed,
            },
            requests,
        })
    }

    /// An outside commit (a user edit, an ingest): a new version when the
    /// content changes.
    pub fn write(
        &mut self,
        place: &str,
        subject: &str,
        content: Content,
        now: Duration,
    ) -> Result<Quiesced<()>, ExecError> {
        assert!(
            matches!(self.net.place_def(place).kind, PlaceKind::Stored { .. }),
            "an outside write to the derived place '{place}'"
        );
        self.advance(now);
        self.tokens
            .put(&Place::new(place), &Subject::new(subject), content);
        self.quiesced((), now)
    }

    pub fn why_not(&self, transition: &str, subject: &str) -> Option<WhyNot> {
        let t = self.net.transition_def(transition);
        self.decide(t, &Subject::new(subject)).err()
    }

    /// Runs timers due at `now`, then the auto transitions to quiescence.
    /// Returns the effects to run.
    pub fn step(&mut self, now: Duration) -> Result<Vec<EffectRequest>, ExecError> {
        self.advance(now);
        self.timers(now);
        self.quiesce(now).map_err(ExecError::NoQuiescence)
    }

    fn quiesced<T>(&mut self, value: T, now: Duration) -> Result<Quiesced<T>, ExecError> {
        let requests = self.quiesce(now).map_err(ExecError::NoQuiescence)?;
        Ok(Quiesced { value, requests })
    }

    fn takes_failure_port(&self, t: &TransitionDef) -> bool {
        self.net
            .transitions
            .iter()
            .filter_map(|d| d.on_failure.as_ref())
            .any(|port| t.consumes.contains(port))
    }

    /// Fires every enabled auto transition until none is enabled, and takes
    /// the effect requests; `Err` holds the rounds run. It fires no timer.
    fn quiesce(&mut self, now: Duration) -> Result<Vec<EffectRequest>, usize> {
        if self.twin(Twin::NoQuiescence) {
            return Ok(std::mem::take(&mut self.requests));
        }
        for _ in 0..MAX_ROUNDS {
            let mut started = Vec::new();
            for i in 0..self.net.transitions.len() {
                let t = &self.net.transitions[i];
                if t.trigger != Trigger::Auto
                    || self.twin(Twin::KeepWorkFailed) && self.takes_failure_port(t)
                {
                    continue;
                }
                let name = t.name.clone();
                for s in self.subjects(t) {
                    let t = &self.net.transitions[i];
                    if let Ok((trace, input)) = self.decide(t, &s) {
                        let occurrence = match self.net.answered_of(&name) {
                            Some(p) => match self.answered_field(&p, &s, ANSWER_ID_KEY) {
                                Value::String(answer) => work_occurrence(&name, &answer),
                                other => panic!("the {p} answer id of {s} is {other:?}"),
                            },
                            None => self.mint(&name),
                        };
                        started.push(self.start(&name, s, occurrence, trace, input, now));
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

    /// A manual fire: `ACCEPTED(occurrence)` at bind (NET-manual.a). A pure
    /// body runs at once; an effect is a request in the result.
    ///
    /// `value` is the user input (UFT-adopt.a; empty for none). It joins the
    /// bound input after the bind, so no guard reads it; a field the bind
    /// also produces is an impossible state and panics. A fire of `Answer.T`
    /// takes a [`OccurrenceId::Caller`] id equal to its [`ANSWER_ID_KEY`].
    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
        value: Content,
        now: Duration,
    ) -> Result<Quiesced<Accepted>, FireError> {
        self.advance(now);
        let t = self.net.transition_def(transition);
        if t.trigger != Trigger::Manual {
            return Err(FireError::NotManual(transition.to_string()));
        }
        if self.occurrences.contains_key(&occurrence) {
            return Err(FireError::DuplicateOccurrence(occurrence));
        }
        let subject = Subject::new(subject);
        let (trace, mut input) = self
            .decide(t, &subject)
            .map_err(|why| FireError::NotEnabled {
                transition: transition.to_string(),
                subject: subject.clone(),
                why,
            })?;
        let name = t.name.clone();
        if let Some(work) = self.net.work_of_answer(transition) {
            let OccurrenceId::Caller(id) = &occurrence else {
                panic!("'{transition}' fired with the non-caller occurrence {occurrence}");
            };
            assert_eq!(
                value.get(ANSWER_ID_KEY),
                Some(&Value::String(id.to_string())),
                "'{transition}' fired with an answer id other than its occurrence"
            );
            self.answers.insert(id.clone(), (work, subject.clone()));
        }
        for (k, v) in value {
            merge_value(&mut input, &k, v, transition);
        }
        let id = self.start(&name, subject, occurrence.clone(), trace, input, now);
        self.run_body(id, now);
        let requests = self.quiesce(now).map_err(FireError::NoQuiescence)?;
        Ok(Quiesced {
            value: Accepted(occurrence),
            requests,
        })
    }

    /// Hands back the result of an effect request. The result
    /// closes only its own attempt; the occurrence settles by [`outcome`] over
    /// all its attempts. A late result of an abandoned attempt counts the same
    /// way, and a late `Unknown` keeps that attempt open.
    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult,
        now: Duration,
    ) -> Result<Quiesced<Completion>, ExecError> {
        self.advance(now);
        let live = self.in_flight.remove(&attempt);
        let occurrence = match &live {
            Some(a) => a.occurrence.clone(),
            None => self
                .attempts
                .iter()
                .find(|(_, seen)| seen.get(&attempt) == Some(&Seen::Open))
                .map(|(o, _)| o.clone())
                .ok_or(ExecError::UnknownAttempt(attempt))?,
        };
        let seen = match (&result, &live) {
            (EffectResult::Done(_), _) => Seen::Done,
            (EffectResult::Failed(_), _) => Seen::Failed,
            (EffectResult::Unknown(_), Some(_)) => Seen::Unknown,
            (EffectResult::Unknown(_), None) => Seen::Open,
        };
        let completion = self.observe(&occurrence, attempt, seen, live, result, now);
        self.quiesced(completion, now)
    }

    /// Records what one attempt showed, then settles its occurrence by
    /// [`outcome`]. `live` is the attempt when it was in flight until now.
    fn observe(
        &mut self,
        occurrence: &OccurrenceId,
        id: AttemptId,
        seen: Seen,
        live: Option<Attempt>,
        result: EffectResult,
        now: Duration,
    ) -> Completion {
        let (seen, result) = match result {
            EffectResult::Unknown(cause) if self.twin(Twin::UnknownIsFailure) => {
                (Seen::Failed, EffectResult::Failed(cause))
            }
            result => (seen, result),
        };
        let late = live.is_none();
        if late
            && (seen == Seen::Done && self.twin(Twin::DropLateDone)
                || seen == Seen::Failed && self.twin(Twin::DropLateFailed))
        {
            return Completion::DroppedLate;
        }
        let probe = live.as_ref().is_some_and(|a| a.probe);
        let settles_alone = seen == Seen::Failed && self.twin(Twin::FailedSettlesAlone);
        let attempts = self
            .attempts
            .get_mut(occurrence)
            .expect("every attempt is in its occurrence's set");
        attempts.insert(id, seen);
        if seen == Seen::Failed && (probe || settles_alone) {
            // A probe reads the goal: no attempt that already answered took effect.
            for s in attempts.values_mut() {
                if *s == Seen::Unknown || settles_alone {
                    *s = Seen::Failed;
                }
            }
        }
        if matches!(
            self.occurrences.get(occurrence),
            Some(Some(
                Settled::Done | Settled::OverChangedInputs { .. } | Settled::Failed(_)
            ))
        ) {
            if let Some(a) = live {
                self.release_running(&a.transition, &a.subject);
            }
            return Completion::DroppedLate;
        }
        if let (Some(a), EffectResult::Unknown(_)) = (&live, &result)
            && let Some(probe) = self.continuation(a)
        {
            self.issue(a.clone(), probe, now);
            return Completion::Recorded;
        }
        match outcome(&self.attempts[occurrence]) {
            Outcome::Running => Completion::StillUnknown,
            Outcome::Done => {
                let EffectResult::Done(outputs) = result else {
                    panic!("{occurrence} is Done by an earlier attempt but did not settle");
                };
                let a = self.attempt_of(occurrence, live);
                self.record(a, outputs, now);
                Completion::Recorded
            }
            Outcome::Failed => {
                let EffectResult::Failed(cause) = result else {
                    panic!("{occurrence} is Failed by earlier attempts but did not settle");
                };
                let cause = if probe {
                    Cause::definite(format!("the goal does not hold: {}", cause.text))
                } else {
                    cause
                };
                let a = self.attempt_of(occurrence, live);
                self.fail(a, cause, now);
                Completion::Recorded
            }
            Outcome::Unknown => {
                let Some(a) = live else {
                    return Completion::StillUnknown;
                };
                let cause = match result {
                    EffectResult::Unknown(cause) => cause,
                    EffectResult::Failed(cause) => Cause::definite(format!(
                        "{}; an earlier attempt may have taken effect",
                        cause.text
                    )),
                    EffectResult::Done(_) => unreachable!("a Done settles Done"),
                };
                self.park(a, cause, now);
                Completion::Recorded
            }
        }
    }

    /// The new attempt the receiver kind runs after an unknown outcome:
    /// `Some(probe)`, or `None` when the outcome parks.
    fn continuation(&mut self, a: &Attempt) -> Option<bool> {
        let t = self.net.transition_def(&a.transition);
        let Body::Effectful(receiver) = &t.body else {
            panic!("a pure firing of '{}' has an unknown outcome", a.transition);
        };
        let (retry, receiver) = (t.retry, *receiver);
        match receiver {
            Receiver::Keyed if !a.probe => {
                let unbounded = self.twin(Twin::NoCountBound);
                let run = self
                    .runs
                    .get_mut(&(a.transition.clone(), a.subject.clone()))?;
                let retry = retry?;
                (unbounded || run.attempts < retry.max_attempts).then(|| {
                    run.attempts += 1;
                    false
                })
            }
            Receiver::StateProbe if !a.probe => Some(true),
            _ => None,
        }
    }

    /// The attempt an occurrence settles with: the live one, else the parked
    /// one, else a copy of the one still in flight.
    fn attempt_of(&mut self, occurrence: &OccurrenceId, live: Option<Attempt>) -> Attempt {
        if let Some(a) = live {
            return a;
        }
        if self.parked.contains_key(occurrence) {
            let a = self.unpark(occurrence).expect("parked");
            self.occupy_running(&a);
            return a;
        }
        self.in_flight
            .values()
            .find(|a| &a.occurrence == occurrence)
            .cloned()
            .expect("an unsettled occurrence is parked or has an attempt in flight")
    }

    /// Takes a parked occurrence out of [`OUTCOME_UNKNOWN`].
    fn unpark(&mut self, occurrence: &OccurrenceId) -> Result<Attempt, ExecError> {
        let a = self
            .parked
            .remove(occurrence)
            .ok_or_else(|| ExecError::NotParked(occurrence.clone()))?;
        self.tokens.mark_slot(
            &Place::new(OUTCOME_UNKNOWN),
            &Subject::new(&occurrence.to_string()),
            None,
        );
        let (transition, subject) = (a.transition.clone(), a.subject.clone());
        self.mark_unknown(&transition, &subject);
        Ok(a)
    }

    /// Resolves an occurrence parked in [`OUTCOME_UNKNOWN`].
    pub fn resolve(
        &mut self,
        occurrence: &OccurrenceId,
        resolution: Resolution,
        now: Duration,
    ) -> Result<Quiesced<()>, ExecError> {
        self.advance(now);
        if let (Resolution::Retry, Some(a)) = (&resolution, self.parked.get(occurrence))
            && self
                .tokens
                .token(&Place::running(&a.transition), &a.subject)
                .is_some()
        {
            return Err(ExecError::RetryWhileRunning {
                transition: a.transition.to_string(),
                subject: a.subject.clone(),
            });
        }
        let a = self.unpark(occurrence)?;
        match resolution {
            Resolution::Verify(outputs) => {
                self.occupy_running(&a);
                self.record(a, outputs, now);
            }
            Resolution::Retry => {
                self.occupy_running(&a);
                self.occurrences.insert(occurrence.clone(), None);
                self.issue(a, false, now);
            }
            Resolution::Drop => {
                for (p, _) in &a.taken {
                    self.commit_consumption(p, &a.subject);
                }
                let cause = Cause::definite("dropped by the user");
                self.occurrences
                    .insert(occurrence.clone(), Some(Settled::Failed(cause.clone())));
                self.run_failed(&a.transition, &a.subject, &cause, now);
            }
        }
        self.quiesced((), now)
    }

    fn advance(&mut self, now: Duration) {
        assert!(
            now >= self.now,
            "the clock went back from {:?} to {now:?}",
            self.now
        );
        self.now = now;
    }

    fn mint(&mut self, transition: &Arc<str>) -> OccurrenceId {
        self.next_minted += 1;
        OccurrenceId::Minted {
            transition: transition.clone(),
            n: self.next_minted,
        }
    }

    fn subjects(&self, t: &TransitionDef) -> BTreeSet<Subject> {
        t.consumes
            .iter()
            .chain(&t.reads)
            .flat_map(|p| self.tokens.subjects(p))
            .collect()
    }

    fn available(&self, place: &Place, subject: &Subject) -> bool {
        self.tokens
            .slot(place, subject)
            .is_some_and(|s| s.token.is_some() && !s.reserved)
    }

    fn holder(&self, place: &Place, subject: &Subject) -> Option<Holder> {
        let holds = |a: &Attempt| &a.subject == subject && a.taken.iter().any(|(p, _)| p == place);
        if let Some(a) = self.in_flight.values().find(|a| holds(a)) {
            return Some(Holder::Running(a.transition.clone()));
        }
        self.parked
            .iter()
            .find(|(_, a)| holds(a))
            .map(|(o, _)| Holder::OutcomeUnknown(o.clone()))
    }

    /// The enabledness check that decides a firing: its read trace and its
    /// bound input, or the first reason it cannot fire.
    fn decide(&self, t: &TransitionDef, s: &Subject) -> Result<(Trace, Content), WhyNot> {
        if self.tokens.token(&t.running(), s).is_some() {
            return Err(WhyNot::InFlight);
        }
        if let Some(due) = self.retry_due(&t.name, s) {
            return Err(WhyNot::RetryPending { due });
        }
        let present = |p: &Place| self.tokens.token(p, s).is_some();
        if let Some(p) = t.consumes.iter().find(|p| !self.available(p, s)) {
            return Err(WhyNot::Needs {
                place: p.clone(),
                held_by: self.holder(p, s),
            });
        }
        if let Some(p) = t.reads.iter().find(|p| !present(p)) {
            return Err(WhyNot::Needs {
                place: p.clone(),
                held_by: self.holder(p, s),
            });
        }
        if let Some(p) = t.inhibits.iter().find(|p| present(p)) {
            return Err(WhyNot::Inhibited(p.clone()));
        }
        let mut trace = Trace::default();
        for p in t.consumes.iter().chain(&t.reads).chain(&t.inhibits) {
            trace.row(&self.tokens, p, s);
        }
        let answered = self.net.answered_of(&t.name);
        if let Some(p) = &answered
            && !self.twin(Twin::OccurrenceNotInTrace)
        {
            let answer = self.answered_field(p, s, ANSWER_ID_KEY);
            trace
                .row(&self.tokens, p, s)
                .fields
                .insert(ANSWER_ID_KEY.to_string(), Some(answer));
        }
        let mut input = (t.bind)(&mut self.reader(&mut trace), s).ok_or(WhyNot::Guard)?;
        if t.trigger == Trigger::Auto
            && let Some(mark) = self.marks.get(&(t.name.clone(), s.clone()))
            && mark.key == self.key(&trace)
        {
            return Err(WhyNot::FailedOnThisInput(mark.cause.clone()));
        }
        if let Some(p) = &answered {
            let value = self.answered_field(p, s, ANSWER_INPUT_KEY);
            merge_value(&mut input, ANSWER_INPUT_KEY, value, &t.name);
        }
        Ok((trace, input))
    }

    /// A field of the `Answered` token a work firing consumes.
    fn answered_field(&self, place: &Place, subject: &Subject, field: &str) -> Value {
        self.tokens
            .token(place, subject)
            .expect("decide checked every consumed token")
            .content
            .get(field)
            .cloned()
            .unwrap_or_else(|| panic!("the {place} token of {subject} has no '{field}'"))
    }

    fn reader<'a>(&'a self, trace: &'a mut Trace) -> Reader<'a> {
        Reader {
            net: &self.net,
            tokens: &self.tokens,
            trace,
            hide_value: !self.twin(Twin::ValueBeforeBind),
        }
    }

    /// The content hash of a read trace: blake3 over a canonical encoding of
    /// what was read, never of versions.
    fn key(&self, trace: &Trace) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        encode_rows(&mut h, trace, self.twin(Twin::MarkByVersion));
        h.finalize()
    }

    /// The take: inputs move into `<T>.Running`, with no version change.
    fn start(
        &mut self,
        transition: &Arc<str>,
        subject: Subject,
        occurrence: OccurrenceId,
        trace: Trace,
        input: Content,
        now: Duration,
    ) -> AttemptId {
        let t = self.net.transition_def(transition);
        let consumes = t.consumes.clone();
        let mut taken = Vec::new();
        for p in consumes {
            let hidden = self.twin(Twin::HiddenReservation);
            let slot = self.tokens.slot_mut(&p, &subject);
            let token = if hidden {
                slot.reserved = true;
                slot.token.clone()
            } else {
                slot.token.take()
            }
            .expect("decide checked every consumed token");
            taken.push((p, token));
        }
        self.runs
            .entry((transition.clone(), subject.clone()))
            .or_insert(Run {
                started: now,
                attempts: 0,
            })
            .attempts += 1;
        self.occurrences.insert(occurrence.clone(), None);
        let a = Attempt {
            occurrence,
            transition: transition.clone(),
            subject,
            taken,
            trace,
            input,
            started: now,
            probe: false,
        };
        self.occupy_running(&a);
        self.insert_attempt(a)
    }

    fn retry_due(&self, transition: &str, subject: &Subject) -> Option<Duration> {
        let token = self
            .tokens
            .token(&Place::retry_pending(transition), subject)?;
        let Some(Value::Integer(nanos)) = token.content.get("due") else {
            panic!("'{transition}.RetryPending' of {subject} has no integer due");
        };
        Some(Duration::from_nanos(
            u64::try_from(*nanos).expect("a due time is not negative"),
        ))
    }

    fn set_retry_due(&mut self, key: &(Arc<str>, Subject), due: Option<Duration>) {
        let content = due.map(|due| {
            let nanos = i64::try_from(due.as_nanos()).expect("a due time fits i64 nanoseconds");
            Content::from([("due".to_string(), Value::Integer(nanos))])
        });
        self.tokens
            .mark_slot(&Place::retry_pending(&key.0), &key.1, content);
    }

    fn end_run(&mut self, key: &(Arc<str>, Subject)) {
        self.runs.remove(key);
        self.set_retry_due(key, None);
    }

    fn occupy_running(&mut self, a: &Attempt) {
        self.tokens.mark_slot(
            &Place::running(&a.transition),
            &a.subject,
            Some(Content::from([(
                "occurrence".to_string(),
                Value::String(a.occurrence.to_string()),
            )])),
        );
    }

    /// `<T>.Running` empties when its last attempt ends. A resolved parked
    /// occurrence can end while a newer one of the same subject still runs.
    fn release_running(&mut self, transition: &Arc<str>, subject: &Subject) {
        match self
            .in_flight
            .values()
            .find(|o| &o.transition == transition && &o.subject == subject)
            .cloned()
        {
            Some(other) => self.occupy_running(&other),
            None => self
                .tokens
                .mark_slot(&Place::running(transition), subject, None),
        }
    }

    fn insert_attempt(&mut self, a: Attempt) -> AttemptId {
        let id = AttemptId(self.next_attempt);
        self.next_attempt += 1;
        self.in_flight.insert(id, a);
        id
    }

    /// A pure body runs and records now; an effect becomes a request.
    fn run_body(&mut self, id: AttemptId, now: Duration) {
        let a = &self.in_flight[&id];
        match &self.net.transition_def(&a.transition).body {
            Body::Pure(body) => {
                let body = body.clone();
                let a = self.in_flight.remove(&id).expect("just inserted");
                match body(&a.subject, &a.input) {
                    Ok(outputs) => self.record(a, outputs, now),
                    Err(cause) => self.fail(a, cause, now),
                }
            }
            Body::Effectful(_) => {
                let a = a.clone();
                self.request(id, &a);
            }
        }
    }

    fn request(&mut self, id: AttemptId, a: &Attempt) {
        self.attempts
            .entry(a.occurrence.clone())
            .or_default()
            .insert(id, Seen::Running);
        self.requests.push(EffectRequest {
            attempt: id,
            occurrence: a.occurrence.clone(),
            transition: a.transition.clone(),
            subject: a.subject.clone(),
            input: a.input.clone(),
            probe: a.probe,
        });
    }

    /// A new attempt of an occurrence that is already in `<T>.Running`.
    fn issue(&mut self, mut a: Attempt, probe: bool, now: Duration) {
        a.started = now;
        a.probe = probe;
        let id = self.insert_attempt(a.clone());
        self.request(id, &a);
    }

    /// The first row of the trace whose version moved since it was read.
    fn stale(&self, trace: &Trace) -> Option<(Place, Subject)> {
        if self.twin(Twin::NoCas) {
            return None;
        }
        trace
            .rows
            .iter()
            .find(|((p, s), r)| self.tokens.version(p, s) != r.version)
            .map(|(k, _)| k.clone())
    }

    /// The record never clobbers: facts commit, rows compare-and-set against
    /// the read trace (NET-fire.a). A stale effect happened, so each row goes
    /// to its place's candidate, applies on a data-class place, or is withheld,
    /// and a mark on its input stays: the effect never runs twice on it.
    fn record(&mut self, a: Attempt, outputs: Vec<Output>, now: Duration) {
        let t = self.net.transition_def(&a.transition);
        let pure = matches!(t.body, Body::Pure(_));
        let (facts, rows): (Vec<_>, Vec<_>) = outputs
            .into_iter()
            .partition(|o| t.facts.contains(&o.place));
        for f in facts {
            self.tokens.put(&f.place, &f.subject, f.content);
        }
        let Some((place, subject)) = self.stale(&a.trace) else {
            self.marks
                .remove(&(a.transition.clone(), a.subject.clone()));
            return self.commit(a, rows);
        };
        let cause = Cause::definite(format!(
            "stale inputs: {place} of {subject} changed since '{}' read it",
            a.transition
        ));
        if pure {
            self.rollback(&a);
            if let Some(run) = self
                .runs
                .get_mut(&(a.transition.clone(), a.subject.clone()))
            {
                run.attempts -= 1;
            }
            self.occurrences
                .insert(a.occurrence.clone(), Some(Settled::Failed(cause)));
            return;
        }
        let trace = a.trace.as_value();
        let (mut kept, mut routes) = (Vec::new(), Vec::new());
        for o in &rows {
            let route = if let Some(c) = self.net.candidate_of(&o.place) {
                let base = a
                    .trace
                    .rows
                    .get(&(o.place.clone(), o.subject.clone()))
                    .map_or(Value::Null, |r| fields_value(&r.fields));
                let content = Content::from([
                    ("base".to_string(), base),
                    (
                        "output".to_string(),
                        Value::Object(o.content.clone().into_iter().collect()),
                    ),
                    ("trace".to_string(), trace.clone()),
                ]);
                kept.push(Output::new(c, &o.subject, content));
                Route::Candidate(c.clone())
            } else if self.net.places[&o.place].class == Class::Data {
                kept.push(o.clone());
                Route::Applied
            } else {
                Route::Withheld
            };
            routes.push(Routed {
                place: o.place.clone(),
                subject: o.subject.clone(),
                route,
            });
        }
        if (rows.is_empty() || routes.iter().any(|r| r.route == Route::Withheld))
            && self.twin(Twin::StaleDoneReturnsInput)
        {
            return self.fail(a, cause, now);
        }
        let run_key = (a.transition.clone(), a.subject.clone());
        if self
            .marks
            .get(&run_key)
            .is_some_and(|m| m.key != self.key(&a.trace))
        {
            self.marks.remove(&run_key);
        }
        let occurrence = a.occurrence.clone();
        self.commit(a, kept);
        self.occurrences.insert(
            occurrence,
            Some(Settled::OverChangedInputs { cause, routes }),
        );
    }

    fn commit(&mut self, a: Attempt, rows: Vec<Output>) {
        let mut before = BTreeMap::new();
        if let OccurrenceId::Work { .. } = &a.occurrence {
            let answered = self.net.answered_of(&a.transition);
            let keep_answered = self.twin(Twin::UndoRestoresAnswered);
            for (p, token) in &a.taken {
                let rewritten = self
                    .tokens
                    .slot(p, &a.subject)
                    .is_some_and(|s| s.token.is_some() && !s.reserved);
                if !rewritten && (keep_answered || Some(p) != answered.as_ref()) {
                    before.insert((p.clone(), a.subject.clone()), Some(token.content.clone()));
                }
            }
            for o in &rows {
                before
                    .entry((o.place.clone(), o.subject.clone()))
                    .or_insert_with(|| {
                        self.tokens
                            .token(&o.place, &o.subject)
                            .map(|t| t.content.clone())
                    });
            }
        }
        for (p, _) in &a.taken {
            self.commit_consumption(p, &a.subject);
        }
        for o in rows {
            self.tokens.put(&o.place, &o.subject, o.content);
        }
        if !before.is_empty() {
            let delta = before
                .into_iter()
                .map(|((p, s), c)| {
                    let version = self.tokens.version(&p, &s);
                    ((p, s), (c, version))
                })
                .collect();
            self.deltas.insert(a.occurrence.clone(), delta);
        }
        self.release_running(&a.transition, &a.subject);
        self.end_run(&(a.transition.clone(), a.subject.clone()));
        self.occurrences.insert(a.occurrence, Some(Settled::Done));
    }

    /// The consumed token leaves the net; its place changes only now. A newer
    /// token written meanwhile already carries a newer version.
    fn commit_consumption(&mut self, place: &Place, subject: &Subject) {
        let slot = self.tokens.slot_mut(place, subject);
        if slot.reserved {
            slot.reserved = false;
            slot.token = None;
            slot.version += 1;
        } else if slot.token.is_none() {
            slot.version += 1;
        }
    }

    /// The inputs go back with no version change. A token written meanwhile
    /// is newer than the taken one, so it stays and the taken one is dropped.
    fn rollback(&mut self, a: &Attempt) {
        for (p, token) in &a.taken {
            let slot = self.tokens.slot_mut(p, &a.subject);
            slot.reserved = false;
            if slot.token.is_none() {
                slot.token = Some(token.clone());
            }
        }
        self.release_running(&a.transition, &a.subject);
    }

    fn set_mark(&mut self, a: &Attempt, cause: &Cause) {
        if self.twin(Twin::NoMark) {
            return;
        }
        let key = self.key(&a.trace);
        self.marks.insert(
            (a.transition.clone(), a.subject.clone()),
            Mark {
                key,
                cause: cause.clone(),
            },
        );
    }

    /// A definite failure: the inputs go back and the failure mark is set.
    fn fail(&mut self, a: Attempt, cause: Cause, now: Duration) {
        self.rollback(&a);
        self.set_mark(&a, &cause);
        self.occurrences
            .insert(a.occurrence.clone(), Some(Settled::Failed(cause.clone())));
        self.run_failed(&a.transition, &a.subject, &cause, now);
    }

    /// Schedules a retry while the count bound allows, else settles the run
    /// failed and puts the failure port token.
    fn run_failed(
        &mut self,
        transition: &Arc<str>,
        subject: &Subject,
        cause: &Cause,
        now: Duration,
    ) {
        let t = self.net.transition_def(transition);
        let (retry, on_failure) = (t.retry, t.on_failure.clone());
        let key = (transition.clone(), subject.clone());
        let unbounded = self.twin(Twin::NoCountBound);
        let Some(run) = self.runs.get(&key) else {
            return;
        };
        if cause.retryable
            && let Some(retry) = retry
            && (unbounded || run.attempts < retry.max_attempts)
        {
            self.set_retry_due(&key, Some(now + retry.backoff));
            return;
        }
        self.end_run(&key);
        if let Some(port) = on_failure {
            self.tokens.put(&port, subject, cause_content(cause));
        }
    }

    fn park(&mut self, a: Attempt, cause: Cause, now: Duration) {
        self.release_running(&a.transition, &a.subject);
        self.tokens.mark_slot(
            &Place::new(OUTCOME_UNKNOWN),
            &Subject::new(&a.occurrence.to_string()),
            Some(Content::from([
                (
                    "transition".to_string(),
                    Value::String(a.transition.to_string()),
                ),
                ("subject".to_string(), Value::String(a.subject.to_string())),
                ("cause".to_string(), Value::String(cause.text.clone())),
            ])),
        );
        self.set_mark(&a, &cause);
        self.occurrences.insert(
            a.occurrence.clone(),
            Some(Settled::OutcomeUnknown(cause.clone())),
        );
        // The run stays open: only Drop or the run timer lets the next producer fire.
        if self.twin(Twin::UnknownSettlesFailed) {
            let settle = Cause::definite(format!("outcome unknown: {}", cause.text));
            self.run_failed(&a.transition, &a.subject, &settle, now);
        }
        let (transition, subject) = (a.transition.clone(), a.subject.clone());
        self.parked.insert(a.occurrence.clone(), a);
        self.mark_unknown(&transition, &subject);
    }

    /// `<T>.OutcomeUnknown` of a subject holds while any occurrence of `T`
    /// for it is parked, so the run is not idle: the token names one of them.
    fn mark_unknown(&mut self, transition: &Arc<str>, subject: &Subject) {
        let occurrence = self
            .parked
            .values()
            .find(|a| &a.transition == transition && &a.subject == subject)
            .map(|a| a.occurrence.to_string());
        self.tokens.mark_slot(
            &Place::outcome_unknown(transition),
            subject,
            occurrence.map(|o| Content::from([("occurrence".to_string(), Value::String(o))])),
        );
    }

    fn timers(&mut self, now: Duration) {
        let hung: Vec<AttemptId> = self
            .in_flight
            .iter()
            .filter(|(_, a)| {
                self.net
                    .transition_def(&a.transition)
                    .abandon
                    .is_some_and(|d| a.started + d <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in hung {
            let a = self.in_flight.remove(&id).expect("listed above");
            let cause = Cause::definite(format!("abandoned: no result since {:?}", a.started));
            let occurrence = a.occurrence.clone();
            self.observe(
                &occurrence,
                id,
                Seen::Open,
                Some(a),
                EffectResult::Unknown(cause),
                now,
            );
        }
        let keys: Vec<_> = self.runs.keys().cloned().collect();
        for key in keys {
            let t = self.net.transition_def(&key.0);
            let (on_timeout, run_timer) = (t.on_timeout.clone(), t.run_timer);
            let run = &self.runs[&key];
            if run_timer.is_some_and(|d| run.started + d <= now) {
                self.end_run(&key);
                if let Some(port) = on_timeout {
                    let cause = Cause::definite(format!("run timer of '{}' expired", key.0));
                    self.tokens.put(&port, &key.1, cause_content(&cause));
                }
            } else if self.retry_due(&key.0, &key.1).is_some_and(|due| due <= now) {
                self.set_retry_due(&key, None);
                self.marks.remove(&key);
            }
        }
    }
}

fn cause_content(cause: &Cause) -> Content {
    Content::from([("cause".to_string(), Value::String(cause.text.clone()))])
}

/// Adds the user value to a bound input after the bind, so no guard reads it.
fn merge_value(input: &mut Content, field: &str, value: Value, transition: &str) {
    assert!(
        input.insert(field.to_string(), value).is_none(),
        "the user input field '{field}' of '{transition}' collides with a bound field"
    );
}

fn encode_rows(h: &mut blake3::Hasher, trace: &Trace, with_version: bool) {
    for ((p, s), row) in &trace.rows {
        encode_str(h, p.as_str());
        encode_str(h, s.as_str());
        h.update(&[u8::from(row.present)]);
        if with_version {
            h.update(&row.version.to_le_bytes());
        }
        for (field, value) in &row.fields {
            encode_str(h, field);
            match value {
                None => {
                    h.update(&[0]);
                }
                Some(v) => {
                    h.update(&[1]);
                    encode_value(h, v);
                }
            }
        }
    }
}

fn encode_str(h: &mut blake3::Hasher, s: &str) {
    h.update(&(s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}

fn encode_value(h: &mut blake3::Hasher, v: &Value) {
    match v {
        Value::Removed(_) => panic!("a token holds a removal write intent"),
        Value::String(s) => {
            h.update(&[1]);
            encode_str(h, s);
        }
        Value::Integer(i) => {
            h.update(&[2]);
            h.update(&i.to_le_bytes());
        }
        Value::Float(f) => {
            h.update(&[3]);
            h.update(&f.to_bits().to_le_bytes());
        }
        Value::Boolean(b) => {
            h.update(&[4, u8::from(*b)]);
        }
        Value::DateTime(s) => {
            h.update(&[5]);
            encode_str(h, s);
        }
        Value::Json(s) => {
            h.update(&[6]);
            encode_str(h, s);
        }
        Value::Array(items) => {
            h.update(&[7]);
            h.update(&(items.len() as u64).to_le_bytes());
            for item in items {
                encode_value(h, item);
            }
        }
        Value::Object(map) => {
            h.update(&[8]);
            h.update(&(map.len() as u64).to_le_bytes());
            let sorted: BTreeMap<_, _> = map.iter().collect();
            for (k, item) in sorted {
                encode_str(h, k);
                encode_value(h, item);
            }
        }
        Value::Null => {
            h.update(&[9]);
        }
    }
}
