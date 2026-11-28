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
//! Answers (UFT-adopt.a): [`Executor::answer_ask`] fires the manual `Answer.T`
//! with the user value. The value joins the input of the work `T` after its
//! bind, so no guard reads it. The answer id is in the read trace of `T`, so a
//! new answer is a new failure mark key. [`Executor::settled`] of an answer
//! follows its work occurrence ([`work_occurrence`]), and [`Executor::undo`]
//! withdraws the answer and reverses its work's process-class delta in one
//! step. [`Executor::derive`] and [`ask_hash_with`] read the derived places
//! outside a firing.

use std::borrow::Borrow;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::ids::AnswerId;
use crate::ids::AskHash;
pub use crate::ids::OccurrenceId;

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

/// The one row of a pool place: every subject takes from it and returns to it.
pub const POOL_SUBJECT: &str = "pool";

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

    /// `<T>.UndoPending`: a token keyed by an answer id while the undo of
    /// that answer still owes rows a later firing held. Its `held`
    /// field lists them, each with the holder to name if that holder used it.
    pub fn undo_pending(transition: &str) -> Self {
        Self::new(&format!("{transition}.UndoPending"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttemptId(pub u64);

pub type Record<V> = BTreeMap<String, V>;

/// The token classes of NET-class.a. This executor holds process class and
/// an in-memory stand-in for data class: whole-content slots, no claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Process,
    Device,
    Data,
}

/// Reads the marking through a [`Reader`], so every read lands in the trace.
/// A closure (ruling 1.a), so a runtime IR lowers like Rust code. It reads
/// nothing else: the executor decides it again only when a row it read changes.
pub type Read<V> = Arc<dyn Fn(&mut Reader<'_, V>, &Subject) -> Option<Record<V>> + Send + Sync>;

/// A pure firing: its outputs from its bound input, or a definite failure.
pub type PureBody<V> =
    Arc<dyn Fn(&Subject, &Record<V>) -> Result<Vec<Output<V>>, Cause> + Send + Sync>;

/// The places and transition of an ask on a work transition: the derived
/// `offer` of the rows a user may answer, the manual `answer` transition that
/// puts the `answered` token the work consumes, and the `expired` place of an
/// answer that no longer applies, whose subject is the answer id.
#[derive(Clone, Debug)]
pub struct Ask {
    pub offer: Place,
    pub answer: Arc<str>,
    pub answered: Place,
    pub expired: Place,
}

/// The fields of an answer token the executor itself reads.
#[derive(Clone, Copy, Debug)]
pub struct AnswerKeys {
    /// The answer id: a text that keys the work's failure mark, so a new
    /// answer is a new mark key.
    pub id: &'static str,
    /// The user value: no reader sees it; the executor adds it to the bound
    /// input of the work.
    pub input: &'static str,
}

/// How answer tokens look as net data.
pub struct AnswerCodec<V> {
    pub keys: AnswerKeys,
    /// An `answered` token from net data, or the field it lacks.
    pub parse: fn(Record<V>) -> Result<Answered<V>, &'static str>,
    /// The field an `expired` token lacks.
    pub check_expired: fn(&Record<V>) -> Result<(), &'static str>,
    /// The failure cause of an answer whose ask expired.
    pub expired_cause: fn() -> String,
}

impl<V> Clone for AnswerCodec<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for AnswerCodec<V> {}

/// The field a bind reads the cause of a [`Payload::Failure`] token by.
pub const FAILURE_CAUSE_FIELD: &str = "cause";

#[derive(Clone)]
pub enum PlaceKind<V> {
    /// `candidate`: the place declares an apply transition, whose input this
    /// candidate place is (NET-cand.a).
    Stored { candidate: Option<Place> },
    /// No identity or version: a read of it traces its sources' content.
    Derived(Read<V>),
}

/// How many tokens one (place, subject) row holds (REV2-cand.c), and what a
/// put with no compare-and-set behind it may do to the row (322/E3.b). A fact
/// into a row its own firing read compares against the token it read whatever
/// the capacity says (322/E3.c), so the capacity decides the other puts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capacity {
    /// One, kept: a put that no compare-and-set backs refuses rather than
    /// drop the held token (REV2-cand.b). The default.
    Single,
    /// One, replaced: a fact put replaces the held token, because the net
    /// declares this row the writing transitions' own output (322/E3.b).
    Register,
    /// Any number, oldest first: a put appends and a firing binds and
    /// consumes the oldest, so no put ever drops a waiting token.
    Fifo,
}

#[derive(Clone)]
pub struct PlaceDef<V> {
    pub class: Class,
    pub kind: PlaceKind<V>,
    pub capacity: Capacity,
    /// `N` interchangeable tokens shared by every subject, on a
    /// [`Capacity::Fifo`] place.
    pub pool: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output<V> {
    pub place: Place,
    pub subject: Subject,
    pub content: Record<V>,
}

impl<V: Clone + PartialEq + fmt::Debug> Output<V> {
    pub fn new(place: &str, subject: &Subject, content: Record<V>) -> Self {
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
pub enum Body<V> {
    Pure(PureBody<V>),
    Effectful(Receiver),
}

impl<V: Clone + PartialEq + fmt::Debug> Body<V> {
    pub fn pure(
        body: impl Fn(&Subject, &Record<V>) -> Result<Vec<Output<V>>, Cause> + Send + Sync + 'static,
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

#[derive(Clone)]
pub struct TransitionDef<V> {
    pub name: Arc<str>,
    pub trigger: Trigger,
    pub consumes: Vec<Place>,
    pub reads: Vec<Place>,
    pub inhibits: Vec<Place>,
    /// The places it produces into. Among the enabled producers of one place
    /// only the first in `priority` order fires in a round (ADR 0032 §9.1).
    pub produces: Vec<Place>,
    /// The higher fires first; on equal priority, the earlier in the net.
    pub priority: i32,
    /// The guard and the binding in one: `None` is a false guard, `Some` is
    /// the input the body sees. Its reads are the firing's read trace.
    pub bind: Read<V>,
    pub body: Body<V>,
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

impl<V: Clone + PartialEq + fmt::Debug> TransitionDef<V> {
    pub fn new(name: &str, trigger: Trigger, bind: Read<V>, body: Body<V>) -> Self {
        Self {
            name: Arc::from(name),
            trigger,
            consumes: Vec::new(),
            reads: Vec::new(),
            inhibits: Vec::new(),
            produces: Vec::new(),
            priority: 0,
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

    pub fn produces(mut self, place: &str) -> Self {
        self.produces.push(Place::new(place));
        self
    }

    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
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

/// How the executor meets the token value `V`: plain functions the host
/// passes, so `V` needs no trait.
pub struct Host<V> {
    /// The bytes of a value in a mark key and an ask hash. Law: two values
    /// that are `==` encode to the same bytes.
    pub encode: fn(&V) -> Vec<u8>,
    /// A text as a value: the cause field of a port, an answer id.
    pub text: fn(&str) -> V,
    /// The text a value holds, if it is a text.
    pub as_text: fn(&V) -> Option<&str>,
    /// One probe fact from a JSON line: its subject (the `subject_field`)
    /// and its record, or why the line is not a fact.
    pub fact: FactFn<V>,
    pub answer: AnswerCodec<V>,
}

pub type FactFn<V> = fn(serde_json::Value, &str) -> Result<(Subject, Record<V>), String>;

impl<V> Clone for Host<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for Host<V> {}

/// A runtime net, built in Rust. [`Executor::new`] checks it.
#[derive(Clone)]
pub struct NetDef<V> {
    host: Host<V>,
    places: BTreeMap<Place, PlaceDef<V>>,
    transitions: Vec<TransitionDef<V>>,
    /// Keyed by the work transition.
    asks: BTreeMap<Arc<str>, Ask>,
    /// The places [`NetDef::check`] declares for the executor's own markers.
    owned: BTreeSet<Place>,
    /// The latest `now` whose every timer deadline fits i64 nanoseconds, the
    /// form a deadline is stored in; set by [`NetDef::check`].
    last_now: Duration,
}

impl<V: Clone + PartialEq + fmt::Debug> NetDef<V> {
    pub fn new(host: Host<V>) -> Self {
        Self {
            host,
            places: BTreeMap::new(),
            transitions: Vec::new(),
            asks: BTreeMap::new(),
            owned: BTreeSet::new(),
            last_now: Duration::ZERO,
        }
    }

    pub fn place(mut self, name: &str, class: Class) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class,
                kind: PlaceKind::Stored { candidate: None },
                capacity: Capacity::Single,
                pool: None,
            },
        );
        self
    }

    /// A single-token place whose fact put replaces the held token: the net
    /// declares the row the writing transitions' own output (322/E3.b). The
    /// `OnDisk` and `Base` places of the write-back net are registers.
    pub fn register(mut self, name: &str, class: Class) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class,
                kind: PlaceKind::Stored { candidate: None },
                capacity: Capacity::Register,
                pool: None,
            },
        );
        self
    }

    /// A place whose row holds a FIFO of tokens: every candidate place is one
    /// (REV2-cand.c).
    pub fn fifo(mut self, name: &str, class: Class) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class,
                kind: PlaceKind::Stored { candidate: None },
                capacity: Capacity::Fifo,
                pool: None,
            },
        );
        self
    }

    /// A [`fifo`](Self::fifo) place that starts with `size` interchangeable
    /// tokens, shared by every subject: a firing that consumes it takes the
    /// oldest, and a put returns one.
    pub fn pool(mut self, name: &str, class: Class, size: u32) -> Self {
        self = self.fifo(name, class);
        self.places.get_mut(name).expect("just inserted").pool = Some(size);
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
                capacity: Capacity::Single,
                pool: None,
            },
        );
        self
    }

    pub fn derived(mut self, name: &str, derive: Read<V>) -> Self {
        self.places.insert(
            Place::new(name),
            PlaceDef {
                class: Class::Process,
                kind: PlaceKind::Derived(derive),
                capacity: Capacity::Single,
                pool: None,
            },
        );
        self
    }

    pub fn transition(mut self, t: TransitionDef<V>) -> Self {
        self.transitions.push(t);
        self
    }

    /// Declares `work` an ask: the net must hold the places and transition
    /// `ask` names.
    pub fn ask(mut self, work: &str, ask: Ask) -> Self {
        self.asks.insert(Arc::from(work), ask);
        self
    }

    /// The `answered` place when `transition` is the work of an ask.
    fn answered_of(&self, transition: &str) -> Option<Place> {
        self.asks.get(transition).map(|a| a.answered.clone())
    }

    fn is_answered(&self, place: &str) -> bool {
        self.asks.values().any(|a| *a.answered == *place)
    }

    /// The work when `transition` is the answer transition of an ask.
    fn work_of_answer(&self, transition: &str) -> Option<Arc<str>> {
        self.asks
            .iter()
            .find(|(_, a)| *a.answer == *transition)
            .map(|(w, _)| w.clone())
    }

    fn place_def(&self, place: &str) -> &PlaceDef<V> {
        self.places
            .get(place)
            .unwrap_or_else(|| panic!("the net reads place '{place}', which it does not declare"))
    }

    fn transition_def(&self, name: &str) -> &TransitionDef<V> {
        self.find_transition(name).unwrap_or_else(|e| panic!("{e}"))
    }

    fn find_transition(&self, name: &str) -> Result<&TransitionDef<V>, UnknownTransition> {
        self.transitions
            .iter()
            .find(|t| &*t.name == name)
            .ok_or_else(|| UnknownTransition(name.to_string()))
    }

    /// A place a write or retraction from outside the net may change.
    fn outside_place(&self, place: &str) -> Result<(), ExecError> {
        match self.places.get(place).map(|d| &d.kind) {
            None => Err(ExecError::UndeclaredPlace(place.to_string())),
            Some(PlaceKind::Derived(_)) => Err(ExecError::DerivedPlace(place.to_string())),
            Some(PlaceKind::Stored { .. }) if self.owned.contains(place) => {
                Err(ExecError::ExecutorPlace(place.to_string()))
            }
            Some(PlaceKind::Stored { .. }) => Ok(()),
        }
    }

    /// The answer field an `Answered` or `Expired` token in `place` lacks:
    /// the work's firing reads the answer id and the value, the lowering the
    /// seen hash, and the answer's settlement the expiry reason.
    fn missing_answer_field(&self, place: &str, content: &Record<V>) -> Option<&'static str> {
        if self.is_answered(place) {
            (self.host.answer.parse)(content.clone()).err()
        } else if self.asks.values().any(|a| *a.expired == *place) {
            (self.host.answer.check_expired)(content).err()
        } else {
            None
        }
    }

    /// A token for net data in `place`; an answer must have passed
    /// [`Self::missing_answer_field`].
    fn payload(&self, place: &str, content: Record<V>) -> Payload<V> {
        if self.is_answered(place) {
            Payload::Answered((self.host.answer.parse)(content).expect("checked at the write"))
        } else {
            Payload::Record(content)
        }
    }

    /// Why a firing may not put `outputs`: a place an outside write may not
    /// write either.
    fn refused_output(&self, outputs: &[Output<V>]) -> Option<ExecError> {
        outputs
            .iter()
            .find_map(|o| match self.outside_place(&o.place) {
                Err(e) => Some(e),
                Ok(()) => self
                    .missing_answer_field(&o.place, &o.content)
                    .map(|field| ExecError::AnswerField {
                        place: o.place.to_string(),
                        field,
                    }),
            })
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
                Place::undo_pending(&t.name),
            ] {
                if self.places.contains_key(&reserved) {
                    return Err(LoadError::ReservedPlace(reserved));
                }
                self.owned.insert(reserved.clone());
                self.places.insert(
                    reserved,
                    PlaceDef {
                        class: Class::Process,
                        kind: PlaceKind::Stored { candidate: None },
                        capacity: Capacity::Single,
                        pool: None,
                    },
                );
            }
        }
        let unknown = Place::new(OUTCOME_UNKNOWN);
        if self.places.contains_key(&unknown) {
            return Err(LoadError::ReservedPlace(unknown));
        }
        self.owned.insert(unknown.clone());
        self.places.insert(
            unknown,
            PlaceDef {
                class: Class::Process,
                kind: PlaceKind::Stored { candidate: None },
                capacity: Capacity::Single,
                pool: None,
            },
        );
        for (name, def) in &self.places {
            if def.class == Class::Device {
                return Err(LoadError::ClassNotSupported {
                    place: name.clone(),
                    class: def.class,
                });
            }
            if let PlaceKind::Stored { candidate: Some(c) } = &def.kind {
                let Some(cand) = self.places.get(c) else {
                    return Err(LoadError::UndeclaredPlace {
                        user: name.to_string(),
                        place: c.clone(),
                    });
                };
                if cand.capacity != Capacity::Fifo {
                    return Err(LoadError::CandidateNotFifo {
                        user: name.to_string(),
                        place: c.clone(),
                    });
                }
            }
        }
        for t in &self.transitions {
            if let Some((first, second, place)) = repeated_input([
                ("consumes", &t.consumes),
                ("reads", &t.reads),
                ("inhibits", &t.inhibits),
            ]) {
                return Err(LoadError::InputTwice {
                    transition: t.name.to_string(),
                    first,
                    second,
                    place: Place::new(place),
                });
            }
            let arcs = t.consumes.iter().chain(&t.reads).chain(&t.inhibits);
            let ports = t.facts.iter().chain(&t.on_failure).chain(&t.on_timeout);
            for p in arcs.clone().chain(ports.clone()) {
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
            for p in arcs.chain(ports) {
                if matches!(self.places[p].kind, PlaceKind::Derived(_)) {
                    return Err(LoadError::ArcOnDerived {
                        transition: t.name.to_string(),
                        place: p.clone(),
                    });
                }
            }
            // A read or an inhibitor arc on an executor marker is valid: "not
            // while T runs" inhibits `T.Running`.
            let writes = t.consumes.iter().chain(&t.produces).chain(&t.facts);
            if let Some(p) = writes
                .chain(&t.on_failure)
                .chain(&t.on_timeout)
                .find(|p| self.owned.contains(*p))
            {
                return Err(LoadError::WritesExecutorPlace {
                    transition: t.name.to_string(),
                    place: p.clone(),
                });
            }
            if let Some((p, field)) = t
                .on_failure
                .iter()
                .chain(&t.on_timeout)
                .find_map(|p| Some((p, self.missing_answer_field(p, &Record::<V>::new())?)))
            {
                return Err(LoadError::PortLacksAnswerField {
                    transition: t.name.to_string(),
                    place: p.clone(),
                    field,
                });
            }
            if t.trigger == Trigger::Auto && t.consumes.is_empty() && t.reads.is_empty() {
                return Err(LoadError::NoSubject(t.name.to_string()));
            }
            if matches!(t.body, Body::Effectful(Receiver::Keyed)) && t.retry.is_none() {
                return Err(LoadError::KeyedWithoutCountBound(t.name.to_string()));
            }
        }
        let longest = self
            .transitions
            .iter()
            .flat_map(|t| [t.retry.map(|r| r.backoff), t.run_timer, t.abandon])
            .flatten()
            .max()
            .unwrap_or_default();
        self.last_now = MAX_NOW.checked_sub(longest).ok_or_else(|| {
            let t = self
                .transitions
                .iter()
                .find(|t| {
                    [t.retry.map(|r| r.backoff), t.run_timer, t.abandon].contains(&Some(longest))
                })
                .expect("the longest timer is a transition's");
            LoadError::TimerOutOfRange {
                transition: t.name.to_string(),
                timer: longest,
            }
        })?;
        for (work, ask) in &self.asks {
            if let Some(of) = self.work_of_answer(work) {
                return Err(LoadError::AskWorkIsAnswer {
                    work: work.to_string(),
                    of: of.to_string(),
                });
            }
            let place = |name: &Place, derived: bool| {
                self.places
                    .get(name)
                    .is_some_and(|d| matches!(d.kind, PlaceKind::Derived(_)) == derived)
                    .then_some(())
                    .ok_or_else(|| name.to_string())
            };
            let transition = |name: &str, ok: &dyn Fn(&TransitionDef<V>) -> bool| {
                self.transitions
                    .iter()
                    .any(|t| *t.name == *name && ok(t))
                    .then_some(())
                    .ok_or_else(|| name.to_string())
            };
            place(&ask.offer, true)
                .and(place(&ask.answered, false))
                .and(place(&ask.expired, false))
                .and(transition(&ask.answer, &|t| {
                    t.trigger == Trigger::Manual
                        && matches!(t.body, Body::Pure(_))
                        && t.retry.is_none()
                }))
                .and(transition(work, &|t| t.consumes.contains(&ask.answered)))
                .map_err(|missing| LoadError::IncompleteAsk {
                    work: work.to_string(),
                    missing,
                })?;
        }
        Ok(())
    }
}

