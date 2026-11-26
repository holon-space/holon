//! Every executor call a caller can make returns a value or an error; none
//! panics. The nets are drawn from the net file format plus derived places
//! and lowered asks, the bindings and bodies from a small program type, and
//! the calls from every public operation with arguments that match the net
//! and arguments that contradict it.
//!
//! The error forms are matched by their `Debug` names, so the properties
//! compile on an executor that has none of them yet and fail there for the
//! panic.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use holon_api::TypeHint;
use holon_api::Value;
use holon_net::exec::AttemptId;
use holon_net::exec::Cause;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::OUTCOME_UNKNOWN;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::POOL_SUBJECT;
use holon_net::exec::Payload;
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Resolution;
use holon_net::exec::Subject;
use holon_net::exec::Token;
use holon_net::exec::WhyNot;
use holon_net::file::CommandSpec;
use holon_net::file::DurationSpec;
use holon_net::file::EffectSpec;
use holon_net::file::ExitCodeMap;
use holon_net::file::NetFile;
use holon_net::file::Outcome;
use holon_net::file::PlaceCapacity;
use holon_net::file::PlaceSpec;
use holon_net::file::Receiver;
use holon_net::file::ShellEffect;
use holon_net::file::TimerSpec;
use holon_net::file::TransitionSpec;
use holon_net::file::Trigger;
use holon_net::load::Logic;
use holon_net::load::net_def;
use holon_net::uft::Generated;
use holon_net::uft::NativeBody;
use holon_net::uft::NativeParts;
use holon_net::uft::answer::SEEN_FIELD;
use holon_net::uft::lower;
use holon_transition_contract::ANSWER_ID_KEY;
use holon_transition_contract::ANSWER_INPUT_KEY;
use holon_transition_contract::AnswerCall;
use holon_transition_contract::AnswerId;
use holon_transition_contract::ArcKind;
use holon_transition_contract::AskDecl;
use holon_transition_contract::AskHash;
use holon_transition_contract::BodyDecl;
use holon_transition_contract::HlArc;
use holon_transition_contract::NativeRef;
use holon_transition_contract::PlaceName;
use holon_transition_contract::SubjectName;
use holon_transition_contract::TransitionDescriptor;
use holon_transition_contract::TransitionName;
use indexmap::IndexMap;
use proptest::prelude::*;
use proptest::sample::select;

mod forms;

use forms::Answer;

use crate::exec_support::Checked;
use crate::exec_support::values::value;

const PLACES: [&str; 4] = ["P0", "P1", "P2", "P3"];
const DERIVED: [&str; 2] = ["D0", "D1"];
const TRANSITIONS: [&str; 4] = ["T0", "T1", "T2", "T3"];
const WORKS: [&str; 2] = ["W0", "W1"];
/// The field a body tags its outputs with, so a test sees who produced them.
const BY: &str = "by";

// ------------------------------------------------------------ generators

/// Field names: a small alphabet, so a caller value and a bind overlap often,
/// the answer keys, and any other text.
fn field() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => select(vec!["x", "y", "v"]).prop_map(str::to_owned),
        1 => select(vec![ANSWER_ID_KEY, ANSWER_INPUT_KEY, SEEN_FIELD, BY]).prop_map(str::to_owned),
        1 => ".{0,4}",
    ]
}

fn fields() -> impl Strategy<Value = Vec<(String, Value)>> {
    prop::collection::vec((field(), value()), 0..4)
}

fn subject() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => select(vec!["s", "t"]).prop_map(str::to_owned),
        1 => Just(String::new()),
        1 => Just(POOL_SUBJECT.to_owned()),
        1 => ".{0,6}",
    ]
}

/// A name a caller passes: one of a list the net holds (by index) or any
/// other text, including near misses and names the executor generates.
#[derive(Clone, Debug)]
enum Name {
    Held(usize),
    Other(String),
}

fn other_name() -> impl Strategy<Value = String> {
    prop_oneof![
        select(vec![
            "Nope", "", " ", "t0", "T0 ", " T0", "T", "T00", "p0", "P0 ", "Ghost", "D9", "W9",
        ])
        .prop_map(str::to_owned),
        select(vec!["T0", "W0", "P0"]).prop_flat_map(|n| {
            select(vec![
                "Running",
                "RetryPending",
                "OutcomeUnknown",
                "UndoPending",
                "x",
            ])
            .prop_map(move |r| format!("{n}.{r}"))
        }),
        select(vec![
            "Answer",
            "Asks",
            "Answered",
            "Expired",
            "WorkFailed",
            "ClearFailed"
        ])
        .prop_map(|r| format!("{r}.T0")),
        ".{0,8}",
    ]
}

fn name() -> impl Strategy<Value = Name> {
    prop_oneof![
        3 => any::<usize>().prop_map(Name::Held),
        1 => other_name().prop_map(Name::Other),
    ]
}

impl Name {
    fn of(&self, held: &[String]) -> String {
        match self {
            Name::Held(i) if !held.is_empty() => held[i % held.len()].clone(),
            Name::Held(_) => "Nope".to_owned(),
            Name::Other(s) => s.clone(),
        }
    }
}

#[derive(Clone, Debug)]
enum Subj {
    /// The subject the binding is asked for.
    Own,
    Fixed(String),
}

impl Subj {
    fn of(&self, own: &str) -> String {
        match self {
            Subj::Own => own.to_owned(),
            Subj::Fixed(s) => s.clone(),
        }
    }
}

fn subj() -> impl Strategy<Value = Subj> {
    prop_oneof![3 => Just(Subj::Own), 1 => subject().prop_map(Subj::Fixed)]
}

/// One read a binding makes through the [`Reader`].
#[derive(Clone, Debug)]
enum ReadOp {
    Field(Name, Subj, String),
    Text(Name, Subj, String),
    Present(Name, Subj),
    Free(Name),
    Traced(Box<ReadOp>),
}

impl ReadOp {
    /// A read of a token's fields, which an executor place refuses.
    fn reads_fields(&self) -> bool {
        match self {
            ReadOp::Field(..) | ReadOp::Text(..) => true,
            ReadOp::Traced(r) => r.reads_fields(),
            _ => false,
        }
    }

    fn is_free(&self) -> bool {
        match self {
            ReadOp::Free(_) => true,
            ReadOp::Traced(r) => r.is_free(),
            _ => false,
        }
    }

    fn place(&self) -> &Name {
        match self {
            ReadOp::Field(p, ..)
            | ReadOp::Text(p, ..)
            | ReadOp::Present(p, _)
            | ReadOp::Free(p) => p,
            ReadOp::Traced(r) => r.place(),
        }
    }

    fn run(&self, r: &mut Reader<'_>, s: &Subject, readable: &[String]) {
        match self {
            ReadOp::Field(p, subj, f) => {
                r.field(&p.of(readable), &subj.of(s), f);
            }
            ReadOp::Text(p, subj, f) => {
                r.text(&p.of(readable), &subj.of(s), f);
            }
            ReadOp::Present(p, subj) => {
                r.present(&p.of(readable), &subj.of(s));
            }
            ReadOp::Free(p) => {
                r.free(&p.of(readable));
            }
            ReadOp::Traced(inner) => {
                r.traced(|r| inner.run(r, s, readable));
            }
        }
    }
}

fn read_op() -> impl Strategy<Value = ReadOp> {
    let flat = prop_oneof![
        (name(), subj(), field()).prop_map(|(p, s, f)| ReadOp::Field(p, s, f)),
        (name(), subj(), field()).prop_map(|(p, s, f)| ReadOp::Text(p, s, f)),
        (name(), subj()).prop_map(|(p, s)| ReadOp::Present(p, s)),
        name().prop_map(ReadOp::Free),
    ];
    flat.prop_recursive(1, 2, 1, |inner| {
        inner.prop_map(|r| ReadOp::Traced(Box::new(r)))
    })
}

/// A binding: its reads, then the bound input, or `None` (the guard is
/// false). The result does not depend on the values read, so a test knows
/// the bound fields without running the executor.
#[derive(Clone, Debug)]
struct BindProg {
    reads: Vec<ReadOp>,
    out: Option<Vec<(String, Value)>>,
}

fn bind_prog() -> impl Strategy<Value = BindProg> {
    (
        prop::collection::vec(read_op(), 0..4),
        prop_oneof![1 => Just(None), 5 => fields().prop_map(Some)],
    )
        .prop_map(|(reads, out)| BindProg { reads, out })
}

/// An output of a body or a result; its place is one the net declares
/// (an output naming any other place is a follow-up card).
#[derive(Clone, Debug)]
struct OutGen {
    place: usize,
    subject: Subj,
    fields: Vec<(String, Value)>,
}

fn outs() -> impl Strategy<Value = Vec<OutGen>> {
    prop::collection::vec(
        (any::<usize>(), subj(), fields()).prop_map(|(place, subject, fields)| OutGen {
            place,
            subject,
            fields,
        }),
        0..3,
    )
}

