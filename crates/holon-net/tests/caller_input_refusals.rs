//! One property per caller input that must come back as an error or a
//! defined outcome, never a panic or an abort: small explicit nets, with the
//! subjects, names, sizes, contents and times drawn. Each refusal has an
//! exclusion beside it: the same shape with a valid input is accepted.
//!
//! The error forms are matched by `is_err` and an unchanged marking, or by a
//! `Debug` substring where the form is fixed, so the properties compile on an
//! executor that has none of the new forms yet.

use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use holon_api::RemovedTag;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::OUTCOME_UNKNOWN;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::SEEN_FIELD;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::file::CommandSpec;
use holon_net::file::DurationSpec;
use holon_net::file::EffectSpec;
use holon_net::file::ExitCodeMap;
use holon_net::file::NetFile;
use holon_net::file::Outcome;
use holon_net::file::PlaceCapacity;
use holon_net::file::PlaceSpec;
use holon_net::file::ShellEffect;
use holon_net::file::TransitionSpec;
use holon_net::load::Logic;
use holon_net::load::net_def;
use holon_net::uft::Generated;
use holon_net::uft::NativeBody;
use holon_net::uft::NativeParts;
use holon_net::uft::lower;
use holon_transition_contract::ANSWER_ID_KEY;
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

const I64_NS: u64 = i64::MAX as u64;
const W: &str = "W";
const BY: &str = "by";
/// The nesting of derived rows a derive may reach.
const MAX_NESTED_DERIVED_ROWS: usize = 128;

// ------------------------------------------------------------- observing

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

fn names_error<T: Debug, E: Debug>(r: &Result<T, E>, form: &str) -> bool {
    matches!(r, Err(e) if format!("{e:?}").contains(form))
}

/// The stored places `stored`, the executor's places of `transitions`, and
/// `OutcomeUnknown`.
fn places(stored: &[&str], transitions: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = stored.iter().map(|p| (*p).to_owned()).collect();
    for t in transitions {
        for p in [
            Place::running(t),
            Place::retry_pending(t),
            Place::outcome_unknown(t),
            Place::undo_pending(t),
        ] {
            all.push(p.to_string());
        }
    }
    all.push(OUTCOME_UNKNOWN.to_owned());
    all
}

/// Everything a caller reads of the executor except its clock, as text so a
/// NaN compares equal to itself.
fn snapshot(exec: &Executor, places: &[String]) -> String {
    let mut out = String::new();
    for p in places {
        for s in exec.tokens().subjects(p) {
            out += &format!("{p}/{s}: {:?}\n", exec.tokens().contents(p, &s));
        }
    }
    out += &format!("marks {:?}\n", exec.marks().collect::<Vec<_>>());
    out += &format!("retrying {:?}\n", exec.retrying().collect::<Vec<_>>());
    out += &format!("in_flight {:?}\n", exec.in_flight().collect::<Vec<_>>());
    out += &format!("occurrences {:?}\n", exec.occurrences().collect::<Vec<_>>());
    out += &format!("conditions {:?}\n", exec.conditions());
    out
}

fn premise<T, E: Debug>(what: &str, r: Result<T, E>) -> Result<T, TestCaseError> {
    r.map_err(|e| TestCaseError::fail(format!("premise: {what}: {e:?}")))
}

fn load(net: NetDef) -> Result<Executor, TestCaseError> {
    premise("the net loads", no_panic("load", || Executor::new(net))?)
}

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

fn content() -> impl Strategy<Value = Content> {
    prop::collection::btree_map(
        prop_oneof![
            3 => select(vec!["x", "due", "held", "occurrence", "reason", ANSWER_ID_KEY, SEEN_FIELD])
                .prop_map(str::to_owned),
            1 => ".{0,4}",
        ],
        value(),
        0..4,
    )
}

fn subject() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => select(vec!["s", "t"]).prop_map(str::to_owned),
        1 => Just(String::new()),
        1 => ".{0,6}",
    ]
}

/// Any name, near misses of the nets' names included; a test filters out
/// the names its net declares.
fn other_name() -> impl Strategy<Value = String> {
    prop_oneof![
        select(vec![
            "Nope",
            "",
            " ",
            "in",
            "In ",
            "Out.x",
            "D9",
            "Asks.T",
            "Answered.T",
            "W.x"
        ])
        .prop_map(str::to_owned),
        ".{0,8}",
    ]
}

/// Holds when `name` is none of `declared`.
fn undeclared(declared: &[String]) -> impl Strategy<Value = String> + use<> {
    let declared = declared.to_vec();
    other_name().prop_filter("a name the net declares", move |n| !declared.contains(n))
}

fn trigger() -> impl Strategy<Value = Trigger> {
    prop_oneof![Just(Trigger::Auto), Just(Trigger::Manual)]
}

fn bind_true() -> holon_net::exec::Read {
    Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new()))
}

fn tagged(by: &str) -> Content {
    Content::from([(BY.to_owned(), Value::String(by.to_owned()))])
}

// ------------------------------------------------------------- the nets

/// A lowered ask `W` over a stored `P` (read) beside a fifo `Out`; a pure
/// body puts one token tagged `W` into `Out`.
fn ask_net(
    net: NetDef,
    bind: holon_net::exec::Read,
    effectful: bool,
) -> Result<NetDef, TestCaseError> {
    let d = TransitionDescriptor {
        name: TransitionName(W.to_owned()),
        arcs: vec![HlArc {
            kind: ArcKind::Read,
            place: PlaceName("P".to_owned()),
        }],
        body: BodyDecl::Native {
            symbol: NativeRef("w".to_owned()),
            effectful,
        },
        irreversible: false,
        timers: vec![],
        guard: None,
        ask: Some(AskDecl {
            input: None,
            preview: None,
            persist: false,
        }),
        undoes: None,
    };
    let body = if effectful {
        NativeBody::Effectful(Receiver::None)
    } else {
        NativeBody::Pure(Arc::new(|s: &Subject, _: &Content| {
            Ok(vec![Output::new("Out", s, tagged(W))])
        }))
    };
    let parts = NativeParts {
        bind,
        body,
        preview: None,
    };
    let net = net.place("P", Class::Process).fifo("Out", Class::Process);
    premise("the ask lowers", lower(net, &d, Some(parts)))
}

fn ask_transitions() -> Vec<String> {
    [
        Generated::Answer,
        Generated::ExpireAnswer,
        Generated::ReleaseFailed,
        Generated::ClearFailed,
    ]
    .iter()
    .map(|g| g.name(W))
    .chain([W.to_owned()])
    .collect()
}

fn ask_places() -> Vec<String> {
    let transitions = ask_transitions();
    let t: Vec<&str> = transitions.iter().map(String::as_str).collect();
    let mut all = places(&["P", "Out"], &t);
    for g in [
        Generated::Answered,
        Generated::Expired,
        Generated::WorkFailed,
    ] {
        all.push(g.name(W));
    }
    all
}

fn answer_call(subject: &str, seen: AskHash, id: &str) -> AnswerCall {
    AnswerCall {
        transition: TransitionName(W.to_owned()),
        subject: SubjectName(subject.to_owned()),
        seen,
        answer: AnswerId(id.to_owned()),
        value: None,
    }
}

/// The hash the `Asks` row of `W` for `subject` offers.
fn offered(exec: &Executor, subject: &str) -> Result<Option<AskHash>, TestCaseError> {
    let rows = no_panic("asks", || exec.asks())?;
    Ok(rows.ok().and_then(|rows| {
        rows.into_iter()
            .find(|r| r.transition.0 == W && r.subject.0 == subject)
            .map(|r| r.hash)
    }))
}

fn effect_requests_of(requests: &[EffectRequest], transition: &str) -> usize {
    requests
        .iter()
        .filter(|r| &*r.transition == transition)
        .count()
}

// --------------------------------- a body output into an unstored place