/// The first place that two input arcs of one transition name: the field of
/// the first arc, the field of the second, and the place.
pub(crate) fn repeated_input<'a, P: Borrow<str>>(
    arcs: [(&'static str, &'a [P]); 3],
) -> Option<(&'static str, &'static str, &'a str)> {
    let mut named = BTreeMap::new();
    for (field, places) in arcs {
        for place in places {
            if let Some(first) = named.insert(place.borrow(), field) {
                return Some((first, field, place.borrow()));
            }
        }
    }
    None
}

/// How a refusal says which input arcs name one place.
pub(crate) fn input_twice(first: &str, second: &str, place: &str) -> String {
    if first == second {
        format!("`{first}` names the place `{place}` twice")
    } else {
        format!("`{first}` and `{second}` both name the place `{place}`")
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
    #[error(
        "'{user}' is applied from '{place}', which is not declared FIFO; a candidate place must hold every waiting token (REV2-cand.c)"
    )]
    CandidateNotFifo { user: String, place: Place },
    #[error("place '{place}' is {class:?} class; this executor holds process and data class only")]
    ClassNotSupported { place: Place, class: Class },
    #[error(
        "transition '{transition}' consumes the data-class place '{place}'; data class has no claims yet"
    )]
    ConsumesData { transition: String, place: Place },
    #[error("transition '{transition}' has an arc on the derived place '{place}'")]
    ArcOnDerived { transition: String, place: Place },
    #[error(
        "transition '{transition}' consumes or puts '{place}', which holds the executor's own markers"
    )]
    WritesExecutorPlace { transition: String, place: Place },
    #[error(
        "transition '{transition}' has a failure or timeout port on '{place}', whose token needs the field '{field}' that a cause token lacks"
    )]
    PortLacksAnswerField {
        transition: String,
        place: Place,
        field: &'static str,
    },
    #[error(
        "transition '{transition}': {}; a place is one input arc of a transition",
        input_twice(.first, .second, .place)
    )]
    InputTwice {
        transition: String,
        first: &'static str,
        second: &'static str,
        place: Place,
    },
    #[error("auto transition '{0}' has no consume or read arc to find its subjects by")]
    NoSubject(String),
    #[error("'{0}' has a Keyed receiver but no retry count bound")]
    KeyedWithoutCountBound(String),
    #[error(
        "the ask '{work}' lacks '{missing}': it needs a derived Asks, a stored Answered and Expired, a manual Answer, and a work that consumes Answered"
    )]
    IncompleteAsk { work: String, missing: String },
    #[error(
        "the ask '{work}' is the Answer transition of the ask '{of}'; an Answer is fired by the caller, never as a work"
    )]
    AskWorkIsAnswer { work: String, of: String },
    #[error("the derived place '{}' reads itself through {}", .0[0], cycle_text(.0))]
    DerivedCycle(Vec<Place>),
    #[error("transition '{transition}' has a timer of {timer:?}, past the i64 nanosecond range")]
    TimerOutOfRange { transition: String, timer: Duration },
}

/// The deepest nesting of derived rows one evaluation runs: the bound keeps
/// a derive that recurses over ever new subjects off the stack's end.
const MAX_DERIVED_DEPTH: usize = 128;

/// The clock a deadline fits: deadlines are stored as i64 nanoseconds.
const MAX_NOW: Duration = Duration::from_nanos(i64::MAX as u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Token<V> {
    pub id: u64,
    pub envelope: Envelope,
    pub payload: Payload<V>,
}

/// The header of a token, outside its stamp: a change here moves no version
/// (ADR 0032 §9.1, REV11-running.a).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    pub next_try: Option<Duration>,
}

/// What a token holds. `Record`, `Failure`, `Candidate` and `Answered` are
/// net data; the other variants are the executor's own tokens in the places
/// [`NetDef::check`] declares for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload<V> {
    Record(Record<V>),
    /// A failure or timeout port token: why the run failed. A bind reads it
    /// as the field [`FAILURE_CAUSE_FIELD`].
    Failure {
        cause: Cause,
    },
    /// A late or upgrading result in a candidate place (NET-cand.a).
    Candidate(Box<Candidate<V>>),
    /// An `Answered.T` token: the user answer the work of `T` consumes.
    Answered(Answered<V>),
    /// `<T>.Running`: names one occurrence with an attempt running.
    Running {
        occurrence: OccurrenceId,
    },
    /// `<T>.RetryPending`: when the pending retry fires.
    RetryPending {
        due: Duration,
    },
    /// `<T>.OutcomeUnknown`: names one occurrence parked for the subject.
    OutcomeUnknown {
        occurrence: OccurrenceId,
    },
    /// [`OUTCOME_UNKNOWN`], keyed by the parked occurrence.
    Parked {
        transition: Arc<str>,
        subject: Subject,
        cause: Cause,
    },
    /// `<T>.UndoPending`, keyed by the answer: the rows its undo still owes,
    /// each with the holder that kept it.
    UndoPending {
        answer: Arc<str>,
        held: Vec<(Place, Subject, Holder)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answered<V> {
    pub id: Arc<str>,
    /// The user value; only the work's bound input holds it.
    pub input: V,
    /// The other fields, which readers see.
    pub record: Record<V>,
}

/// A field of an `answered` token as a reader sees it: the user value only
/// when `show_input`.
fn answered_field<V: Clone + PartialEq + fmt::Debug>(
    host: &Host<V>,
    a: &Answered<V>,
    field: &str,
    show_input: bool,
) -> Option<V> {
    let keys = host.answer.keys;
    if field == keys.id {
        Some((host.text)(&a.id))
    } else if field == keys.input {
        show_input.then(|| a.input.clone())
    } else {
        a.record.get(field).cloned()
    }
}

/// How far an answer got: undone, its work occurrence and that one's
/// settlement (`None` while it runs), its `expired` token, or waiting.
#[derive(Debug)]
pub enum AnswerProgress<'a, V> {
    Undone,
    Work {
        occurrence: OccurrenceId,
        settled: Option<Settled>,
    },
    Expired {
        place: &'a Place,
        record: &'a Record<V>,
    },
    Waiting,
}

/// What a stale effect would have written to a place with a candidate: the
/// row as the firing read it, the output, and the firing's read trace.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate<V> {
    /// The fields the firing read of the row, `None` when it read none.
    pub base: Option<Record<Option<V>>>,
    pub output: Record<V>,
    pub trace: Trace<V>,
}

impl<V: fmt::Debug> Token<V> {
    /// The net data of a token in a net place.
    pub fn record(&self) -> &Record<V> {
        match &self.payload {
            Payload::Record(r) => r,
            other => panic!("token {} holds no record: {other:?}", self.id),
        }
    }
}

#[derive(Debug, PartialEq)]
struct Slot<V> {
    /// Changes only when a record or an outside write commits a change.
    version: u64,
    /// The transition whose commit set the current version, `None` for a
    /// write from outside the net. An undo reads it to tell the holder of a
    /// row from a later writer (D191-rewritten.a).
    writer: Option<Arc<str>>,
    /// Oldest first; a [`Capacity::Single`] place holds at most one.
    tokens: VecDeque<Token<V>>,
    /// Only the hidden-reservation twin sets it: a real [`Executor::start`]
    /// takes the token out, so no second attempt can bind it and no two
    /// attempts ever hold one slot. That is why
    /// [`Executor::commit_consumption`] needs no owner check.
    reserved: bool,
}

impl<V> Default for Slot<V> {
    fn default() -> Self {
        Self {
            version: 0,
            writer: None,
            tokens: VecDeque::new(),
            reserved: false,
        }
    }
}

/// Who a commit is by: a transition, or `None` for a write from outside the
/// net (a user edit, an ingest, an undo).
type Writer<'a> = Option<&'a Arc<str>>;

/// A put into a pool that already holds its size.
#[derive(Debug)]
struct PoolFull;

/// The owned marking: a token queue and a version per (place, subject). Only
/// a [`Capacity::Fifo`] place ever holds more than one token.
#[derive(Debug, PartialEq)]
pub struct Tokens<V> {
    slots: BTreeMap<Place, BTreeMap<Subject, Slot<V>>>,
    next_id: u64,
    fifo: BTreeSet<Place>,
    /// Pool places and their size: one shared row, whatever the subject.
    pools: BTreeMap<Place, u32>,
    /// Pool tokens a running firing holds; they count against the size.
    lent: BTreeMap<Place, u32>,
    /// Every row changed since the executor last looked: what it decides again.
    touched: BTreeSet<(Place, Subject)>,
}

impl<V> Default for Tokens<V> {
    fn default() -> Self {
        Self {
            slots: BTreeMap::new(),
            next_id: 0,
            fifo: BTreeSet::new(),
            pools: BTreeMap::new(),
            lent: BTreeMap::new(),
            touched: BTreeSet::new(),
        }
    }
}

impl<V: Clone + PartialEq + fmt::Debug> Tokens<V> {
    /// The oldest token: the one a firing binds and consumes (REV2-cand.c).
    pub fn token(&self, place: &str, subject: &str) -> Option<&Token<V>> {
        self.slot(place, subject).and_then(|s| s.tokens.front())
    }

    /// Every token the row holds, oldest first.
    pub fn row(&self, place: &str, subject: &str) -> impl Iterator<Item = &Token<V>> {
        self.slot(place, subject)
            .into_iter()
            .flat_map(|s| &s.tokens)
    }

    /// How many tokens of a pool are free: not held by a running firing.
    pub fn free(&self, place: &str) -> usize {
        self.row(place, POOL_SUBJECT).count()
    }