fn outputs(outs: &[OutGen], places: &[String], own: &str, by: Option<&str>) -> Vec<Output> {
    outs.iter()
        .map(|o| {
            let mut content: Content = o.fields.iter().cloned().collect();
            if let Some(by) = by {
                content.insert(BY.to_owned(), Value::String(by.to_owned()));
            }
            Output::new(
                &places[o.place % places.len()],
                &Subject::new(&o.subject.of(own)),
                content,
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
enum BodyProg {
    Outputs(Vec<OutGen>),
    Fails { retryable: bool },
}

fn body_prog() -> impl Strategy<Value = BodyProg> {
    prop_oneof![
        4 => outs().prop_map(BodyProg::Outputs),
        1 => any::<bool>().prop_map(|retryable| BodyProg::Fails { retryable }),
    ]
}

impl BodyProg {
    /// The pure body; it tags every output with the transition's name.
    fn body(&self, by: &str, places: &[String]) -> holon_net::exec::PureBody {
        let body = self.clone();
        let by = by.to_owned();
        let places = places.to_vec();
        Arc::new(move |s: &Subject, _: &Content| match &body {
            BodyProg::Fails { retryable: true } => Err(Cause::retryable("boom")),
            BodyProg::Fails { retryable: false } => Err(Cause::definite("boom")),
            BodyProg::Outputs(_) if places.is_empty() => Ok(vec![]),
            BodyProg::Outputs(outs) => Ok(outputs(outs, &places, s, Some(&by))),
        })
    }
}

#[derive(Clone, Debug)]
struct PlaceGen {
    capacity: PlaceCapacity,
    pool: Option<u32>,
}

/// Capacity and pool as the file holds them; the parser refuses a pool of 0
/// and a pool on a capacity other than fifo, so neither is drawn.
fn place_gen() -> impl Strategy<Value = PlaceGen> {
    prop_oneof![
        Just(PlaceCapacity::Single),
        Just(PlaceCapacity::Fifo),
        Just(PlaceCapacity::Register),
    ]
    .prop_flat_map(|capacity| {
        let pool = if capacity == PlaceCapacity::Fifo {
            prop_oneof![Just(None), (1u32..4).prop_map(Some)].boxed()
        } else {
            Just(None).boxed()
        };
        pool.prop_map(move |pool| PlaceGen { capacity, pool })
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Consumes,
    Reads,
    Inhibits,
    None,
}

fn role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Consumes),
        Just(Role::Reads),
        Just(Role::Inhibits),
        Just(Role::None),
    ]
}

/// The run timer, the abandon timer, and the retry (count bound, backoff).
type TimersGen = (Option<u64>, Option<u64>, Option<(u32, u64)>);

#[derive(Clone, Debug)]
struct TransGen {
    trigger: Trigger,
    priority: i32,
    /// One role per stored place: a place is on at most one input arc of a
    /// transition (a place on two arcs is a sibling card's loader defect).
    roles: Vec<Role>,
    produces: Vec<usize>,
    facts: Vec<usize>,
    on_failure: Option<usize>,
    on_timeout: Option<usize>,
    effect: Option<Receiver>,
    timers: Option<TimersGen>,
    bind: BindProg,
    body: BodyProg,
}

fn trans_gen(places: usize) -> impl Strategy<Value = TransGen> {
    let ms = 1u64..5_000;
    (
        (
            prop_oneof![Just(Trigger::Auto), Just(Trigger::Manual)],
            -2i32..3,
            prop::collection::vec(role(), places),
            prop::collection::vec(0..places, 0..3),
            prop::collection::vec(0..places, 0..2),
            prop::option::of(0..places),
            prop::option::of(0..places),
        ),
        (
            prop::option::of(prop_oneof![
                Just(Receiver::Keyed),
                Just(Receiver::StateProbe),
                Just(Receiver::None),
            ]),
            prop::option::of((
                prop::option::of(ms.clone()),
                prop::option::of(ms.clone()),
                prop::option::of((1u32..4, ms)),
            )),
            bind_prog(),
            body_prog(),
        ),
    )
        .prop_map(
            |(
                (trigger, priority, roles, produces, facts, on_failure, on_timeout),
                (effect, timers, bind, body),
            )| TransGen {
                trigger,
                priority,
                roles,
                produces,
                facts,
                on_failure,
                on_timeout,
                effect,
                timers,
                bind,
                body,
            },
        )
}

/// A lowered ask: a manual HL transition over the stored places.
#[derive(Clone, Debug)]
struct WorkGen {
    roles: Vec<Role>,
    puts: Vec<usize>,
    effect: Option<Receiver>,
    input: Option<TypeHint>,
    irreversible: bool,
    /// The other work this one undoes, by index.
    undoes: Option<usize>,
    bind: BindProg,
    body: BodyProg,
}

fn type_hint() -> impl Strategy<Value = Option<TypeHint>> {
    prop_oneof![
        Just(None),
        Just(Some(TypeHint::Enum {
            type_name: "Pick".to_owned(),
            variants: vec!["A".to_owned(), "B".to_owned()],
        })),
    ]
}

fn work_gen(places: usize) -> impl Strategy<Value = WorkGen> {
    (
        prop::collection::vec(role(), places),
        prop::collection::vec(0..places, 0..2),
        prop::option::of(prop_oneof![
            Just(Receiver::StateProbe),
            Just(Receiver::None)
        ]),
        type_hint(),
        any::<bool>(),
        prop::option::of(0..WORKS.len()),
        bind_prog(),
        body_prog(),
    )
        .prop_map(
            |(roles, puts, effect, input, irreversible, undoes, bind, body)| WorkGen {
                roles,
                puts,
                effect,
                input,
                irreversible,
                undoes,
                bind,
                body,
            },
        )
}

#[derive(Clone, Debug)]
struct NetGen {
    places: Vec<PlaceGen>,
    transitions: Vec<TransGen>,
    /// Derived places `D<i>`: what each reads and returns.
    derived: Vec<BindProg>,
    works: Vec<WorkGen>,
}

fn net_gen() -> impl Strategy<Value = NetGen> {
    prop::collection::vec(place_gen(), 1..=PLACES.len()).prop_flat_map(|places| {
        let n = places.len();
        (
            prop::collection::vec(trans_gen(n), 1..=TRANSITIONS.len()),
            prop::collection::vec(bind_prog(), 0..=DERIVED.len()),
            prop::collection::vec(work_gen(n), 0..=WORKS.len()),
            prop::option::weighted(0.5, pool_read()),
            prop::option::weighted(0.3, retry_input()),
        )
            .prop_map(move |(transitions, derived, works, pool_read, retry)| {
                let mut net = NetGen {
                    places: places.clone(),
                    transitions,
                    derived,
                    works,
                };
                if let Some(read) = pool_read {
                    read.apply(&mut net);
                }
                if let Some(retry) = retry {
                    retry.apply(&mut net);
                }
                net
            })
    })
}

/// A transition whose bind reads a pool place it has no arc on: a change of
/// the pool reaches it only through what its bind read.
#[derive(Clone, Debug)]
struct PoolRead {
    place: prop::sample::Index,
    size: u32,
    transition: prop::sample::Index,
    /// The read of place `Held(place)`.
    read: fn(Name, Subj, String) -> ReadOp,
    subject: Subj,
    field: String,
    drain: Option<Drain>,
}

/// The reader takes `input`, reads nothing but the pool and fails
/// definitely, so its failure mark holds what it read of the pool. The
/// drainer reads `input` and takes from the pool: the pool changes after the
/// mark is set. Both are pure auto transitions with no other arcs.
#[derive(Clone, Debug)]
struct Drain {
    drainer: prop::sample::Index,
    input: prop::sample::Index,
}

fn pool_read() -> impl Strategy<Value = PoolRead> {
    let kinds: Vec<fn(Name, Subj, String) -> ReadOp> = vec![
        ReadOp::Field,
        ReadOp::Text,
        |p, s, _| ReadOp::Present(p, s),
        |p, _, _| ReadOp::Free(p),
    ];
    let index = any::<prop::sample::Index>;
    let drain = (index(), index()).prop_map(|(drainer, input)| Drain { drainer, input });
    (
        (index(), 1u32..4, index()),
        (select(kinds), subj(), field()),
        prop::option::of(drain),
    )
        .prop_map(
            |((place, size, transition), (read, subject, field), drain)| PoolRead {
                place,
                size,
                transition,
                read,
                subject,
                field,
                drain,
            },
        )
}

/// A pure auto transition that takes `consumes`, reads `reads` and has no
/// other arc or read.
fn plain(t: &mut TransGen, consumes: usize, reads: Option<usize>, body: BodyProg) {
    t.trigger = Trigger::Auto;
    t.roles.fill(Role::None);
    t.roles[consumes] = Role::Consumes;
    if let Some(r) = reads {
        t.roles[r] = Role::Reads;
    }
    t.effect = None;
    t.on_failure = None;
    t.on_timeout = None;
    t.timers = None;
    t.bind = BindProg {
        reads: vec![],
        out: Some(vec![]),
    };
    t.body = body;
}

impl PoolRead {
    fn apply(self, net: &mut NetGen) {
        let p = self.place.index(net.places.len());
        net.places[p] = PlaceGen {
            capacity: PlaceCapacity::Fifo,
            pool: Some(self.size),
        };
        let t = self.transition.index(net.transitions.len());
        if let Some(drain) = self.drain {
            let q = drain.input.index(net.places.len());
            let d = drain.drainer.index(net.transitions.len());
            if q != p && d != t {
                plain(
                    &mut net.transitions[d],
                    p,
                    Some(q),
                    BodyProg::Outputs(vec![]),
                );
                plain(
                    &mut net.transitions[t],
                    q,
                    None,
                    BodyProg::Fails { retryable: false },
                );
            }
        }
        let t = &mut net.transitions[t];
        t.roles[p] = Role::None;
        // A stored place's index is its position in every readable list.
        t.bind
            .reads
            .push((self.read)(Name::Held(p), self.subject, self.field));
    }
}

/// An auto transition takes place `input`, reads nothing and runs a Keyed
/// effect, which retries a retryable failure after a backoff. A second one
/// does the same, or (`false`) consumes the returned input with no effect.
/// [`calls`] fails each Keyed first try and writes `input` again while the
/// retries wait.
#[derive(Clone, Debug)]
struct RetryInput {
    input: prop::sample::Index,
    first: prop::sample::Index,
    second: Option<(prop::sample::Index, bool)>,
}

fn retry_input() -> impl Strategy<Value = RetryInput> {
    let index = any::<prop::sample::Index>;
    (index(), index(), prop::option::of((index(), any::<bool>()))).prop_map(
        |(input, first, second)| RetryInput {
            input,
            first,
            second,
        },
    )
}

impl RetryInput {
    fn apply(self, net: &mut NetGen) {
        let q = self.input.index(net.places.len());
        net.places[q].pool = None;
        let n = net.transitions.len();
        for (t, keyed) in [Some((self.first, true)), self.second]
            .into_iter()
            .flatten()
        {
            let t = &mut net.transitions[t.index(n)];
            plain(t, q, None, BodyProg::Outputs(vec![]));
            t.effect = keyed.then_some(Receiver::Keyed);
        }
    }
}

/// How [`calls`] writes a retry's input again: the same content, or new
/// content in a field no bind of the shape reads.
#[derive(Clone, Copy, Debug)]
enum Rewrite {
    Equal,
    NewUnread,
}

fn duration(ms: u64) -> DurationSpec {
    serde_yaml::from_str(&format!("{ms}ms")).expect("a whole number of ms is a duration")
}

fn shell_effect() -> EffectSpec {
    EffectSpec::Shell(ShellEffect {
        command: CommandSpec {
            argv: vec!["true".to_owned()],
            working_dir: ".".to_owned(),
            env_allow: vec![],
            timeout: duration(1_000),
        },
        log_path: "logs/{attempt}.log".to_owned(),
        exit_codes: ExitCodeMap {
            done: vec![0],
            failed: vec![1],
            other: Outcome::OutcomeUnknown,
        },
    })
}

/// A work's binding that returns the answer id or the seen field panics in
/// the lowering; a follow-up card holds it.
fn without_answer_fields(bind: &BindProg) -> BindProg {
    let mut bind = bind.clone();
    if let Some(out) = &mut bind.out {
        out.retain(|(k, _)| k != ANSWER_ID_KEY && k != SEEN_FIELD);
    }
    bind
}

impl NetGen {
    /// The input of the [`RetryInput`] shape and how many transitions retry
    /// on it.
    fn retry_input(&self) -> Option<(usize, usize)> {
        let mut found: Option<(usize, usize)> = None;
        for t in &self.transitions {
            let consumes: Vec<usize> = (0..t.roles.len())
                .filter(|&i| t.roles[i] == Role::Consumes)
                .collect();
            if t.trigger == Trigger::Auto
                && t.effect == Some(Receiver::Keyed)
                && t.bind.reads.is_empty()
                && t.roles.iter().filter(|r| **r != Role::None).count() == 1
                && let [q] = consumes[..]
                && self.places[q].pool.is_none()
                && found.is_none_or(|(p, _)| p == q)
            {
                found = Some((q, found.map_or(1, |(_, k)| k + 1)));
            }
        }
        found
    }

    /// The input of a drained pool reader: a pure auto transition that fails
    /// definitely, reads only a pool it has no arc on and takes one place.
    fn drained_input(&self) -> Option<usize> {
        self.transitions.iter().find_map(|t| {
            let [read] = t.bind.reads.as_slice() else {
                return None;
            };
            let Name::Held(p) = *read.place() else {
                return None;
            };
            let consumes: Vec<usize> = (0..t.roles.len())
                .filter(|&i| t.roles[i] == Role::Consumes)
                .collect();
            (t.trigger == Trigger::Auto
                && t.effect.is_none()
                && matches!(t.body, BodyProg::Fails { retryable: false })
                && self.places.get(p).is_some_and(|g| g.pool.is_some())
                && t.roles[p] == Role::None
                && consumes.len() == 1)
                .then(|| consumes[0])
        })
    }

    fn stored(&self) -> Vec<String> {
        (0..self.places.len())
            .map(|i| PLACES[i].to_owned())
            .collect()
    }

    fn derived_names(&self) -> Vec<String> {
        (0..self.derived.len())
            .map(|i| DERIVED[i].to_owned())
            .collect()
    }

    fn work_names(&self) -> Vec<String> {
        (0..self.works.len()).map(|i| WORKS[i].to_owned()).collect()
    }

    /// The stored places the lowering adds for each work.
    fn work_places(&self) -> Vec<String> {
        self.work_names()
            .iter()
            .flat_map(|w| {
                [
                    Generated::Answered,
                    Generated::Expired,
                    Generated::WorkFailed,
                ]
                .map(|g| g.name(w))
            })
            .collect()
    }

    fn asks_places(&self) -> Vec<String> {
        self.work_names()
            .iter()
            .map(|w| Generated::Asks.name(w))
            .collect()
    }

    /// The places a work's binding reads by name.
    fn readable(&self) -> Vec<String> {
        [self.stored(), self.derived_names(), self.work_places()].concat()
    }

    /// The places the binding of a file transition reads by name: also the
    /// `Asks` rows, whose derive runs a work's binding. A work never reads
    /// one, since a work that reads its own `Asks` recurses without end.
    fn transition_readable(&self) -> Vec<String> {
        [self.readable(), self.asks_places()].concat()
    }

    /// The places derived place `d` reads: stored ones and the derived ones
    /// before it. A cycle of derived places recurses without end; the
    /// ignored `a_derived_place_that_reads_itself_does_not_overflow` holds it.
    fn derived_readable(&self, d: usize) -> Vec<String> {
        [
            self.stored(),
            self.work_places(),
            self.derived_names()[..d].to_vec(),
        ]
        .concat()
    }

    /// The places a caller writes and retracts by name. `Answered.<W>` is
    /// left out: an outside token there without the answer fields is a
    /// follow-up card.
    fn writable(&self) -> Vec<String> {
        let answered: Vec<String> = self
            .work_names()
            .iter()
            .map(|w| Generated::Answered.name(w))
            .collect();
        let work_places = self
            .work_places()
            .into_iter()
            .filter(|p| !answered.contains(p))
            .collect();
        [
            self.stored(),
            self.derived_names(),
            self.asks_places(),
            work_places,
        ]
        .concat()
    }

    fn file_transitions(&self) -> Vec<String> {
        (0..self.transitions.len())
            .map(|i| TRANSITIONS[i].to_owned())
            .collect()
    }

    /// Every transition the net holds, the lowering's included.
    fn transition_names(&self) -> Vec<String> {
        let mut names = self.file_transitions();
        for w in self.work_names() {
            for g in [
                Generated::Answer,
                Generated::ExpireAnswer,
                Generated::ReleaseFailed,
                Generated::ClearFailed,
            ] {
                names.push(g.name(&w));
            }
            names.push(w);
        }
        names
    }

    /// The net file. A transition gets at least one input arc and a keyed
    /// receiver a count bound, since the parser refuses either missing.
    fn file(&self) -> NetFile {
        let names = self.stored();
        let places = names
            .iter()
            .zip(&self.places)
            .map(|(n, p)| {
                (
                    n.clone(),
                    PlaceSpec {
                        capacity: p.capacity,
                        pool: p.pool,
                    },
                )
            })
            .collect();
        let mut effects = IndexMap::new();
        let transitions = self
            .transitions
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let [consumes, reads, inhibits] = self.arcs(i);
                let mut timers = t.timers.map(|(run, abandon, retry)| TimerSpec {
                    run: run.map(duration),
                    abandon: abandon.map(duration),
                    count_bound: retry.map(|r| r.0),
                    backoff: retry.map(|r| duration(r.1)),
                });
                if timers.as_ref().is_some_and(|t| {
                    t.run.is_none() && t.abandon.is_none() && t.count_bound.is_none()
                }) {
                    timers = None;
                }
                if t.effect == Some(Receiver::Keyed)
                    && timers.as_ref().and_then(|t| t.count_bound).is_none()
                {
                    let mut keyed = timers.unwrap_or(TimerSpec {
                        run: None,
                        abandon: None,
                        count_bound: None,
                        backoff: None,
                    });
                    keyed.count_bound = Some(2);
                    keyed.backoff = Some(duration(10));
                    timers = Some(keyed);
                }
                let effect = t.effect.map(|_| {
                    let name = format!("E{i}");
                    effects.insert(name.clone(), shell_effect());
                    name
                });
                (
                    TRANSITIONS[i].to_owned(),
                    TransitionSpec {
                        trigger: t.trigger,
                        priority: t.priority,
                        consumes,
                        reads,
                        inhibits,
                        produces: t.produces.iter().map(|p| names[*p].clone()).collect(),
                        facts: t.facts.iter().map(|p| names[*p].clone()).collect(),
                        // A failure routed into a pool panics in the code; a
                        // follow-up card holds it.
                        on_failure: t
                            .on_failure
                            .filter(|p| self.places[*p].pool.is_none())
                            .map(|p| names[p].clone()),
                        on_timeout: t
                            .on_timeout
                            .filter(|p| self.places[*p].pool.is_none())
                            .map(|p| names[p].clone()),
                        effect,
                        receiver: t.effect,
                        timers,
                    },
                )
            })
            .collect();
        NetFile {
            name: "gen".to_owned(),
            places,
            transitions,
            effects,
            probes: IndexMap::new(),
        }
    }

    /// The consume, read and inhibit arcs of file transition `i`.
    fn arcs(&self, i: usize) -> [Vec<String>; 3] {
        let names = self.stored();
        let t = &self.transitions[i];
        let on = |want: Role| -> Vec<String> {
            t.roles
                .iter()
                .enumerate()
                .filter(|(_, r)| **r == want)
                .map(|(p, _)| names[p].clone())
                .collect()
        };
        let mut consumes = on(Role::Consumes);
        let reads = on(Role::Reads);
        let inhibits = on(Role::Inhibits);
        if consumes.is_empty() && reads.is_empty() && inhibits.is_empty() {
            consumes.push(names[i % names.len()].clone());
        }
        [consumes, reads, inhibits]
    }

    fn bind_fn(&self, prog: &BindProg, readable: Vec<String>) -> holon_net::exec::Read {
        let prog = prog.clone();
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            for op in &prog.reads {
                op.run(r, s, &readable);
            }
            prog.out.as_ref().map(|f| f.iter().cloned().collect())
        })
    }

    fn logic(&self, transition: &str) -> Logic {
        let i = TRANSITIONS
            .iter()
            .position(|t| *t == transition)
            .unwrap_or_else(|| panic!("the loader asked for the logic of '{transition}'"));
        let t = &self.transitions[i];
        let bind = self.bind_fn(&t.bind, self.transition_readable());
        if t.effect.is_some() {
            return Logic::effect(bind);
        }
        Logic::pure(bind, t.body.body(transition, &self.output_places()))
    }

    /// The places a body puts into. A put without compare-and-set into a
    /// held single place, and a put into a pool, panic in the code; a
    /// follow-up card holds both.
    fn output_places(&self) -> Vec<String> {
        self.stored()
            .into_iter()
            .zip(&self.places)
            .filter(|(_, g)| g.capacity != PlaceCapacity::Single && g.pool.is_none())
            .map(|(p, _)| p)
            .collect()
    }

    fn descriptor(&self, i: usize) -> TransitionDescriptor {
        let w = &self.works[i];
        let names = self.stored();
        let mut arcs: Vec<HlArc> = w
            .roles
            .iter()
            .enumerate()
            .filter_map(|(p, r)| {
                let kind = match r {
                    Role::Consumes => ArcKind::Take,
                    Role::Reads => ArcKind::Read,
                    Role::Inhibits => ArcKind::Inhibit,
                    Role::None => return None,
                };
                Some(HlArc {
                    kind,
                    place: PlaceName(names[p].clone()),
                })
            })
            .collect();
        arcs.extend(w.puts.iter().map(|p| HlArc {
            kind: ArcKind::Put,
            place: PlaceName(names[*p].clone()),
        }));
        TransitionDescriptor {
            name: TransitionName(WORKS[i].to_owned()),
            arcs,
            body: BodyDecl::Native {
                symbol: NativeRef(format!("work{i}")),
                effectful: w.effect.is_some(),
            },
            irreversible: w.irreversible,
            timers: vec![],
            guard: None,
            ask: Some(AskDecl {
                input: w.input.clone(),
                preview: None,
                persist: false,
            }),
            undoes: w
                .undoes
                .filter(|u| *u != i && *u < self.works.len())
                .map(|u| TransitionName(WORKS[u].to_owned())),
        }
    }

    fn parts(&self, i: usize) -> NativeParts {
        let w = &self.works[i];
        NativeParts {
            bind: self.bind_fn(&without_answer_fields(&w.bind), self.readable()),
            body: match w.effect {
                Some(r) => NativeBody::Effectful(match r {
                    Receiver::Keyed => holon_net::exec::Receiver::Keyed,
                    Receiver::StateProbe => holon_net::exec::Receiver::StateProbe,
                    Receiver::None => holon_net::exec::Receiver::None,
                }),
                None => NativeBody::Pure(w.body.body(WORKS[i], &self.output_places())),
            },
            preview: None,
        }
    }

    /// The net: the file, its derived places, then each lowered work.
    /// `Err` is a refusal of the loader or the lowering.
    fn net(&self) -> Result<NetDef, String> {
        let yaml = self.file().to_yaml();
        let mut net = net_def("nets/gen.yaml", &yaml, |t| self.logic(t))
            .map_err(|e| format!("net_def: {e}"))?;
        for (i, d) in self.derived.iter().enumerate() {
            net = net.derived(DERIVED[i], self.bind_fn(d, self.derived_readable(i)));
        }
        for i in 0..self.works.len() {
            net = lower(net, &self.descriptor(i), Some(self.parts(i)))
                .map_err(|e| format!("lower: {e}"))?;
        }
        Ok(net)
    }

    fn executor(&self) -> Result<Checked, String> {
        Executor::new(self.net()?)
            .map(Checked::from)
            .map_err(|e| format!("Executor::new: {e}"))
    }

    /// Whether a read of `place` reaches a place the net does not declare.
    fn ghost(&self, place: &str) -> bool {
        if let Some(d) = DERIVED.iter().position(|d| *d == place)
            && d < self.derived.len()
        {
            let readable = self.derived_readable(d);
            return self.derived[d]
                .reads
                .iter()
                .any(|r| self.read_is_ghost(r, &readable));
        }
        // Whether an `Asks` row runs its work's binding depends on the work's
        // marking, which the model does not keep, so such a read is no
        // certain ghost.
        !self.declares(place)
    }

    fn read_is_ghost(&self, r: &ReadOp, readable: &[String]) -> bool {
        let p = r.place().of(readable);
        // `free` counts tokens and never runs a derived place.
        if r.reads_fields() && self.is_executor_place(&p) {
            true
        } else if r.is_free() {
            !self.declares(&p)
        } else {
            self.ghost(&p)
        }
    }

    fn is_executor_place(&self, place: &str) -> bool {
        place == OUTCOME_UNKNOWN
            || self.transition_names().iter().any(|t| {
                [
                    Place::running(t),
                    Place::retry_pending(t),
                    Place::outcome_unknown(t),
                    Place::undo_pending(t),
                ]
                .iter()
                .any(|p| p.as_str() == place)
            })
    }

    /// Every place the net holds: the file's, the lowering's and the
    /// executor's own.
    fn declares(&self, place: &str) -> bool {
        self.readable()
            .iter()
            .chain(&self.asks_places())
            .any(|p| p == place)
            || all_places(self).contains(place)
    }

    fn bind_of(&self, transition: &str) -> Option<&BindProg> {
        if let Some(i) = TRANSITIONS.iter().position(|t| *t == transition) {
            return self.transitions.get(i).map(|t| &t.bind);
        }
        let i = WORKS.iter().position(|t| *t == transition)?;
        self.works.get(i).map(|w| &w.bind)
    }

    /// Whether the binding of `transition` reads, directly or through a
    /// derived place, a place the net does not declare.
    fn reads_undeclared(&self, transition: &str) -> bool {
        let readable = if TRANSITIONS.contains(&transition) {
            self.transition_readable()
        } else {
            self.readable()
        };
        self.bind_of(transition)
            .is_some_and(|b| b.reads.iter().any(|r| self.read_is_ghost(r, &readable)))
    }

    /// Whether `transition` is a work whose binding produces the field the
    /// answer of its ask fills.
    fn binds_answer_input(&self, transition: &str) -> bool {
        WORKS
            .iter()
            .position(|t| *t == transition)
            .is_some_and(|i| {
                self.works.get(i).is_some_and(|w| {
                    w.bind
                        .out
                        .as_ref()
                        .is_some_and(|f| f.iter().any(|(k, _)| k == ANSWER_INPUT_KEY))
                })
            })
    }

    /// The transitions whose firing a caller must never see.
    fn ghost_transitions(&self) -> BTreeSet<String> {
        [self.file_transitions(), self.work_names()]
            .concat()
            .into_iter()
            .filter(|t| self.reads_undeclared(t) || self.binds_answer_input(t))
            .collect()
    }
}

