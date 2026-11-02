//! Every executor call a caller can make returns a value or an error; none
//! panics. The nets are drawn from the net file format plus derived places
//! and lowered asks, the bindings and bodies from a small program type, and
//! the calls from every public operation with arguments that match the net
//! and arguments that contradict it. This machine also writes into pools,
//! the executor's own places and `Answered`, puts outputs into every place
//! name, lets derived places read each other in cycles, and draws a `now`
//! past the i64 nanosecond range.
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
use holon_net::exec::AskMeta;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::HASH_FIELD;
use holon_net::exec::NetDef;
use holon_net::exec::OUTCOME_UNKNOWN;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::POOL_SUBJECT;
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Resolution;
use holon_net::exec::Retry;
use holon_net::exec::SEEN_FIELD;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::WhyNot;
use holon_net::exec::answer_transition;
use holon_net::exec::answered_place;
use holon_net::exec::asks_place;
use holon_net::exec::expired_place;
use holon_net::exec::hash_text;
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

// The `forms` module of `caller_inputs` is private to it.
#[allow(clippy::duplicate_mod)]
#[path = "caller_inputs/forms.rs"]
mod forms;

use forms::Answer;

const PLACES: [&str; 4] = ["P0", "P1", "P2", "P3"];
const DERIVED: [&str; 2] = ["D0", "D1"];
const TRANSITIONS: [&str; 4] = ["T0", "T1", "T2", "T3"];
const WORKS: [&str; 2] = ["W0", "W1"];
/// The field a body tags its outputs with, so a test sees who produced them.
const BY: &str = "by";

// ------------------------------------------------------------ generators

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Boolean),
        any::<i64>().prop_map(Value::Integer),
        any::<f64>().prop_map(Value::Float),
        ".{0,6}".prop_map(Value::String),
        ".{0,6}".prop_map(Value::DateTime),
        ".{0,6}".prop_map(Value::Json),
    ];
    leaf.prop_recursive(2, 6, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
            prop::collection::hash_map(".{0,3}", inner, 0..3).prop_map(Value::Object),
        ]
    })
}

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

/// An output of a body or a result: into a place of the net's output list,
/// or into any other name.
#[derive(Clone, Debug)]
struct OutGen {
    place: Name,
    subject: Subj,
    fields: Vec<(String, Value)>,
}