    pub fn version(&self, place: &str, subject: &str) -> u64 {
        self.slot(place, subject).map_or(0, |s| s.version)
    }

    /// Who committed the current version: see [`Slot::writer`].
    fn writer(&self, place: &str, subject: &str) -> Option<Arc<str>> {
        self.slot(place, subject).and_then(|s| s.writer.clone())
    }

    /// The subjects holding a token in `place`.
    pub fn subjects(&self, place: &str) -> Vec<Subject> {
        self.slots.get(place).map_or_else(Vec::new, |m| {
            m.iter()
                .filter(|(_, s)| !s.tokens.is_empty())
                .map(|(k, _)| k.clone())
                .collect()
        })
    }

    /// The subject a row of `place` is stored and touched under: a pool is
    /// one row shared by every subject.
    fn row_subject<'a>(&self, place: &str, subject: &'a str) -> &'a str {
        if self.pools.contains_key(place) {
            POOL_SUBJECT
        } else {
            subject
        }
    }

    fn slot(&self, place: &str, subject: &str) -> Option<&Slot<V>> {
        self.slots
            .get(place)
            .and_then(|m| m.get(self.row_subject(place, subject)))
    }

    /// Whether `subjects(place)` lists `subject`.
    fn holds(&self, place: &str, subject: &str) -> bool {
        self.slots
            .get(place)
            .and_then(|m| m.get(subject))
            .is_some_and(|s| !s.tokens.is_empty())
    }

    fn slot_mut(&mut self, place: &Place, subject: &Subject) -> &mut Slot<V> {
        let subject = Subject::new(self.row_subject(place, subject));
        self.touched.insert((place.clone(), subject.clone()));
        self.slots
            .entry(place.clone())
            .or_default()
            .entry(subject)
            .or_default()
    }

    /// Commits `content`; unchanged content is no change and no new version.
    /// A FIFO place appends, so a put never drops a waiting token
    /// (REV2-cand.c). A pool that holds its size, lent tokens included,
    /// refuses the put and changes nothing.
    fn put(
        &mut self,
        place: &Place,
        subject: &Subject,
        content: Record<V>,
        by: Writer,
    ) -> Result<(), PoolFull> {
        self.put_with(place, subject, Payload::Record(content), by)
    }

    /// [`put`](Self::put) of any payload. Equal content leaves the held
    /// token as it is.
    fn put_with(
        &mut self,
        place: &Place,
        subject: &Subject,
        payload: Payload<V>,
        by: Writer,
    ) -> Result<(), PoolFull> {
        if let Some(&size) = self.pools.get(place) {
            let held = self.slot(place, subject).map_or(0, |s| s.tokens.len());
            let lent = self.lent.get(place).copied().unwrap_or(0);
            if held + lent as usize >= size as usize {
                return Err(PoolFull);
            }
        }
        let id = self.next_id;
        let fifo = self.fifo.contains(place);
        let slot = self.slot_mut(place, subject);
        if fifo {
            slot.tokens.push_back(Token {
                id,
                envelope: Envelope::default(),
                payload,
            });
        } else if slot.tokens.front().is_some_and(|t| t.payload == payload) {
            return Ok(());
        } else {
            slot.tokens = VecDeque::from([Token {
                id,
                envelope: Envelope::default(),
                payload,
            }]);
        }
        slot.version += 1;
        slot.writer = by.cloned();
        self.next_id += 1;
        Ok(())
    }

    /// An input goes back with no version change: into a FIFO at its place
    /// by token id, which grows with write order, or into a single-token row
    /// unless a newer token `superseded` it (see [`Executor::superseded`]).
    /// Returns whether the token went back into the row.
    fn restore(
        &mut self,
        place: &Place,
        subject: &Subject,
        token: &Token<V>,
        superseded: bool,
    ) -> bool {
        self.unlend(place);
        let fifo = self.fifo.contains(place);
        let slot = self.slot_mut(place, subject);
        slot.reserved = false;
        if superseded || slot.tokens.iter().any(|t| t.id == token.id) {
            return false;
        }
        if fifo {
            let at = slot.tokens.partition_point(|t| t.id < token.id);
            slot.tokens.insert(at, token.clone());
        } else {
            slot.tokens.push_back(token.clone());
        }
        true
    }

    fn lend(&mut self, place: &Place) {
        if self.pools.contains_key(place) {
            *self.lent.entry(place.clone()).or_default() += 1;
        }
    }

    fn unlend(&mut self, place: &Place) {
        if self.pools.contains_key(place) {
            let lent = self.lent.entry(place.clone()).or_default();
            assert!(*lent > 0, "pool {place} gets back a token it never lent");
            *lent -= 1;
        }
    }

    /// Commits the removal of a token: a new version.
    fn remove(&mut self, place: &Place, subject: &Subject, by: Writer) {
        let slot = self.slot_mut(place, subject);
        assert!(
            slot.tokens.pop_front().is_some(),
            "a removal of the empty {place} of {subject}"
        );
        slot.version += 1;
        slot.writer = by.cloned();
    }

    /// Places an executor-owned token (Running, OutcomeUnknown): a move, not
    /// a commit, so no version changes.
    fn mark_slot(&mut self, place: &Place, subject: &Subject, payload: Option<Payload<V>>) {
        assert!(
            !matches!(payload, Some(Payload::Record(_))),
            "{place} of {subject} is an executor place and holds no record"
        );
        let id = self.next_id;
        self.next_id += 1;
        self.slot_mut(place, subject).tokens = payload
            .map(|payload| Token {
                id,
                envelope: Envelope::default(),
                payload,
            })
            .into_iter()
            .collect();
    }
}

/// Everything a firing's enabledness check read, captured once.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace<V> {
    rows: BTreeMap<(Place, Subject), RowRead<V>>,
    /// The first place read that the net does not declare.
    undeclared: Option<Place>,
    /// The derived rows being evaluated, outermost first.
    evaluating: Vec<(Place, Subject)>,
    /// The first derived cycle met: a derived row that reads itself.
    cycle: Option<Vec<Place>>,
}

#[derive(Clone, Debug, PartialEq)]
struct RowRead<V> {
    version: u64,
    present: bool,
    fields: BTreeMap<String, Option<V>>,
    /// The header cause, when a reader asked for it.
    cause: Option<Option<Cause>>,
    /// The output of a candidate token, when a reader asked for it.
    candidate: Option<Option<Record<V>>>,
    /// Read only through [`Reader::committed`]: no restore can change it.
    committed: bool,
}

impl<V> Default for Trace<V> {
    fn default() -> Self {
        Self {
            rows: BTreeMap::new(),
            undeclared: None,
            evaluating: Vec::new(),
            cycle: None,
        }
    }
}

impl<V> Default for RowRead<V> {
    fn default() -> Self {
        Self {
            version: 0,
            present: false,
            fields: BTreeMap::new(),
            cause: None,
            candidate: None,
            committed: false,
        }
    }
}

impl<V: Clone + PartialEq + fmt::Debug> Trace<V> {
    fn row(&mut self, tokens: &Tokens<V>, place: &str, subject: &str) -> &mut RowRead<V> {
        let row = self.committed_row(tokens, place, subject, tokens.token(place, subject));
        row.committed = false;
        row
    }

    /// The row as the committed view `front` shows it.
    fn committed_row(
        &mut self,
        tokens: &Tokens<V>,
        place: &str,
        subject: &str,
        front: Option<&Token<V>>,
    ) -> &mut RowRead<V> {
        self.rows
            .entry((Place::new(place), Subject::new(subject)))
            .or_insert_with(|| RowRead {
                version: tokens.version(place, subject),
                present: front.is_some(),
                fields: BTreeMap::new(),
                cause: None,
                candidate: None,
                committed: true,
            })
    }

    /// Every row read: place, subject, its version at the read, and the
    /// fields read (`None` for an absent field).
    pub fn rows(
        &self,
    ) -> impl Iterator<Item = (&Place, &Subject, u64, &BTreeMap<String, Option<V>>)> {
        self.rows
            .iter()
            .map(|((p, s), r)| (p, s, r.version, &r.fields))
    }
}

/// The view a guard, a binding or a derived place reads the marking through.
pub struct Reader<'a, V> {
    exec: &'a Executor<V>,
    trace: &'a mut Trace<V>,
    /// The value of an `Answered` token reads as absent.
    hide_value: bool,
    /// Inside [`Reader::committed`].
    committed: bool,
}

impl<'a, V: Clone + PartialEq + fmt::Debug> Reader<'a, V> {
    pub fn field(&mut self, place: &str, subject: &str, field: &str) -> Option<V> {
        let net = &self.exec.net;
        if !self.declared(place) {
            return None;
        }
        match &net.place_def(place).kind {
            PlaceKind::Derived(_) => self.derived(place, &Subject::new(subject))?.remove(field),
            // An executor place or a candidate holds no fields: a read of one
            // refuses the firing like a read of an undeclared place.
            PlaceKind::Stored { .. }
                if net.owned.contains(place)
                    || self
                        .token(place, subject)
                        .is_some_and(|t| matches!(t.payload, Payload::Candidate(_))) =>
            {
                self.trace
                    .undeclared
                    .get_or_insert_with(|| Place::new(place));
                None
            }
            PlaceKind::Stored { .. } => {
                let value = self.token(place, subject).and_then(|t| match &t.payload {
                    Payload::Answered(a) => answered_field(&net.host, a, field, !self.hide_value),
                    Payload::Failure { cause } => {
                        (field == FAILURE_CAUSE_FIELD).then(|| (net.host.text)(&cause.text))
                    }
                    Payload::Record(r) => r.get(field).cloned(),
                    other => unreachable!("the arm above refuses a read of {other:?}"),
                });
                self.row(place, subject)
                    .fields
                    .insert(field.to_string(), value.clone());
                value
            }
        }
    }

    /// The free tokens of a pool, for a guard such as "start only while
    /// `cpu > 0`". The guard decides when the firing starts, so a later change
    /// of the pool does not make the firing stale.
    pub fn free(&mut self, place: &str) -> usize {
        if !self.declared(place) {
            return 0;
        }
        self.trace.row(&self.exec.tokens, place, POOL_SUBJECT);
        self.exec.tokens.free(place)
    }

    /// The cause in the header of a failure or timeout port token.
    pub fn cause(&mut self, place: &str, subject: &str) -> Option<Cause> {
        if !self.declared(place) {
            return None;
        }
        let cause = self
            .exec
            .tokens
            .token(place, subject)
            .and_then(|t| match &t.payload {
                Payload::Failure { cause } => Some(cause.clone()),
                _ => None,
            });
        self.trace.row(&self.exec.tokens, place, subject).cause = Some(cause.clone());
        cause
    }

    /// The output of the oldest token of a candidate place when it is a
    /// candidate; the apply transition reads it (NET-cand.a).
    pub fn candidate_output(&mut self, place: &str, subject: &str) -> Option<Record<V>> {
        if !self.declared(place) {
            return None;
        }
        let output = self
            .exec
            .tokens
            .token(place, subject)
            .and_then(|t| match &t.payload {
                Payload::Candidate(c) => Some(c.output.clone()),
                _ => None,
            });
        self.trace.row(&self.exec.tokens, place, subject).candidate = Some(output.clone());
        output
    }

    pub fn present(&mut self, place: &str, subject: &str) -> bool {
        let net = &self.exec.net;
        if !self.declared(place) {
            return false;
        }
        match &net.place_def(place).kind {
            PlaceKind::Derived(_) => self.derived(place, &Subject::new(subject)).is_some(),
            PlaceKind::Stored { .. } => self.row(place, subject).present,
        }
    }

    /// Runs `f` on the committed view: a token that an open take holds, and
    /// would put back if it failed, reads as still in its row. Such a read
    /// waits on no take, as no take's outcome changes it.
    pub fn committed<R>(&mut self, f: impl FnOnce(&mut Reader<'_, V>) -> R) -> R {
        let was = std::mem::replace(&mut self.committed, true);
        let r = f(self);
        self.committed = was;
        r
    }

    fn token(&self, place: &str, subject: &str) -> Option<&'a Token<V>> {
        if self.committed {
            self.exec.committed_front(place, subject)
        } else {
            self.exec.tokens.token(place, subject)
        }
    }

    fn row(&mut self, place: &str, subject: &str) -> &mut RowRead<V> {
        let tokens = &self.exec.tokens;
        if self.committed {
            let front = self.exec.committed_front(place, subject);
            self.trace.committed_row(tokens, place, subject, front)
        } else {
            self.trace.row(tokens, place, subject)
        }
    }

    /// An undeclared place reads as empty and refuses the firing that read it.
    fn declared(&mut self, place: &str) -> bool {
        let declared = self.exec.net.places.contains_key(place);
        if !declared {
            self.trace
                .undeclared
                .get_or_insert_with(|| Place::new(place));
        }
        declared
    }

    /// A derived row that reads itself reads as empty and refuses what
    /// evaluated it.
    fn derived(&mut self, place: &str, s: &Subject) -> Option<Record<V>> {
        let PlaceKind::Derived(derive) = &self.exec.net.place_def(place).kind else {
            panic!("'{place}' is no derived place");
        };
        let row = (Place::new(place), s.clone());
        let stack = &self.trace.evaluating;
        let cycle: Option<Vec<Place>> = match stack.iter().position(|r| *r == row) {
            Some(at) => Some(stack[at..].iter().map(|(p, _)| p.clone()).collect()),
            // shortcut: a recursion this deep over new subjects reads as a
            // cycle of its places; raise the bound when a real net derives
            // deeper.
            None if stack.len() >= MAX_DERIVED_DEPTH => {
                let mut places: Vec<Place> = Vec::new();
                for (p, _) in stack {
                    if !places.contains(p) {
                        places.push(p.clone());
                    }
                }
                Some(places)
            }
            None => None,
        };
        if let Some(cycle) = cycle {
            self.trace.cycle.get_or_insert(cycle);
            return None;
        }
        self.trace.evaluating.push(row);
        let content = derive(self, s);
        self.trace.evaluating.pop();
        content
    }