// ------------------------------------------------------------- observing

/// The latest `now` a test passes: production's `now` counts from the UNIX
/// epoch, and a due time past i64 nanoseconds (year 2262) is a follow-up card.
const MAX_NOW_NS: u64 = 1 << 62;

fn no_panic<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, TestCaseError> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let msg = p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        TestCaseError::fail(format!("{what} panicked: {msg}"))
    })
}

/// Every place a snapshot lists: the net's own and the executor's.
fn all_places(net: &NetGen) -> BTreeSet<String> {
    let mut places: BTreeSet<String> = [net.stored(), net.work_places()]
        .concat()
        .into_iter()
        .collect();
    for t in net.transition_names() {
        places.insert(Place::running(&t).to_string());
        places.insert(Place::retry_pending(&t).to_string());
        places.insert(Place::outcome_unknown(&t).to_string());
        places.insert(Place::undo_pending(&t).to_string());
    }
    places.insert(OUTCOME_UNKNOWN.to_owned());
    places
}

/// Everything a caller can read of the executor except its clock, as text
/// so a NaN field compares equal to itself.
fn snapshot(exec: &Executor, net: &NetGen) -> String {
    let mut out = String::new();
    for p in all_places(net) {
        for s in exec.tokens().subjects(&p) {
            out += &format!(
                "{p}/{s}: {:?}\n",
                exec.tokens()
                    .row(&p, &s)
                    .map(|t| &t.payload)
                    .collect::<Vec<_>>()
            );
        }
    }
    out += &format!("marks {:?}\n", exec.marks().collect::<Vec<_>>());
    out += &format!("retrying {:?}\n", exec.retrying().collect::<Vec<_>>());
    out += &format!("in_flight {:?}\n", exec.in_flight().collect::<Vec<_>>());
    out += &format!("occurrences {:?}\n", exec.occurrences().collect::<Vec<_>>());
    out += &format!("conditions {:?}\n", exec.conditions());
    out
}