fn outs() -> impl Strategy<Value = Vec<OutGen>> {
    prop::collection::vec(
        (name(), subj(), fields()).prop_map(|(place, subject, fields)| OutGen {
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
                &o.place.of(places),
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
    /// `Some`: the ask is built by hand through `NetDef::ask` instead of the
    /// lowering.
    hand: Option<HandGen>,
}

/// A hand-built ask: its `Asks` row and its `Answer` transition are drawn.
#[derive(Clone, Debug)]
struct HandGen {
    row: RowHash,
    /// The `Answer` binding returns its fields only while the `Asks` row
    /// stands.
    gated: bool,
    bind: BindProg,
    /// An answer key the `Answer` binding also returns.
    planted: Option<&'static str>,
    body: AnswerBody,
    /// Inhibitor arcs of `Answer` from the work's `Answered`, `Running` and
    /// `WorkFailed`.
    inhibits: [bool; 3],
    /// The run timer and the retry (count bound, backoff) of `Answer`.
    run_timer: Option<u64>,
    retry: Option<(u32, u64)>,
}

/// The hash field of an `Asks` row.
#[derive(Clone, Debug)]
enum RowHash {
    Hash([u8; 32]),
    Missing,
    Other(Value),
}

#[derive(Clone, Debug)]
enum AnswerBody {
    /// Puts its input into `Answered`, as the lowered `Answer` does.
    Answered,
    Prog(BodyProg),
    Effectful(Receiver),
}

fn hand_gen() -> impl Strategy<Value = HandGen> {
    let ms = 1u64..5_000;
    (
        prop_oneof![
            4 => any::<[u8; 32]>().prop_map(RowHash::Hash),
            1 => Just(RowHash::Missing),
            1 => value().prop_map(RowHash::Other),
        ],
        any::<bool>(),
        bind_prog(),
        prop_oneof![
            1 => Just(None),
            1 => select(vec![ANSWER_ID_KEY, SEEN_FIELD, ANSWER_INPUT_KEY]).prop_map(Some),
        ],
        prop_oneof![
            2 => Just(AnswerBody::Answered),
            1 => body_prog().prop_map(AnswerBody::Prog),
            2 => prop_oneof![
                Just(Receiver::Keyed),
                Just(Receiver::StateProbe),
                Just(Receiver::None),
            ]
            .prop_map(AnswerBody::Effectful),
        ],
        any::<[bool; 3]>(),
        prop::option::of(ms.clone()),
        prop::option::of((1u32..4, ms)),
    )
        .prop_map(
            |(row, gated, bind, planted, body, inhibits, run_timer, retry)| HandGen {
                row,
                gated,
                bind,
                planted,
                body,
                inhibits,
                run_timer,
                retry,
            },
        )
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
        prop_oneof![2 => Just(None), 1 => hand_gen().prop_map(Some)],
    )
        .prop_map(
            |(roles, puts, effect, input, irreversible, undoes, bind, body, hand)| WorkGen {
                roles,
                puts,
                effect,
                input,
                irreversible,
                undoes,
                bind,
                body,
                hand,
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
        )
            .prop_map(move |(transitions, derived, works)| NetGen {
                places: places.clone(),
                transitions,
                derived,
                works,
            })
    })
}

fn receiver(r: Receiver) -> holon_net::exec::Receiver {
    match r {
        Receiver::Keyed => holon_net::exec::Receiver::Keyed,
        Receiver::StateProbe => holon_net::exec::Receiver::StateProbe,
        Receiver::None => holon_net::exec::Receiver::None,
    }
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

impl NetGen {
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

    /// A hand-built ask whose `Asks` row carries no valid hash text.
    fn has_bad_row_hash(&self) -> bool {
        self.works.iter().any(|w| {
            w.hand
                .as_ref()
                .is_some_and(|h| !matches!(h.row, RowHash::Hash(_)))
        })
    }

    /// The stored, derived and lowered places.
    fn readable(&self) -> Vec<String> {
        [self.stored(), self.derived_names(), self.work_places()].concat()
    }

    /// The places a binding of a file transition or a work reads by name:
    /// also the `Asks` rows, whose derive runs a work's binding.
    fn transition_readable(&self) -> Vec<String> {
        [self.readable(), self.asks_places()].concat()
    }

    /// The executor's own places of every transition, and `OutcomeUnknown`.
    fn executor_places(&self) -> Vec<String> {
        let mut places: Vec<String> = self
            .transition_names()
            .iter()
            .flat_map(|t| {
                [
                    Place::running(t),
                    Place::retry_pending(t),
                    Place::outcome_unknown(t),
                    Place::undo_pending(t),
                ]
                .map(|p| p.to_string())
            })
            .collect();
        places.push(OUTCOME_UNKNOWN.to_owned());
        places
    }

    fn answered_places(&self) -> Vec<String> {
        self.work_names()
            .iter()
            .map(|w| Generated::Answered.name(w))
            .collect()
    }

    /// The places only the executor writes: a write or retract from outside
    /// is refused.
    fn executor_owned(&self, place: &str) -> bool {
        self.executor_places().iter().any(|p| p == place)
    }

    /// The places a caller writes and retracts by name.
    fn writable(&self) -> Vec<String> {
        [
            self.stored(),
            self.derived_names(),
            self.asks_places(),
            self.work_places(),
            self.executor_places(),
        ]
        .concat()
    }

    fn pools(&self) -> Vec<(String, u32)> {
        self.stored()
            .into_iter()
            .zip(&self.places)
            .filter_map(|(p, g)| Some((p, g.pool?)))
            .collect()
    }

    /// A read of each derived place and `Asks` row: the derived places and
    /// `Asks` rows it evaluates, and whether the load sees it. An `Asks` row
    /// runs its work's binding only while the work's input arcs hold; on the
    /// initial marking a take or read arc never holds and an inhibitor arc on
    /// a pool, which starts full, never does either.
    fn derive_edges(&self) -> Vec<(String, bool, Vec<String>)> {
        fn evaluated(reads: &[ReadOp], readable: &[String], nodes: &[String]) -> Vec<String> {
            reads
                .iter()
                .filter(|r| !r.is_free())
                .map(|r| r.place().of(readable))
                .filter(|p| nodes.contains(p))
                .collect()
        }
        let nodes = [self.derived_names(), self.asks_places()].concat();
        let mut edges: Vec<(String, bool, Vec<String>)> = self
            .derived
            .iter()
            .enumerate()
            .map(|(i, d)| {
                (
                    DERIVED[i].to_owned(),
                    true,
                    evaluated(&d.reads, &self.transition_readable(), &nodes),
                )
            })
            .collect();
        for (k, w) in self.works.iter().enumerate() {
            let always = !w.roles.iter().zip(&self.places).any(|(r, p)| {
                matches!(r, Role::Consumes | Role::Reads)
                    || (matches!(r, Role::Inhibits) && p.pool.is_some())
            });
            edges.push((
                Generated::Asks.name(WORKS[k]),
                always,
                evaluated(&w.bind.reads, &self.transition_readable(), &nodes),
            ));
        }
        edges
    }

    /// The derived places and `Asks` rows from which a cycle of evaluations
    /// is reachable; with `always`, only through reads that never depend on
    /// the marking, so a load that evaluates each derive once sees them.
    fn cyclic(&self, always: bool) -> BTreeSet<String> {
        let edges = self.derive_edges();
        let next = |n: &str| -> Vec<String> {
            edges
                .iter()
                .find(|(m, a, _)| m == n && (*a || !always))
                .map(|(_, _, to)| to.clone())
                .unwrap_or_default()
        };
        let reach = |n: &str| -> BTreeSet<String> {
            let mut seen = BTreeSet::new();
            let mut todo = next(n);
            while let Some(m) = todo.pop() {
                if seen.insert(m.clone()) {
                    todo.extend(next(&m));
                }
            }
            seen
        };
        let on_cycle = |n: &str| reach(n).contains(n);
        edges
            .iter()
            .map(|(n, ..)| n.clone())
            .filter(|n| on_cycle(n) || reach(n).iter().any(|m| on_cycle(m)))
            .collect()
    }

    fn file_transitions(&self) -> Vec<String> {
        (0..self.transitions.len())
            .map(|i| TRANSITIONS[i].to_owned())
            .collect()
    }

    /// The last `now` a call accepts: the end of the i64 nanosecond range
    /// minus the net's longest timer, so no timer started then ends past it.
    fn last_now(&self) -> Duration {
        let longest = self
            .transitions
            .iter()
            .flat_map(|t| {
                let (run, abandon, retry) = t.timers.unwrap_or((None, None, None));
                let keyed_backoff =
                    (t.effect == Some(Receiver::Keyed) && retry.is_none()).then_some(10);
                [run, abandon, retry.map(|r| r.1), keyed_backoff]
            })
            .chain(
                self.works
                    .iter()
                    .filter_map(|w| w.hand.as_ref())
                    .flat_map(|h| {
                        let keyed_backoff =
                            (matches!(h.body, AnswerBody::Effectful(Receiver::Keyed))
                                && h.retry.is_none())
                            .then_some(10);
                        [h.run_timer, h.retry.map(|r| r.1), keyed_backoff]
                    }),
            )
            .flatten()
            .max()
            .unwrap_or(0);
        Duration::from_nanos(I64_NS) - Duration::from_millis(longest)
    }

    /// Every transition the net holds, the lowering's included.
    fn transition_names(&self) -> Vec<String> {
        let mut names = self.file_transitions();
        for (w, work) in self.work_names().into_iter().zip(&self.works) {
            let generated: &[Generated] = match work.hand {
                Some(_) => &[Generated::Answer],
                None => &[
                    Generated::Answer,
                    Generated::ExpireAnswer,
                    Generated::ReleaseFailed,
                    Generated::ClearFailed,
                ],
            };
            for g in generated {
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
                        on_failure: t.on_failure.map(|p| names[p].clone()),
                        on_timeout: t.on_timeout.map(|p| names[p].clone()),
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

    /// The places a body or a result puts into by index: every place the
    /// net declares, derived places and `Asks` rows included, but no single
    /// place that is a fact of a file transition: a fact into a held single
    /// place panics, which is out of this card's scope.
    fn output_places(&self) -> Vec<String> {
        let facts: BTreeSet<usize> = self
            .transitions
            .iter()
            .flat_map(|t| t.facts.iter().copied())
            .collect();
        let stored = self
            .stored()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| {
                self.places[*i].capacity != PlaceCapacity::Single || !facts.contains(i)
            })
            .map(|(_, p)| p);
        stored
            .chain(self.derived_names())
            .chain(self.asks_places())
            .chain(self.work_places())
            .collect()
    }

    /// Every place name a body of the net puts into.
    fn body_outputs(&self) -> BTreeSet<String> {
        let places = self.output_places();
        self.transitions
            .iter()
            .map(|t| &t.body)
            .chain(self.works.iter().map(|w| &w.body))
            .flat_map(|b| match b {
                BodyProg::Outputs(outs) => outs.iter().map(|o| o.place.of(&places)).collect(),
                BodyProg::Fails { .. } => vec![],
            })
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
            bind: self.bind_fn(&w.bind, self.transition_readable()),
            body: match w.effect {
                Some(r) => NativeBody::Effectful(receiver(r)),
                None => NativeBody::Pure(w.body.body(WORKS[i], &self.output_places())),
            },
            preview: None,
        }
    }

    /// Work `i` as an ask built by hand: the places and transitions the load
    /// checks an ask for, with the drawn `Asks` row and `Answer`.
    fn hand(&self, net: NetDef, i: usize, h: &HandGen) -> NetDef {
        let w = &self.works[i];
        let t = WORKS[i];
        let names = self.stored();
        let (answered, failed) = (answered_place(t), Generated::WorkFailed.name(t));
        let running = Place::running(t).to_string();
        let arcs = |role: Role| -> Vec<String> {
            w.roles
                .iter()
                .zip(&names)
                .filter(|(r, _)| **r == role)
                .map(|(_, p)| p.clone())
                .collect()
        };
        let needs = [arcs(Role::Consumes), arcs(Role::Reads)].concat();
        let blocks = arcs(Role::Inhibits);
        let busy = [
            answered.clone(),
            running.clone(),
            Place::retry_pending(t).to_string(),
            Place::outcome_unknown(t).to_string(),
            failed.clone(),
        ];
        let row = match &h.row {
            RowHash::Hash(b) => Some(Value::String(hash_text(&AskHash(*b)))),
            RowHash::Missing => None,
            RowHash::Other(v) => Some(v.clone()),
        };
        let situation = self.bind_fn(&w.bind, self.transition_readable());
        let asks: holon_net::exec::Read = Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            if busy.iter().chain(&blocks).any(|p| r.present(p, s))
                || needs.iter().any(|p| !r.present(p, s))
            {
                return None;
            }
            situation(r, s)?;
            Some(
                row.iter()
                    .map(|v| (HASH_FIELD.to_owned(), v.clone()))
                    .collect(),
            )
        });

        let answer_bind: holon_net::exec::Read = {
            let (prog, readable) = (h.bind.clone(), self.transition_readable());
            let (gate, gated, planted) = (asks_place(t), h.gated, h.planted);
            Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
                for op in &prog.reads {
                    op.run(r, s, &readable);
                }
                if gated && !r.present(&gate, s) {
                    return None;
                }
                let mut bound: Content = prog.out.as_ref()?.iter().cloned().collect();
                if let Some(k) = planted {
                    bound.insert(k.to_owned(), Value::String("planted".to_owned()));
                }
                Some(bound)
            })
        };
        let answer_body = match &h.body {
            AnswerBody::Answered => {
                let answered = answered.clone();
                Body::pure(move |s, input| Ok(vec![Output::new(&answered, s, input.clone())]))
            }
            AnswerBody::Prog(b) => Body::Pure(b.body(&answer_transition(t), &self.output_places())),
            AnswerBody::Effectful(r) => Body::Effectful(receiver(*r)),
        };
        let mut answer = TransitionDef::new(
            &answer_transition(t),
            holon_net::exec::Trigger::Manual,
            answer_bind,
            answer_body,
        );
        for (p, on) in [&answered, &running, &failed].into_iter().zip(h.inhibits) {
            if on {
                answer = answer.inhibits(p);
            }
        }
        answer.run_timer = h.run_timer.map(Duration::from_millis);
        answer.retry = h.retry.map(|(max_attempts, ms)| Retry {
            backoff: Duration::from_millis(ms),
            max_attempts,
        });

        let body = match w.effect {
            Some(r) => Body::Effectful(receiver(r)),
            None => Body::Pure(w.body.body(t, &self.output_places())),
        };
        let mut work = TransitionDef::new(
            t,
            holon_net::exec::Trigger::Auto,
            self.bind_fn(&w.bind, self.transition_readable()),
            body,
        )
        .consumes(&answered)
        .on_failure(&failed);
        for p in arcs(Role::Consumes) {
            work = work.consumes(&p);
        }
        for p in arcs(Role::Reads) {
            work = work.reads(&p);
        }
        for p in arcs(Role::Inhibits) {
            work = work.inhibits(&p);
        }
        for p in &w.puts {
            work = work.produces(&names[*p]);
        }

        net.place(&answered, Class::Process)
            .place(&failed, Class::Process)
            .place(&expired_place(t), Class::Process)
            .derived(&asks_place(t), asks)
            .ask(
                t,
                AskMeta {
                    input: w.input.clone(),
                    irreversible: w.irreversible,
                    has_preview: false,
                },
            )
            .transition(answer)
            .transition(work)
    }

    /// The net: the file, its derived places, then each lowered work.
    /// `Err` is a refusal of the loader or the lowering.
    fn net(&self) -> Result<NetDef, String> {
        let yaml = self.file().to_yaml();
        let mut net = net_def("nets/gen.yaml", &yaml, |t| self.logic(t))
            .map_err(|e| format!("net_def: {e}"))?;
        for (i, d) in self.derived.iter().enumerate() {
            net = net.derived(DERIVED[i], self.bind_fn(d, self.transition_readable()));
        }
        for (i, w) in self.works.iter().enumerate() {
            net = match &w.hand {
                Some(h) => self.hand(net, i, h),
                None => lower(net, &self.descriptor(i), Some(self.parts(i)))
                    .map_err(|e| format!("lower: {e}"))?,
            };
        }
        Ok(net)
    }

    fn executor(&self) -> Result<Executor, String> {
        Executor::new(self.net()?).map_err(|e| format!("Executor::new: {e}"))
    }

    /// Whether a read of `place` reaches a place the net does not declare.
    fn ghost(&self, place: &str) -> bool {
        self.ghost_from(place, &mut BTreeSet::new())
    }

    /// `ghost`, where `visiting` holds the derived places on the way, so a
    /// cycle of derived places ends.
    fn ghost_from(&self, place: &str, visiting: &mut BTreeSet<String>) -> bool {
        if let Some(d) = DERIVED.iter().position(|d| *d == place)
            && d < self.derived.len()
        {
            if !visiting.insert(place.to_owned()) {
                return false;
            }
            let readable = self.transition_readable();
            return self.derived[d].reads.iter().any(|r| {
                let p = r.place().of(&readable);
                if r.is_free() {
                    !self.declares(&p)
                } else {
                    self.ghost_from(&p, visiting)
                }
            });
        }
        // Whether an `Asks` row runs its work's binding depends on the work's
        // marking, which the model does not keep, so such a read is no
        // certain ghost.
        !self.declares(place)
    }

    /// Whether a derive of `place` can run the binding of a work through an
    /// `Asks` row it reads, directly or through other derived places.
    fn reaches_asks(&self, place: &str, visiting: &mut BTreeSet<String>) -> bool {
        if self.asks_places().iter().any(|a| a == place) {
            return true;
        }
        let Some(d) = DERIVED.iter().position(|d| *d == place) else {
            return false;
        };
        if d >= self.derived.len() || !visiting.insert(place.to_owned()) {
            return false;
        }
        let readable = self.transition_readable();
        self.derived[d]
            .reads
            .iter()
            .any(|r| !r.is_free() && self.reaches_asks(&r.place().of(&readable), visiting))
    }

    fn read_is_ghost(&self, r: &ReadOp, readable: &[String]) -> bool {
        let p = r.place().of(readable);
        // `free` counts tokens and never runs a derived place.
        if r.is_free() {
            !self.declares(&p)
        } else {
            self.ghost(&p)
        }
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
        let readable = self.transition_readable();
        self.bind_of(transition)
            .is_some_and(|b| b.reads.iter().any(|r| self.read_is_ghost(r, &readable)))
    }

    /// Whether `transition` is a work whose ask the lowering builds.
    fn is_lowered(&self, transition: &str) -> bool {
        WORKS
            .iter()
            .zip(&self.works)
            .any(|(w, g)| *w == transition && g.hand.is_none())
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

    /// Whether `transition` is a lowered work whose binding produces a field
    /// of the `Answered` token the lowering adds to the work's input.
    fn binds_answer_fields(&self, transition: &str) -> bool {
        WORKS
            .iter()
            .position(|t| *t == transition)
            .is_some_and(|i| {
                self.works.get(i).is_some_and(|w| {
                    w.hand.is_none()
                        && w.bind.out.as_ref().is_some_and(|f| {
                            f.iter().any(|(k, _)| k == ANSWER_ID_KEY || k == SEEN_FIELD)
                        })
                })
            })
    }

    /// The transitions whose firing a caller must never see.
    fn ghost_transitions(&self) -> BTreeSet<String> {
        [self.file_transitions(), self.work_names()]
            .concat()
            .into_iter()
            .filter(|t| {
                self.reads_undeclared(t)
                    || self.binds_answer_input(t)
                    || self.binds_answer_fields(t)
            })
            .collect()
    }
}