    /// Runs `f` and returns, besides its result, the trace of what `f` read,
    /// e.g. for an [`ask_hash_with`] of a binding. The reads stay in this trace
    /// too.
    pub fn traced<R>(&mut self, f: impl FnOnce(&mut Reader<'_, V>) -> R) -> (R, Trace<V>) {
        let mut sub = Trace {
            evaluating: self.trace.evaluating.clone(),
            ..Trace::default()
        };
        let r = f(&mut Reader {
            exec: self.exec,
            trace: &mut sub,
            hide_value: self.hide_value,
            committed: self.committed,
        });
        for (k, row) in &sub.rows {
            let into = self.trace.rows.entry(k.clone()).or_insert_with(|| RowRead {
                fields: BTreeMap::new(),
                cause: None,
                candidate: None,
                ..row.clone()
            });
            into.committed &= row.committed;
            into.fields.extend(row.fields.clone());
            if row.cause.is_some() {
                into.cause = row.cause.clone();
            }
            if row.candidate.is_some() {
                into.candidate = row.candidate.clone();
            }
        }
        if self.trace.undeclared.is_none() {
            self.trace.undeclared = sub.undeclared.clone();
        }
        if self.trace.cycle.is_none() {
            self.trace.cycle = sub.cycle.clone();
        }
        (r, sub)
    }

    /// A field as a string; `None` when absent or not a string.
    pub fn text(&mut self, place: &str, subject: &str, field: &str) -> Option<String> {
        let value = self.field(place, subject, field)?;
        (self.exec.net.host.as_text)(&value).map(str::to_owned)
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
    /// The bind read this place, which the net does not declare.
    ReadsUndeclared(Place),
    /// The bind read a derived row that reads itself through these places.
    DerivedCycle(Vec<Place>),
    /// The bind of an ask's work produces a field the answer fills: its
    /// value, its id or the seen ask hash.
    BindsAnswerInput,
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
pub struct EffectRequest<V> {
    pub attempt: AttemptId,
    pub occurrence: OccurrenceId,
    pub transition: Arc<str>,
    pub subject: Subject,
    pub input: Record<V>,
    /// A `StateProbe` check: answer `Done` when the goal holds.
    pub probe: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectResult<V> {
    Done(Vec<Output<V>>),
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

#[derive(Clone)]
pub enum Resolution<V> {
    /// The effect happened: record these outputs.
    Verify(Vec<Output<V>>),
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
pub struct Quiesced<T, V> {
    pub value: T,
    pub requests: Vec<EffectRequest<V>>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FireError {
    #[error(transparent)]
    UnknownTransition(#[from] UnknownTransition),
    #[error(transparent)]
    ClockWentBack(#[from] ClockWentBack),
    #[error(transparent)]
    ClockOutOfRange(#[from] ClockOutOfRange),
    #[error("'{0}' is not a manual transition")]
    NotManual(String),
    #[error("the user input field '{field}' of '{transition}' collides with a bound field")]
    ValueCollides { transition: String, field: String },
    #[error("'{transition}' fired as '{occurrence}', which is not the answer id of its value")]
    AnswerOccurrence {
        transition: String,
        occurrence: OccurrenceId,
    },
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
    #[error(transparent)]
    ClockWentBack(#[from] ClockWentBack),
    #[error(transparent)]
    ClockOutOfRange(#[from] ClockOutOfRange),
    #[error("the net does not declare the place '{0}'")]
    UndeclaredPlace(String),
    #[error("'{0}' is a derived place: only the net writes it")]
    DerivedPlace(String),
    #[error("'{0}' holds the executor's own markers: only the executor writes it")]
    ExecutorPlace(String),
    #[error("an '{place}' token needs the answer field '{field}'")]
    AnswerField { place: String, field: &'static str },
    #[error("the pool '{place}' already holds its size")]
    PoolFull { place: String },
    #[error("'{place}' holds no token of '{subject}' to retract")]
    NothingToRetract { place: String, subject: String },
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

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("no transition '{0}' in the net")]
pub struct UnknownTransition(pub String);

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DeriveError {
    #[error("the net does not declare the place '{0}'")]
    UndeclaredPlace(String),
    #[error("'{0}' is a stored place, not a derived one")]
    StoredPlace(String),
    #[error("the derived place '{place}' reads '{read}', which the net does not declare")]
    ReadsUndeclared { place: Place, read: Place },
    #[error("the derived place '{place}' reads itself through {}", cycle_text(.cycle))]
    Cycle { place: Place, cycle: Vec<Place> },
    #[error("the row of '{place}' for '{subject}' has no ask hash: {reason}")]
    AskRowHash {
        place: Place,
        subject: String,
        reason: String,
    },
}

fn cycle_text(cycle: &[Place]) -> String {
    let names: Vec<&str> = cycle.iter().map(Place::as_str).collect();
    names.join(" -> ")
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("the clock went back from {clock:?} to {now:?}")]
pub struct ClockWentBack {
    pub clock: Duration,
    pub now: Duration,
}

/// A `now` past `last`, the latest one whose timer deadlines this net can
/// still store.
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("the clock {now:?} is past {last:?}, the latest this net's timers allow")]
pub struct ClockOutOfRange {
    pub now: Duration,
    pub last: Duration,
}

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
    /// The place is a pool that already holds its size: left as it is.
    PoolFull,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum UndoError {
    #[error(transparent)]
    ClockWentBack(#[from] ClockWentBack),
    #[error(transparent)]
    ClockOutOfRange(#[from] ClockOutOfRange),
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
    /// A reader records at once, although a take that hid a token of a row
    /// it read is still open.
    NoCommitWait,
}

/// An auto pair that a check from scratch finds enabled at quiescence: a
/// break of the Level-dead law (ADR 0032 NET-level.a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelViolation {
    pub transition: Arc<str>,
    pub subject: Subject,
    /// The bound input and the rows of its arcs, for a failure message.
    pub marking: String,
}

impl fmt::Display for LevelViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} for {} is enabled at quiescence: {}",
            self.transition, self.subject, self.marking
        )
    }
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
    /// The cause of the last failed attempt while a retry is due or runs.
    failed: Option<Cause>,
}

/// A held reader's wait: the row it read and the open take's occurrence.
pub type WaitEdge = (Place, Subject, OccurrenceId);

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
    /// The undo of `answer` could not reverse `place` of `subject`: the
    /// firing that held the row used it.
    UndoHeld {
        answer: AnswerId,
        place: Place,
        subject: Subject,
        holder: Arc<str>,
    },
    /// The undo of `answer` could not reverse `place` of `subject`: a writer
    /// other than the holder of the row rewrote it, and a record never
    /// clobbers a later write (D191-rewritten.a). `writer` is the transition
    /// that wrote it, or `None` for a write from outside the net.
    UndoRewritten {
        answer: AnswerId,
        place: Place,
        subject: Subject,
        writer: Option<Arc<str>>,
    },
    /// The bind of `transition` for `subject` read `place`, which the net does
    /// not declare, so the transition does not fire.
    ReadsUndeclared {
        transition: Arc<str>,
        subject: Subject,
        place: Place,
    },
    /// The bind of the ask's work `work` for `subject` produces a field the
    /// answer fills, so the work never fires on an answer.
    BindsAnswerInput { work: Arc<str>, subject: Subject },
    /// `transition` put a token into the pool `place`, which already held its
    /// size, so the token was not put.
    PoolFull { place: Place, transition: Arc<str> },
    /// The bind of `transition` for `subject` read a derived row that reads
    /// itself through `cycle`, so the transition does not fire.
    DerivedCycle {
        transition: Arc<str>,
        subject: Subject,
        cycle: Vec<Place>,
    },
    /// `reader` read a row while the parked `take` held a token of it, so
    /// the reader's commit waits until the take is resolved.
    WaitsOnParkedTake {
        reader: Arc<str>,
        subject: Subject,
        reader_occurrence: OccurrenceId,
        take: Arc<str>,
        take_occurrence: OccurrenceId,
    },
}

/// The holder an undo names when that holder used the row.
fn holder_name(holder: &Holder) -> Arc<str> {
    match holder {
        Holder::Running(t) => t.clone(),
        Holder::OutcomeUnknown(o) => Arc::from(o.to_string()),
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoQuiescence {
                work,
                subject,
                steps,
            } => write!(
                f,
                "the answer of {work} for {subject} left the net unquiesced after {steps} rounds"
            ),
            Self::UndoHeld {
                answer,
                place,
                subject,
                holder,
            } => write!(
                f,
                "cannot undo {}: {place} of {subject} is used by {holder}",
                answer.0
            ),
            Self::UndoRewritten {
                answer,
                place,
                subject,
                writer,
            } => write!(
                f,
                "cannot undo {}: {place} of {subject} was rewritten by a later write{}",
                answer.0,
                match writer {
                    Some(w) => format!(" of {w}"),
                    None => String::new(),
                }
            ),
            Self::ReadsUndeclared {
                transition,
                subject,
                place,
            } => write!(
                f,
                "{transition} does not fire for {subject}: its bind reads {place}, which the net \
                 does not declare"
            ),
            Self::BindsAnswerInput { work, subject } => write!(
                f,
                "{work} does not fire for {subject}: its bind produces a field the answer fills \
                 (its value, its id or the seen ask hash)"
            ),
            Self::PoolFull { place, transition } => write!(
                f,
                "{transition} put a token into the pool {place}, which already held its size: \
                 the token was not put"
            ),
            Self::DerivedCycle {
                transition,
                subject,
                cycle,
            } => write!(
                f,
                "{transition} does not fire for {subject}: its bind reads a derived place that \
                 reads itself through {}",
                cycle_text(cycle)
            ),
            Self::WaitsOnParkedTake {
                reader,
                subject,
                reader_occurrence,
                take,
                take_occurrence,
            } => write!(
                f,
                "{reader}[{subject}] ({reader_occurrence}) waits for {take} ({take_occurrence}), \
                 whose outcome is unknown"
            ),
        }
    }
}

#[derive(Clone)]
struct Attempt<V> {
    occurrence: OccurrenceId,
    transition: Arc<str>,
    subject: Subject,
    taken: Vec<(Place, Token<V>)>,
    /// The version of each consumed row at the take.
    taken_at: BTreeMap<Place, u64>,
    trace: Trace<V>,
    input: Record<V>,
    started: Duration,
    probe: bool,
    /// The tokens that open takes held from a row this attempt read, at its
    /// bind: (place, subject, token id) and the take's occurrence.
    waits_on: BTreeMap<(Place, Subject, u64), OccurrenceId>,
    /// The read rows that a failed take put one of those tokens back into:
    /// the read is no longer true in the committed state.
    stale_rows: BTreeSet<(Place, Subject)>,
}

pub struct Executor<V> {
    net: NetDef<V>,
    tokens: Tokens<V>,
    marks: BTreeMap<(Arc<str>, Subject), Mark>,
    runs: BTreeMap<(Arc<str>, Subject), Run>,
    in_flight: BTreeMap<AttemptId, Attempt<V>>,
    parked: BTreeMap<OccurrenceId, Attempt<V>>,
    /// Readers that recorded while a take they read through is open: the
    /// attempt and its outputs, recorded when the last such take settles.
    held: BTreeMap<OccurrenceId, (Attempt<V>, Vec<Output<V>>)>,
    attempts: BTreeMap<OccurrenceId, BTreeMap<AttemptId, Seen>>,
    // ponytail: settled occurrences are kept forever; add retention with the
    // first long-running user.
    occurrences: BTreeMap<OccurrenceId, Option<Settled>>,
    /// Answer id → (work transition, subject).
    answers: BTreeMap<Arc<str>, (Arc<str>, Subject)>,
    undone: BTreeSet<Arc<str>>,
    deltas: BTreeMap<OccurrenceId, Delta<V>>,
    conditions: Vec<Condition>,
    requests: Vec<EffectRequest<V>>,
    /// The auto transitions whose enabledness reads each place for the
    /// subject it decides (its inputs, inhibitors and run state).
    watchers: Watchers,
    /// Who declined on a read of a row: decided again when the row changes.
    waiters: BTreeMap<(Place, Subject), BTreeSet<Decision>>,
    /// Per transition, the auto producers of a place it produces that come
    /// before it in priority order.
    preferred: Vec<Vec<usize>>,
    /// The (transition, subject) pairs to decide again. A pair outside it
    /// decides as it did last time, because no row it read changed since.
    pending: BTreeMap<Arc<str>, BTreeSet<Subject>>,
    /// Transitions to decide again for every subject: a pool they take from
    /// changed, which is one row shared by every subject.
    pending_all: BTreeSet<Arc<str>>,
    now: Duration,
    next_attempt: u64,
    next_minted: u64,
    #[cfg(feature = "naive-twins")]
    twin: Option<Twin>,
}

type Watchers = BTreeMap<Place, Vec<Arc<str>>>;

/// A (transition, subject) pair: one enabledness decision.
type Decision = (Arc<str>, Subject);

/// What the commit of a work occurrence of an ask changed: the content before
/// it and the version after it, per (place, subject).
type Delta<V> = BTreeMap<(Place, Subject), (Option<Payload<V>>, u64)>;

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
/// failure mark keys (EXEC-inc0 D.a); `encode` is the host's value encoder.
pub fn ask_hash_with<V: Clone + PartialEq + fmt::Debug>(
    encode: fn(&V) -> Vec<u8>,
    transition: &str,
    subject: &Subject,
    trace: &Trace<V>,
) -> AskHash {
    let mut h = blake3::Hasher::new();
    encode_str(&mut h, transition);
    encode_str(&mut h, subject);
    encode_rows(&mut h, trace, false, encode);
    AskHash(*h.finalize().as_bytes())
}

/// The hex text of an ask hash, as the `Asks` row and `Answered` carry it.
pub fn hash_text(hash: &AskHash) -> String {
    blake3::Hash::from_bytes(hash.0).to_hex().to_string()
}

pub fn hash_from_text(text: &str) -> Result<AskHash, blake3::HexError> {
    Ok(AskHash(*blake3::Hash::from_hex(text)?.as_bytes()))
}

/// Per place, the auto transitions that watch it; per transition, its
/// preferred producers.
fn caches<V: Clone + PartialEq + fmt::Debug>(net: &NetDef<V>) -> (Watchers, Vec<Vec<usize>>) {
    let mut watchers = Watchers::new();
    for t in net
        .transitions
        .iter()
        .filter(|t| t.trigger == Trigger::Auto)
    {
        let run_state = [t.running(), Place::retry_pending(&t.name)];
        for p in (t
            .consumes
            .iter()
            .chain(&t.reads)
            .chain(&t.inhibits)
            .cloned())
        .chain(run_state)
        .chain(net.answered_of(&t.name))
        {
            watchers.entry(p).or_default().push(t.name.clone());
        }
    }
    let preferred = net
        .transitions
        .iter()
        .enumerate()
        .map(|(i, t)| {
            net.transitions
                .iter()
                .enumerate()
                .filter(|&(j, u)| {
                    u.trigger == Trigger::Auto
                        && u.produces.iter().any(|p| t.produces.contains(p))
                        && (u.priority, Reverse(j)) > (t.priority, Reverse(i))
                })
                .map(|(j, _)| j)
                .collect()
        })
        .collect();
    (watchers, preferred)
}

impl<V: Clone + PartialEq + fmt::Debug> Executor<V> {
    pub fn new(mut net: NetDef<V>) -> Result<Self, LoadError> {
        net.check()?;
        let exec = Self::on(net);
        exec.refuse_derived_cycles()?;
        Ok(exec)
    }

    /// The executor of a checked net, on its initial marking.
    fn on(net: NetDef<V>) -> Self {
        let fifo = net
            .places
            .iter()
            .filter(|(_, d)| d.capacity == Capacity::Fifo)
            .map(|(p, _)| p.clone())
            .collect();
        let pools: BTreeMap<_, _> = net
            .places
            .iter()
            .filter_map(|(p, d)| Some((p.clone(), d.pool?)))
            .collect();
        let mut tokens = Tokens {
            fifo,
            pools: pools.clone(),
            ..Tokens::default()
        };
        for (place, size) in &pools {
            for _ in 0..*size {
                tokens
                    .put(place, &Subject::new(POOL_SUBJECT), Record::<V>::new(), None)
                    .expect("an empty pool takes its size");
            }
        }
        let (watchers, preferred) = caches(&net);
        Self {
            net,
            tokens,
            marks: BTreeMap::new(),
            runs: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            parked: BTreeMap::new(),
            held: BTreeMap::new(),
            attempts: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            answers: BTreeMap::new(),
            undone: BTreeSet::new(),
            deltas: BTreeMap::new(),
            conditions: Vec::new(),
            requests: Vec::new(),
            watchers,
            waiters: BTreeMap::new(),
            preferred,
            pending: BTreeMap::new(),
            pending_all: BTreeSet::new(),
            now: Duration::ZERO,
            next_attempt: 0,
            next_minted: 0,
            #[cfg(feature = "naive-twins")]
            twin: None,
        }
    }

    /// A second executor on this net and twin, on the initial marking.
    pub fn fresh(&self) -> Self {
        Self {
            #[cfg(feature = "naive-twins")]
            twin: self.twin,
            ..Self::on(self.net.clone())
        }
    }

    /// Drops every cache the executor rebuilds from its net and marking, as
    /// a restart does (Crash law): the next quiescence decides every auto
    /// pair again.
    pub fn crash(&mut self) {
        (self.watchers, self.preferred) = caches(&self.net);
        self.waiters.clear();
        self.pending.clear();
        self.pending_all = self
            .net
            .transitions
            .iter()
            .filter(|t| t.trigger == Trigger::Auto)
            .map(|t| t.name.clone())
            .collect();
    }

    /// Whether no decline is remembered and every auto transition decides
    /// again for every subject: the state `crash` leaves.
    pub fn decides_all_again(&self) -> bool {
        self.waiters.is_empty()
            && self
                .net
                .transitions
                .iter()
                .filter(|t| t.trigger == Trigger::Auto)
                .all(|t| self.pending_all.contains(&t.name))
    }

    /// Evaluates every derived place once on the initial marking: a cycle it
    /// meets there is a load refusal, one only a later marking reveals
    /// refuses the call that evaluates it.
    fn refuse_derived_cycles(&self) -> Result<(), LoadError> {
        let probe = Subject::new("");
        for (place, def) in &self.net.places {
            if let PlaceKind::Derived(_) = def.kind {
                let mut trace = Trace::default();
                self.reader(&mut trace).derived(place, &probe);
                if let Some(cycle) = trace.cycle {
                    return Err(LoadError::DerivedCycle(cycle));
                }
            }
        }
        Ok(())
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

    pub fn host(&self) -> Host<V> {
        self.net.host
    }

    pub fn tokens(&self) -> &Tokens<V> {
        &self.tokens
    }

    /// The auto pairs enabled when every subject of their consumed and read
    /// places is decided again, with the exclusions of the quiesce loop.
    /// Empty after every successful quiescence (Level-dead law). It runs the
    /// bind of every such pair.
    pub fn level_violations(&self) -> Vec<LevelViolation> {
        let none = BTreeSet::new();
        let mut found = Vec::new();
        for (i, t) in self.net.transitions.iter().enumerate() {
            if t.trigger != Trigger::Auto
                || self.twin(Twin::KeepWorkFailed) && self.takes_failure_port(t)
            {
                continue;
            }
            for s in self.subjects(t) {
                let Ok((_, input)) = self.decide(t, &s) else {
                    continue;
                };
                if self.preferred_enabled(i, &s, &none).is_some() {
                    continue;
                }
                let rows = |ps: &[Place]| {
                    ps.iter()
                        .map(|p| {
                            let row: Vec<_> = self.tokens.row(p, &s).map(|t| &t.payload).collect();
                            format!("{p}={row:?}")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                found.push(LevelViolation {
                    transition: t.name.clone(),
                    subject: s.clone(),
                    marking: format!(
                        "input {input:?}; consumes [{}] reads [{}] inhibits [{}]",
                        rows(&t.consumes),
                        rows(&t.reads),
                        rows(&t.inhibits),
                    ),
                });
            }
        }
        found
    }

    pub fn mark(&self, transition: &str, subject: &str) -> Option<&Mark> {
        self.marks
            .get(&(Arc::from(transition), Subject::new(subject)))
    }

    pub fn marks(&self) -> impl Iterator<Item = (&str, &Subject, &Mark)> {
        self.marks.iter().map(|((t, s), m)| (&**t, s, m))
    }

    /// The retries running now, with the cause of the attempt they retry: the
    /// failure holds until the retry's outcome is known.
    pub fn retrying(&self) -> impl Iterator<Item = (&str, &Subject, &Cause)> {
        self.in_flight.values().filter_map(|a| {
            let run = self.runs.get(&(a.transition.clone(), a.subject.clone()))?;
            Some((&*a.transition, &a.subject, run.failed.as_ref()?))
        })
    }

    /// The attempt number of each attempt running now: 1 for the first try
    /// of its run, one more for each retry of the same run.
    pub fn attempt_numbers(&self) -> impl Iterator<Item = (AttemptId, u32)> {
        self.in_flight.iter().filter_map(|(id, a)| {
            let run = self.runs.get(&(a.transition.clone(), a.subject.clone()))?;
            Some((*id, run.attempts))
        })
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
        if self
            .tokens
            .token(&self.net.asks[work].expired, id)
            .is_some()
        {
            return Ok(Some(Settled::Failed(Cause::definite((self
                .net
                .host
                .answer
                .expired_cause)(
            )))));
        }
        if self.undone.contains(id) {
            return Ok(Some(Settled::Failed(Cause::definite(
                "undone before the work ran",
            ))));
        }
        Ok(None)
    }

    /// Fires the answer transition of the ask on `work` with `value`; the
    /// answer id is the occurrence. The clock check runs first, then the
    /// lookup of `work`, whose miss is [`FireError::UnknownTransition`].
    pub fn answer_ask(
        &mut self,
        work: &str,
        subject: &str,
        id: &str,
        value: Record<V>,
        now: Duration,
    ) -> Result<Quiesced<(), V>, FireError> {
        self.advance::<FireError>(now)?;
        let Some(ask) = self.net.asks.get(work) else {
            return Err(UnknownTransition(work.to_string()).into());
        };
        let answer = ask.answer.clone();
        match self.fire(&answer, subject, OccurrenceId::caller(id), value, now) {
            Ok(fired) => Ok(Quiesced {
                value: (),
                requests: fired.requests,
            }),
            Err(FireError::NoQuiescence(steps)) => {
                self.conditions.push(Condition::NoQuiescence {
                    work: Arc::from(work),
                    subject: Subject::new(subject),
                    steps,
                });
                Err(FireError::NoQuiescence(steps))
            }
            Err(e) => Err(e),
        }
    }

    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }

    /// The readers whose commit waits on an open take, with the rows and the
    /// take occurrences they wait on.
    pub fn held(&self) -> Vec<(OccurrenceId, Vec<WaitEdge>)> {
        self.held
            .values()
            .map(|(a, _)| {
                let edges = a
                    .waits_on
                    .iter()
                    .map(|((p, s, _), take)| (p.clone(), s.clone(), take.clone()))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                (a.occurrence.clone(), edges)
            })
            .collect()
    }

    /// How far the answer `id` got; `None` for an answer this executor
    /// never took.
    pub fn answer_progress(&self, id: &str) -> Option<AnswerProgress<'_, V>> {
        let (work, _) = self.answers.get(id)?;
        if self.undone.contains(id) {
            return Some(AnswerProgress::Undone);
        }
        let occurrence = work_occurrence(work, id);
        Some(match self.occurrences.get(&occurrence) {
            Some(settled) => AnswerProgress::Work {
                occurrence,
                settled: settled.clone(),
            },
            None => {
                let place = &self.net.asks[work].expired;
                match self.tokens.token(place, id) {
                    Some(t) => AnswerProgress::Expired {
                        place,
                        record: t.record(),
                    },
                    None => AnswerProgress::Waiting,
                }
            }
        })
    }

    /// The asks of the net, keyed by their work transition.
    pub fn declared_asks(&self) -> &BTreeMap<Arc<str>, Ask> {
        &self.net.asks
    }

    /// Evaluates the derived `place` for one subject, outside any firing.
    pub fn derive(&self, place: &str, subject: &Subject) -> Result<Option<Record<V>>, DeriveError> {
        self.derived_place(place)?;
        let mut trace = Trace::default();
        let row = self.reader(&mut trace).derived(place, subject);
        match (trace.undeclared, trace.cycle) {
            (Some(read), _) => Err(DeriveError::ReadsUndeclared {
                place: Place::new(place),
                read,
            }),
            (None, Some(cycle)) => Err(DeriveError::Cycle {
                place: Place::new(place),
                cycle,
            }),
            (None, None) => Ok(row),
        }
    }

    fn derived_place(&self, place: &str) -> Result<(), DeriveError> {
        match self.net.places.get(place).map(|d| &d.kind) {
            None => Err(DeriveError::UndeclaredPlace(place.to_string())),
            Some(PlaceKind::Stored { .. }) => Err(DeriveError::StoredPlace(place.to_string())),
            Some(PlaceKind::Derived(_)) => Ok(()),
        }
    }

    /// Evaluates the derived `place` for every subject a stored place holds.
    pub fn derive_all(&self, place: &str) -> Result<Vec<(Subject, Record<V>)>, DeriveError> {
        self.derived_place(place)?;
        let subjects: BTreeSet<Subject> = self
            .net
            .places
            .iter()
            .filter(|(p, d)| {
                matches!(d.kind, PlaceKind::Stored { .. }) && p.as_str() != OUTCOME_UNKNOWN
            })
            .flat_map(|(p, _)| self.tokens.subjects(p))
            .collect();
        let mut rows = Vec::new();
        for s in subjects {
            if let Some(c) = self.derive(place, &s)? {
                rows.push((s, c));
            }
        }
        Ok(rows)
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
    ) -> Result<Quiesced<Undone, V>, UndoError> {
        self.advance::<UndoError>(now)?;
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
        let answered = self.net.asks[&work].answered.clone();
        let withdrawn = self
            .tokens
            .token(&answered, &subject)
            .is_some_and(|t| matches!(&t.payload, Payload::Answered(a) if *a.id == **id));
        if withdrawn {
            self.tokens.remove(&answered, &subject, None);
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
                match self.tokens.put_with(&place, &subject, content, None) {
                    Ok(()) => ReversalOutcome::Restored,
                    Err(PoolFull) => ReversalOutcome::PoolFull,
                }
            } else {
                self.tokens.remove(&place, &subject, None);
                ReversalOutcome::Removed
            };
            reversed.push(Reversal {
                place,
                subject,
                outcome,
            });
        }
        let held: Vec<_> = reversed
            .iter()
            .filter_map(|r| match &r.outcome {
                ReversalOutcome::Held(h) => Some((r.place.clone(), r.subject.clone(), h.clone())),
                _ => None,
            })
            .collect();
        if !held.is_empty() {
            self.tokens.mark_slot(
                &Place::undo_pending(&work),
                &Subject::new(id),
                Some(Payload::UndoPending {
                    answer: id.clone(),
                    held,
                }),
            );
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
        content: Record<V>,
        now: Duration,
    ) -> Result<Quiesced<(), V>, ExecError> {
        self.advance::<ExecError>(now)?;
        self.net.outside_place(place)?;
        if let Some(field) = self.net.missing_answer_field(place, &content) {
            return Err(ExecError::AnswerField {
                place: place.to_string(),
                field,
            });
        }
        let payload = self.net.payload(place, content);
        self.tokens
            .put_with(&Place::new(place), &Subject::new(subject), payload, None)
            .map_err(|PoolFull| ExecError::PoolFull {
                place: place.to_string(),
            })?;
        self.quiesced((), now)
    }

    /// An outside removal of the token of `subject`, such as a probe that no
    /// longer reports it.
    pub fn retract(
        &mut self,
        place: &str,
        subject: &str,
        now: Duration,
    ) -> Result<Quiesced<(), V>, ExecError> {
        self.advance::<ExecError>(now)?;
        self.net.outside_place(place)?;
        if self.tokens.token(place, subject).is_none() {
            return Err(ExecError::NothingToRetract {
                place: place.to_string(),
                subject: subject.to_string(),
            });
        }
        self.tokens
            .remove(&Place::new(place), &Subject::new(subject), None);
        self.quiesced((), now)
    }

    pub fn why_not(
        &self,
        transition: &str,
        subject: &str,
    ) -> Result<Option<WhyNot>, UnknownTransition> {
        let t = self.net.find_transition(transition)?;
        Ok(self.decide(t, &Subject::new(subject)).err())
    }

    /// Runs timers due at `now`, then the auto transitions to quiescence.
    /// Returns the effects to run.
    pub fn step(&mut self, now: Duration) -> Result<Vec<EffectRequest<V>>, ExecError> {
        self.advance::<ExecError>(now)?;
        self.timers(now);
        self.quiesce(now).map_err(ExecError::NoQuiescence)
    }

    fn quiesced<T>(&mut self, value: T, now: Duration) -> Result<Quiesced<T, V>, ExecError> {
        let requests = self.quiesce(now).map_err(ExecError::NoQuiescence)?;
        Ok(Quiesced { value, requests })
    }

    fn takes_failure_port(&self, t: &TransitionDef<V>) -> bool {
        self.net
            .transitions
            .iter()
            .filter_map(|d| d.on_failure.as_ref())
            .any(|port| t.consumes.contains(port))
    }

    /// Fires every enabled auto transition until none is enabled, and takes
    /// the effect requests; `Err` holds the rounds run. It fires no timer.
    fn quiesce(&mut self, now: Duration) -> Result<Vec<EffectRequest<V>>, usize> {
        if self.twin(Twin::NoQuiescence) {
            return Ok(std::mem::take(&mut self.requests));
        }
        for _ in 0..MAX_ROUNDS {
            self.apply_pending_undos();
            self.release_held(now);
            let mut started = Vec::new();
            let mut fired = BTreeSet::new();
            for i in 0..self.net.transitions.len() {
                let t = &self.net.transitions[i];
                if t.trigger != Trigger::Auto
                    || self.twin(Twin::KeepWorkFailed) && self.takes_failure_port(t)
                {
                    continue;
                }
                let name = t.name.clone();
                for s in self.due(i) {
                    let t = &self.net.transitions[i];
                    let mut trace = Trace::default();
                    match self.decide_into(t, &s, &mut trace) {
                        Ok(input) => {
                            if let Some(j) = self.preferred_enabled(i, &s, &fired) {
                                let first = self.net.transitions[j].name.clone();
                                self.pending.entry(first).or_default().insert(s.clone());
                                self.pending.entry(name.clone()).or_default().insert(s);
                                continue;
                            }
                            fired.insert((i, s.clone()));
                            let occurrence = match self.net.answered_of(&name) {
                                Some(p) => work_occurrence(&name, &self.answered(&p, &s).id),
                                None => self.mint(&name),
                            };
                            started.push(self.start(&name, s, occurrence, trace, input, now));
                        }
                        Err(why) => {
                            let c = match why {
                                WhyNot::ReadsUndeclared(place) => {
                                    Some(Condition::ReadsUndeclared {
                                        transition: name.clone(),
                                        subject: s.clone(),
                                        place,
                                    })
                                }
                                WhyNot::BindsAnswerInput => Some(Condition::BindsAnswerInput {
                                    work: name.clone(),
                                    subject: s.clone(),
                                }),
                                WhyNot::DerivedCycle(cycle) => Some(Condition::DerivedCycle {
                                    transition: name.clone(),
                                    subject: s.clone(),
                                    cycle,
                                }),
                                _ => None,
                            };
                            if let Some(c) = c
                                && !self.conditions.contains(&c)
                            {
                                self.conditions.push(c);
                            }
                            for (place, subject) in trace.rows.into_keys() {
                                let subject =
                                    Subject::new(self.tokens.row_subject(&place, &subject));
                                self.waiters
                                    .entry((place, subject))
                                    .or_default()
                                    .insert((name.clone(), s.clone()));
                            }
                        }
                    }
                }
            }
            if started.is_empty() {
                self.derive_parked_waits();
                return Ok(std::mem::take(&mut self.requests));
            }
            for id in started {
                self.run_body(id, now);
            }
        }
        Err(MAX_ROUNDS)
    }

    /// A producer that comes before `i` in priority order and fires for `s`
    /// in this round, or did.
    fn preferred_enabled(
        &self,
        i: usize,
        s: &Subject,
        fired: &BTreeSet<(usize, Subject)>,
    ) -> Option<usize> {
        self.preferred[i].iter().copied().find(|&j| {
            let u = &self.net.transitions[j];
            fired.contains(&(j, s.clone()))
                || !(self.twin(Twin::KeepWorkFailed) && self.takes_failure_port(u))
                    && self.decide(u, s).is_ok()
        })
    }

    /// Settles what an undo could not reverse while a later firing held the
    /// row. A released row is reversed, unless a commit moved its
    /// version: the holder's own commit is disclosed as
    /// [`Condition::UndoHeld`], any other writer's as
    /// [`Condition::UndoRewritten`], and the row stays as that writer left
    /// it (D191-rewritten.a).
    fn apply_pending_undos(&mut self) {
        for i in 0..self.net.transitions.len() {
            let work = self.net.transitions[i].name.clone();
            let pending = Place::undo_pending(&work);
            for answer in self.tokens.subjects(&pending) {
                let Payload::UndoPending { held, .. } = &self
                    .tokens
                    .token(&pending, &answer)
                    .expect("a listed subject holds a token")
                    .payload
                else {
                    panic!("{pending} of {answer} holds no UndoPending token");
                };
                let held = held.clone();
                let occurrence = work_occurrence(&work, &answer.to_string());
                let mut keep = Vec::new();
                for (place, subject, holder) in held {
                    if self.holder(&place, &subject).is_some() {
                        keep.push((place, subject, holder));
                        continue;
                    }
                    let (before, version) =
                        self.deltas[&occurrence][&(place.clone(), subject.clone())].clone();
                    if self.tokens.version(&place, &subject) != version {
                        let writer = self.tokens.writer(&place, &subject);
                        let answer = AnswerId(answer.to_string());
                        let holder = holder_name(&holder);
                        self.conditions
                            .push(if writer.as_deref() == Some(&*holder) {
                                Condition::UndoHeld {
                                    answer,
                                    place,
                                    subject,
                                    holder,
                                }
                            } else {
                                Condition::UndoRewritten {
                                    answer,
                                    place,
                                    subject,
                                    writer,
                                }
                            });
                        continue;
                    }
                    match before {
                        Some(content) => {
                            if let Err(PoolFull) =
                                self.tokens.put_with(&place, &subject, content, None)
                            {
                                self.disclose(Condition::PoolFull {
                                    place,
                                    transition: work.clone(),
                                });
                            }
                        }
                        None => self.tokens.remove(&place, &subject, None),
                    }
                }
                let next = (!keep.is_empty()).then(|| Payload::UndoPending {
                    answer: Arc::from(answer.as_str()),
                    held: keep,
                });
                self.tokens.mark_slot(&pending, &answer, next);
            }
        }
    }

    /// A manual fire: `ACCEPTED(occurrence)` at bind (NET-manual.a). A pure
    /// body runs at once; an effect is a request in the result.
    ///
    /// `value` is the user input (UFT-adopt.a; empty for none). It joins the
    /// bound input after the bind, so no guard reads it; a field the bind
    /// also produces is an impossible state and panics. A fire of `Answer.T`
    /// takes a [`OccurrenceId::Caller`] id equal to its [`AnswerId`].
    pub fn fire(
        &mut self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
        value: Record<V>,
        now: Duration,
    ) -> Result<Quiesced<Accepted, V>, FireError> {
        self.advance::<FireError>(now)?;
        let t = self.net.find_transition(transition)?;
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
        if let Some(field) = value.keys().find(|k| input.contains_key(*k)) {
            return Err(FireError::ValueCollides {
                transition: transition.to_string(),
                field: field.clone(),
            });
        }
        if let Some(work) = self.net.work_of_answer(transition) {
            let answer = match &occurrence {
                OccurrenceId::Caller(id)
                    if value
                        .get(self.net.host.answer.keys.id)
                        .and_then(self.net.host.as_text)
                        == Some(&**id) =>
                {
                    id.clone()
                }
                _ => {
                    return Err(FireError::AnswerOccurrence {
                        transition: transition.to_string(),
                        occurrence,
                    });
                }
            };
            self.answers.insert(answer, (work, subject.clone()));
        }
        input.extend(value);
        let id = self.start(&name, subject, occurrence.clone(), trace, input, now);
        self.run_body(id, now);
        let requests = self.quiesce(now).map_err(FireError::NoQuiescence)?;
        Ok(Quiesced {
            value: Accepted(occurrence),
            requests,
        })
    }

    /// Hands back the result of an effect request. The result
    /// closes only its own attempt; the occurrence settles by `outcome` over
    /// all its attempts. A late result of an abandoned attempt counts the same
    /// way, and a late `Unknown` keeps that attempt open.
    pub fn complete(
        &mut self,
        attempt: AttemptId,
        result: EffectResult<V>,
        now: Duration,
    ) -> Result<Quiesced<Completion, V>, ExecError> {
        self.advance::<ExecError>(now)?;
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
        live: Option<Attempt<V>>,
        result: EffectResult<V>,
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
            // A probe reads the goal: no attempt that already answered took
            // effect.
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
        if self.held.contains_key(occurrence) {
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
    fn continuation(&mut self, a: &Attempt<V>) -> Option<bool> {
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
    fn attempt_of(&mut self, occurrence: &OccurrenceId, live: Option<Attempt<V>>) -> Attempt<V> {
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
    fn unpark(&mut self, occurrence: &OccurrenceId) -> Result<Attempt<V>, ExecError> {
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
        resolution: Resolution<V>,
        now: Duration,
    ) -> Result<Quiesced<(), V>, ExecError> {
        self.advance::<ExecError>(now)?;
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
                for (p, token) in &a.taken {
                    self.commit_consumption(p, &a.subject, token.id, &a.transition);
                }
                let cause = Cause::definite("dropped by the user");
                self.occurrences
                    .insert(occurrence.clone(), Some(Settled::Failed(cause.clone())));
                self.run_failed(&a.transition, &a.subject, &cause, now);
            }
        }
        self.quiesced((), now)
    }

    /// Moves the clock to `now`, or refuses it and changes nothing.
    fn advance<E: From<ClockWentBack> + From<ClockOutOfRange>>(
        &mut self,
        now: Duration,
    ) -> Result<(), E> {
        if now < self.now {
            return Err(ClockWentBack {
                clock: self.now,
                now,
            }
            .into());
        }
        if now > self.net.last_now {
            return Err(ClockOutOfRange {
                now,
                last: self.net.last_now,
            }
            .into());
        }
        self.now = now;
        Ok(())
    }

    fn mint(&mut self, transition: &Arc<str>) -> OccurrenceId {
        self.next_minted += 1;
        OccurrenceId::Minted {
            transition: transition.clone(),
            n: self.next_minted,
        }
    }

    /// The subjects to decide `transitions[i]` for: those it may fire for
    /// whose decision may have changed since it was last taken.
    fn due(&mut self, i: usize) -> Vec<Subject> {
        let t = &self.net.transitions[i];
        for row in std::mem::take(&mut self.tokens.touched) {
            let (place, subject) = &row;
            for name in self.watchers.get(place).into_iter().flatten() {
                if self.tokens.pools.contains_key(place) {
                    self.pending_all.insert(name.clone());
                } else {
                    self.pending
                        .entry(name.clone())
                        .or_default()
                        .insert(subject.clone());
                }
            }
            for (name, s) in self.waiters.remove(&row).into_iter().flatten() {
                self.pending.entry(name).or_default().insert(s);
            }
        }
        let pending = self.pending.remove(&t.name).unwrap_or_default();
        if self.pending_all.remove(&t.name) {
            return self.subjects(t);
        }
        let due = pending
            .into_iter()
            .filter(|s| {
                t.consumes
                    .iter()
                    .chain(&t.reads)
                    .any(|p| self.tokens.holds(p, s))
            })
            .collect();
        self.in_pool_order(t, due)
    }

    /// Decides `(transition, subject)` again in the next quiescence.
    fn redecide(&mut self, (transition, subject): &(Arc<str>, Subject)) {
        self.pending
            .entry(transition.clone())
            .or_default()
            .insert(subject.clone());
    }

    /// The subjects `t` may fire for.
    fn subjects(&self, t: &TransitionDef<V>) -> Vec<Subject> {
        let all = t
            .consumes
            .iter()
            .chain(&t.reads)
            .flat_map(|p| self.tokens.subjects(p))
            .collect();
        self.in_pool_order(t, all)
    }

    /// A firing that takes from a pool serves the subject with the oldest
    /// waiting token first.
    fn in_pool_order(&self, t: &TransitionDef<V>, subjects: BTreeSet<Subject>) -> Vec<Subject> {
        let mut subjects: Vec<_> = subjects.into_iter().collect();
        if t.consumes.iter().any(|p| self.tokens.pools.contains_key(p)) {
            subjects.sort_by_key(|s| {
                t.consumes
                    .iter()
                    .filter(|p| !self.tokens.pools.contains_key(*p))
                    .filter_map(|p| self.tokens.token(p, s))
                    .map(|token| token.id)
                    .min()
            });
        }
        subjects
    }

    fn available(&self, place: &Place, subject: &Subject) -> bool {
        self.tokens
            .slot(place, subject)
            .is_some_and(|s| !s.tokens.is_empty() && !s.reserved)
    }

    pub(crate) fn holder(&self, place: &Place, subject: &Subject) -> Option<Holder> {
        let holds =
            |a: &Attempt<V>| &a.subject == subject && a.taken.iter().any(|(p, _)| p == place);
        if let Some(a) = self
            .in_flight
            .values()
            .chain(self.held.values().map(|(a, _)| a))
            .find(|a| holds(a))
        {
            return Some(Holder::Running(a.transition.clone()));
        }
        self.parked
            .iter()
            .find(|(_, a)| holds(a))
            .map(|(o, _)| Holder::OutcomeUnknown(o.clone()))
    }

    /// The enabledness check that decides a firing: its read trace and its
    /// bound input, or the first reason it cannot fire.
    fn decide(&self, t: &TransitionDef<V>, s: &Subject) -> Result<(Trace<V>, Record<V>), WhyNot> {
        let mut trace = Trace::default();
        let input = self.decide_into(t, s, &mut trace)?;
        Ok((trace, input))
    }

    /// [`decide`](Self::decide) that leaves in `trace` what a refusal read
    /// beyond the transition's own rows of `s`.
    fn decide_into(
        &self,
        t: &TransitionDef<V>,
        s: &Subject,
        trace: &mut Trace<V>,
    ) -> Result<Record<V>, WhyNot> {
        if self.tokens.token(&t.running(), s).is_some() {
            return Err(WhyNot::InFlight);
        }
        let bound = self.bind_input(t, s, trace);
        if matches!(
            bound,
            Err(WhyNot::ReadsUndeclared(_) | WhyNot::DerivedCycle(_))
        ) {
            return bound;
        }
        let mark = self.marks.get(&(t.name.clone(), s.clone()));
        let same_input = bound.is_ok() && mark.is_some_and(|m| m.key == self.key(trace));
        // ADR 0032 Q6.a: new input starts a new run at once, not after the
        // backoff.
        if let Some(due) = self.retry_due(&t.name, s)
            && (bound.is_err() || same_input || mark.is_none())
        {
            return Err(WhyNot::RetryPending { due });
        }
        let mut input = bound?;
        if t.trigger == Trigger::Auto
            && same_input
            && let Some(mark) = mark
        {
            return Err(WhyNot::FailedOnThisInput(mark.cause.clone()));
        }
        if let Some(p) = &self.net.answered_of(&t.name) {
            let key = self.net.host.answer.keys.input;
            if input.contains_key(key) {
                return Err(WhyNot::BindsAnswerInput);
            }
            input.insert(key.to_owned(), self.answered(p, s).input.clone());
        }
        Ok(input)
    }

    /// The bound input of `t` for `s` from its arcs, its answer and its bind,
    /// with every row read in `trace`.
    fn bind_input(
        &self,
        t: &TransitionDef<V>,
        s: &Subject,
        trace: &mut Trace<V>,
    ) -> Result<Record<V>, WhyNot> {
        let present = |p: &Place| self.tokens.token(p, s).is_some();
        let asked = self.net.answered_of(&t.name).is_some();
        if let Some(p) = t
            .consumes
            .iter()
            .find(|p| !self.available(p, s) || asked && self.front_taker(p, s).is_some())
        {
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
        for p in t.consumes.iter().chain(&t.reads).chain(&t.inhibits) {
            trace.row(&self.tokens, p, s);
        }
        let answered = self.net.answered_of(&t.name);
        if let Some(p) = &answered
            && !self.twin(Twin::OccurrenceNotInTrace)
        {
            let id = (self.net.host.text)(&self.answered(p, s).id);
            let field = self.net.host.answer.keys.id.to_owned();
            trace.row(&self.tokens, p, s).fields.insert(field, Some(id));
        }
        let bound = (t.bind)(&mut self.reader(trace), s);
        if let Some(place) = &trace.undeclared {
            return Err(WhyNot::ReadsUndeclared(place.clone()));
        }
        if let Some(cycle) = &trace.cycle {
            return Err(WhyNot::DerivedCycle(cycle.clone()));
        }
        bound.ok_or(WhyNot::Guard)
    }

    /// The `Answered` token a work firing consumes.
    fn answered(&self, place: &Place, subject: &Subject) -> &Answered<V> {
        let token = self
            .tokens
            .token(place, subject)
            .expect("decide checked every consumed token");
        match &token.payload {
            Payload::Answered(a) => a,
            other => panic!("the {place} token of {subject} holds {other:?}"),
        }
    }

    fn reader<'a>(&'a self, trace: &'a mut Trace<V>) -> Reader<'a, V> {
        Reader {
            exec: self,
            trace,
            hide_value: !self.twin(Twin::ValueBeforeBind),
            committed: false,
        }
    }

    /// The content hash of a read trace: blake3 over a canonical encoding of
    /// what was read, never of versions.
    fn key(&self, trace: &Trace<V>) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        encode_rows(
            &mut h,
            trace,
            self.twin(Twin::MarkByVersion),
            self.net.host.encode,
        );
        h.finalize()
    }

    /// The take: inputs move into `<T>.Running`, with no version change.
    fn start(
        &mut self,
        transition: &Arc<str>,
        subject: Subject,
        occurrence: OccurrenceId,
        trace: Trace<V>,
        input: Record<V>,
        now: Duration,
    ) -> AttemptId {
        let t = self.net.transition_def(transition);
        let consumes = t.consumes.clone();
        let (mut taken, mut taken_at) = (Vec::new(), BTreeMap::new());
        let key = (transition.clone(), subject.clone());
        if self.retry_due(transition, &subject).is_some() {
            self.end_run(&key);
        }
        for p in consumes {
            let hidden = self.twin(Twin::HiddenReservation);
            let slot = self.tokens.slot_mut(&p, &subject);
            taken_at.insert(p.clone(), slot.version);
            let token = if hidden {
                slot.reserved = true;
                slot.tokens.front().cloned()
            } else {
                slot.tokens.pop_front()
            }
            .expect("decide checked every consumed token");
            self.tokens.lend(&p);
            taken.push((p, token));
        }
        let waits_on = self.held_from_read_rows(&trace, &taken, &subject, now);
        let run = self.runs.entry(key).or_insert(Run {
            started: now,
            attempts: 0,
            failed: None,
        });
        run.attempts += 1;
        self.occurrences.insert(occurrence.clone(), None);
        let a = Attempt {
            occurrence,
            transition: transition.clone(),
            subject,
            taken,
            taken_at,
            trace,
            input,
            started: now,
            probe: false,
            waits_on,
            stale_rows: BTreeSet::new(),
        };
        self.occupy_running(&a);
        self.insert_attempt(a)
    }

    /// The tokens that open takes hold from the rows `trace` read, except
    /// the rows the new attempt consumes itself, pools, and tokens a failed
    /// take cannot put back because a newer one superseded them.
    fn held_from_read_rows(
        &self,
        trace: &Trace<V>,
        taken: &[(Place, Token<V>)],
        subject: &Subject,
        now: Duration,
    ) -> BTreeMap<(Place, Subject, u64), OccurrenceId> {
        let mut held = BTreeMap::new();
        for ((p, s), _) in trace.rows.iter().filter(|(_, r)| !r.committed) {
            if s == subject && taken.iter().any(|(own, _)| own == p)
                || self.tokens.pools.contains_key(p)
            {
                continue;
            }
            for o in self.live_attempts() {
                assert!(o.started <= now, "a take open at a bind started after it");
                if &o.subject == s {
                    held.extend(
                        o.taken
                            .iter()
                            .filter(|(tp, _)| tp == p && !self.superseded(p, s, o))
                            .map(|(_, t)| ((p.clone(), s.clone(), t.id), o.occurrence.clone())),
                    );
                }
            }
        }
        held
    }

    fn retry_due(&self, transition: &str, subject: &Subject) -> Option<Duration> {
        let token = self
            .tokens
            .token(&Place::retry_pending(transition), subject)?;
        let Payload::RetryPending { due } = token.payload else {
            panic!("'{transition}.RetryPending' of {subject} holds {token:?}");
        };
        Some(due)
    }

    fn set_retry_due(&mut self, key: &(Arc<str>, Subject), due: Option<Duration>) {
        let payload = due.map(|due| Payload::RetryPending { due });
        self.tokens
            .mark_slot(&Place::retry_pending(&key.0), &key.1, payload);
    }

    fn end_run(&mut self, key: &(Arc<str>, Subject)) {
        self.runs.remove(key);
        self.set_retry_due(key, None);
    }

    fn occupy_running(&mut self, a: &Attempt<V>) {
        self.tokens.mark_slot(
            &Place::running(&a.transition),
            &a.subject,
            Some(Payload::Running {
                occurrence: a.occurrence.clone(),
            }),
        );
    }

    /// `<T>.Running` empties when its last attempt ends. A resolved parked
    /// occurrence can end while a newer one of the same subject still runs.
    fn release_running(&mut self, transition: &Arc<str>, subject: &Subject) {
        match self
            .in_flight
            .values()
            .chain(self.held.values().map(|(a, _)| a))
            .find(|o| &o.transition == transition && &o.subject == subject)
            .cloned()
        {
            Some(other) => self.occupy_running(&other),
            None => self
                .tokens
                .mark_slot(&Place::running(transition), subject, None),
        }
    }

    /// Whether a failed `take` drops its token of `place` rather than put
    /// it back: a single-token row moved since the take, which only a newer
    /// token can do, whether it is still in the row, held by a later take,
    /// or already consumed.
    fn superseded(&self, place: &Place, subject: &Subject, take: &Attempt<V>) -> bool {
        !self.tokens.fifo.contains(place)
            && self.tokens.version(place, subject) != take.taken_at[place]
    }

    /// The token a row shows in the committed state: its own, or one that an
    /// open take holds from it and would put back on failure. In a FIFO row
    /// that is the oldest by token id; a single-token row's own token is
    /// newer than a held one.
    fn committed_front(&self, place: &str, subject: &str) -> Option<&Token<V>> {
        let (place, subject) = (Place::new(place), Subject::new(subject));
        if self.tokens.pools.contains_key(&place) {
            return self.tokens.token(&place, &subject);
        }
        let held = self
            .live_attempts()
            .filter(|a| a.subject == subject)
            .flat_map(|a| {
                a.taken
                    .iter()
                    .filter(|(p, _)| *p == place && !self.superseded(p, &subject, a))
                    .map(|(_, t)| t)
            });
        if self.tokens.fifo.contains(&place) {
            held.chain(self.tokens.token(&place, &subject))
                .min_by_key(|t| t.id)
        } else {
            self.tokens
                .token(&place, &subject)
                .or_else(|| held.max_by_key(|t| t.id))
        }
    }

    /// The open take that holds the committed front of a row ahead of the
    /// row's own head. An asked work takes only the front its situation
    /// showed, so it waits for that take.
    fn front_taker(&self, place: &Place, subject: &Subject) -> Option<&Attempt<V>> {
        let front = self.committed_front(place, subject)?.id;
        if self
            .tokens
            .token(place, subject)
            .is_some_and(|t| t.id == front)
        {
            return None;
        }
        self.live_attempts()
            .find(|a| a.taken.iter().any(|(p, t)| p == place && t.id == front))
    }

    /// The attempts whose takes are open: in flight, parked or held.
    fn live_attempts(&self) -> impl Iterator<Item = &Attempt<V>> {
        self.in_flight
            .values()
            .chain(self.parked.values())
            .chain(self.held.values().map(|(a, _)| a))
    }

    fn live_attempts_mut(&mut self) -> impl Iterator<Item = &mut Attempt<V>> {
        self.in_flight
            .values_mut()
            .chain(self.parked.values_mut())
            .chain(self.held.values_mut().map(|(a, _)| a))
    }

    /// Records every held reader whose takes all committed or failed, or that
    /// a write made stale, until none is left: one release can end the wait
    /// of another. No outcome of a take makes a stale reader current again.
    fn release_held(&mut self, now: Duration) {
        while let Some(occurrence) = self
            .held
            .iter()
            .find(|(_, (a, _))| a.waits_on.is_empty() || self.stale(a).is_some())
            .map(|(o, _)| o.clone())
        {
            let (a, outputs) = self.held.remove(&occurrence).expect("found above");
            self.record(a, outputs, now);
        }
    }

    /// Every wait on a parked take, derived again from both sources: a held
    /// reader with an edge to it, and an asked work whose committed front it
    /// holds. Each lasts while the take is parked and the reader waits.
    fn derive_parked_waits(&mut self) {
        let mut waits = BTreeMap::new();
        for r in self.live_attempts() {
            for take in r.waits_on.values() {
                if let Some(t) = self.parked.get(take) {
                    waits.insert(
                        (take.clone(), r.occurrence.clone()),
                        Condition::WaitsOnParkedTake {
                            reader: r.transition.clone(),
                            subject: r.subject.clone(),
                            reader_occurrence: r.occurrence.clone(),
                            take: t.transition.clone(),
                            take_occurrence: take.clone(),
                        },
                    );
                }
            }
        }
        for t in &self.net.transitions {
            let Some(answered) = self.net.answered_of(&t.name) else {
                continue;
            };
            for s in self.tokens.subjects(&answered) {
                for p in &t.consumes {
                    let Some(take) = self
                        .front_taker(p, &s)
                        .filter(|a| self.parked.contains_key(&a.occurrence))
                    else {
                        continue;
                    };
                    let reader_occurrence =
                        work_occurrence(&t.name, &self.answered(&answered, &s).id);
                    waits.insert(
                        (take.occurrence.clone(), reader_occurrence.clone()),
                        Condition::WaitsOnParkedTake {
                            reader: t.name.clone(),
                            subject: s.clone(),
                            reader_occurrence,
                            take: take.transition.clone(),
                            take_occurrence: take.occurrence.clone(),
                        },
                    );
                }
            }
        }
        self.conditions
            .retain(|c| !matches!(c, Condition::WaitsOnParkedTake { .. }));
        self.conditions.extend(waits.into_values());
    }

    fn insert_attempt(&mut self, a: Attempt<V>) -> AttemptId {
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

    fn request(&mut self, id: AttemptId, a: &Attempt<V>) {
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
    fn issue(&mut self, mut a: Attempt<V>, probe: bool, now: Duration) {
        a.started = now;
        a.probe = probe;
        let id = self.insert_attempt(a.clone());
        self.request(id, &a);
    }

    /// The READ row a failed take put a missed token back into, else the
    /// first READ row of the trace whose version moved since it was read.
    ///
    /// A consumed row cannot be stale (REV11-running.a): the firing holds it,
    /// because [`start`] took its token out of the place. A write that lands
    /// in the empty slot moves that slot's version without touching anything
    /// the firing read.
    ///
    /// [`start`]: Executor::start
    fn stale(&self, a: &Attempt<V>) -> Option<(Place, Subject)> {
        if self.twin(Twin::NoCas) {
            return None;
        }
        if let Some(row) = a.stale_rows.first() {
            return Some(row.clone());
        }
        a.trace
            .rows
            .iter()
            .filter(|((p, s), _)| *s != a.subject || a.taken.iter().all(|(held, _)| held != p))
            .filter(|((p, _), _)| !self.tokens.pools.contains_key(p))
            .find(|((p, s), r)| self.tokens.version(p, s) != r.version)
            .map(|(k, _)| k.clone())
    }

    /// The record never clobbers: every output compares and sets against the
    /// read trace (NET-fire.a), a fact included, so the staleness of the trace
    /// decides before any fact is put (322/E3.c). A stale effect happened, so
    /// each row goes to its place's candidate, applies on a data-class place,
    /// or is withheld, and a mark on its input stays: the effect never runs
    /// twice on it. A reader with open edges waits in `held`, unless a write
    /// already made it stale: no take's outcome can make it current again.
    fn record(&mut self, a: Attempt<V>, outputs: Vec<Output<V>>, now: Duration) {
        if !a.waits_on.is_empty() && self.stale(&a).is_none() && !self.twin(Twin::NoCommitWait) {
            self.occurrences.insert(a.occurrence.clone(), None);
            self.held.insert(a.occurrence.clone(), (a, outputs));
            return;
        }
        if let Some(e) = self.net.refused_output(&outputs) {
            let cause = Cause::definite(format!("'{}' output refused: {e}", a.transition));
            return self.fail(a, cause, now);
        }
        let t = self.net.transition_def(&a.transition);
        let pure = matches!(t.body, Body::Pure(_));
        let (facts, rows): (Vec<_>, Vec<_>) = outputs
            .into_iter()
            .partition(|o| t.facts.contains(&o.place));
        let Some((place, subject)) = self.stale(&a) else {
            for f in facts {
                self.put_fact(&a, f);
            }
            self.marks
                .remove(&(a.transition.clone(), a.subject.clone()));
            let rows = rows
                .into_iter()
                .map(|o| {
                    let payload = self.net.payload(&o.place, o.content);
                    (o.place, o.subject, payload)
                })
                .collect();
            return self.commit(a, rows);
        };
        let cause = Cause::definite(format!(
            "stale inputs: {place} of {subject} changed since '{}' read it",
            a.transition
        ));
        if pure {
            // A pure firing that never committed is no try.
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
        let (mut kept, mut routes) = (Vec::new(), Vec::new());
        for f in facts {
            if self.moved_read_row(&a, &f) {
                routes.push(Routed {
                    place: f.place.clone(),
                    subject: f.subject.clone(),
                    route: Route::Withheld,
                });
            } else {
                self.put_fact(&a, f);
            }
        }
        for o in &rows {
            let route = if let Some(c) = self.net.candidate_of(&o.place) {
                let base = a
                    .trace
                    .rows
                    .get(&(o.place.clone(), o.subject.clone()))
                    .map(|r| r.fields.clone());
                let candidate = Candidate {
                    base,
                    output: o.content.clone(),
                    trace: a.trace.clone(),
                };
                kept.push((
                    c.clone(),
                    o.subject.clone(),
                    Payload::Candidate(Box::new(candidate)),
                ));
                Route::Candidate(c.clone())
            } else if self.net.places[&o.place].class == Class::Data {
                kept.push((
                    o.place.clone(),
                    o.subject.clone(),
                    Payload::Record(o.content.clone()),
                ));
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
            self.redecide(&run_key);
        }
        let occurrence = a.occurrence.clone();
        // No kept row can bypass the compare-and-set: a candidate place is
        // FIFO by the load check, and a data-class place merges.
        self.commit(a, kept);
        self.occurrences.insert(
            occurrence,
            Some(Settled::OverChangedInputs { cause, routes }),
        );
    }

    /// Commits one fact. A fact into a row the firing READ is a
    /// compare-and-set against the token it read, and the caller reaches this
    /// only while that trace stands, so the put replaces what the firing saw
    /// (322/E3.c). A fact into any other row carries no comparison: the
    /// place's declaration alone says whether it may replace what it finds
    /// (322/E3.b).
    fn put_fact(&mut self, a: &Attempt<V>, f: Output<V>) {
        if !self.read_row(a, &f) {
            self.refuse_blind_overwrite(&f);
        }
        self.put_by(&f.place, &f.subject, f.content, &a.transition);
    }

    /// A firing's put; a token into a full pool is not put, and the net
    /// discloses it once per place and transition.
    fn put_by(&mut self, place: &Place, subject: &Subject, content: Record<V>, by: &Arc<str>) {
        let payload = self.net.payload(place, content);
        self.put_by_with(place, subject, payload, by);
    }

    fn put_by_with(
        &mut self,
        place: &Place,
        subject: &Subject,
        payload: Payload<V>,
        by: &Arc<str>,
    ) {
        if let Err(PoolFull) = self.tokens.put_with(place, subject, payload, Some(by)) {
            self.disclose(Condition::PoolFull {
                place: place.clone(),
                transition: by.clone(),
            });
        }
    }

    /// A failure or timeout port token: no data, the cause in its header.
    fn put_port(&mut self, port: &Place, subject: &Subject, cause: Cause, by: &Arc<str>) {
        self.put_by_with(port, subject, Payload::Failure { cause }, by);
    }

    fn disclose(&mut self, c: Condition) {
        if !self.conditions.contains(&c) {
            self.conditions.push(c);
        }
    }

    /// Whether the compare-and-set of a fact into a row the firing read fails:
    /// the row moved since the firing read it, or a failed take put back a
    /// token the firing missed there, so that token wins and the fact is not
    /// put (322/E3.c). Another input of the same firing having
    /// changed does not withhold this fact — the comparison is against the row
    /// the fact writes.
    fn moved_read_row(&self, a: &Attempt<V>, o: &Output<V>) -> bool {
        let row = (o.place.clone(), o.subject.clone());
        self.read_row(a, o)
            && (a.stale_rows.contains(&row)
                || self.tokens.version(&o.place, &o.subject) != a.trace.rows[&row].version)
    }

    /// Whether the firing read the row this output writes, so that the row's
    /// version in the trace is what a put compares against. Two rows in the
    /// trace are not read: one the firing consumed holds that token, so there
    /// is nothing to compare (REV11-running.a), and one it only inhibited held
    /// nothing, so no token can be lost and the declaration decides
    /// (322/E3.b).
    fn read_row(&self, a: &Attempt<V>, o: &Output<V>) -> bool {
        let t = self.net.transition_def(&a.transition);
        let inhibit_only = t.inhibits.contains(&o.place)
            && !t.reads.contains(&o.place)
            && !t.consumes.contains(&o.place);
        a.trace
            .rows
            .contains_key(&(o.place.clone(), o.subject.clone()))
            && !inhibit_only
            && (o.subject != a.subject || a.taken.iter().all(|(held, _)| *held != o.place))
    }

    /// A put that no compare-and-set backs must not land on an occupied
    /// [`Capacity::Single`] place: its writer never saw the held token, so
    /// the put would drop it, and losing a token is an executor defect in its
    /// own right (REV2-cand.b). For a row the firing did not read, the
    /// place's declaration decides: a [`Capacity::Register`] place is the one
    /// whose fact put replaces (322/E3.b), and a data-class place is exempt
    /// because its outputs merge in Loro instead of replacing a token
    /// (NET-fire.a). Only [`put_fact`] calls this.
    ///
    /// [`put_fact`]: Executor::put_fact
    fn refuse_blind_overwrite(&self, o: &Output<V>) {
        let def = &self.net.places[&o.place];
        if def.capacity != Capacity::Single || def.class == Class::Data {
            return;
        }
        if let Some(held) = self.tokens.token(&o.place, &o.subject) {
            let put = self.net.payload(&o.place, o.content.clone());
            assert!(
                held.payload == put,
                "a put with no compare-and-set into the single-token place {} of {} would drop the held token {:?} for {:?}",
                o.place,
                o.subject,
                held.payload,
                put
            );
        }
    }

    fn commit(&mut self, a: Attempt<V>, rows: Vec<(Place, Subject, Payload<V>)>) {
        let mut before = BTreeMap::new();
        if let OccurrenceId::Work { .. } = &a.occurrence {
            let answered = self.net.answered_of(&a.transition);
            let keep_answered = self.twin(Twin::UndoRestoresAnswered);
            for (p, token) in &a.taken {
                let rewritten = self
                    .tokens
                    .slot(p, &a.subject)
                    .is_some_and(|s| !s.tokens.is_empty() && !s.reserved);
                if !rewritten && (keep_answered || Some(p) != answered.as_ref()) {
                    before.insert((p.clone(), a.subject.clone()), Some(token.payload.clone()));
                }
            }
            for (place, subject, _) in &rows {
                before
                    .entry((place.clone(), subject.clone()))
                    .or_insert_with(|| {
                        self.tokens.token(place, subject).map(|t| t.payload.clone())
                    });
            }
        }
        for (p, token) in &a.taken {
            self.commit_consumption(p, &a.subject, token.id, &a.transition);
        }
        for (place, subject, payload) in rows {
            self.put_by_with(&place, &subject, payload, &a.transition);
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

    /// The consumed token leaves the net; its place changes only now. On a
    /// single-token place a newer token written meanwhile already carries a
    /// newer version; on a FIFO place every token left behind is younger than
    /// the consumed one, so the row did change.
    ///
    /// A reader that bound while this take held the token read the row as it
    /// is now committed: its read version moves along, unless another write
    /// moved the row meanwhile.
    fn commit_consumption(&mut self, place: &Place, subject: &Subject, token: u64, by: &Arc<str>) {
        self.tokens.unlend(place);
        let fifo = self.tokens.fifo.contains(place);
        let slot = self.tokens.slot_mut(place, subject);
        let before = slot.version;
        if slot.reserved {
            slot.reserved = false;
            slot.tokens.pop_front();
            slot.version += 1;
        } else if fifo || slot.tokens.is_empty() {
            slot.version += 1;
        }
        if slot.version != before {
            slot.writer = Some(by.clone());
        }
        let after = slot.version;
        let edge = (place.clone(), subject.clone(), token);
        let row = (place.clone(), subject.clone());
        for r in self.live_attempts_mut() {
            if r.waits_on.remove(&edge).is_some()
                && let Some(read) = r.trace.rows.get_mut(&row)
                && read.version == before
            {
                read.version = after;
            }
        }
    }

    /// The inputs go back with no version change: see [`Tokens::restore`].
    /// A reader that bound while this take held a token goes stale when the
    /// token is back in the row it read; a token dropped for a newer one
    /// leaves the version check to decide.
    fn rollback(&mut self, a: &Attempt<V>) {
        for (p, token) in &a.taken {
            let superseded = self.superseded(p, &a.subject, a);
            let back = self.tokens.restore(p, &a.subject, token, superseded);
            let edge = (p.clone(), a.subject.clone(), token.id);
            for r in self.live_attempts_mut() {
                if r.waits_on.remove(&edge).is_some() && back {
                    r.stale_rows.insert((p.clone(), a.subject.clone()));
                }
            }
        }
        self.release_running(&a.transition, &a.subject);
    }

    fn set_mark(&mut self, a: &Attempt<V>, cause: &Cause) {
        if self.twin(Twin::NoMark) {
            return;
        }
        let key = self.key(&a.trace);
        let run_key = (a.transition.clone(), a.subject.clone());
        self.redecide(&run_key);
        self.marks.insert(
            run_key,
            Mark {
                key,
                cause: cause.clone(),
            },
        );
    }

    /// A definite failure: the inputs go back and the failure mark is set.
    fn fail(&mut self, a: Attempt<V>, cause: Cause, now: Duration) {
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
        let Some(run) = self.runs.get_mut(&key) else {
            return;
        };
        if cause.retryable
            && let Some(retry) = retry
            && (unbounded || run.attempts < retry.max_attempts)
        {
            run.failed = Some(cause.clone());
            self.set_retry_due(&key, Some(now + retry.backoff));
            return;
        }
        self.end_run(&key);
        if let Some(port) = on_failure {
            self.put_port(&port, subject, cause.clone(), transition);
        }
    }

    fn park(&mut self, a: Attempt<V>, cause: Cause, now: Duration) {
        self.release_running(&a.transition, &a.subject);
        self.tokens.mark_slot(
            &Place::new(OUTCOME_UNKNOWN),
            &Subject::new(&a.occurrence.to_string()),
            Some(Payload::Parked {
                transition: a.transition.clone(),
                subject: a.subject.clone(),
                cause: cause.clone(),
            }),
        );
        self.set_mark(&a, &cause);
        self.occurrences.insert(
            a.occurrence.clone(),
            Some(Settled::OutcomeUnknown(cause.clone())),
        );
        // The run stays open: only Drop or the run timer lets the next producer
        // fire.
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
            .map(|a| a.occurrence.clone());
        self.tokens.mark_slot(
            &Place::outcome_unknown(transition),
            subject,
            occurrence.map(|occurrence| Payload::OutcomeUnknown { occurrence }),
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
                    self.put_port(&port, &key.1, cause, &key.0);
                }
            } else if self.retry_due(&key.0, &key.1).is_some_and(|due| due <= now) {
                self.set_retry_due(&key, None);
                self.marks.remove(&key);
                self.redecide(&key);
            }
        }
    }
}

fn encode_rows<V: Clone + PartialEq + fmt::Debug>(
    h: &mut blake3::Hasher,
    trace: &Trace<V>,
    with_version: bool,
    encode: fn(&V) -> Vec<u8>,
) {
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
                    h.update(&encode(v));
                }
            }
        }
        if let Some(cause) = &row.cause {
            h.update(&[2]);
            match cause {
                None => {
                    h.update(&[0]);
                }
                Some(c) => {
                    h.update(&[1, u8::from(c.retryable)]);
                    encode_str(h, &c.text);
                }
            }
        }
        if let Some(output) = &row.candidate {
            h.update(&[3]);
            match output {
                None => {
                    h.update(&[0]);
                }
                Some(o) => {
                    h.update(&[1]);
                    for (field, value) in o {
                        encode_str(h, field);
                        h.update(&encode(value));
                    }
                }
            }
        }
    }
}

fn encode_str(h: &mut blake3::Hasher, s: &str) {
    h.update(&(s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}