/// The transitions whose body put a token anywhere a caller can see.
fn producers(exec: &Executor, net: &NetGen) -> BTreeSet<String> {
    let mut by = BTreeSet::new();
    for p in all_places(net) {
        for s in exec.tokens().subjects(&p) {
            for t in exec.tokens().row(&p, &s) {
                if let Payload::Record(c) = &t.payload
                    && let Some(Value::String(t)) = c.get(BY)
                {
                    by.insert(t.clone());
                }
            }
        }
    }
    by
}

fn names_error<T: Debug, E: Debug>(r: &Result<T, E>, form: &str) -> bool {
    matches!(r, Err(e) if format!("{e:?}").contains(form))
}

// ------------------------------------------------------------ the machine

/// A `now` relative to the executor's clock.
#[derive(Clone, Debug)]
enum Dt {
    Back(u64),
    Same,
    Forward(u64),
}

fn dt() -> impl Strategy<Value = Dt> {
    prop_oneof![
        1 => (1u64..=u64::MAX).prop_map(Dt::Back),
        1 => (1u64..5_000_000_000).prop_map(Dt::Back),
        2 => Just(Dt::Same),
        3 => (1u64..10_000_000_000).prop_map(Dt::Forward),
    ]
}

/// No `Back` from a clock at zero: a `now` before zero does not exist.
fn now(clock: Duration, dt: &Dt) -> Duration {
    match dt {
        Dt::Back(d) => clock.saturating_sub(Duration::from_nanos(*d)),
        Dt::Same => clock,
        Dt::Forward(d) => clock + Duration::from_nanos(*d),
    }
}

#[derive(Clone, Debug)]
enum OccGen {
    Fresh,
    Reuse(usize),
    Minted(Name, u64),
    Work(Name, String),
}

fn occ() -> impl Strategy<Value = OccGen> {
    prop_oneof![
        3 => Just(OccGen::Fresh),
        1 => any::<usize>().prop_map(OccGen::Reuse),
        1 => (name(), 0u64..4).prop_map(|(t, n)| OccGen::Minted(t, n)),
        1 => (name(), ".{0,3}").prop_map(|(t, a)| OccGen::Work(t, a)),
    ]
}

#[derive(Clone, Debug)]
enum Pick {
    Known(usize),
    Bogus(u64),
}

fn pick() -> impl Strategy<Value = Pick> {
    prop_oneof![
        4 => any::<usize>().prop_map(Pick::Known),
        1 => any::<u64>().prop_map(Pick::Bogus),
    ]
}

#[derive(Clone, Debug)]
enum ResultGen {
    Done(Vec<OutGen>),
    Failed(bool),
    Unknown,
}

#[derive(Clone, Debug)]
enum ResolutionGen {
    Verify(Vec<OutGen>),
    Retry,
    Drop,
}

/// The hash an answer says it saw: one an `Asks` row offers, or any other.
#[derive(Clone, Debug)]
enum SeenGen {
    Offered(usize),
    Bogus([u8; 32]),
}

#[derive(Clone, Debug)]
enum Op {
    Write {
        place: Name,
        subject: String,
        content: Vec<(String, Value)>,
        dt: Dt,
    },
    Retract {
        place: Name,
        subject: String,
        dt: Dt,
    },
    Fire {
        transition: Name,
        subject: String,
        occurrence: OccGen,
        value: Vec<(String, Value)>,
        dt: Dt,
    },
    Step {
        dt: Dt,
    },
    WhyNot {
        transition: Name,
        subject: String,
    },
    Complete {
        attempt: Pick,
        result: ResultGen,
        dt: Dt,
    },
    Resolve {
        occurrence: Pick,
        resolution: ResolutionGen,
        dt: Dt,
    },
    /// An answer never goes before the clock: its error type has no clock
    /// form yet, a follow-up card.
    Answer {
        work: Name,
        subject: String,
        seen: SeenGen,
        answer: OccGen,
        value: Option<Value>,
        forward: u64,
    },
    Undo {
        answer: Pick,
        dt: Dt,
    },
    Asks,
    Derive {
        place: Name,
        subject: String,
    },
    DeriveAll {
        place: Name,
    },
}

fn op() -> impl Strategy<Value = Op> {
    let result = prop_oneof![
        outs().prop_map(ResultGen::Done),
        any::<bool>().prop_map(ResultGen::Failed),
        Just(ResultGen::Unknown),
    ];
    let resolution = prop_oneof![
        outs().prop_map(ResolutionGen::Verify),
        Just(ResolutionGen::Retry),
        Just(ResolutionGen::Drop),
    ];
    let seen = prop_oneof![
        4 => any::<usize>().prop_map(SeenGen::Offered),
        1 => any::<[u8; 32]>().prop_map(SeenGen::Bogus),
    ];
    prop_oneof![
        3 => (name(), subject(), fields(), dt()).prop_map(|(place, subject, content, dt)| {
            Op::Write { place, subject, content, dt }
        }),
        1 => (name(), subject(), dt())
            .prop_map(|(place, subject, dt)| Op::Retract { place, subject, dt }),
        3 => (name(), subject(), occ(), fields(), dt()).prop_map(
            |(transition, subject, occurrence, value, dt)| Op::Fire {
                transition,
                subject,
                occurrence,
                value,
                dt
            }
        ),
        2 => dt().prop_map(|dt| Op::Step { dt }),
        2 => (name(), subject())
            .prop_map(|(transition, subject)| Op::WhyNot { transition, subject }),
        2 => (pick(), result, dt())
            .prop_map(|(attempt, result, dt)| Op::Complete { attempt, result, dt }),
        1 => (pick(), resolution, dt()).prop_map(|(occurrence, resolution, dt)| Op::Resolve {
            occurrence,
            resolution,
            dt
        }),
        2 => (
            name(),
            subject(),
            seen,
            occ(),
            prop::option::of(value()),
            0u64..2_000_000_000
        )
            .prop_map(|(work, subject, seen, answer, value, forward)| Op::Answer {
                work,
                subject,
                seen,
                answer,
                value,
                forward
            }),
        1 => (pick(), dt()).prop_map(|(answer, dt)| Op::Undo { answer, dt }),
        1 => Just(Op::Asks),
        1 => (name(), subject()).prop_map(|(place, subject)| Op::Derive { place, subject }),
        1 => name().prop_map(|place| Op::DeriveAll { place }),
    ]
}

/// What the machine remembers between calls: the clock, the effect requests
/// handed out, and the occurrences it saw.
struct Machine {
    net: NetGen,
    exec: Checked,
    /// The largest `now` of every call not refused for the clock.
    clock: Duration,
    requests: Vec<EffectRequest>,
    /// The transition and attempt number of each request, in hand-out order;
    /// `None` for an attempt outside a run.
    attempts: Vec<(Arc<str>, Option<u32>)>,
    occurrences: Vec<OccurrenceId>,
    fresh: u64,
}

impl Machine {
    fn new(net: NetGen, exec: Checked) -> Self {
        Machine {
            net,
            exec,
            clock: Duration::ZERO,
            requests: vec![],
            attempts: vec![],
            occurrences: vec![],
            fresh: 0,
        }
    }

    fn occurrence(&mut self, g: &OccGen) -> OccurrenceId {
        let transitions = self.net.transition_names();
        match g {
            OccGen::Fresh => {
                self.fresh += 1;
                OccurrenceId::caller(&format!("o{}", self.fresh))
            }
            OccGen::Reuse(i) if !self.occurrences.is_empty() => {
                self.occurrences[i % self.occurrences.len()].clone()
            }
            OccGen::Reuse(_) => OccurrenceId::caller("o0"),
            OccGen::Minted(t, n) => OccurrenceId::Minted {
                transition: Arc::from(t.of(&transitions)),
                n: *n,
            },
            OccGen::Work(t, a) => OccurrenceId::Work {
                transition: Arc::from(t.of(&transitions)),
                answer: Arc::from(a.as_str()),
            },
        }
    }

    fn picked_occurrence(&self, p: &Pick) -> OccurrenceId {
        match p {
            Pick::Known(i) if !self.occurrences.is_empty() => {
                self.occurrences[i % self.occurrences.len()].clone()
            }
            Pick::Known(_) => OccurrenceId::caller("o0"),
            Pick::Bogus(n) => OccurrenceId::caller(&format!("bogus{n}")),
        }
    }

    fn refresh(&mut self) {
        let occ: Vec<_> = self.exec.occurrences().map(|(o, _)| o.clone()).collect();
        for o in occ {
            if !self.occurrences.contains(&o) {
                self.occurrences.push(o);
            }
        }
    }

    fn take_requests(&mut self, requests: Option<&Vec<EffectRequest>>) {
        for r in requests.into_iter().flatten() {
            let n = self
                .exec
                .attempt_numbers()
                .find_map(|(id, n)| (id == r.attempt).then_some(n));
            self.attempts.push((r.transition.clone(), n));
            self.requests.push(r.clone());
        }
    }

    /// A call before the clock is refused for the clock and changes
    /// nothing; any other call is not refused for the clock, and its `now`
    /// becomes the clock. Returns whether the call went before the clock.
    fn check_clock<T: Debug, E: Debug>(
        &mut self,
        what: &str,
        now: Duration,
        before: &str,
        r: &Result<T, E>,
    ) -> Result<bool, TestCaseError> {
        if now < self.clock {
            prop_assert!(
                names_error(r, "ClockWentBack"),
                "{what} at {now:?}, before the clock {:?}, was not refused for the clock: {r:?}",
                self.clock
            );
            let after = snapshot(&self.exec, &self.net);
            prop_assert_eq!(
                before,
                after.as_str(),
                "{} before the clock changed the executor",
                what
            );
            return Ok(true);
        }
        prop_assert!(
            !names_error(r, "ClockWentBack"),
            "{what} at {now:?}, not before the clock {:?}, was refused for the clock: {r:?}",
            self.clock
        );
        self.clock = now;
        Ok(false)
    }

    /// A refused call that names `form` changes nothing.
    fn check_refused<T: Debug, E: Debug>(
        &self,
        what: &str,
        form: &str,
        before: &str,
        r: &Result<T, E>,
    ) -> Result<(), TestCaseError> {
        prop_assert!(
            names_error(r, form),
            "{what} was not refused as {form}: {r:?}"
        );
        let after = snapshot(&self.exec, &self.net);
        prop_assert_eq!(
            before,
            after.as_str(),
            "the refused {} changed the executor",
            what
        );
        Ok(())
    }

    /// A write or retract of a place the net does not declare, or of a
    /// derived one, is refused and changes nothing.
    fn check_place_refusal<T: Debug, E: Debug>(
        &self,
        what: &str,
        place: &str,
        before: &str,
        r: &Result<T, E>,
    ) -> Result<(), TestCaseError> {
        let derived = [self.net.derived_names(), self.net.asks_places()].concat();
        if derived.iter().any(|d| d == place) {
            self.check_refused(
                &format!("{what} of the derived {place:?}"),
                "DerivedPlace",
                before,
                r,
            )
        } else if !self.net.declares(place) {
            self.check_refused(
                &format!("{what} of the undeclared {place:?}"),
                "UndeclaredPlace",
                before,
                r,
            )
        } else {
            Ok(())
        }
    }

    /// A write or retract of the executor's reserved places panics in the
    /// code for some of them; a follow-up card holds them.
    fn reserved(&self, place: &str) -> bool {
        all_places(&self.net).contains(place)
            && !self
                .net
                .stored()
                .iter()
                .chain(&self.net.work_places())
                .any(|p| p == place)
    }

    /// A write into a pool place panics in the code; a follow-up card
    /// holds it, so the machine does not draw it.
    fn is_pool(&self, place: &str) -> bool {
        self.net
            .stored()
            .iter()
            .position(|p| p == place)
            .is_some_and(|k| self.net.places[k].pool.is_some())
    }