// ------------------------------------------------------------- observing

/// The last `now` whose nanoseconds fit an i64.
const I64_NS: u64 = i64::MAX as u64;

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
            out += &format!("{p}/{s}: {:?}\n", exec.tokens().contents(&p, &s));
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
            for c in exec.tokens().contents(&p, &s) {
                if let Some(Value::String(t)) = c.get(BY) {
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
    /// A `now` past the i64 nanosecond range, up to `Duration::MAX`.
    Far(Duration),
    /// A `now` this many nanoseconds before the end of the i64 range, so a
    /// timer from it ends past the range.
    Edge(u64),
}

fn far() -> impl Strategy<Value = Duration> {
    prop_oneof![
        (1u64..1_000).prop_map(|d| Duration::from_nanos(I64_NS) + Duration::from_nanos(d)),
        (I64_NS / 1_000_000_000 + 1..=u64::MAX, 0u32..1_000_000_000)
            .prop_map(|(s, n)| Duration::new(s, n)),
        Just(Duration::MAX),
    ]
}

/// One in eight is a `now` past or at the end of the i64 range.
fn dt() -> impl Strategy<Value = Dt> {
    prop_oneof![
        2 => (1u64..=u64::MAX).prop_map(Dt::Back),
        2 => (1u64..5_000_000_000).prop_map(Dt::Back),
        4 => Just(Dt::Same),
        6 => (1u64..10_000_000_000).prop_map(Dt::Forward),
        1 => far().prop_map(Dt::Far),
        1 => (0u64..10_000_000_000).prop_map(Dt::Edge),
    ]
}

/// No `Back` from a clock at zero: a `now` before zero does not exist.
fn now(clock: Duration, dt: &Dt) -> Duration {
    match dt {
        Dt::Back(d) => clock.saturating_sub(Duration::from_nanos(*d)),
        Dt::Same => clock,
        Dt::Forward(d) => clock.saturating_add(Duration::from_nanos(*d)),
        Dt::Far(d) => *d,
        Dt::Edge(d) => Duration::from_nanos(I64_NS - d),
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
    Answer {
        work: Name,
        subject: String,
        seen: SeenGen,
        answer: OccGen,
        value: Option<Value>,
        dt: Dt,
    },
    AnswerSettlement {
        answer: Pick,
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
            dt()
        )
            .prop_map(|(work, subject, seen, answer, value, dt)| Op::Answer {
                work,
                subject,
                seen,
                answer,
                value,
                dt
            }),
        1 => pick().prop_map(|answer| Op::AnswerSettlement { answer }),
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
    exec: Executor,
    /// The largest `now` of every call not refused for the clock.
    clock: Duration,
    requests: Vec<EffectRequest>,
    occurrences: Vec<OccurrenceId>,
    fresh: u64,
    /// Every place name a result put into.
    outputs: BTreeSet<String>,
}

impl Machine {
    fn new(net: NetGen, exec: Executor) -> Self {
        Machine {
            net,
            exec,
            clock: Duration::ZERO,
            requests: vec![],
            occurrences: vec![],
            fresh: 0,
            outputs: BTreeSet::new(),
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
        self.requests
            .extend(requests.into_iter().flatten().cloned());
    }

    /// A call before the clock is refused for the clock and changes
    /// nothing; any other call is not refused for the clock. A call past the
    /// net's last `now` is refused as out of range and changes nothing; any
    /// other call's `now` becomes the clock. Returns whether the call was
    /// refused for its `now`.
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
        let last = self.net.last_now();
        if now > last {
            prop_assert!(
                names_error(r, "ClockOutOfRange"),
                "{what} at {now:?}, past the last now {last:?}, was not refused as out of range: {r:?}"
            );
            let after = snapshot(&self.exec, &self.net);
            prop_assert_eq!(
                before,
                after.as_str(),
                "{} past the last now changed the executor",
                what
            );
            return Ok(true);
        }
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

    /// A refused write or retract changes nothing, unless the call
    /// committed and only the run to quiescence failed.
    fn check_unchanged_if_refused<T: Debug, E: Debug>(
        &self,
        what: &str,
        before: &str,
        r: &Result<T, E>,
    ) -> Result<(), TestCaseError> {
        if r.is_err() && !names_error(r, "NoQuiescence") {
            let after = snapshot(&self.exec, &self.net);
            prop_assert_eq!(
                before,
                after.as_str(),
                "the refused {} changed the executor: {:?}",
                what,
                r
            );
        }
        Ok(())
    }

    /// No call puts a token into a place the net does not hold as stored,
    /// and no pool holds more free tokens than its size.
    fn check_marking(&self) -> Result<(), TestCaseError> {
        let stored = all_places(&self.net);
        for p in self.outputs.iter().chain(&self.net.body_outputs()) {
            if !stored.contains(p) {
                let held = self.exec.tokens().subjects(p);
                prop_assert!(
                    held.is_empty(),
                    "the place {p:?}, which the net does not hold as stored, holds tokens of {held:?}"
                );
            }
        }
        for (p, n) in self.net.pools() {
            let free = self.exec.tokens().free(&p);
            prop_assert!(
                free <= n as usize,
                "the pool {p} of {n} tokens holds {free} free tokens"
            );
        }
        Ok(())
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
                let now = now(self.clock, dt);
                let content: Content = content.iter().cloned().collect();
                let content_of = content.clone();
                let exec = &mut self.exec;
                let r = no_panic(&format!("write({place:?}, {subject:?}, {now:?})"), || {
                    exec.write(&place, subject, content, now)
                })?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                if !self.check_clock("write", now, &before, &r)? {
                    self.check_place_refusal("write", &place, &before, &r)?;
                    let unanswered = self.net.answered_places().contains(&place)
                        && [ANSWER_ID_KEY, SEEN_FIELD]
                            .iter()
                            .any(|k| !matches!(content_of.get(*k), Some(Value::String(_))));
                    if self.net.executor_owned(&place) || unanswered {
                        self.check_refused(
                            &format!("write of the executor's {place:?}"),
                            "",
                            &before,
                            &r,
                        )?;
                    }
                    self.check_unchanged_if_refused("write", &before, &r)?;
                }
            }
            Op::Retract { place, subject, dt } => {
                let place = place.of(&writable);
                let declared = self.net.work_places().contains(&place) || stored.contains(&place);
                // A pool's tokens have no subject: a retract takes any of them.
                let held = if self.net.pools().iter().any(|(p, _)| *p == place) {
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
                    self.check_unchanged_if_refused("retract", &before, &r)?;
                    if self.net.executor_owned(&place) {
                        self.check_refused(
                            &format!("retract of the executor's {place:?}"),
                            "",
                            &before,
                            &r,
                        )?;
                    } else if declared && !held {
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
                    ResultGen::Done(outs) => {
                        let outs = outputs(outs, &self.net.output_places(), "s", None);
                        self.outputs
                            .extend(outs.iter().map(|o| o.place.to_string()));
                        EffectResult::Done(outs)
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
                    ResolutionGen::Verify(outs) => {
                        let outs = outputs(outs, &self.net.output_places(), "s", None);
                        self.outputs
                            .extend(outs.iter().map(|o| o.place.to_string()));
                        Resolution::Verify(outs)
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
                dt,
            } => {
                let work = work.of(&self.net.work_names());
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
                let now = now(self.clock, dt);
                let exec = &mut self.exec;
                let r = no_panic(
                    &format!("answer({work:?}, {subject:?}, {id:?}, {now:?})"),
                    || exec.answer(call, now),
                )?;
                self.take_requests(r.as_ref().ok().map(|q| &q.requests));
                let lowered = self.net.is_lowered(&work);
                // A hand-built `Answer` has its own binding, which does not
                // read what the work's binding reads.
                if !self.check_clock("answer", now, &before, &r)?
                    && lowered
                    && self.net.reads_undeclared(&work)
                {
                    self.check_refused(
                        &format!("answer to {work:?}, whose binding reads an undeclared place"),
                        "",
                        &before,
                        &r,
                    )?;
                }
                // An answer to a changed situation expires instead of putting
                // `Answered`, so no round refuses the work. A hand-built
                // `Answer` may put no `Answered` at all.
                let expired = self
                    .exec
                    .tokens()
                    .subjects(&Generated::Expired.name(&work))
                    .iter()
                    .any(|t| t.as_str() == id);
                if r.is_ok() && !expired && lowered && self.net.binds_answer_input(&work) {
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
            Op::AnswerSettlement { answer } => {
                let id = match self.picked_occurrence(answer) {
                    OccurrenceId::Caller(id) => id.to_string(),
                    other => other.to_string(),
                };
                let exec = &self.exec;
                let _ = no_panic(&format!("answer_settlement({id:?})"), || {
                    format!("{:?}", exec.answer_settlement(&AnswerId(id.clone())))
                })?;
                let after = snapshot(&self.exec, &self.net);
                prop_assert_eq!(&before, &after, "answer_settlement changed the executor");
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
                // L7 refuses the firing of a work that binds the answer input, not
                // its row.
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
                        let cyclic = self.net.cyclic(false);
                        prop_assert!(
                            reads && e.contains("ReadsUndeclared")
                                || self.net.has_bad_row_hash() && e.contains("AskRowHash")
                                || self.net.asks_places().iter().any(|a| cyclic.contains(a)),
                            "asks was refused as {e}, though no work's binding reads an undeclared place and no Asks row lacks a hash"
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
        self.check_marking()?;
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
        // The form of a derive that meets a cycle only a marking reveals is
        // the implementer's contract; it is never a panic or an abort.
        if self.net.cyclic(false).contains(place) {
            return Ok(());
        }
        if self.net.derived_names().iter().any(|d| d == place) {
            if self.net.ghost(place) && evaluated {
                prop_assert!(
                    refused_as("ReadsUndeclared"),
                    "derive of {place:?}, which reads an undeclared place, answered {answer:?}"
                );
            } else if self.net.reaches_asks(place, &mut BTreeSet::new()) {
                // The Asks row runs its work's binding, which may read an
                // undeclared place; the model keeps no work marking.
                prop_assert!(
                    matches!(answer, Answer::Known(_)) || refused_as("ReadsUndeclared"),
                    "derive of {place:?} was refused: {answer:?}"
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
            if net.binds_answer_fields(transition) {
                prop_assert!(
                    w.is_some(),
                    "why_not of {transition:?}, whose binding produces an answer field, reads enabled"
                );
            }
        }
    }
    Ok(())
}

/// The executor of `net`, or `None` when the loader or the lowering refused
/// it. A refusal is an error a caller sees; it must not be a panic.
fn load(net: &NetGen) -> Result<Option<Executor>, TestCaseError> {
    Ok(no_panic("load", || net.executor())?.ok())
}

/// No call panics, a call before the clock or past the i64 nanosecond range
/// is refused and changes nothing, a name the net does not hold is refused, a
/// write into the executor's own places is refused, no pool exceeds its size,
/// no token lands in a place the net does not hold, a binding that reads an
/// undeclared place never fires, and a net whose derived places always
/// evaluate each other in a cycle does not load.
fn caller_calls_keep_the_contract(net: NetGen, ops: &[Op]) -> Result<(), TestCaseError> {
    let cycle = net.cyclic(true);
    let loaded = load(&net)?;
    if !cycle.is_empty() {
        prop_assert!(
            loaded.is_none(),
            "the net loaded although {cycle:?} evaluate each other in a cycle on every marking"
        );
        return Ok(());
    }
    let Some(exec) = loaded else { return Ok(()) };
    let mut m = Machine::new(net, exec);
    for op in ops {
        m.apply(op)?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, max_shrink_time: 60_000, ..ProptestConfig::default() })]

    /// Any sequence of public calls on any net the format allows, the asks
    /// lowered or built by hand, keeps the caller contract.
    #[test]
    fn no_sequence_of_caller_calls_panics_on_any_net(
        net in net_gen(),
        ops in prop::collection::vec(op(), 1..24),
    ) {
        caller_calls_keep_the_contract(net, &ops)?;
    }
}

/// The smallest net with one hand-built ask `W0`, offered while `P0` holds a
/// token.
fn hand_net(h: HandGen) -> NetGen {
    NetGen {
        places: vec![PlaceGen {
            capacity: PlaceCapacity::Fifo,
            pool: None,
        }],
        transitions: vec![TransGen {
            trigger: Trigger::Manual,
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
            roles: vec![Role::Reads],
            puts: vec![],
            effect: None,
            input: None,
            irreversible: false,
            undoes: None,
            bind: BindProg {
                reads: vec![],
                out: Some(vec![]),
            },
            body: BodyProg::Outputs(vec![]),
            hand: Some(h),
        }],
    }
}

fn plain_hand() -> HandGen {
    HandGen {
        row: RowHash::Hash([7; 32]),
        gated: true,
        bind: BindProg {
            reads: vec![],
            out: Some(vec![]),
        },
        planted: None,
        body: AnswerBody::Answered,
        inhibits: [true, false, false],
        run_timer: None,
        retry: None,
    }
}

fn write_p0() -> Op {
    Op::Write {
        place: Name::Other(PLACES[0].to_owned()),
        subject: "s".to_owned(),
        content: vec![],
        dt: Dt::Same,
    }
}

fn answer_w0(answer: OccGen) -> Op {
    Op::Answer {
        work: Name::Other(WORKS[0].to_owned()),
        subject: "s".to_owned(),
        seen: SeenGen::Offered(0),
        answer,
        value: Some(Value::Integer(1)),
        dt: Dt::Forward(1_000_000),
    }
}

fn complete(result: ResultGen) -> Op {
    Op::Complete {
        attempt: Pick::Known(0),
        result,
        dt: Dt::Forward(1_000_000),
    }
}

/// Hand-built asks the wide machine draws only rarely: an `Answer` binding
/// that returns an answer key, an effectful `Answer` answered again while it
/// runs, after a retryable failure and after a definite one, and `Asks` rows
/// without a hash text. Each keeps the caller contract.
#[test]
fn a_hand_built_answer_is_refused_at_load_or_answered_without_panic() {
    let effectful = |r: Receiver, retry: Option<(u32, u64)>| HandGen {
        body: AnswerBody::Effectful(r),
        inhibits: [false; 3],
        retry,
        ..plain_hand()
    };
    let again = [
        write_p0(),
        answer_w0(OccGen::Fresh),
        answer_w0(OccGen::Fresh),
        Op::Step {
            dt: Dt::Forward(1_000_000),
        },
    ];
    let failed = |retryable: bool| {
        vec![
            write_p0(),
            answer_w0(OccGen::Fresh),
            complete(ResultGen::Failed(retryable)),
            answer_w0(OccGen::Fresh),
            answer_w0(OccGen::Reuse(0)),
        ]
    };
    let mut cases: Vec<(String, HandGen, Vec<Op>)> = vec![];
    for key in [ANSWER_ID_KEY, SEEN_FIELD, ANSWER_INPUT_KEY] {
        for gated in [true, false] {
            cases.push((
                format!("Answer binds {key:?}, gated {gated}"),
                HandGen {
                    planted: Some(key),
                    gated,
                    ..plain_hand()
                },
                again.to_vec(),
            ));
        }
    }
    for r in [Receiver::None, Receiver::StateProbe, Receiver::Keyed] {
        cases.push((
            format!("effectful {r:?} Answer answered twice"),
            effectful(r, None),
            again.to_vec(),
        ));
        for retryable in [true, false] {
            cases.push((
                format!("effectful {r:?} Answer answered after a failure, retryable {retryable}"),
                effectful(r, Some((3, 50))),
                failed(retryable),
            ));
            cases.push((
                format!("effectful {r:?} Answer without retry answered after a failure, retryable {retryable}"),
                effectful(r, None),
                failed(retryable),
            ));
        }
    }
    for retryable in [true, false] {
        cases.push((
            format!("pure Answer that fails, retryable {retryable}, answered again"),
            HandGen {
                body: AnswerBody::Prog(BodyProg::Fails { retryable }),
                inhibits: [false; 3],
                retry: Some((3, 50)),
                ..plain_hand()
            },
            again.to_vec(),
        ));
    }
    for row in [
        RowHash::Missing,
        RowHash::Other(Value::Integer(1)),
        RowHash::Other(Value::String("no hash".to_owned())),
    ] {
        cases.push((
            format!("Asks row hash {row:?}"),
            HandGen {
                row,
                ..plain_hand()
            },
            vec![write_p0(), Op::Asks, answer_w0(OccGen::Fresh)],
        ));
    }
    let failures: Vec<String> = cases
        .into_iter()
        .filter_map(|(name, h, ops)| {
            caller_calls_keep_the_contract(hand_net(h), &ops)
                .err()
                .map(|e| format!("{name}: {e}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ------------------------------------------------------ one defect each

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// A binding that reads a place the net does not declare, directly,
    /// through `traced` or through a derived place: no panic, its transition
    /// never fires and `why_not` names the place; an auto transition is
    /// disclosed once in the conditions, a manual fire is refused naming the
    /// place. The same binding over declared places fires.
    #[test]
    fn a_bind_reading_an_undeclared_place_loads_and_never_fires(
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
        t0.body = BodyProg::Outputs(vec![OutGen { place: Name::Other(PLACES[last].to_owned()), subject: Subj::Own, fields: vec![] }]);
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

// ------------------------------------------------------ ask hash text

fn hex_digit(c: char) -> Option<u8> {
    c.to_digit(16).map(|d| d as u8)
}

/// Texts of every shape a caller can pass: arbitrary strings, 64 hex
/// digits in either case, and 64 characters with one non-hex character.
fn hash_text_input() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        prop::collection::vec(any::<u8>(), 0..80)
            .prop_map(|b| b.iter().map(|x| format!("{x:02x}")).collect()),
        "[0-9a-fA-F]{64}",
        ("[0-9a-fA-F]{64}", 0usize..64, any::<char>()).prop_map(|(mut s, at, c)| {
            s.replace_range(at..=at, &c.to_string());
            s
        }),
    ]
}

proptest! {
    /// Any text is parsed to the hash it spells or refused; it never panics.
    #[test]
    fn an_ask_hash_text_parses_or_is_refused(text in hash_text_input()) {
        let expected: Option<Vec<u8>> = (text.chars().count() == 64)
            .then(|| {
                let digits: Option<Vec<u8>> = text.chars().map(hex_digit).collect();
                digits.map(|d| d.chunks(2).map(|p| p[0] << 4 | p[1]).collect())
            })
            .flatten();
        let parsed = std::panic::catch_unwind(|| holon_net::exec::hash_from_text(&text))
            .map_err(|_| TestCaseError::fail(format!("hash_from_text panicked on {text:?}")))?;
        match (parsed, expected) {
            (Ok(h), Some(bytes)) => prop_assert_eq!(h.0.to_vec(), bytes),
            (Err(_), None) => {}
            (Ok(h), None) => prop_assert!(false, "{text:?} parsed to {:?}", h.0),
            (Err(e), Some(_)) => prop_assert!(false, "{text:?} refused: {e}"),
        }
    }
}