/// A `T` that takes `In` and puts one token into each of `into`, beside a
/// fifo `Out` and a derived `D`.
fn output_net(trigger: Trigger, into: Vec<String>) -> NetDef {
    let t = TransitionDef::new(
        "T",
        trigger,
        bind_true(),
        Body::pure(move |s: &Subject, _: &Content| {
            Ok(into
                .iter()
                .map(|p| Output::new(p, s, tagged("T")))
                .collect())
        }),
    )
    .consumes("In");
    NetDef::default()
        .place("In", Class::Process)
        .fifo("Out", Class::Process)
        .derived("D", Arc::new(|_: &mut Reader<'_>, _: &Subject| None))
        .transition(t)
}

fn output_declared() -> Vec<String> {
    let mut all = places(&["In", "Out"], &["T"]);
    all.push("D".to_owned());
    all
}

/// A place an output may not name: undeclared, derived, or owned by the
/// executor.
fn output_target() -> impl Strategy<Value = String> {
    let owned: Vec<String> = places(&[], &["T"]);
    prop_oneof![
        3 => undeclared(&output_declared()),
        1 => Just("D".to_owned()),
        2 => select(owned),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A pure body whose output names a place the net does not declare, or
    /// a derived place: the firing settles Failed with a definite cause that
    /// names the place, nothing of it is put, its input goes back, and the
    /// failure is marked.
    #[test]
    fn a_body_output_into_a_place_the_net_does_not_store_is_refused(
        target in output_target(),
        with_out in any::<bool>(),
        out_first in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let mut into = vec![target.clone()];
        if with_out {
            into.insert(usize::from(!out_first), "Out".to_owned());
        }
        let trigger = if manual { Trigger::Manual } else { Trigger::Auto };
        let mut exec = load(output_net(trigger, into))?;
        let w = no_panic("write In", || exec.write("In", &s, Content::new(), Duration::ZERO))?;
        let _ = premise("the write into In is accepted", w)?;
        if manual {
            let r = no_panic(&format!("fire with an output into {target:?}"), || {
                exec.fire("T", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_secs(1))
            })?;
            let _ = premise("the fire is taken", r)?;
        }
        let _ = no_panic("step", || exec.step(Duration::from_secs(2)))?;
        check_failed_output(&exec, &target, &s)?;
    }

    /// Exclusion: the same body into the declared `Out` fires and puts.
    #[test]
    fn a_body_output_into_a_stored_place_fires(manual in any::<bool>(), s in subject()) {
        let trigger = if manual { Trigger::Manual } else { Trigger::Auto };
        let mut exec = load(output_net(trigger, vec!["Out".to_owned()]))?;
        let _ = premise("the write", no_panic("write", || exec.write("In", &s, Content::new(), Duration::ZERO))?)?;
        if manual {
            let r = no_panic("fire", || {
                exec.fire("T", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_secs(1))
            })?;
            prop_assert!(r.is_ok(), "the fire into Out was refused: {r:?}");
        }
        prop_assert_eq!(exec.tokens().contents("Out", &s), vec![tagged("T")]);
    }
}

fn effect_output_net(trigger: Trigger) -> NetDef {
    let t = TransitionDef::new("T", trigger, bind_true(), Body::Effectful(Receiver::None))
        .consumes("In");
    NetDef::default()
        .fifo("In", Class::Process)
        .fifo("Out", Class::Process)
        .derived("D", Arc::new(|_: &mut Reader<'_>, _: &Subject| None))
        .transition(t)
}

/// The occurrence of `T` settled Failed with a definite cause naming
/// `target`, nothing of it was put, its input went back and it is marked.
fn check_failed_output(exec: &Executor, target: &str, s: &str) -> Result<(), TestCaseError> {
    let settled: Vec<_> = exec
        .occurrences()
        .map(|(o, st)| (o.clone(), st.cloned()))
        .collect();
    prop_assert_eq!(settled.len(), 1, "one firing: {:?}", settled);
    match &settled[0].1 {
        Some(Settled::Failed(cause)) => {
            prop_assert!(!cause.retryable, "the cause is definite: {cause:?}");
            prop_assert!(
                cause.text.contains(target),
                "the cause {cause:?} names {target:?}"
            );
        }
        other => prop_assert!(false, "the firing settled {other:?}, not Failed"),
    }
    let held = exec.tokens().subjects(target);
    prop_assert!(held.is_empty(), "{target:?} holds tokens of {held:?}");
    prop_assert!(
        exec.tokens().subjects("Out").is_empty(),
        "Out holds a token of the failed firing"
    );
    prop_assert_eq!(
        exec.tokens().subjects("In"),
        vec![Subject::new(s)],
        "the input goes back"
    );
    prop_assert!(
        exec.marks()
            .any(|(t, subject, _)| t == "T" && subject.as_str() == s),
        "the failure is marked"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// An effect result, done or verified after an unknown outcome, whose
    /// outputs name a place the net does not store: the firing settles
    /// Failed with a definite cause naming the place, as a pure body's does.
    #[test]
    fn an_effect_result_into_a_place_the_net_does_not_store_fails_the_firing(
        target in output_target(),
        with_out in any::<bool>(),
        out_first in any::<bool>(),
        verify in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let mut exec = load(effect_output_net(if manual { Trigger::Manual } else { Trigger::Auto }))?;
        let req = start_effect(&mut exec, manual, &s)?;
        let mut outs = vec![Output::new(&target, &req.subject, tagged("T"))];
        if with_out {
            outs.insert(usize::from(!out_first), Output::new("Out", &req.subject, tagged("T")));
        }
        if verify {
            let _ = premise("the unknown outcome parks", no_panic("complete", || {
                exec.complete(req.attempt, EffectResult::Unknown(Cause::definite("lost")), Duration::from_secs(1))
            })?)?;
            let r = no_panic("resolve", || exec.resolve(&req.occurrence, Resolution::Verify(outs), Duration::from_secs(2)))?;
            let _ = premise("the resolution is taken", r)?;
        } else {
            let r = no_panic("complete", || exec.complete(req.attempt, EffectResult::Done(outs), Duration::from_secs(1)))?;
            let _ = premise("the result is taken", r)?;
        }
        check_failed_output(&exec, &target, &s)?;
    }
}

// --------------------------------------- a Logic that does not match the file

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

/// Transitions `T<i>` over a fifo `P0`; transition `i` runs a declared
/// effect when `effects[i]`.
fn logic_file(effects: &[(bool, bool)]) -> String {
    let mut fx = IndexMap::new();
    let transitions = effects
        .iter()
        .enumerate()
        .map(|(i, (effect, probe))| {
            let effect = effect.then(|| {
                let name = format!("E{i}");
                fx.insert(name.clone(), shell_effect());
                name
            });
            let receiver = effect.as_ref().map(|_| {
                if *probe {
                    holon_net::file::Receiver::StateProbe
                } else {
                    holon_net::file::Receiver::None
                }
            });
            (
                format!("T{i}"),
                TransitionSpec {
                    trigger: holon_net::file::Trigger::Manual,
                    priority: 0,
                    consumes: vec!["P0".to_owned()],
                    reads: vec![],
                    inhibits: vec![],
                    produces: vec![],
                    facts: vec![],
                    on_failure: None,
                    on_timeout: None,
                    effect,
                    receiver,
                    timers: None,
                },
            )
        })
        .collect();
    NetFile {
        name: "logic".to_owned(),
        places: IndexMap::from([(
            "P0".to_owned(),
            PlaceSpec {
                capacity: PlaceCapacity::Fifo,
                pool: None,
            },
        )]),
        transitions,
        effects: fx,
        probes: IndexMap::new(),
    }
    .to_yaml()
}

/// The logic the loader asks for `T<i>`: an effect logic when `effect`.
fn logic_for(name: &str, effect_of: &[bool]) -> Logic {
    let i: usize = name
        .strip_prefix('T')
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the loader asked for the logic of '{name}'"));
    if effect_of[i] {
        Logic::effect(bind_true())
    } else {
        Logic::pure(bind_true(), Arc::new(|_: &Subject, _: &Content| Ok(vec![])))
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A logic that does not match its transition in the file, a pure one
    /// for a declared effect or an effect one for a transition without:
    /// `net_def` refuses the file.
    #[test]
    fn net_def_refuses_a_logic_that_does_not_match_the_file(
        spec in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>()), 1..4),
        forced in any::<usize>(),
    ) {
        let mut spec = spec;
        let k = forced % spec.len();
        spec[k].2 = true;
        let effects: Vec<(bool, bool)> = spec.iter().map(|(e, p, _)| (*e, *p)).collect();
        let given: Vec<bool> = spec.iter().map(|(e, _, flip)| e ^ flip).collect();
        let yaml = logic_file(&effects);
        let r = no_panic("net_def", || {
            net_def("nets/logic.yaml", &yaml, |t| logic_for(t, &given)).map(|_| ())
        })?;
        prop_assert!(r.is_err(), "net_def accepted logic {given:?} for effects {effects:?}");
    }

    /// Exclusion: a logic that matches every transition loads.
    #[test]
    fn net_def_loads_a_logic_that_matches_the_file(
        effects in prop::collection::vec((any::<bool>(), any::<bool>()), 1..4),
    ) {
        let given: Vec<bool> = effects.iter().map(|(e, _)| *e).collect();
        let yaml = logic_file(&effects);
        let r = no_panic("net_def", || {
            net_def("nets/logic.yaml", &yaml, |t| logic_for(t, &given)).map(|_| ())
        })?;
        prop_assert!(r.is_ok(), "net_def refused a matching logic: {r:?}");
    }
}

// -------------------------------------------- an answer before the clock

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// An answer before the executor's clock is refused for the clock and
    /// changes nothing; at the clock it is not refused for the clock.
    #[test]
    fn an_answer_before_the_clock_is_refused_and_changes_nothing(
        clock_ns in 1u64..=(1 << 62),
        back_ns in 1u64..=u64::MAX,
        s in subject(),
        offered_hash in any::<bool>(),
    ) {
        let mut exec = load(ask_net(NetDef::default(), bind_true(), false)?)?;
        let _ = premise("the write", no_panic("write", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let clock = Duration::from_nanos(clock_ns);
        premise("the step", no_panic("step", || exec.step(clock))?)?;
        let seen = match offered(&exec, &s)? {
            Some(h) if offered_hash => h,
            _ => AskHash([7; 32]),
        };
        let back = clock - Duration::from_nanos(back_ns.min(clock_ns));
        let shown = ask_places();
        let before = snapshot(&exec, &shown);
        let r = no_panic("answer before the clock", || exec.answer(answer_call(&s, seen, "a1"), back))?;
        prop_assert!(names_error(&r, "ClockWentBack"), "the answer at {back:?}, before the clock {clock:?}: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused answer changed the executor");
        let at = no_panic("answer at the clock", || exec.answer(answer_call(&s, seen, "a1"), clock))?;
        prop_assert!(!names_error(&at, "Clock"), "the answer at the clock was refused for the clock: {at:?}");
    }
}

// ------------------------------------------------------------- the pools

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A write into a pool that holds all its tokens is refused and changes
    /// nothing.
    #[test]
    fn a_write_into_a_full_pool_is_refused_and_changes_nothing(
        n in 1u32..4,
        s in subject(),
        c in content(),
        now_ms in 0u64..5_000,
    ) {
        let mut exec = load(NetDef::default().pool("Pool", Class::Process, n))?;
        let shown = places(&["Pool"], &[]);
        let before = snapshot(&exec, &shown);
        let now = Duration::from_millis(now_ms);
        let r = no_panic("write into the full pool", || exec.write("Pool", &s, c, now))?;
        prop_assert!(names_error(&r, "PoolFull"), "the write into the full pool of {n} was not refused as PoolFull: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused write changed the executor");
        prop_assert_eq!(exec.tokens().free("Pool"), n as usize);
    }

    /// Exclusion: after a retract takes one pool token, a write puts it back.
    #[test]
    fn a_write_into_a_pool_with_a_free_slot_is_accepted(
        n in 1u32..4,
        s in subject(),
        t in subject(),
        c in content(),
    ) {
        let mut exec = load(NetDef::default().pool("Pool", Class::Process, n))?;
        let r = no_panic("retract", || exec.retract("Pool", &t, Duration::ZERO))?;
        prop_assert!(r.is_ok(), "a retract of a pool token was refused: {r:?}");
        prop_assert_eq!(exec.tokens().free("Pool"), n as usize - 1);
        let w = no_panic("write", || exec.write("Pool", &s, c, Duration::ZERO))?;
        prop_assert!(w.is_ok(), "the write into the pool with a free slot was refused: {w:?}");
        prop_assert_eq!(exec.tokens().free("Pool"), n as usize);
    }

    /// A retract of a row with no token is refused and changes nothing; the
    /// same retract after a write is accepted.
    #[test]
    fn a_retract_of_a_row_without_a_token_is_refused(
        capacity in 0usize..3,
        s in subject(),
        c in content(),
    ) {
        let net = match capacity {
            0 => NetDef::default().place("P", Class::Process),
            1 => NetDef::default().fifo("P", Class::Process),
            _ => NetDef::default().register("P", Class::Process),
        };
        let mut exec = load(net)?;
        let shown = places(&["P"], &[]);
        let before = snapshot(&exec, &shown);
        let r = no_panic("retract", || exec.retract("P", &s, Duration::ZERO))?;
        prop_assert!(names_error(&r, "NothingToRetract"), "retract of an empty row: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown));
        let _ = premise("the write", no_panic("write", || exec.write("P", &s, c, Duration::ZERO))?)?;
        let r = no_panic("retract", || exec.retract("P", &s, Duration::ZERO))?;
        prop_assert!(r.is_ok(), "the retract of a held row was refused: {r:?}");
    }
}

/// `t` taking `In` (and `Pool` when `takes_pool`), beside two pools of sizes
/// `n` and `m` and a fifo `Out`.
fn pool_net(n: u32, m: u32, t: TransitionDef, takes_pool: bool) -> NetDef {
    let t = if takes_pool { t.consumes("Pool") } else { t };
    NetDef::default()
        .pool("Pool", Class::Process, n)
        .pool("Pool2", Class::Process, m)
        .fifo("In", Class::Process)
        .fifo("Out", Class::Process)
        .transition(t.consumes("In"))
}

fn check_pools(exec: &Executor, n: u32, m: u32) -> Result<(), TestCaseError> {
    for (p, size) in [("Pool", n), ("Pool2", m)] {
        let free = exec.tokens().free(p);
        prop_assert!(
            free <= size as usize,
            "the pool {p} of {size} holds {free} free tokens"
        );
    }
    Ok(())
}

/// The pools stay full and the dropped token of `T` into `pool` is disclosed
/// once.
fn check_full_and_disclosed(
    exec: &Executor,
    n: u32,
    m: u32,
    pool: &str,
) -> Result<(), TestCaseError> {
    for (p, size) in [("Pool", n), ("Pool2", m)] {
        prop_assert_eq!(
            exec.tokens().free(p),
            size as usize,
            "the pool {} stays full",
            p
        );
    }
    let quoted = format!("{pool:?}");
    let disclosed: Vec<String> = exec
        .conditions()
        .iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| c.contains("PoolFull") && c.contains(&quoted) && c.contains("\"T\""))
        .collect();
    prop_assert_eq!(
        disclosed.len(),
        1,
        "PoolFull of {} by T disclosed once: {:?}",
        pool,
        exec.conditions()
    );
    Ok(())
}

fn pool_outputs(k: usize, other: bool, s: &Subject) -> Vec<Output> {
    let mut outs: Vec<Output> = (0..k)
        .map(|_| Output::new("Pool", s, Content::new()))
        .collect();
    if other {
        outs.push(Output::new("Out", s, tagged("T")));
    }
    outs
}

/// Starts the effectful `T` for `s` (an auto write, or a manual fire) and
/// returns its request.
fn start_effect(
    exec: &mut Executor,
    manual: bool,
    s: &str,
) -> Result<EffectRequest, TestCaseError> {
    let w = premise(
        "the write into In",
        no_panic("write", || {
            exec.write("In", s, Content::new(), Duration::ZERO)
        })?,
    )?;
    let requests = if manual {
        premise(
            "the fire",
            no_panic("fire", || {
                exec.fire(
                    "T",
                    s,
                    OccurrenceId::caller("o1"),
                    Content::new(),
                    Duration::ZERO,
                )
            })?,
        )?
        .requests
    } else {
        w.requests
    };
    requests
        .into_iter()
        .next()
        .ok_or_else(|| TestCaseError::fail("premise: T requests its effect"))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A failure port into a pool, the one the firing took from or another
    /// full one: the port token is dropped, the pools stay full and the drop
    /// is disclosed once.
    #[test]
    fn a_failure_port_into_a_pool_never_overfills_it(
        n in 1u32..4,
        m in 1u32..4,
        same in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let port = if same { "Pool" } else { "Pool2" };
        let t = TransitionDef::new(
            "T",
            if manual { Trigger::Manual } else { Trigger::Auto },
            bind_true(),
            Body::pure(|_: &Subject, _: &Content| Err(Cause::definite("boom"))),
        )
        .on_failure(port);
        let mut exec = load(pool_net(n, m, t, true))?;
        let _ = no_panic("write In", || exec.write("In", &s, Content::new(), Duration::ZERO))?;
        check_pools(&exec, n, m)?;
        if manual {
            let _ = no_panic("fire", || {
                exec.fire("T", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_secs(1))
            })?;
            check_pools(&exec, n, m)?;
        }
        let _ = no_panic("step", || exec.step(Duration::from_secs(2)))?;
        check_pools(&exec, n, m)?;
        check_full_and_disclosed(&exec, n, m, port)?;
    }

    /// A timeout port into a full pool: the port token is dropped, the pool
    /// stays full and the drop is disclosed once.
    #[test]
    fn a_timeout_port_into_a_full_pool_never_overfills_it(
        n in 1u32..4,
        run_ms in 1u64..5_000,
        later_ms in 0u64..5_000,
        manual in any::<bool>(),
        s in subject(),
    ) {
        let t = TransitionDef::new(
            "T",
            if manual { Trigger::Manual } else { Trigger::Auto },
            bind_true(),
            Body::Effectful(Receiver::None),
        )
        .run_timer(Duration::from_millis(run_ms))
        .on_timeout("Pool");
        let mut exec = load(pool_net(n, n, t, false))?;
        start_effect(&mut exec, manual, &s)?;
        let at = Duration::from_millis(run_ms + later_ms);
        let _ = no_panic("step past the run timer", || exec.step(at))?;
        check_pools(&exec, n, n)?;
        check_full_and_disclosed(&exec, n, n, "Pool")?;
    }

    /// An effect result whose outputs go into a full pool: the pool tokens
    /// are dropped and disclosed once, the other output commits, the firing
    /// settles Done.
    #[test]
    fn an_effect_result_into_a_full_pool_never_overfills_it(
        n in 1u32..4,
        k in 1usize..4,
        other in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let t = TransitionDef::new(
            "T",
            if manual { Trigger::Manual } else { Trigger::Auto },
            bind_true(),
            Body::Effectful(Receiver::None),
        );
        let mut exec = load(pool_net(n, n, t, false))?;
        let req = start_effect(&mut exec, manual, &s)?;
        let outs = pool_outputs(k, other, &req.subject);
        let r = no_panic("complete into the pool", || {
            exec.complete(req.attempt, EffectResult::Done(outs), Duration::from_secs(1))
        })?;
        check_pools(&exec, n, n)?;
        let _ = premise("the result is taken", r)?;
        check_full_and_disclosed(&exec, n, n, "Pool")?;
        prop_assert_eq!(exec.settled(&req.occurrence).ok().flatten(), Some(Settled::Done));
        prop_assert_eq!(exec.tokens().subjects("Out").len(), usize::from(other), "the other output commits");
    }

    /// A verified resolution whose outputs go into a full pool: the pool
    /// tokens are dropped and disclosed once, the other output commits.
    #[test]
    fn a_verified_resolution_into_a_full_pool_never_overfills_it(
        n in 1u32..4,
        k in 1usize..4,
        other in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let t = TransitionDef::new(
            "T",
            if manual { Trigger::Manual } else { Trigger::Auto },
            bind_true(),
            Body::Effectful(Receiver::None),
        );
        let mut exec = load(pool_net(n, n, t, false))?;
        let req = start_effect(&mut exec, manual, &s)?;
        let _ = premise("the unknown outcome parks", no_panic("complete", || {
            exec.complete(req.attempt, EffectResult::Unknown(Cause::definite("lost")), Duration::from_secs(1))
        })?)?;
        let outs = pool_outputs(k, other, &req.subject);
        let r = no_panic("resolve into the pool", || {
            exec.resolve(&req.occurrence, Resolution::Verify(outs), Duration::from_secs(2))
        })?;
        check_pools(&exec, n, n)?;
        let _ = premise("the resolution is taken", r)?;
        check_full_and_disclosed(&exec, n, n, "Pool")?;
        prop_assert_eq!(exec.tokens().subjects("Out").len(), usize::from(other), "the other output commits");
    }
}

// ------------------------------------ the executor's own places, Answered

/// An auto or manual `T` over a fifo `In` with a retry, effectful or pure,
/// beside the ask `W`.
fn owned_net(trigger: Trigger, effectful: bool) -> Result<NetDef, TestCaseError> {
    let body = if effectful {
        Body::Effectful(Receiver::None)
    } else {
        Body::pure(|_: &Subject, _: &Content| Err(Cause::retryable("boom")))
    };
    let t = TransitionDef::new("T", trigger, bind_true(), body)
        .consumes("In")
        .retry(Duration::from_millis(10), 3);
    ask_net(
        NetDef::default().fifo("In", Class::Process).transition(t),
        bind_true(),
        false,
    )
}

fn owned_shown() -> Vec<String> {
    let mut all = ask_places();
    all.extend(places(&["In"], &["T"]));
    all
}

/// The places only the executor writes.
fn owned_place() -> impl Strategy<Value = String> {
    select(places(&[], &["T", W, &Generated::Answer.name(W)]))
}

/// Contents shaped like the executor's own tokens, or not at all.
fn owned_content() -> impl Strategy<Value = Content> {
    let held_row = prop::collection::btree_map(
        select(vec!["place", "subject", "holder"]).prop_map(str::to_owned),
        value(),
        0..3,
    )
    .prop_map(|m| Value::Object(m.into_iter().collect()));
    prop_oneof![
        content(),
        any::<i64>().prop_map(|d| Content::from([("due".to_owned(), Value::Integer(d))])),
        value().prop_map(|v| Content::from([("due".to_owned(), v)])),
        value().prop_map(|v| Content::from([("held".to_owned(), v)])),
        prop::collection::vec(held_row, 0..3)
            .prop_map(|rows| Content::from([("held".to_owned(), Value::Array(rows))])),
        (value(), value()).prop_map(|(a, s)| Content::from([
            (ANSWER_ID_KEY.to_owned(), a),
            (SEEN_FIELD.to_owned(), s),
        ])),
        ".{0,4}".prop_map(|a| Content::from([(ANSWER_ID_KEY.to_owned(), Value::String(a))])),
        any::<i64>().prop_map(|a| Content::from([
            (ANSWER_ID_KEY.to_owned(), Value::Integer(a)),
            (SEEN_FIELD.to_owned(), Value::String("x".to_owned())),
        ])),
    ]
}

const EXPIRE_REASONS: [&str; 2] = ["ask_gone", "situation_changed"];

/// An `Expired.<W>` content whose `reason` is missing, not text, or no
/// expire reason.
fn expired_content() -> impl Strategy<Value = Content> {
    let reason = prop_oneof![
        value().prop_filter("an expire reason", |v| {
            !matches!(v, Value::String(t) if EXPIRE_REASONS.contains(&t.as_str()))
        }),
        select(vec!["Ask_gone", "ask_gone ", "situation", "expired", ""])
            .prop_map(|t| Value::String(t.to_owned())),
    ];
    (owned_content(), prop::option::of(reason)).prop_map(|(mut c, r)| {
        c.remove("reason");
        if let Some(r) = r {
            c.insert("reason".to_owned(), r);
        }
        c
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// A write into a place only the executor writes (`<T>.Running`,
    /// `.RetryPending`, `.OutcomeUnknown`, `.UndoPending`, `OutcomeUnknown`),
    /// into `Answered.<W>` without a text answer id or seen field, or into
    /// `Expired.<W>` without an expire reason, is refused and changes
    /// nothing.
    #[test]
    fn a_write_into_an_executor_owned_place_is_refused_and_changes_nothing(
        place in owned_place(),
        s in subject(),
        c in owned_content(),
        unanswered in prop::option::of(owned_content().prop_filter(
            "an Answered token with both answer fields as text",
            |c| [ANSWER_ID_KEY, SEEN_FIELD].iter().any(|k| !matches!(c.get(*k), Some(Value::String(_)))),
        )),
        expired in prop::option::of(expired_content()),
        trigger in trigger(),
        effectful in any::<bool>(),
    ) {
        let mut exec = load(owned_net(trigger, effectful)?)?;
        let (place, c) = match (unanswered, expired) {
            (Some(c), _) => (Generated::Answered.name(W), c),
            (None, Some(c)) => (Generated::Expired.name(W), c),
            (None, None) => (place, c),
        };
        let shown = owned_shown();
        let before = snapshot(&exec, &shown);
        let r = no_panic(&format!("write({place:?}, {c:?})"), || exec.write(&place, &s, c, Duration::ZERO))?;
        let form = if place.starts_with("Answered.") || place.starts_with("Expired.") { "AnswerField" } else { "ExecutorPlace" };
        prop_assert!(names_error(&r, form), "the write into {place:?} was not refused as {form}: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused write changed the executor");
    }

    /// Exclusion: a write into `Expired.<W>` whose reason is an expire
    /// reason is accepted.
    #[test]
    fn a_write_into_expired_with_an_expire_reason_is_accepted(
        c in owned_content(),
        reason in select(EXPIRE_REASONS.to_vec()),
        s in subject(),
    ) {
        let mut exec = load(owned_net(Trigger::Auto, false)?)?;
        let mut c = c;
        c.insert("reason".to_owned(), Value::String(reason.to_owned()));
        let place = Generated::Expired.name(W);
        let r = no_panic("write Expired", || exec.write(&place, &s, c, Duration::ZERO))?;
        prop_assert!(r.is_ok(), "the write into {place:?} with the reason {reason:?} was refused: {r:?}");
    }

    /// A retract of a token the executor put into its own place (`T.Running`
    /// of a running effect, `OutcomeUnknown` of a parked one) is refused and
    /// changes nothing.
    #[test]
    fn a_retract_of_an_executor_owned_token_is_refused_and_changes_nothing(
        parked in any::<bool>(),
        manual in any::<bool>(),
        s in subject(),
    ) {
        let mut exec = load(owned_net(if manual { Trigger::Manual } else { Trigger::Auto }, true)?)?;
        let req = start_effect(&mut exec, manual, &s)?;
        let (place, subject) = if parked {
            let _ = premise("the unknown outcome parks", no_panic("complete", || {
                exec.complete(req.attempt, EffectResult::Unknown(Cause::definite("lost")), Duration::ZERO)
            })?)?;
            (OUTCOME_UNKNOWN.to_owned(), req.occurrence.to_string())
        } else {
            (Place::running("T").to_string(), s.clone())
        };
        prop_assert!(
            !exec.tokens().subjects(&place).is_empty(),
            "premise: {place} holds a token"
        );
        let shown = owned_shown();
        let before = snapshot(&exec, &shown);
        let r = no_panic(&format!("retract({place:?}, {subject:?})"), || {
            exec.retract(&place, &subject, Duration::from_secs(1))
        })?;
        prop_assert!(names_error(&r, "ExecutorPlace"), "the retract of {place:?}/{subject:?} was not refused as ExecutorPlace: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused retract changed the executor");
    }

    /// Whatever a write into an executor-owned or lowered place does, no
    /// later call panics: step, why_not, asks, derive_all, answer,
    /// answer_settlement, undo.
    #[test]
    fn no_call_after_a_write_into_an_executor_or_answer_place_panics(
        target in prop_oneof![
            owned_place().prop_map(|p| (p, false)),
            Just((Generated::Answered.name(W), false)),
            select(vec![Generated::Expired.name(W), Generated::WorkFailed.name(W)])
                .prop_map(|p| (p, true)),
        ],
        to_answer in any::<bool>(),
        s in subject(),
        c in owned_content(),
        trigger in trigger(),
        effectful in any::<bool>(),
    ) {
        let (place, lowered) = target;
        let mut exec = load(owned_net(trigger, effectful)?)?;
        let _ = premise("the write into P", no_panic("write P", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        // An answer that saw another situation expires into `Expired.W` keyed
        // by its id, so answer_settlement reads that token.
        let _ = no_panic("answer a1", || exec.answer(answer_call(&s, AskHash([7; 32]), "a1"), Duration::from_secs(1)))?;
        let row = if lowered || to_answer { "a1".to_owned() } else { s.clone() };
        let _ = no_panic(&format!("write({place:?}, {row:?}, {c:?})"), || {
            exec.write(&place, &row, c, Duration::from_secs(2))
        })?;
        let _ = no_panic("step", || exec.step(Duration::from_secs(3)))?;
        for t in ["T".to_owned(), W.to_owned(), Generated::Answer.name(W), Generated::ExpireAnswer.name(W)] {
            let _ = no_panic(&format!("why_not({t})"), || format!("{:?}", exec.why_not(&t, &s)))?;
        }
        let _ = no_panic("asks", || format!("{:?}", exec.asks()))?;
        let _ = no_panic("derive_all", || format!("{:?}", exec.derive_all(&Generated::Asks.name(W))))?;
        let _ = no_panic("answer_settlement", || format!("{:?}", exec.answer_settlement(&AnswerId("a1".to_owned()))))?;
        let seen = offered(&exec, &s)?.unwrap_or(AskHash([0; 32]));
        let _ = no_panic("answer a2", || format!("{:?}", exec.answer(answer_call(&s, seen, "a2"), Duration::from_secs(4))))?;
        let _ = no_panic("write In", || format!("{:?}", exec.write("In", &s, Content::new(), Duration::from_secs(5))))?;
        let _ = no_panic("step", || format!("{:?}", exec.step(Duration::from_secs(6))))?;
        let _ = no_panic("undo a1", || format!("{:?}", exec.undo(&OccurrenceId::caller("a1"), Duration::from_secs(7))))?;
        let _ = no_panic("answer_settlement", || format!("{:?}", exec.answer_settlement(&AnswerId("a2".to_owned()))))?;
    }
}

// ------------------------------------------------- derived place cycles

/// The read a derive makes of another derived place.
fn reads(r: &mut Reader<'_>, place: &str, s: &Subject, kind: usize) -> bool {
    match kind {
        0 => r.present(place, s),
        1 => r.field(place, s, "x").is_some(),
        _ => r.text(place, s, "x").is_some(),
    }
}

fn reading(place: &'static str, kind: usize) -> holon_net::exec::Read {
    Arc::new(move |r: &mut Reader<'_>, s: &Subject| reads(r, place, s, kind).then(Content::new))
}

/// Derived places that read each other: `D` reads itself (`two` false), or
/// `D` reads `D1` and `D1` reads `D`; an auto `T` over `In` reads `D` when
/// `read_by_t`.
fn cycle_net(two: bool, kind: usize, read_by_t: bool) -> NetDef {
    let mut net = NetDef::default().fifo("In", Class::Process);
    net = if two {
        net.derived("D", reading("D1", kind))
            .derived("D1", reading("D", kind))
    } else {
        net.derived("D", reading("D", kind))
    };
    let bind = if read_by_t {
        reading("D", kind)
    } else {
        bind_true()
    };
    net.transition(
        TransitionDef::new(
            "T",
            Trigger::Auto,
            bind,
            Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
        )
        .consumes("In"),
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// A derived place that reads itself, or two that read each other, on
    /// every marking: the net does not load.
    #[test]
    fn a_net_whose_derived_places_read_each_other_does_not_load(
        two in any::<bool>(),
        kind in 0usize..3,
        read_by_t in any::<bool>(),
    ) {
        let r = no_panic("load", || Executor::new(cycle_net(two, kind, read_by_t)).map(|_| ()))?;
        prop_assert!(names_error(&r, "DerivedCycle"), "a net with a derived cycle was not refused as DerivedCycle");
    }

    /// Whatever the load does with a derived cycle, no call aborts: a write
    /// that wakes `T`, a step, derive and derive_all.
    #[test]
    fn no_call_on_a_net_with_a_derived_cycle_aborts(
        two in any::<bool>(),
        kind in 0usize..3,
        read_by_t in any::<bool>(),
        s in subject(),
    ) {
        let Ok(mut exec) = no_panic("load", || Executor::new(cycle_net(two, kind, read_by_t)))? else {
            return Ok(());
        };
        let _ = no_panic("write", || format!("{:?}", exec.write("In", &s, Content::new(), Duration::ZERO)))?;
        let _ = no_panic("step", || format!("{:?}", exec.step(Duration::from_secs(1))))?;
        let _ = no_panic("derive", || format!("{:?}", exec.derive("D", &Subject::new(&s))))?;
        let _ = no_panic("derive_all", || format!("{:?}", exec.derive_all("D")))?;
    }

    /// Exclusion: a chain of derived places, `D1` reads `D`, loads and
    /// derives.
    #[test]
    fn a_net_whose_derived_places_form_a_chain_loads(kind in 0usize..3, s in subject()) {
        let net = NetDef::default()
            .fifo("In", Class::Process)
            .derived("D", Arc::new(|r: &mut Reader<'_>, s: &Subject| r.present("In", s).then(Content::new)))
            .derived("D1", reading("D", kind))
            .transition(
                TransitionDef::new("T", Trigger::Auto, reading("D1", kind), Body::pure(|_: &Subject, _: &Content| Ok(vec![])))
                    .consumes("In"),
            );
        let mut exec = load(net)?;
        let _ = premise("the write", no_panic("write", || exec.write("In", &s, Content::new(), Duration::ZERO))?)?;
        let d = no_panic("derive", || exec.derive("D1", &Subject::new(&s)))?;
        prop_assert!(d.is_ok(), "derive of the chain was refused: {d:?}");
    }

    /// A derived place that reads itself only while `P` holds a token of the
    /// subject: no call aborts; a derive that meets the cycle is an error,
    /// and a derive for a subject without that token answers.
    #[test]
    fn a_derived_cycle_that_a_marking_reveals_is_an_error_of_the_call(
        kind in 0usize..3,
        read_by_t in any::<bool>(),
        s in subject(),
        t in subject(),
    ) {
        prop_assume!(s != t);
        let d: holon_net::exec::Read = Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            (r.present("P", s) && reads(r, "D", s, kind)).then(Content::new)
        });
        let bind = if read_by_t { reading("D", kind) } else { bind_true() };
        let net = NetDef::default()
            .fifo("P", Class::Process)
            .derived("D", d)
            .transition(
                TransitionDef::new("T", Trigger::Auto, bind, Body::pure(|_: &Subject, _: &Content| Ok(vec![])))
                    .consumes("P"),
            );
        let mut exec = load(net)?;
        let _ = no_panic("write P", || format!("{:?}", exec.write("P", &s, Content::new(), Duration::ZERO)))?;
        let _ = no_panic("step", || format!("{:?}", exec.step(Duration::from_secs(1))))?;
        let other = no_panic("derive without the token", || exec.derive("D", &Subject::new(&t)))?;
        prop_assert!(matches!(other, Ok(None)), "derive of D for {t:?}, which holds no P token: {other:?}");
        let kept = exec.tokens().subjects("P").iter().any(|h| h.as_str() == s);
        prop_assert_eq!(kept, read_by_t, "T fires exactly when its bind does not meet the cycle");
        if !read_by_t {
            return Ok(());
        }
        let met = no_panic("derive into the cycle", || exec.derive("D", &Subject::new(&s)))?;
        prop_assert!(names_error(&met, "Cycle"), "derive of D for {s:?}, which meets the cycle, answered {met:?}");
        let all = no_panic("derive_all into the cycle", || exec.derive_all("D"))?;
        prop_assert!(names_error(&all, "Cycle"), "derive_all of D, which meets the cycle, answered {all:?}");
    }
}

// ------------------------------------ a work bind with an answer field

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A lowered work whose binding produces the answer id or the seen
    /// field: answering its ask never panics, and the work never fires.
    #[test]
    fn a_work_binding_an_answer_field_never_fires(
        field in select(vec![ANSWER_ID_KEY, SEEN_FIELD]),
        v in value(),
        extra in prop::collection::btree_map("[a-z]{1,3}", value(), 0..2),
        effectful in any::<bool>(),
        s in subject(),
    ) {
        let mut out = extra;
        out.insert(field.to_owned(), v);
        let bind: holon_net::exec::Read = Arc::new(move |_: &mut Reader<'_>, _: &Subject| Some(out.clone()));
        let mut exec = load(ask_net(NetDef::default(), bind, effectful)?)?;
        let _ = premise("the write", no_panic("write", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let seen = offered(&exec, &s)?.unwrap_or(AskHash([0; 32]));
        let r = no_panic("answer", || exec.answer(answer_call(&s, seen, "a1"), Duration::from_secs(1)))?;
        let mut requests = r.map(|q| q.requests).unwrap_or_default();
        requests.extend(no_panic("step", || exec.step(Duration::from_secs(2)))?.unwrap_or_default());
        prop_assert!(exec.tokens().subjects("Out").is_empty(), "{W} fired although its binding produces {field:?}");
        prop_assert_eq!(effect_requests_of(&requests, W), 0, "{} was requested although its binding produces {:?}", W, field);
        let why = no_panic("why_not", || exec.why_not(W, &s))?;
        prop_assert!(matches!(why, Ok(Some(_))), "why_not of {W} reads {why:?}");
    }
}

// ------------------------------------------------ a now past i64 nanoseconds

fn far() -> impl Strategy<Value = Duration> {
    prop_oneof![
        (1u64..1_000).prop_map(|d| Duration::from_nanos(I64_NS) + Duration::from_nanos(d)),
        (I64_NS / 1_000_000_000 + 1..=u64::MAX, 0u32..1_000_000_000)
            .prop_map(|(s, n)| Duration::new(s, n)),
        Just(Duration::MAX),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Every call that takes a `now` past the i64 nanosecond range is
    /// refused and changes nothing.
    #[test]
    fn a_call_past_the_i64_nanosecond_range_is_refused_and_changes_nothing(
        now in far(),
        which in 0usize..8,
        manual in any::<bool>(),
        s in subject(),
    ) {
        let mut exec = load(owned_net(if manual { Trigger::Manual } else { Trigger::Auto }, true)?)?;
        let _ = premise("the write into P", no_panic("write P", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let req = start_effect(&mut exec, manual, &s)?;
        let seen = offered(&exec, &s)?.unwrap_or(AskHash([0; 32]));
        let shown = owned_shown();
        let before = snapshot(&exec, &shown);
        let r = no_panic(&format!("call {which} at {now:?}"), || match which {
            0 => format!("{:?}", exec.fire("T", &s, OccurrenceId::caller("o2"), Content::new(), now)),
            1 => format!("{:?}", exec.step(now)),
            2 => format!("{:?}", exec.write("In", &s, Content::new(), now)),
            3 => format!("{:?}", exec.retract("P", &s, now)),
            4 => format!("{:?}", exec.complete(req.attempt, EffectResult::Done(vec![]), now)),
            5 => format!("{:?}", exec.resolve(&req.occurrence, Resolution::Drop, now)),
            6 => format!("{:?}", exec.undo(&OccurrenceId::caller("o1"), now)),
            _ => format!("{:?}", exec.answer(answer_call(&s, seen, "a1"), now)),
        })?;
        prop_assert!(r.starts_with("Err") && r.contains("ClockOutOfRange"), "call {which} at {now:?} was not refused as ClockOutOfRange: {r}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "call {} past the range changed the executor", which);
    }

    /// A `now` in range whose backoff, run timer or abandon timer ends past
    /// the i64 nanosecond range: the call is refused and changes nothing,
    /// since a saturated due time would be a wrong value.
    #[test]
    fn a_timer_ending_past_the_i64_nanosecond_range_is_refused_and_changes_nothing(
        back_ns in 0u64..5_000_000_000,
        timer_ms in 1u64..5_000,
        kind in 0usize..3,
        s in subject(),
    ) {
        let timer = Duration::from_millis(timer_ms);
        let t = match kind {
            0 => TransitionDef::new("T", Trigger::Auto, bind_true(), Body::pure(|_: &Subject, _: &Content| Err(Cause::retryable("boom"))))
                .retry(timer, 3),
            1 => TransitionDef::new("T", Trigger::Auto, bind_true(), Body::Effectful(Receiver::None)).run_timer(timer),
            _ => TransitionDef::new("T", Trigger::Auto, bind_true(), Body::Effectful(Receiver::None)).abandon(timer),
        };
        let mut exec = load(NetDef::default().fifo("In", Class::Process).transition(t.consumes("In")))?;
        let now = Duration::from_nanos(I64_NS - back_ns);
        let shown = places(&["In"], &["T"]);
        let before = snapshot(&exec, &shown);
        let w = no_panic("write at the edge", || format!("{:?}", exec.write("In", &s, Content::new(), now)))?;
        if now + timer > Duration::from_nanos(I64_NS) {
            prop_assert!(w.contains("ClockOutOfRange"), "the write at {now:?} with a timer of {timer:?} was not refused as ClockOutOfRange: {w}");
            prop_assert_eq!(&before, &snapshot(&exec, &shown), "the refused write changed the executor");
        }
        let before = snapshot(&exec, &shown);
        let end = no_panic("step at the end", || format!("{:?}", exec.step(Duration::from_nanos(I64_NS))))?;
        prop_assert!(end.contains("ClockOutOfRange"), "the step at the end with a timer of {timer:?} was not refused as ClockOutOfRange: {end}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused step changed the executor");
    }
}

// ------------------------- an answer whose Asks derive reads an undeclared
// place

/// The read of `ghost` a work binding makes, directly, through `traced` or
/// through the derived `D`.
fn ghost_read(r: &mut Reader<'_>, ghost: &str, s: &Subject, kind: usize) {
    match kind {
        0 => {
            r.field(ghost, s, "x");
        }
        1 => {
            r.text(ghost, s, "x");
        }
        2 => {
            r.present(ghost, s);
        }
        3 => {
            r.free(ghost);
        }
        4 => {
            r.traced(|r| r.present(ghost, s));
        }
        _ => {
            r.present("D", s);
        }
    }
}

fn ghost_net(ghost: &str, kind: usize) -> Result<NetDef, TestCaseError> {
    let g = ghost.to_owned();
    let d = ghost.to_owned();
    let bind: holon_net::exec::Read = Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
        ghost_read(r, &g, s, kind);
        Some(Content::new())
    });
    let net = NetDef::default().derived(
        "D",
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| r.present(&d, s).then(Content::new)),
    );
    ask_net(net, bind, false)
}

fn ghost_declared() -> Vec<String> {
    let mut all = ask_places();
    all.extend(["D".to_owned(), Generated::Asks.name(W)]);
    all
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// An answer to an ask whose work binding reads a place the net does not
    /// declare is refused and changes nothing; the work never fires, and
    /// `why_not` of the answer transition names the read.
    #[test]
    fn an_answer_whose_ask_reads_an_undeclared_place_is_refused(
        ghost in undeclared(&ghost_declared()),
        kind in 0usize..6,
        s in subject(),
        hash in any::<[u8; 32]>(),
    ) {
        let mut exec = load(ghost_net(&ghost, kind)?)?;
        let _ = premise("the write", no_panic("write", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let shown = ask_places();
        let before = snapshot(&exec, &shown);
        let r = no_panic("answer", || exec.answer(answer_call(&s, AskHash(hash), "a1"), Duration::from_secs(1)))?;
        prop_assert!(names_error(&r, "ReadsUndeclared"), "the answer to an ask that reads {ghost:?} was not refused as ReadsUndeclared: {r:?}");
        prop_assert_eq!(before, snapshot(&exec, &shown), "the refused answer changed the executor");
        let _ = no_panic("step", || exec.step(Duration::from_secs(2)))?;
        prop_assert!(exec.tokens().subjects("Out").is_empty(), "{W} fired although its binding reads {ghost:?}");
        let why = no_panic("why_not", || exec.why_not(&Generated::Answer.name(W), &s))?;
        let text = format!("{why:?}");
        prop_assert!(
            text.contains("ReadsUndeclared"),
            "why_not of {} does not name the read of {:?}: {}",
            Generated::Answer.name(W),
            ghost,
            text
        );
    }

    /// Exclusion: the same ask over declared places takes the answer and its
    /// work fires.
    #[test]
    fn an_answer_whose_ask_reads_declared_places_fires_the_work(s in subject()) {
        let mut exec = load(ghost_net("P", 2)?)?;
        let _ = premise("the write", no_panic("write", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let seen = offered(&exec, &s)?;
        prop_assert!(seen.is_some(), "premise: the ask of {s:?} is offered");
        let r = no_panic("answer", || exec.answer(answer_call(&s, seen.unwrap_or(AskHash([0; 32])), "a1"), Duration::from_secs(1)))?;
        prop_assert!(r.is_ok(), "the answer was refused: {r:?}");
        prop_assert_eq!(exec.tokens().contents("Out", &s), vec![tagged(W)]);
    }
}

// ------------------------------------------- inputs out of this card's scope

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// A caller value that holds a removal write intent panics when a read
    /// trace hashes it; out of this card's scope. Run with `--run-ignored`.
    #[test]
    #[ignore = "out of scope: a removal write intent in a token panics"]
    fn a_caller_value_holding_a_removal_does_not_panic(s in subject(), manual in any::<bool>()) {
        let bind: holon_net::exec::Read = Arc::new(|r: &mut Reader<'_>, s: &Subject| {
            r.field("In", s, "x");
            None
        });
        let t = TransitionDef::new(
            "T",
            if manual { Trigger::Manual } else { Trigger::Auto },
            bind,
            Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
        )
        .consumes("In");
        let mut exec = load(NetDef::default().fifo("In", Class::Process).transition(t))?;
        let removal = Content::from([("x".to_owned(), Value::Removed(RemovedTag))]);
        let _ = no_panic("write", || format!("{:?}", exec.write("In", &s, removal, Duration::ZERO)))?;
        let _ = no_panic("why_not", || format!("{:?}", exec.why_not("T", &s)))?;
        let _ = no_panic("fire", || {
            format!("{:?}", exec.fire("T", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_secs(1)))
        })?;
    }
}

// ------------------------- a fire of an answer transition whose occurrence is
// not the answer id of its value

fn answer_occurrence(kind: usize, id: &str) -> OccurrenceId {
    match kind {
        0 => OccurrenceId::caller(id),
        1 => OccurrenceId::Minted {
            transition: Arc::from(Generated::Answer.name(W)),
            n: 1,
        },
        _ => OccurrenceId::Work {
            transition: Arc::from(W),
            answer: Arc::from(id),
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// A manual fire of `Answer.W` is taken only as the caller occurrence
    /// whose id its value names under the answer id key; any other is
    /// refused as AnswerOccurrence and changes nothing.
    #[test]
    fn a_fire_of_an_answer_as_another_occurrence_is_refused_and_changes_nothing(
        kind in 0usize..3,
        id in prop_oneof![3 => Just("a1".to_owned()), 1 => ".{1,4}"],
        named in prop_oneof![
            Just(None),
            Just(Some(Value::String("a1".to_owned()))),
            ".{0,4}".prop_map(|v| Some(Value::String(v))),
            value().prop_map(Some),
        ],
        effectful in any::<bool>(),
        s in subject(),
    ) {
        let mut exec = load(ask_net(NetDef::default(), bind_true(), effectful)?)?;
        let _ = premise("the write into P", no_panic("write P", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let mut value = Content::new();
        if let Some(v) = &named {
            value.insert(ANSWER_ID_KEY.to_owned(), v.clone());
        }
        let answer = Generated::Answer.name(W);
        let shown = ask_places();
        let before = snapshot(&exec, &shown);
        let occurrence = answer_occurrence(kind, &id);
        let r = no_panic("fire Answer.W", || exec.fire(&answer, &s, occurrence, value, Duration::from_secs(1)))?;
        if kind == 0 && named == Some(Value::String(id.clone())) {
            prop_assert!(r.is_ok(), "the fire as the named caller occurrence {id:?} was refused: {r:?}");
        } else {
            prop_assert!(names_error(&r, "AnswerOccurrence"), "the fire as occurrence kind {kind} naming {named:?} was not refused as AnswerOccurrence: {r:?}");
            prop_assert_eq!(before, snapshot(&exec, &shown), "the refused fire changed the executor");
        }
    }
}

// ------------------------- a derived cycle that a marking reveals, by call

/// `D` reads itself while `P` holds a token of the subject; the auto `T`
/// over `In` reads `D` when `t_reads`, the ask `W` over `P` when `w_reads`.
fn revealed_cycle_net(kind: usize, t_reads: bool, w_reads: bool) -> Result<NetDef, TestCaseError> {
    let d: holon_net::exec::Read = Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
        (r.present("P", s) && reads(r, "D", s, kind)).then(Content::new)
    });
    let t_bind = if t_reads {
        reading("D", kind)
    } else {
        bind_true()
    };
    let net = NetDef::default()
        .fifo("In", Class::Process)
        .derived("D", d)
        .transition(
            TransitionDef::new(
                "T",
                Trigger::Auto,
                t_bind,
                Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
            )
            .consumes("In"),
        );
    let w_bind = if w_reads {
        reading("D", kind)
    } else {
        bind_true()
    };
    ask_net(net, w_bind, false)
}

fn cycle_conditions_of_t(exec: &Executor) -> usize {
    exec.conditions()
        .iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| c.contains("DerivedCycle") && c.contains("transition: \"T\""))
        .count()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A cycle the initial marking hides loads; once `P` holds the subject,
    /// every call that evaluates the cycle names it: the auto `T` does not
    /// fire and discloses it once, why_not, derive, derive_all, asks and
    /// answer return their cycle forms, and a refused answer changes nothing.
    #[test]
    fn every_call_that_meets_a_revealed_derived_cycle_names_it(
        kind in 0usize..3,
        t_reads in any::<bool>(),
        w_reads in any::<bool>(),
        s in subject(),
        t in subject(),
    ) {
        prop_assume!(s != t);
        let mut exec = load(revealed_cycle_net(kind, t_reads, w_reads)?)?;
        let _ = premise("the write into P", no_panic("write P", || exec.write("P", &s, Content::new(), Duration::ZERO))?)?;
        let _ = premise("the write into In", no_panic("write In", || exec.write("In", &s, Content::new(), Duration::ZERO))?)?;
        for at in [1, 2] {
            let _ = no_panic("step", || format!("{:?}", exec.step(Duration::from_secs(at))))?;
            if t_reads {
                prop_assert_eq!(exec.tokens().subjects("In"), vec![Subject::new(&s)], "T fired through the cycle");
                prop_assert_eq!(cycle_conditions_of_t(&exec), 1, "the cycle of T disclosed once: {:?}", exec.conditions());
            } else {
                prop_assert!(exec.tokens().subjects("In").is_empty(), "T, which does not read D, did not fire");
                prop_assert_eq!(cycle_conditions_of_t(&exec), 0, "a cycle disclosed for T, which does not read D");
            }
        }
        if t_reads {
            let why = no_panic("why_not T", || exec.why_not("T", &s))?;
            prop_assert!(matches!(&why, Ok(Some(_))) && format!("{why:?}").contains("DerivedCycle"), "why_not of T: {why:?}");
        }
        let met = no_panic("derive into the cycle", || exec.derive("D", &Subject::new(&s)))?;
        prop_assert!(names_error(&met, "Cycle"), "derive of D for {s:?}: {met:?}");
        let all = no_panic("derive_all", || exec.derive_all("D"))?;
        prop_assert!(names_error(&all, "Cycle"), "derive_all of D: {all:?}");
        let other = no_panic("derive without the token", || exec.derive("D", &Subject::new(&t)))?;
        prop_assert!(matches!(other, Ok(None)), "derive of D for {t:?}: {other:?}");
        let asks = no_panic("asks", || exec.asks())?;
        if w_reads {
            prop_assert!(names_error(&asks, "Cycle"), "asks with W reading D: {asks:?}");
            let mut shown = ask_places();
            shown.extend(places(&["In"], &["T"]));
            let before = snapshot(&exec, &shown);
            let r = no_panic("answer", || exec.answer(answer_call(&s, AskHash([0; 32]), "a1"), Duration::from_secs(3)))?;
            prop_assert!(names_error(&r, "DerivedCycle"), "answer through the cycle: {r:?}");
            prop_assert_eq!(before, snapshot(&exec, &shown), "the refused answer changed the executor");
        } else {
            prop_assert!(asks.is_ok(), "asks with W not reading D: {asks:?}");
        }
    }
}

// --------------------------- ports and arcs on derived and executor places

/// A fact, a failure port, a timeout port, a produces arc or a consume arc
/// of `T` into `place`.
fn link(t: TransitionDef, kind: usize, place: &str) -> TransitionDef {
    match kind {
        0 => t.fact(place),
        1 => t.on_failure(place),
        2 => t.on_timeout(place),
        3 => t.produces(place),
        _ => t.consumes(place),
    }
}

/// An effectful `T` with a run timer over `In`, a manual pure `T2` over
/// `In2`, a fifo `Out` and a derived `D`; `edit` adds links to `T`.
fn linked_net(trigger: Trigger, edit: impl FnOnce(TransitionDef) -> TransitionDef) -> NetDef {
    let t = TransitionDef::new("T", trigger, bind_true(), Body::Effectful(Receiver::None))
        .run_timer(Duration::from_secs(1))
        .consumes("In");
    let t2 = TransitionDef::new(
        "T2",
        Trigger::Manual,
        bind_true(),
        Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
    )
    .consumes("In2");
    NetDef::default()
        .fifo("In", Class::Process)
        .fifo("In2", Class::Process)
        .fifo("Out", Class::Process)
        .derived("D", Arc::new(|_: &mut Reader<'_>, _: &Subject| None))
        .transition(edit(t))
        .transition(t2)
}

fn executor_place() -> impl Strategy<Value = String> {
    select(places(&[], &["T", "T2"]))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A fact, failure port or timeout port into a derived place does not
    /// load and names the place.
    #[test]
    fn a_port_into_a_derived_place_does_not_load(kind in 0usize..3, trigger in trigger()) {
        let r = no_panic("load", || Executor::new(linked_net(trigger, |t| link(t, kind, "D"))).map(|_| ()))?;
        prop_assert!(names_error(&r, "ArcOnDerived") && names_error(&r, "\"D\""), "a port into D: {r:?}");
    }

    /// A put (fact, failure port, timeout port, produces) or a take on a
    /// place only the executor writes does not load and names the place.
    #[test]
    fn a_port_or_take_on_an_executor_place_does_not_load(
        kind in 0usize..5,
        place in executor_place(),
        trigger in trigger(),
    ) {
        let r = no_panic("load", || Executor::new(linked_net(trigger, |t| link(t, kind, &place))).map(|_| ()))?;
        prop_assert!(
            names_error(&r, "WritesExecutorPlace") && names_error(&r, &format!("{place:?}")),
            "link kind {kind} on {place:?}: {r:?}"
        );
    }

    /// Exclusion: the same links on the stored `Out` load.
    #[test]
    fn a_port_or_take_on_a_stored_place_loads(kind in 0usize..5, trigger in trigger()) {
        let r = no_panic("load", || Executor::new(linked_net(trigger, |t| link(t, kind, "Out"))).map(|_| ()))?;
        prop_assert!(r.is_ok(), "link kind {kind} on Out: {r:?}");
    }

    /// Exclusion: a read or inhibit arc of the auto `T3` over `In2` on an
    /// executor place loads, and gates `T3` by that place: while `T` runs
    /// for `s` only `T.Running` holds `s`, so `T3` fires exactly when it
    /// reads `T.Running` or inhibits an empty place; no later call panics.
    #[test]
    fn a_read_or_inhibit_on_an_executor_place_loads_and_gates(
        inhibit in any::<bool>(),
        place in executor_place(),
        s in subject(),
    ) {
        let t3 = TransitionDef::new("T3", Trigger::Auto, bind_true(), Body::pure(|s: &Subject, _: &Content| {
            Ok(vec![Output::new("Out", s, tagged("T3"))])
        }))
        .consumes("In2");
        let t3 = if inhibit { t3.inhibits(&place) } else { t3.reads(&place) };
        let mut exec = load(linked_net(Trigger::Auto, |t| t).transition(t3))?;
        let _ = premise("the write into In", no_panic("write In", || exec.write("In", &s, Content::new(), Duration::ZERO))?)?;
        prop_assert_eq!(exec.tokens().subjects("T.Running"), vec![Subject::new(&s)], "premise: T runs for s");
        let _ = premise("the write into In2", no_panic("write In2", || exec.write("In2", &s, Content::new(), Duration::ZERO))?)?;
        let fired = !exec.tokens().subjects("Out").is_empty();
        prop_assert_eq!(fired, (place == "T.Running") != inhibit, "T3 with {} on {:?}", if inhibit { "inhibit" } else { "read" }, place);
        let _ = no_panic("fire T2", || format!("{:?}", exec.fire("T2", &s, OccurrenceId::caller("o1"), Content::new(), Duration::from_millis(500))))?;
        let _ = no_panic("step past the run timer", || format!("{:?}", exec.step(Duration::from_secs(2))))?;
        let _ = no_panic("why_not T3", || format!("{:?}", exec.why_not("T3", &s)))?;
        let _ = no_panic("step", || format!("{:?}", exec.step(Duration::from_secs(3))))?;
    }
}

// ------------------------------------------- timers past the i64 ns range

/// A run, abandon or backoff timer of `T` of `timer`.
fn timed_net(kind: usize, timer: Duration) -> NetDef {
    let t = TransitionDef::new(
        "T",
        Trigger::Auto,
        bind_true(),
        Body::Effectful(Receiver::None),
    )
    .consumes("In");
    let t = match kind {
        0 => t.run_timer(timer),
        1 => t.abandon(timer),
        _ => t.retry(timer, 2),
    };
    NetDef::default().fifo("In", Class::Process).transition(t)
}

fn past_i64_ns() -> impl Strategy<Value = Duration> {
    prop_oneof![
        (1..=u64::MAX - I64_NS)
            .prop_map(|over| Duration::from_nanos(I64_NS) + Duration::from_nanos(over)),
        (I64_NS / 1_000_000_000 + 1..=u64::MAX, 0u32..1_000_000_000)
            .prop_map(|(secs, nanos)| Duration::new(secs, nanos)),
        Just(Duration::MAX),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// A timer longer than i64::MAX ns does not load.
    #[test]
    fn a_timer_longer_than_the_i64_nanosecond_range_does_not_load(kind in 0usize..3, timer in past_i64_ns()) {
        let r = no_panic("load", || Executor::new(timed_net(kind, timer)).map(|_| ()))?;
        prop_assert!(names_error(&r, "TimerOutOfRange"), "timer kind {kind} of {timer:?}: {r:?}");
    }

    /// Exclusion: a timer of up to i64::MAX ns loads.
    #[test]
    fn a_timer_within_the_i64_nanosecond_range_loads(kind in 0usize..3, back in prop_oneof![Just(0u64), 0..I64_NS]) {
        let timer = Duration::from_nanos(I64_NS - back);
        let r = no_panic("load", || Executor::new(timed_net(kind, timer)).map(|_| ()))?;
        prop_assert!(r.is_ok(), "timer kind {kind} of {timer:?}: {r:?}");
    }
}

// ---------------------- a derived recursion over new subjects at the load

/// `D` for `s` reads `D` for `s` plus one `x`, while `s` is shorter than
/// `bound` (`None`: always); at the bottom it holds `x`.
fn recursion_net(kind: usize, bound: Option<usize>) -> NetDef {
    let d: holon_net::exec::Read = Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
        let x = || Content::from([("x".to_owned(), Value::String("1".to_owned()))]);
        if bound.is_some_and(|b| s.as_str().len() >= b) {
            return Some(x());
        }
        reads(r, "D", &Subject::new(&format!("{}x", s.as_str())), kind).then(x)
    });
    NetDef::default()
        .fifo("In", Class::Process)
        .derived("D", d)
        .transition(
            TransitionDef::new(
                "T",
                Trigger::Auto,
                bind_true(),
                Body::pure(|_: &Subject, _: &Content| Ok(vec![])),
            )
            .consumes("In"),
        )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

    /// A derived recursion that never repeats a row and never ends is
    /// refused at load as a derived cycle, never a stack overflow.
    #[test]
    fn a_derived_recursion_over_new_subjects_does_not_load(kind in 0usize..3) {
        let r = no_panic("load", || Executor::new(recursion_net(kind, None)).map(|_| ()))?;
        prop_assert!(names_error(&r, "DerivedCycle"), "the endless recursion: {r:?}");
    }

    /// A recursion of `bound + 1` nested derived rows: up to the nesting
    /// bound of 128 rows it loads and derives; deeper it is refused at
    /// load as a derived cycle.
    #[test]
    fn a_bounded_derived_recursion_loads_up_to_the_nesting_bound(
        kind in 0usize..3,
        bound in prop_oneof![1usize..64, select(vec![126usize, 127, 128, 129, 200])],
    ) {
        let r = no_panic("load", || Executor::new(recursion_net(kind, Some(bound))))?;
        if bound < MAX_NESTED_DERIVED_ROWS {
            let exec = premise("the bounded recursion loads", r)?;
            let d = no_panic("derive", || exec.derive("D", &Subject::new("")))?;
            prop_assert!(matches!(d, Ok(Some(_))), "derive of {} nested rows: {d:?}", bound + 1);
        } else {
            let r = r.map(|_| ());
            prop_assert!(names_error(&r, "DerivedCycle"), "{} nested rows: {r:?}", bound + 1);
        }
    }
}

// ------------------------------------- ports into the answer places of an ask

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// A failure or timeout port into `Answered.W` or `Expired.W` puts a
    /// cause token without the answer fields: the net does not load and
    /// names the place.
    #[test]
    fn a_port_into_an_answer_place_does_not_load(
        timeout in any::<bool>(),
        expired in any::<bool>(),
        trigger in trigger(),
    ) {
        let place = if expired { Generated::Expired.name(W) } else { Generated::Answered.name(W) };
        let t = TransitionDef::new("T", trigger, bind_true(), Body::Effectful(Receiver::None))
            .run_timer(Duration::from_secs(1))
            .consumes("In");
        let t = if timeout { t.on_timeout(&place) } else { t.on_failure(&place) };
        let net = ask_net(NetDef::default().fifo("In", Class::Process), bind_true(), false)?.transition(t);
        let r = no_panic("load", || Executor::new(net).map(|_| ()))?;
        prop_assert!(
            names_error(&r, "PortLacksAnswerField") && names_error(&r, &format!("{place:?}")),
            "a {} port into {place:?}: {r:?}", if timeout { "timeout" } else { "failure" }
        );
    }
}