    /// The token a stored single-token row holds, with the row's version.
    fn held(&self, place: &str, subject: &str) -> Option<(Token, u64)> {
        let k = self.net.stored().iter().position(|p| p == place)?;
        if self.net.places[k].capacity == PlaceCapacity::Fifo {
            return None;
        }
        let tokens = self.exec.tokens();
        let token = tokens.token(place, subject)?.clone();
        Some((token, tokens.version(place, subject)))
    }

    fn apply(&mut self, op: &Op) -> Result<(), TestCaseError> {
        let stored = self.net.stored();
        let writable = self.net.writable();
        let transitions = self.net.transition_names();
        let before = snapshot(&self.exec, &self.net);
        match op {
            Op::Write {
                place,
                subject,
                content,
                dt,
            } => {
                let place = place.of(&writable);
                if self.is_pool(&place) || self.reserved(&place) {
                    return Ok(());
                }
                let now = now(self.clock, dt);
                let content: Content = content.iter().cloned().collect();
                let held = self.held(&place, subject);
                let exec = &mut self.exec;
                let r = no_panic(&format!("write({place:?}, {subject:?}, {now:?})"), || {
                    exec.write(&place, subject, content.clone(), now)
                })?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                if !self.check_clock("write", now, &before, &r)? {
                    self.check_place_refusal("write", &place, &before, &r)?;
                }
                an_equal_rewrite_keeps_the_token(held, &content, self.held(&place, subject))?;
            }
            Op::Retract { place, subject, dt } => {
                let place = place.of(&writable);
                if self.reserved(&place) {
                    return Ok(());
                }
                let declared = self.net.work_places().contains(&place) || stored.contains(&place);
                // A pool's tokens have no subject: a retract takes any of them.
                let held = if self.is_pool(&place) {
                    self.exec.tokens().free(&place) > 0
                } else {
                    self.exec
                        .tokens()
                        .subjects(&place)
                        .iter()
                        .any(|t| t.as_str() == subject.as_str())
                };
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(&format!("retract({place:?}, {subject:?}, {now:?})"), || {
                    exec.retract(&place, subject, now)
                })?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                if !self.check_clock("retract", now, &before, &r)? {
                    self.check_place_refusal("retract", &place, &before, &r)?;
                    if declared && !held {
                        self.check_refused(
                            &format!("retract of {place:?}/{subject:?}, which holds no token"),
                            "NothingToRetract",
                            &before,
                            &r,
                        )?;
                    }
                }
            }
            Op::Fire {
                transition,
                subject,
                occurrence,
                value,
                dt,
            } => {
                let transition = transition.of(&transitions);
                let now = now(self.clock, dt);
                let occurrence = self.occurrence(occurrence);
                let value_fields: BTreeSet<String> = value.iter().map(|(k, _)| k.clone()).collect();
                let value: Content = value.iter().cloned().collect();
                let answer_ok = matches!(
                    &occurrence,
                    OccurrenceId::Caller(id)
                        if value.get(ANSWER_ID_KEY) == Some(&Value::String(id.to_string()))
                );
                let exec = &mut self.exec;
                let r = no_panic(
                    &format!("fire({transition:?}, {subject:?}, {occurrence}, {value:?}, {now:?})"),
                    || exec.fire(&transition, subject, occurrence.clone(), value, now),
                )?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                if self.check_clock("fire", now, &before, &r)? {
                } else if !transitions.contains(&transition) {
                    self.check_refused(
                        &format!("fire of the unknown {transition:?}"),
                        "UnknownTransition",
                        &before,
                        &r,
                    )?;
                } else if self.net.file_transitions().contains(&transition)
                    && let Some(bound) = self.net.bind_of(&transition).and_then(|b| b.out.as_ref())
                {
                    let bound: BTreeSet<String> = bound.iter().map(|(k, _)| k.clone()).collect();
                    if !bound.is_disjoint(&value_fields) {
                        prop_assert!(
                            r.is_err(),
                            "fire of {transition:?} with value fields {value_fields:?} that repeat bound fields {bound:?} was accepted"
                        );
                        let after = snapshot(&self.exec, &self.net);
                        prop_assert_eq!(&before, &after, "the refused fire changed the executor");
                    }
                } else if transition.starts_with("Answer.") && !answer_ok {
                    prop_assert!(
                        r.is_err(),
                        "fire of {transition:?} with an occurrence that is not its answer id was accepted"
                    );
                }
            }
            Op::Step { dt } => {
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(&format!("step({now:?})"), || exec.step(now))?;
                self.take_requests(r.as_ref().ok());
                self.check_clock("step", now, &before, &r)?;
            }
            Op::WhyNot {
                transition,
                subject,
            } => {
                let transition = transition.of(&transitions);
                let exec = &self.exec;
                let answer = no_panic(&format!("why_not({transition:?}, {subject:?})"), || {
                    forms::of(exec.why_not(&transition, subject))
                })?;
                check_why_not(&self.net, &transition, &answer)?;
                let after = snapshot(&self.exec, &self.net);
                prop_assert_eq!(&before, &after, "why_not changed the executor");
            }
            Op::Complete {
                attempt,
                result,
                dt,
            } => {
                let attempt = match attempt {
                    Pick::Known(i) if !self.requests.is_empty() => {
                        self.requests[i % self.requests.len()].attempt
                    }
                    Pick::Known(_) => AttemptId(0),
                    Pick::Bogus(n) => AttemptId(*n),
                };
                let result = match result {
                    // An effect result puts like a body: into Fifo non-pool
                    // places only.
                    ResultGen::Done(outs) => {
                        let into = self.net.output_places();
                        let outs = if into.is_empty() { &[][..] } else { &outs[..] };
                        EffectResult::Done(outputs(outs, &into, "s", None))
                    }
                    ResultGen::Failed(true) => EffectResult::Failed(Cause::retryable("boom")),
                    ResultGen::Failed(false) => EffectResult::Failed(Cause::definite("boom")),
                    ResultGen::Unknown => EffectResult::Unknown(Cause::definite("lost")),
                };
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(
                    &format!("complete({attempt:?}, {result:?}, {now:?})"),
                    || exec.complete(attempt, result, now),
                )?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                self.check_clock("complete", now, &before, &r)?;
            }
            Op::Resolve {
                occurrence,
                resolution,
                dt,
            } => {
                let occurrence = self.picked_occurrence(occurrence);
                let resolution = match resolution {
                    // A verified result puts like a body: into Fifo non-pool
                    // places only.
                    ResolutionGen::Verify(outs) => {
                        let into = self.net.output_places();
                        let outs = if into.is_empty() { &[][..] } else { &outs[..] };
                        Resolution::Verify(outputs(outs, &into, "s", None))
                    }
                    ResolutionGen::Retry => Resolution::Retry,
                    ResolutionGen::Drop => Resolution::Drop,
                };
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(&format!("resolve({occurrence}, {now:?})"), || {
                    exec.resolve(&occurrence, resolution, now)
                })?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                self.check_clock("resolve", now, &before, &r)?;
            }
            Op::Answer {
                work,
                subject,
                seen,
                answer,
                value,
                forward,
            } => {
                let work = work.of(&self.net.work_names());
                // An answer to a work whose binding reads an undeclared place
                // panics in the code; a follow-up card holds it.
                if self.net.reads_undeclared(&work) {
                    return Ok(());
                }
                let exec = &self.exec;
                let rows = match no_panic("asks", || forms::of(exec.asks()))? {
                    Answer::Known(rows) => rows,
                    Answer::Refused(_) => vec![],
                };
                let seen = match seen {
                    SeenGen::Offered(i) if !rows.is_empty() => rows[i % rows.len()].hash,
                    SeenGen::Offered(_) => AskHash([0; 32]),
                    SeenGen::Bogus(h) => AskHash(*h),
                };
                let id = self.occurrence(answer).to_string();
                let call = AnswerCall {
                    transition: TransitionName(work.clone()),
                    subject: SubjectName(subject.clone()),
                    seen,
                    answer: AnswerId(id.clone()),
                    value: value.clone(),
                };
                let now = self.clock + Duration::from_nanos(*forward);
                let exec = &mut self.exec;
                let r = no_panic(
                    &format!("answer({work:?}, {subject:?}, {id:?}, {now:?})"),
                    || exec.answer(call, now),
                )?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                self.check_clock("answer", now, &before, &r)?;
                // An answer to a changed situation expires instead of putting
                // `Answered`, so no round refuses the work.
                let expired = self
                    .exec
                    .tokens()
                    .subjects(&Generated::Expired.name(&work))
                    .iter()
                    .any(|t| t.as_str() == id);
                if r.is_ok() && !expired && self.net.binds_answer_input(&work) {
                    let conditions = format!("{:?}", self.exec.conditions());
                    let why = format!("{:?}", self.exec.why_not(&work, subject));
                    prop_assert!(
                        conditions.contains("BindsAnswerInput")
                            && conditions.contains(&format!("{work:?}")),
                        "an accepted answer to {work:?}, whose binding produces the answer input, is not disclosed: {conditions}; why_not: {why}; answer: {r:?}; marking: {}",
                        snapshot(&self.exec, &self.net)
                    );
                }
            }
            Op::Undo { answer, dt } => {
                let answer = self.picked_occurrence(answer);
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(&format!("undo({answer}, {now:?})"), || {
                    exec.undo(&answer, now)
                })?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                self.check_clock("undo", now, &before, &r)?;
            }
            Op::Asks => {
                let exec = &self.exec;
                // L7 refuses the firing of a work that binds the answer input,
                // not its row.
                let ghosts: BTreeSet<String> = self
                    .net
                    .work_names()
                    .into_iter()
                    .filter(|w| self.net.reads_undeclared(w))
                    .collect();
                match no_panic("asks", || forms::of(exec.asks()))? {
                    Answer::Known(rows) => {
                        for row in &rows {
                            prop_assert!(
                                !ghosts.contains(&row.transition.0),
                                "asks lists {:?}, whose binding reads an undeclared place",
                                row.transition
                            );
                        }
                    }
                    Answer::Refused(e) => {
                        let reads = self
                            .net
                            .work_names()
                            .iter()
                            .any(|w| self.net.reads_undeclared(w));
                        prop_assert!(
                            reads && e.contains("ReadsUndeclared"),
                            "asks was refused as {e}, though no work's binding reads an undeclared place"
                        );
                    }
                }
            }
            Op::Derive { place, subject } => {
                let place = place.of(&[self.net.readable(), self.net.asks_places()].concat());
                let exec = &self.exec;
                let answer = no_panic(&format!("derive({place:?}, {subject:?})"), || {
                    forms::of(exec.derive(&place, &Subject::new(subject)))
                })?;
                self.check_derive(&place, &answer, true)?;
            }
            Op::DeriveAll { place } => {
                let place = place.of(&[self.net.readable(), self.net.asks_places()].concat());
                let exec = &self.exec;
                let answer = no_panic(&format!("derive_all({place:?})"), || {
                    forms::of(exec.derive_all(&place))
                })?;
                // `derive_all` runs the derive once per subject that holds a
                // token in a stored place.
                let evaluated = all_places(&self.net)
                    .iter()
                    .filter(|p| p.as_str() != OUTCOME_UNKNOWN)
                    .any(|p| !self.exec.tokens().subjects(p).is_empty());
                self.check_derive(&place, &answer, evaluated)?;
            }
        }
        self.refresh();
        self.check_no_ghost_firing()
    }

    /// `derive` of a place that is not derived is refused; of a derived one
    /// that reads an undeclared place too, never answered as a value.
    fn check_derive<T: Debug>(
        &self,
        place: &str,
        answer: &Answer<T>,
        evaluated: bool,
    ) -> Result<(), TestCaseError> {
        let refused_as = |form: &str| matches!(answer, Answer::Refused(e) if e.contains(form));
        if self.net.derived_names().iter().any(|d| d == place) {
            if self.net.ghost(place) && evaluated {
                prop_assert!(
                    refused_as("ReadsUndeclared"),
                    "derive of {place:?}, which reads an undeclared place, answered {answer:?}"
                );
            } else {
                prop_assert!(
                    matches!(answer, Answer::Known(_)),
                    "derive of {place:?} was refused: {answer:?}"
                );
            }
        } else if self.net.asks_places().iter().any(|d| d == place) {
            // The Asks derive reaches the work's binding only while the work
            // is idle and its arcs hold, which this test does not model.
            if let Answer::Refused(e) = answer {
                prop_assert!(
                    e.contains("ReadsUndeclared"),
                    "derive of {place:?} was refused: {e}"
                );
            }
        } else if self.net.declares(place) {
            prop_assert!(
                refused_as("StoredPlace"),
                "derive of the stored {place:?} answered {answer:?}"
            );
        } else {
            prop_assert!(
                refused_as("UndeclaredPlace"),
                "derive of the undeclared {place:?} answered {answer:?}"
            );
        }
        Ok(())
    }

    /// A binding that reads an undeclared place never fires its transition:
    /// no body output, no effect request.
    fn check_no_ghost_firing(&self) -> Result<(), TestCaseError> {
        let conditions = format!("{:?}", self.exec.conditions());
        prop_assert!(
            !conditions.contains("BindsAnswerInput")
                || self
                    .net
                    .work_names()
                    .iter()
                    .any(|w| self.net.binds_answer_input(w)),
            "a BindsAnswerInput condition, though no work binds the answer input: {conditions}"
        );
        let ghosts = self.net.ghost_transitions();
        let by = producers(&self.exec, &self.net);
        for t in &ghosts {
            prop_assert!(
                !by.contains(t),
                "{t} fired although its binding reads an undeclared place"
            );
        }
        for r in &self.requests {
            prop_assert!(
                !ghosts.contains(&*r.transition),
                "{} was requested although its binding reads an undeclared place",
                r.transition
            );
        }
        Ok(())
    }
}

/// `why_not` of a name the net does not hold is the error form; of a held
/// one, the old answer, which never reads "enabled" for a binding that reads
/// an undeclared place.
fn check_why_not(
    net: &NetGen,
    transition: &str,
    answer: &Answer<Option<WhyNot>>,
) -> Result<(), TestCaseError> {
    let held = net.transition_names().iter().any(|t| t == transition);
    match answer {
        Answer::Refused(e) => {
            prop_assert!(
                !held,
                "why_not of the held transition {transition:?} was refused: {e}"
            );
            prop_assert!(
                e.contains("UnknownTransition"),
                "why_not of {transition:?} was refused as {e}"
            );
        }
        Answer::Known(w) => {
            prop_assert!(
                held,
                "why_not of {transition:?}, which the net does not hold, answered {w:?}"
            );
            if net.reads_undeclared(transition) {
                prop_assert!(
                    w.is_some(),
                    "why_not of {transition:?}, whose binding reads an undeclared place, reads enabled"
                );
            }
            if net.binds_answer_input(transition) {
                prop_assert!(
                    w.is_some(),
                    "why_not of {transition:?}, whose binding produces the answer input, reads enabled"
                );
            }
        }
    }
    Ok(())
}

/// The executor of `net`, or `None` when the loader or the lowering refused
/// it. A refusal is an error a caller sees; it must not be a panic.
fn load(net: &NetGen) -> Result<Option<Checked>, TestCaseError> {
    Ok(no_panic("load", || net.executor())?.ok())
}

/// Any sequence of public calls on any net the format allows: no call
/// panics, a call before the clock is refused and changes nothing, a name
/// the net does not hold is refused, a caller value that repeats a bound
/// field is refused, and a binding that reads an undeclared place never
/// fires. The generator finds kept as their state run first.
#[test]
fn no_sequence_of_caller_calls_panics() {
    // The third field: the attempt number of each of T0's requests; a retry
    // of the same run counts on, a new run starts at 1 again.
    let found: [(&str, FoundCase, Option<&[u32]>); 9] = [
        (
            "free_of_an_answer_place_counts_its_answer",
            free_of_an_answer_place_counts_its_answer(),
            None,
        ),
        (
            "a_pool_token_back_from_a_failed_run_joins_the_next_run",
            a_pool_token_back_from_a_failed_run_joins_the_next_run(),
            None,
        ),
        (
            "an_answer_for_the_pool_subject_read_through_free_does_not_panic",
            an_answer_for_the_pool_subject_read_through_free_does_not_panic(),
            None,
        ),
        (
            "a_pool_read_wakes_its_marked_reader",
            a_pool_read_wakes_its_marked_reader(),
            None,
        ),
        (
            "an_equal_rewrite_keeps_the_retry_count",
            an_equal_rewrite_keeps_the_retry_count(),
            Some(&[1, 2]),
        ),
        (
            "new_unread_content_keeps_the_retry_count",
            new_unread_content_keeps_the_retry_count(),
            Some(&[1, 2]),
        ),
        (
            "a_write_during_the_run_keeps_the_retry_count",
            a_write_during_the_run_keeps_the_retry_count(),
            Some(&[1, 2]),
        ),
        (
            "new_read_content_starts_a_new_run",
            new_read_content_starts_a_new_run(),
            Some(&[1, 1]),
        ),
        (
            "a_retry_input_consumed_by_another_keeps_the_run",
            a_retry_input_consumed_by_another_keeps_the_run(),
            Some(&[1, 2]),
        ),
    ];
    for (name, (net, ops), t0_attempts) in found {
        if let Err(e) = net.executor() {
            panic!("static example {name}: the net does not load: {e}");
        }
        let m = run_found(net, &ops).unwrap_or_else(|e| panic!("static example {name}: {e}"));
        if let Some(want) = t0_attempts {
            let got: Vec<Option<u32>> = m
                .attempts
                .iter()
                .filter(|(t, _)| &**t == TRANSITIONS[0])
                .map(|(_, n)| *n)
                .collect();
            let want: Vec<Option<u32>> = want.iter().copied().map(Some).collect();
            assert_eq!(
                got, want,
                "static example {name}: attempt numbers of T0's requests"
            );
        }
    }
    // PROPTEST_CASES above 512 raises the count, as for a proptest! test.
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: ProptestConfig::default().cases.max(512),
        max_shrink_time: 60_000,
        source_file: Some(file!()),
        ..ProptestConfig::default()
    });
    runner
        .run(&calls(), |(net, ops)| run_calls(net, &ops))
        .unwrap();
}

/// A generator find, kept as its state: T0 binds by reading the pool P0 it
/// has no arc on and fails definitely; T1 drains the pool. When the pool
/// empties, T0's input changes, so its failure mark no longer holds and T0
/// must be decided again.
fn a_pool_read_wakes_its_marked_reader() -> FoundCase {
    let net = NetGen {
        places: vec![
            PlaceGen {
                capacity: PlaceCapacity::Fifo,
                pool: Some(3),
            },
            PlaceGen {
                capacity: PlaceCapacity::Single,
                pool: None,
            },
        ],
        transitions: vec![
            TransGen {
                trigger: Trigger::Auto,
                priority: 0,
                roles: vec![Role::None, Role::Consumes],
                produces: vec![],
                facts: vec![],
                on_failure: None,
                on_timeout: None,
                effect: None,
                timers: None,
                bind: BindProg {
                    reads: vec![ReadOp::Field(Name::Held(0), Subj::Own, "x".to_owned())],
                    out: Some(vec![]),
                },
                body: BodyProg::Fails { retryable: false },
            },
            TransGen {
                trigger: Trigger::Auto,
                priority: 0,
                roles: vec![Role::Consumes, Role::Reads],
                produces: vec![],
                facts: vec![],
                on_failure: None,
                on_timeout: None,
                effect: None,
                timers: None,
                bind: BindProg {
                    reads: vec![],
                    out: Some(vec![]),
                },
                body: BodyProg::Outputs(vec![]),
            },
        ],
        derived: vec![],
        works: vec![],
    };
    let ops = vec![Op::Write {
        place: Name::Held(1),
        subject: "s".to_string(),
        content: vec![("v".to_owned(), Value::Array(vec![]))],
        dt: Dt::Same,
    }];
    (net, ops)
}

/// A generator find, kept as its state: T0's retry waits on a token that an
/// outside write gives equal content again; the retry is the same run.
fn an_equal_rewrite_keeps_the_retry_count() -> FoundCase {
    let t = |roles: Vec<Role>, effect, reads, out, body| TransGen {
        trigger: Trigger::Auto,
        priority: 0,
        roles,
        produces: vec![],
        facts: vec![],
        on_failure: None,
        on_timeout: None,
        effect,
        timers: None,
        bind: BindProg { reads, out },
        body,
    };
    let net = NetGen {
        places: vec![
            PlaceGen {
                capacity: PlaceCapacity::Single,
                pool: None,
            },
            PlaceGen {
                capacity: PlaceCapacity::Fifo,
                pool: Some(1),
            },
            PlaceGen {
                capacity: PlaceCapacity::Single,
                pool: None,
            },
        ],
        transitions: vec![
            t(
                vec![Role::None, Role::None, Role::None],
                Some(Receiver::Keyed),
                vec![],
                Some(vec![]),
                BodyProg::Outputs(vec![]),
            ),
            t(
                vec![Role::Reads, Role::Consumes, Role::None],
                None,
                vec![],
                Some(vec![]),
                BodyProg::Outputs(vec![]),
            ),
            t(
                vec![Role::Consumes, Role::None, Role::None],
                None,
                vec![ReadOp::Field(Name::Held(1), Subj::Own, "x".to_owned())],
                Some(vec![]),
                BodyProg::Fails { retryable: false },
            ),
            t(
                vec![Role::Consumes, Role::Consumes, Role::Consumes],
                None,
                vec![],
                None,
                BodyProg::Outputs(vec![]),
            ),
        ],
        derived: vec![],
        works: vec![],
    };
    let ops = vec![
        Op::Write {
            place: Name::Held(0),
            subject: "s".to_owned(),
            content: vec![],
            dt: Dt::Same,
        },
        Op::Complete {
            attempt: Pick::Known(0),
            result: ResultGen::Failed(true),
            dt: Dt::Back(1),
        },
        Op::Write {
            place: Name::Held(411133119497800110),
            subject: "s".to_owned(),
            content: vec![],
            dt: Dt::Back(1),
        },
        Op::Step {
            dt: Dt::Forward(10000000),
        },
    ];
    (net, ops)
}

/// A generator find, kept as its state: while T0's retry waits, an outside
/// write gives P0 new content in a field T0's bind does not read, so the
/// input is the same and the retry is the same run.
fn new_unread_content_keeps_the_retry_count() -> FoundCase {
    let (net, mut ops) = an_equal_rewrite_keeps_the_retry_count();
    ops[1] = Op::Complete {
        attempt: Pick::Known(0),
        result: ResultGen::Failed(true),
        dt: Dt::Same,
    };
    ops[2] = Op::Write {
        place: Name::Held(0),
        subject: "s".to_owned(),
        content: vec![("x".to_owned(), Value::Boolean(false))],
        dt: Dt::Same,
    };
    (net, ops)
}

/// While T0 runs, an outside write puts new content into the place it took
/// from; the try fails retryably and the written token stays, so the retry
/// is the same run on that token.
fn a_write_during_the_run_keeps_the_retry_count() -> FoundCase {
    let (net, mut ops) = new_unread_content_keeps_the_retry_count();
    ops.swap(1, 2);
    (net, ops)
}

/// While T0's retry waits, an outside write changes the field its bind reads:
/// new input, so T0 starts a new run at once.
fn new_read_content_starts_a_new_run() -> FoundCase {
    let (mut net, ops) = new_unread_content_keeps_the_retry_count();
    net.transitions[0].bind.reads = vec![ReadOp::Field(Name::Held(0), Subj::Own, "x".to_owned())];
    (net, ops)
}

/// While T0's retry waits, T1 consumes the token T0 failed on, and an outside
/// write puts a new token with equal content: the retry is the same run.
fn a_retry_input_consumed_by_another_keeps_the_run() -> FoundCase {
    let t = |effect| TransGen {
        trigger: Trigger::Auto,
        priority: 0,
        roles: vec![Role::Consumes],
        produces: vec![],
        facts: vec![],
        on_failure: None,
        on_timeout: None,
        effect,
        timers: None,
        bind: BindProg {
            reads: vec![],
            out: Some(vec![]),
        },
        body: BodyProg::Outputs(vec![]),
    };
    let net = NetGen {
        places: vec![PlaceGen {
            capacity: PlaceCapacity::Single,
            pool: None,
        }],
        transitions: vec![t(Some(Receiver::Keyed)), t(None)],
        derived: vec![],
        works: vec![],
    };
    let write = || Op::Write {
        place: Name::Held(0),
        subject: "s".to_owned(),
        content: vec![],
        dt: Dt::Same,
    };
    let ops = vec![
        write(),
        Op::Complete {
            attempt: Pick::Known(0),
            result: ResultGen::Failed(true),
            dt: Dt::Same,
        },
        Op::Step {
            dt: Dt::Forward(10_000_000),
        },
        write(),
    ];
    (net, ops)
}

/// A net and the calls on it; a drained pool reader ([`Drain`]) gets a first
/// write of its input, which the drawn calls rarely make.
fn calls() -> impl Strategy<Value = (NetGen, Vec<Op>)> {
    let rewrite = prop_oneof![Just(Rewrite::Equal), Just(Rewrite::NewUnread)];
    (net_gen(), prop::collection::vec(op(), 1..24), rewrite).prop_map(|(net, mut ops, rewrite)| {
        if let Some((q, runs)) = net.retry_input() {
            let write = |content| Op::Write {
                place: Name::Held(q),
                subject: "s".to_owned(),
                content,
                dt: Dt::Same,
            };
            let mut first = vec![write(vec![])];
            first.extend((0..runs).map(|i| Op::Complete {
                attempt: Pick::Known(i),
                result: ResultGen::Failed(true),
                dt: Dt::Same,
            }));
            first.push(write(match rewrite {
                Rewrite::Equal => vec![],
                Rewrite::NewUnread => vec![("w".to_owned(), Value::Boolean(true))],
            }));
            first.push(Op::Step {
                dt: Dt::Forward(10_000_000),
            });
            ops.splice(0..0, first);
        }
        if let Some(q) = net.drained_input() {
            ops.insert(
                0,
                Op::Write {
                    place: Name::Held(q),
                    subject: "s".to_owned(),
                    content: vec![],
                    dt: Dt::Same,
                },
            );
        }
        (net, ops)
    })
}

/// An outside write of the content a single-token row holds leaves the held
/// token as it was: its id and header too, not only its payload. A row that
/// a firing of the same call wrote again has a new version and is out of scope.
fn an_equal_rewrite_keeps_the_token(
    before: Option<(Token, u64)>,
    written: &Content,
    after: Option<(Token, u64)>,
) -> Result<(), TestCaseError> {
    let (Some((before, v0)), Some((after, v1))) = (before, after) else {
        return Ok(());
    };
    if before.payload == Payload::Record(written.clone()) && v0 == v1 {
        prop_assert_eq!(&after, &before, "an equal rewrite replaced the held token");
    }
    Ok(())
}

/// A generator find kept as its state: a net and the calls on it.
type FoundCase = (NetGen, Vec<Op>);

fn run_calls(net: NetGen, ops: &[Op]) -> Result<(), TestCaseError> {
    let Some(exec) = load(&net)? else {
        return Ok(());
    };
    run(net, exec, ops).map(drop)
}

/// A static example runs like a drawn case; its net must load.
fn run_found(net: NetGen, ops: &[Op]) -> Result<Machine, TestCaseError> {
    let exec = load(&net)?.expect("a static example's net loads");
    run(net, exec, ops)
}

fn run(net: NetGen, exec: Checked, ops: &[Op]) -> Result<Machine, TestCaseError> {
    let mut m = Machine::new(net, exec);
    for op in ops {
        m.apply(op)?;
    }
    Ok(m)
}

/// A generator find, kept as its state: a bind counts the free tokens of an
/// answer place whose subject is the pool subject.
fn free_of_an_answer_place_counts_its_answer() -> FoundCase {
    let net = NetGen {
        places: vec![PlaceGen {
            capacity: PlaceCapacity::Single,
            pool: None,
        }],
        transitions: vec![TransGen {
            trigger: Trigger::Auto,
            priority: 0,
            roles: vec![Role::None],
            produces: vec![],
            facts: vec![],
            on_failure: None,
            on_timeout: None,
            effect: None,
            timers: None,
            bind: BindProg {
                reads: vec![],
                out: None,
            },
            body: BodyProg::Outputs(vec![]),
        }],
        derived: vec![],
        works: vec![WorkGen {
            roles: vec![Role::Inhibits],
            puts: vec![],
            effect: None,
            input: None,
            irreversible: false,
            undoes: None,
            bind: BindProg {
                reads: vec![ReadOp::Traced(Box::new(ReadOp::Free(Name::Held(
                    2536311783032131285,
                ))))],
                out: Some(vec![]),
            },
            body: BodyProg::Outputs(vec![]),
        }],
    };
    let ops = [Op::Answer {
        work: Name::Held(0),
        subject: "pool".to_string(),
        seen: SeenGen::Offered(0),
        answer: OccGen::Fresh,
        value: None,
        forward: 0,
    }];
    (net, ops.to_vec())
}

/// A generator find, kept as its state: a pool token goes back after a
/// failed run and a new run of the same consumer takes it.
fn a_pool_token_back_from_a_failed_run_joins_the_next_run() -> FoundCase {
    let net = NetGen {
        places: vec![
            PlaceGen {
                capacity: PlaceCapacity::Fifo,
                pool: Some(3),
            },
            PlaceGen {
                capacity: PlaceCapacity::Single,
                pool: None,
            },
        ],
        transitions: vec![TransGen {
            trigger: Trigger::Auto,
            priority: 0,
            roles: vec![Role::Consumes, Role::Consumes],
            produces: vec![],
            facts: vec![],
            on_failure: None,
            on_timeout: None,
            effect: Some(Receiver::Keyed),
            timers: None,
            bind: BindProg {
                reads: vec![ReadOp::Traced(Box::new(ReadOp::Field(
                    Name::Held(1),
                    Subj::Own,
                    "x".to_owned(),
                )))],
                out: Some(vec![]),
            },
            body: BodyProg::Outputs(vec![]),
        }],
        derived: vec![],
        works: vec![],
    };
    // The second write replaces the read field, so the failure mark lets T0
    // fire again.
    let write = |x| Op::Write {
        place: Name::Held(1),
        subject: "s".to_string(),
        content: vec![("x".to_owned(), Value::Integer(x))],
        dt: Dt::Same,
    };
    let ops = [
        write(0),
        Op::Complete {
            attempt: Pick::Known(0),
            result: ResultGen::Failed(false),
            dt: Dt::Same,
        },
        write(1),
    ];
    (net, ops.to_vec())
}

/// A generator find, kept as its state: a caller answer into the subject a
/// pool row uses, while the work's bind counts the free tokens of a place.
fn an_answer_for_the_pool_subject_read_through_free_does_not_panic() -> FoundCase {
    let net = NetGen {
        places: vec![PlaceGen {
            capacity: PlaceCapacity::Single,
            pool: None,
        }],
        transitions: vec![TransGen {
            trigger: Trigger::Auto,
            priority: 0,
            roles: vec![Role::None],
            produces: vec![],
            facts: vec![],
            on_failure: None,
            on_timeout: None,
            effect: None,
            timers: None,
            bind: BindProg {
                reads: vec![],
                out: None,
            },
            body: BodyProg::Outputs(vec![]),
        }],
        derived: vec![],
        works: vec![WorkGen {
            roles: vec![Role::Inhibits],
            puts: vec![],
            effect: None,
            input: None,
            irreversible: false,
            undoes: None,
            bind: BindProg {
                reads: vec![ReadOp::Traced(Box::new(ReadOp::Free(Name::Held(
                    13364452132945421913,
                ))))],
                out: Some(vec![]),
            },
            body: BodyProg::Outputs(vec![]),
        }],
    };
    let ops = [Op::Answer {
        work: Name::Held(0),
        subject: POOL_SUBJECT.to_owned(),
        seen: SeenGen::Offered(0),
        answer: OccGen::Fresh,
        value: None,
        forward: 0,
    }];
    (net, ops.to_vec())
}

// ------------------------------------------------------ one defect each

/// A net of manual file transitions only, so no auto firing takes the
/// inputs a test writes before its manual fire; binds read nothing, since
/// the machine draws the reads.
fn manual_net() -> impl Strategy<Value = NetGen> {
    net_gen().prop_map(|mut n| {
        for t in &mut n.transitions {
            t.trigger = Trigger::Manual;
            t.bind.reads.clear();
        }
        n.works.clear();
        n
    })
}

/// Writes one token into every non-pool place transition `i` consumes or
/// reads, for subject `s`; a pool holds its tokens from the start.
fn enable(exec: &mut Checked, net: &NetGen, i: usize, s: &str) -> Result<(), TestCaseError> {
    let [consumes, reads, _] = net.arcs(i);
    for p in consumes.iter().chain(&reads) {
        let k = PLACES.iter().position(|q| q == p).expect("a file place");
        if net.places[k].pool.is_some() {
            continue;
        }
        let w = no_panic("write", || exec.write(p, s, Content::new(), Duration::ZERO))?;
        prop_assert!(w.is_ok(), "premise: the write into {p} is accepted: {w:?}");
    }
    Ok(())
}

fn without_inhibitors(net: &mut NetGen, i: usize) {
    for r in &mut net.transitions[i].roles {
        if *r == Role::Inhibits {
            *r = Role::None;
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// The value of a manual fire joins the bound input; a field in both is
    /// refused as a collision, and the refusal changes nothing.
    #[test]
    fn fire_refuses_a_value_that_repeats_a_bound_field(
        mut net in manual_net(),
        bound in prop::collection::vec((field(), value()), 1..4),
        repeat in prop::collection::vec(any::<usize>(), 1..3),
        extra in prop::option::of(value()),
        s in subject(),
    ) {
        net.transitions[0].bind = BindProg { reads: vec![], out: Some(bound.clone()) };
        net.transitions[0].body = BodyProg::Outputs(vec![]);
        without_inhibitors(&mut net, 0);
        let Some(mut exec) = load(&net)? else { return Ok(()) };
        enable(&mut exec, &net, 0, &s)?;
        let before = snapshot(&exec, &net);
        let mut value: Content = repeat
            .iter()
            .map(|i| (bound[i % bound.len()].0.clone(), Value::Integer(7)))
            .collect();
        if let Some(v) = extra {
            value.insert("caller-only".to_owned(), v);
        }
        let r = no_panic("fire", || {
            exec.fire("T0", &s, OccurrenceId::caller("o1"), value, Duration::ZERO)
        })?;
        prop_assert!(
            names_error(&r, "ValueCollides"),
            "fire with a value that repeats a bound field: {r:?}"
        );
        prop_assert_eq!(before, snapshot(&exec, &net));
    }

    /// Exclusion: a value whose fields the bind does not produce is accepted.
    #[test]
    fn fire_accepts_a_value_beside_the_bound_fields(
        mut net in manual_net(),
        bound in prop::collection::vec((field(), value()), 0..4),
        value in fields(),
        s in subject(),
    ) {
        let bound_keys: BTreeSet<_> = bound.iter().map(|(k, _)| k.clone()).collect();
        let value: Content = value.into_iter().filter(|(k, _)| !bound_keys.contains(k)).collect();
        net.transitions[0].bind = BindProg { reads: vec![], out: Some(bound) };
        net.transitions[0].body = BodyProg::Outputs(vec![]);
        without_inhibitors(&mut net, 0);
        let Some(mut exec) = load(&net)? else { return Ok(()) };
        enable(&mut exec, &net, 0, &s)?;
        let r = no_panic("fire", || {
            exec.fire("T0", &s, OccurrenceId::caller("o1"), value, Duration::ZERO)
        })?;
        prop_assert!(r.is_ok(), "fire with a disjoint value was refused: {r:?}");
    }

    /// `why_not` of a name the net does not hold is the error form; of every
    /// name it holds, the old answer.
    #[test]
    fn why_not_refuses_a_transition_the_net_does_not_hold(
        net in net_gen(),
        other in other_name(),
        s in subject(),
    ) {
        let Some(exec) = load(&net)? else { return Ok(()) };
        for t in net.transition_names().iter().chain([&other]) {
            let answer = no_panic(&format!("why_not({t:?})"), || forms::of(exec.why_not(t, &s)))?;
            check_why_not(&net, t, &answer)?;
        }
    }

    /// A fire of a transition the net does not hold is refused and changes
    /// nothing.
    #[test]
    fn fire_refuses_a_transition_the_net_does_not_hold(
        net in net_gen(),
        other in other_name(),
        s in subject(),
    ) {
        prop_assume!(!net.transition_names().contains(&other));
        let Some(mut exec) = load(&net)? else { return Ok(()) };
        let before = snapshot(&exec, &net);
        let r = no_panic("fire", || {
            exec.fire(&other, &s, OccurrenceId::caller("o1"), Content::new(), Duration::ZERO)
        })?;
        prop_assert!(names_error(&r, "UnknownTransition"), "fire of {other:?}: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &net));
    }

    /// Every call that takes `now`, with a `now` before the executor's clock,
    /// is refused for the clock and changes nothing; at the clock it is not
    /// refused for the clock.
    #[test]
    fn a_call_before_the_clock_is_refused_and_changes_nothing(
        net in net_gen(),
        clock_ns in 1u64..=MAX_NOW_NS,
        back_ns in 1u64..=u64::MAX,
        which in 0usize..7,
        s in subject(),
    ) {
        let Some(mut exec) = load(&net)? else { return Ok(()) };
        let clock = Duration::from_nanos(clock_ns);
        let first = no_panic("step", || exec.step(clock))?;
        prop_assert!(
            !names_error(&first, "ClockWentBack"),
            "premise: a step forward is not refused for the clock: {first:?}"
        );
        let back = clock - Duration::from_nanos(back_ns.min(clock_ns));
        let place = net.stored()[0].clone();
        let call = |exec: &mut Checked, now: Duration| -> String {
            match which {
                0 => format!("{:?}", exec.fire("T0", &s, OccurrenceId::caller("o1"), Content::new(), now)),
                1 => format!("{:?}", exec.step(now)),
                2 => format!("{:?}", exec.write(&place, &s, Content::new(), now)),
                3 => format!("{:?}", exec.retract(&place, &s, now)),
                4 => format!("{:?}", exec.complete(AttemptId(1), EffectResult::Done(vec![]), now)),
                5 => format!("{:?}", exec.resolve(&OccurrenceId::caller("o1"), Resolution::Drop, now)),
                _ => format!("{:?}", exec.undo(&OccurrenceId::caller("o1"), now)),
            }
        };
        let before = snapshot(&exec, &net);
        let refused = no_panic(&format!("call {which} before the clock"), || call(&mut exec, back))?;
        prop_assert!(
            refused.contains("ClockWentBack"),
            "call {which} at {back:?}, before the clock {clock:?}: {refused}"
        );
        prop_assert_eq!(&before, &snapshot(&exec, &net));
        // A write into a pool panics in the code; a follow-up card holds it.
        if which == 2 && net.places[0].pool.is_some() {
            return Ok(());
        }
        let at = no_panic(&format!("call {which} at the clock"), || call(&mut exec, clock))?;
        prop_assert!(
            !at.contains("ClockWentBack"),
            "call {which} at the clock {clock:?} was refused for the clock: {at}"
        );
    }

    /// A refused call still moves the clock to its `now`.
    #[test]
    fn a_refused_fire_still_moves_the_clock(
        net in net_gen(),
        other in other_name(),
        later in 2u64..1_000_000,
        between in 1u64..1_000_000,
    ) {
        prop_assume!(!net.transition_names().contains(&other));
        let Some(mut exec) = load(&net)? else { return Ok(()) };
        let r = no_panic("fire", || {
            exec.fire(&other, "s", OccurrenceId::caller("o1"), Content::new(), Duration::from_nanos(later))
        })?;
        prop_assert!(r.is_err(), "premise: the fire of {other:?} is refused");
        let between = Duration::from_nanos(between.min(later - 1));
        let step = no_panic("step", || exec.step(between))?;
        prop_assert!(
            names_error(&step, "ClockWentBack"),
            "a step at {between:?} after a refused fire at {later}ns: {step:?}"
        );
    }

    /// A binding that reads a place the net does not declare, directly,
    /// through `traced` or through a derived place: no panic, its transition
    /// never fires and `why_not` names the place; an auto transition is
    /// disclosed once in the conditions, a manual fire is refused naming the
    /// place. The same binding over declared places fires.
    #[test]
    fn a_bind_reading_an_undeclared_place_never_fires(
        mut net in net_gen(),
        ghost in other_name(),
        kind in 0usize..6,
        manual in any::<bool>(),
        s in subject(),
    ) {
        if net.places.len() < 2 {
            net.places.push(PlaceGen { capacity: PlaceCapacity::Single, pool: None });
            for t in &mut net.transitions {
                t.roles.push(Role::None);
            }
        }
        net.works.clear();
        net.derived = vec![BindProg { reads: vec![], out: Some(vec![]) }];
        prop_assume!(!net.declares(&ghost));
        net.places[0] = PlaceGen { capacity: PlaceCapacity::Single, pool: None };
        let last = net.places.len() - 1;
        net.places[last] = PlaceGen { capacity: PlaceCapacity::Fifo, pool: None };
        let t0 = &mut net.transitions[0];
        t0.trigger = if manual { Trigger::Manual } else { Trigger::Auto };
        t0.effect = None;
        t0.timers = None;
        t0.on_failure = None;
        t0.on_timeout = None;
        t0.roles = vec![Role::None; last + 1];
        t0.roles[0] = Role::Consumes;
        t0.bind = BindProg {
            reads: vec![ReadOp::Field(Name::Held(0), Subj::Own, "x".to_owned())],
            out: Some(vec![]),
        };
        t0.body = BodyProg::Outputs(vec![OutGen { place: last, subject: Subj::Own, fields: vec![] }]);
        for t in &mut net.transitions[1..] {
            t.trigger = Trigger::Manual;
            t.bind.reads.clear();
        }
        let names = net.stored();
        type Run = (bool, Answer<Option<WhyNot>>, String, String);
        // A closure's reads are invisible to the loader, so a ghost read never
        // decides whether the net loads.
        let run = |net: &NetGen| -> Result<Option<Run>, TestCaseError> {
            let Some(mut exec) = load(net)? else {
                prop_assert!(false, "the net does not load: {:?}", no_panic("load", || net.executor().err())?);
                return Ok(None);
            };
            let w = no_panic("write", || exec.write(&names[0], &s, Content::new(), Duration::ZERO))?;
            prop_assert!(!names_error(&w, "ClockWentBack"));
            let first = if manual {
                format!("{:?}", no_panic("fire", || {
                    exec.fire("T0", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_secs(1))
                })?)
            } else {
                format!("{:?}", no_panic("step", || exec.step(Duration::from_secs(1)))?)
            };
            let _ = no_panic("step", || exec.step(Duration::from_secs(2)))?;
            let why = no_panic("why_not", || forms::of(exec.why_not("T0", &s)))?;
            let conditions = format!("{:?}", exec.conditions());
            Ok(Some((producers(&exec, net).contains("T0"), why, first, conditions)))
        };
        let declared = run(&net)?;
        prop_assert!(
            matches!(declared, Some((true, ..))),
            "premise: T0 over declared places fires: {declared:?}"
        );
        let g = Name::Other(ghost.clone());
        let read = match kind {
            0 => ReadOp::Field(g, Subj::Own, "x".to_owned()),
            1 => ReadOp::Text(g, Subj::Own, "x".to_owned()),
            2 => ReadOp::Present(g, Subj::Own),
            3 => ReadOp::Free(g),
            4 => ReadOp::Traced(Box::new(ReadOp::Field(g, Subj::Own, "x".to_owned()))),
            _ => {
                net.derived[0].reads.push(ReadOp::Present(g, Subj::Own));
                let d = net.readable().iter().position(|p| p == DERIVED[0]).expect("D0 is readable");
                ReadOp::Field(Name::Held(d), Subj::Own, "x".to_owned())
            }
        };
        net.transitions[0].bind.reads.insert(0, read);
        if let Some((fired, why, first, conditions)) = run(&net)? {
            prop_assert!(!fired, "T0 fired although its binding reads the undeclared {ghost:?}");
            let named = format!("{ghost:?}");
            if manual {
                prop_assert!(
                    first.starts_with("Err") && first.contains("ReadsUndeclared") && first.contains(&named),
                    "the manual fire of T0 is not refused naming the undeclared {ghost:?}: {first}"
                );
            } else {
                prop_assert!(
                    conditions.matches("ReadsUndeclared").count() == 1 && conditions.contains(&named),
                    "the conditions do not disclose T0's read of {ghost:?} exactly once: {conditions}"
                );
            }
            prop_assert!(
                matches!(&why, Answer::Known(Some(w))
                    if format!("{w:?}").contains("ReadsUndeclared")
                        && format!("{w:?}").contains(&format!("{ghost:?}"))),
                "why_not of T0 does not name the undeclared {ghost:?}: {why:?}"
            );
        }
    }
}

// --------------------------------------------- inputs of a follow-up card

/// A manual `T` that takes `In` and puts one token into `into`, beside a
/// derived place `D`.
fn output_net(into: &str) -> Result<Checked, TestCaseError> {
    use holon_net::exec::Body;
    use holon_net::exec::Class;
    use holon_net::exec::TransitionDef;
    let into = into.to_owned();
    let t = TransitionDef::new(
        "T",
        holon_net::exec::Trigger::Manual,
        Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new())),
        Body::pure(move |s: &Subject, _: &Content| Ok(vec![Output::new(&into, s, Content::new())])),
    )
    .consumes("In");
    let net = holon_net::exec::new_net()
        .place("In", Class::Process)
        .derived("D", Arc::new(|_: &mut Reader<'_>, _: &Subject| None))
        .transition(t);
    let exec = no_panic("load", || Executor::new(net).map(Checked::from))?;
    exec.map_err(|e| TestCaseError::fail(format!("premise: the net loads: {e}")))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Caller inputs that still panic and belong to a follow-up card: a write
    /// or retract into the executor's own places, a body output naming a
    /// place the net does not declare or a derived one, and an answer before
    /// the clock. Run with `--run-ignored` to see them.
    #[test]
    #[ignore = "follow-up card: these caller inputs still panic"]
    fn follow_up_caller_inputs_do_not_panic(
        net in net_gen(),
        which in 0usize..5,
        target in other_name(),
        s in subject(),
    ) {
        let owned = [
            Place::running("T0").to_string(),
            OUTCOME_UNKNOWN.to_owned(),
            Place::outcome_unknown("T0").to_string(),
        ];
        match which {
            0 | 1 => {
                let Some(mut exec) = load(&net)? else { return Ok(()) };
                let p = &owned[s.len() % owned.len()];
                if which == 0 {
                    let _ = no_panic(&format!("write({p:?})"), || exec.write(p, &s, Content::new(), Duration::ZERO))?;
                } else {
                    let _ = no_panic(&format!("retract({p:?})"), || exec.retract(p, &s, Duration::ZERO))?;
                }
            }
            2 | 3 => {
                let into = if which == 2 { target } else { "D".to_owned() };
                prop_assume!(into != "In");
                let mut exec = output_net(&into)?;
                let w = no_panic("write", || exec.write("In", &s, Content::new(), Duration::ZERO))?;
                prop_assert!(w.is_ok(), "premise: the write is accepted: {w:?}");
                let _ = no_panic(&format!("fire with an output into {into:?}"), || {
                    exec.fire("T", &s, OccurrenceId::caller("o1"), Content::new(), Duration::ZERO)
                })?;
            }
            _ => {
                let Some(mut exec) = load(&net)? else { return Ok(()) };
                let _ = no_panic("step", || exec.step(Duration::from_secs(2)))?;
                let call = AnswerCall {
                    transition: TransitionName("W0".to_owned()),
                    subject: SubjectName(s.clone()),
                    seen: AskHash([0; 32]),
                    answer: AnswerId("a1".to_owned()),
                    value: None,
                };
                let _ = no_panic("answer before the clock", || exec.answer(call, Duration::from_secs(1)))?;
            }
        }
    }
}

/// A derived place that reads itself recurses without end. Run with
/// `--run-ignored` to see it.
#[test]
#[ignore = "follow-up card: a derived place that reads itself overflows the stack"]
fn a_derived_place_that_reads_itself_does_not_overflow() {
    use holon_net::exec::Body;
    use holon_net::exec::Class;
    use holon_net::exec::TransitionDef;
    let t = TransitionDef::new(
        "T",
        holon_net::exec::Trigger::Auto,
        Arc::new(|r: &mut Reader<'_>, s: &Subject| r.present("D", s).then(Content::new)),
        Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
    )
    .consumes("In");
    let net = holon_net::exec::new_net()
        .place("In", Class::Process)
        .derived(
            "D",
            Arc::new(|r: &mut Reader<'_>, s: &Subject| r.present("D", s).then(Content::new)),
        )
        .transition(t);
    if let Ok(mut exec) = Executor::new(net).map(Checked::from) {
        let _ = exec.write("In", "s", Content::new(), Duration::ZERO);
    }
}
