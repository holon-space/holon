//! The text of every executor error names the net's own words: places,
//! transitions, fields. No text names a Rust type parameter such as `<V>`,
//! which means nothing to the author of a net file.
//!
//! The errors are drawn variant by variant. Each `*_values` function lists
//! the drawn text of one error with an exhaustive `match`, so a new variant
//! does not compile until the generator draws it.

use std::sync::Arc;
use std::time::Duration;

use holon_net::NetLoadError;
use holon_net::exec::AttemptId;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::ClockOutOfRange;
use holon_net::exec::ClockWentBack;
use holon_net::exec::DeriveError;
use holon_net::exec::ExecError;
use holon_net::exec::FireError;
use holon_net::exec::Holder;
use holon_net::exec::LoadError;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::exec::UndoError;
use holon_net::exec::UnknownTransition;
use holon_net::exec::WhyNot;
use holon_net::exec::ask_places;
use holon_net::exec::new_net;
use holon_net::file::NetFileError;
use holon_net::file::PlaceCapacity;
use holon_net::file::Section;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;

use crate::exec_support::load;
use crate::exec_support::names::other_name;

#[derive(Debug)]
enum AnyError {
    Load(LoadError),
    Fire(FireError),
    Exec(ExecError),
    Derive(DeriveError),
    Undo(UndoError),
    NetLoad(NetLoadError),
    NetFile(NetFileError),
}

impl AnyError {
    fn text(&self) -> String {
        match self {
            Self::Load(e) => e.to_string(),
            Self::Fire(e) => e.to_string(),
            Self::Exec(e) => e.to_string(),
            Self::Derive(e) => e.to_string(),
            Self::Undo(e) => e.to_string(),
            Self::NetLoad(e) => e.to_string(),
            Self::NetFile(e) => e.to_string(),
        }
    }

    fn values(&self) -> Vec<String> {
        match self {
            Self::Load(e) => load_values(e),
            Self::Fire(e) => fire_values(e),
            Self::Exec(e) => exec_values(e),
            Self::Derive(e) => derive_values(e),
            Self::Undo(e) => undo_values(e),
            Self::NetLoad(e) => net_load_values(e),
            Self::NetFile(e) => net_file_values(e),
        }
    }
}

/// Holds when the text of `e`, with every drawn value removed, names no
/// type parameter. A drawn name such as `<V>` is the caller's word, not
/// the executor's.
fn names_no_type_parameter(e: &AnyError) -> Result<(), TestCaseError> {
    let mut values = e.values();
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    let mut rest = e.text();
    for v in values.iter().filter(|v| !v.is_empty()) {
        rest = rest.replace(v.as_str(), "");
        rest = rest.replace(&v.escape_debug().to_string(), "");
    }
    prop_assert!(
        type_parameter(&rest).is_none(),
        "{e:?} names the type parameter {:?} in {:?}",
        type_parameter(&rest),
        e.text()
    );
    Ok(())
}

/// The first `<X>` in `text` whose `X` is an upper-case letter followed
/// by letters and digits.
fn type_parameter(text: &str) -> Option<&str> {
    text.match_indices('<').find_map(|(start, _)| {
        let rest = &text[start + 1..];
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric())?;
        (end > 0 && rest.as_bytes()[0].is_ascii_uppercase() && rest[end..].starts_with('>'))
            .then(|| &text[start..start + end + 2])
    })
}

#[test]
fn no_error_text_names_a_type_parameter() {
    let found = [AnyError::Load(LoadError::IncompleteAsk {
        work: "W0".to_owned(),
        missing: "Answered.W0".to_owned(),
    })];
    for e in &found {
        names_no_type_parameter(e).unwrap_or_else(|f| panic!("static example: {f}"));
    }
    TestRunner::new(Config {
        cases: 512,
        failure_persistence: None,
        ..Config::default()
    })
    .run(&any_error(), |e| names_no_type_parameter(&e))
    .unwrap();
}

/// The ask completeness check is reachable from a net: an ask whose
/// `Expired` place the net does not declare.
#[test]
fn an_ask_without_expired_is_an_incomplete_ask() {
    let net = new_net()
        .derived("Asks.W0", Arc::new(|_: &mut Reader<'_>, _: &Subject| None))
        .place("Answered.W0", Class::Process)
        .ask("W0", ask_places("W0"));
    assert_eq!(
        load(net).err(),
        Some(LoadError::IncompleteAsk {
            work: "W0".to_owned(),
            missing: "Expired.W0".to_owned(),
        })
    );
}

fn any_error() -> BoxedStrategy<AnyError> {
    prop_oneof![
        load_error().prop_map(AnyError::Load),
        fire_error().prop_map(AnyError::Fire),
        exec_error().prop_map(AnyError::Exec),
        derive_error().prop_map(AnyError::Derive),
        undo_error().prop_map(AnyError::Undo),
        net_load_error().prop_map(AnyError::NetLoad),
        net_file_error().prop_map(AnyError::NetFile),
    ]
    .boxed()
}

fn place() -> impl Strategy<Value = Place> {
    other_name().prop_map(|n| Place::new(&n))
}

fn subject() -> impl Strategy<Value = Subject> {
    other_name().prop_map(|n| Subject::new(&n))
}

fn duration() -> impl Strategy<Value = Duration> {
    any::<u64>().prop_map(Duration::from_nanos)
}

/// The words the executor puts in a `&'static str` field: arc kinds and
/// answer fields.
fn word() -> impl Strategy<Value = &'static str> {
    select(vec![
        "consumes", "reads", "inhibits", "cause", "value", "id", "hash",
    ])
}

fn class() -> impl Strategy<Value = Class> {
    select(vec![Class::Process, Class::Device, Class::Data])
}

fn cause() -> impl Strategy<Value = Cause> {
    (other_name(), any::<bool>()).prop_map(|(text, retryable)| Cause { text, retryable })
}

fn occurrence() -> impl Strategy<Value = OccurrenceId> {
    prop_oneof![
        other_name().prop_map(|id| OccurrenceId::caller(&id)),
        (other_name(), any::<u64>()).prop_map(|(t, n)| OccurrenceId::Minted {
            transition: Arc::from(t),
            n,
        }),
        (other_name(), other_name()).prop_map(|(t, a)| OccurrenceId::Work {
            transition: Arc::from(t),
            answer: Arc::from(a),
        }),
    ]
}

fn occurrence_values(o: &OccurrenceId) -> Vec<String> {
    match o {
        OccurrenceId::Caller(id) => vec![id.to_string()],
        OccurrenceId::Minted { transition, n: _ } => vec![transition.to_string()],
        OccurrenceId::Work { transition, answer } => {
            vec![transition.to_string(), answer.to_string()]
        }
    }
}

fn holder() -> impl Strategy<Value = Holder> {
    prop_oneof![
        other_name().prop_map(|n| Holder::Running(Arc::from(n))),
        occurrence().prop_map(Holder::OutcomeUnknown),
    ]
}

fn holder_values(h: &Holder) -> Vec<String> {
    match h {
        Holder::Running(n) => vec![n.to_string()],
        Holder::OutcomeUnknown(o) => occurrence_values(o),
    }
}

fn why_not() -> BoxedStrategy<WhyNot> {
    prop_oneof![
        Just(WhyNot::InFlight),
        duration().prop_map(|due| WhyNot::RetryPending { due }),
        (place(), proptest::option::of(holder()))
            .prop_map(|(place, held_by)| WhyNot::Needs { place, held_by }),
        place().prop_map(WhyNot::Inhibited),
        Just(WhyNot::Guard),
        cause().prop_map(WhyNot::FailedOnThisInput),
        place().prop_map(WhyNot::ReadsUndeclared),
        vec(place(), 0..4).prop_map(WhyNot::DerivedCycle),
        Just(WhyNot::BindsAnswerInput),
    ]
    .boxed()
}

fn why_not_values(w: &WhyNot) -> Vec<String> {
    match w {
        WhyNot::InFlight
        | WhyNot::RetryPending { due: _ }
        | WhyNot::Guard
        | WhyNot::BindsAnswerInput => vec![],
        WhyNot::Needs { place, held_by } => {
            let mut v = vec![place.to_string()];
            v.extend(held_by.iter().flat_map(holder_values));
            v
        }
        WhyNot::Inhibited(p) | WhyNot::ReadsUndeclared(p) => vec![p.to_string()],
        WhyNot::FailedOnThisInput(c) => vec![c.text.clone()],
        WhyNot::DerivedCycle(ps) => ps.iter().map(Place::to_string).collect(),
    }
}

fn load_error() -> BoxedStrategy<LoadError> {
    let n = other_name;
    prop_oneof![
        n().prop_map(LoadError::DuplicateTransition),
        place().prop_map(LoadError::ReservedPlace),
        (n(), place()).prop_map(|(user, place)| LoadError::UndeclaredPlace { user, place }),
        (n(), place()).prop_map(|(user, place)| LoadError::CandidateNotFifo { user, place }),
        (place(), class()).prop_map(|(place, class)| LoadError::ClassNotSupported { place, class }),
        (n(), place())
            .prop_map(|(transition, place)| LoadError::ConsumesData { transition, place }),
        (n(), place())
            .prop_map(|(transition, place)| LoadError::ArcOnDerived { transition, place }),
        (n(), place())
            .prop_map(|(transition, place)| LoadError::WritesExecutorPlace { transition, place }),
        (n(), place(), word()).prop_map(|(transition, place, field)| {
            LoadError::PortLacksAnswerField {
                transition,
                place,
                field,
            }
        }),
        (n(), word(), word(), place()).prop_map(|(transition, first, second, place)| {
            LoadError::InputTwice {
                transition,
                first,
                second,
                place,
            }
        }),
        n().prop_map(LoadError::NoSubject),
        n().prop_map(LoadError::KeyedWithoutCountBound),
        (n(), n()).prop_map(|(work, missing)| LoadError::IncompleteAsk { work, missing }),
        (n(), n()).prop_map(|(work, of)| LoadError::AskWorkIsAnswer { work, of }),
        vec(place(), 1..4).prop_map(LoadError::DerivedCycle),
        (n(), duration())
            .prop_map(|(transition, timer)| LoadError::TimerOutOfRange { transition, timer }),
    ]
    .boxed()
}

fn load_values(e: &LoadError) -> Vec<String> {
    match e {
        LoadError::DuplicateTransition(n)
        | LoadError::NoSubject(n)
        | LoadError::KeyedWithoutCountBound(n) => vec![n.clone()],
        LoadError::ReservedPlace(p) => vec![p.to_string()],
        LoadError::UndeclaredPlace { user, place }
        | LoadError::CandidateNotFifo { user, place } => {
            vec![user.clone(), place.to_string()]
        }
        LoadError::ClassNotSupported { place, class: _ } => vec![place.to_string()],
        LoadError::ConsumesData { transition, place }
        | LoadError::ArcOnDerived { transition, place }
        | LoadError::WritesExecutorPlace { transition, place }
        | LoadError::PortLacksAnswerField {
            transition,
            place,
            field: _,
        }
        | LoadError::InputTwice {
            transition,
            first: _,
            second: _,
            place,
        } => vec![transition.clone(), place.to_string()],
        LoadError::IncompleteAsk { work, missing } => vec![work.clone(), missing.clone()],
        LoadError::AskWorkIsAnswer { work, of } => vec![work.clone(), of.clone()],
        LoadError::DerivedCycle(ps) => ps.iter().map(Place::to_string).collect(),
        LoadError::TimerOutOfRange {
            transition,
            timer: _,
        } => vec![transition.clone()],
    }
}

fn clock_went_back() -> impl Strategy<Value = ClockWentBack> {
    (duration(), duration()).prop_map(|(clock, now)| ClockWentBack { clock, now })
}

fn clock_out_of_range() -> impl Strategy<Value = ClockOutOfRange> {
    (duration(), duration()).prop_map(|(now, last)| ClockOutOfRange { now, last })
}

fn fire_error() -> BoxedStrategy<FireError> {
    let n = other_name;
    prop_oneof![
        n().prop_map(|t| FireError::UnknownTransition(UnknownTransition(t))),
        clock_went_back().prop_map(FireError::ClockWentBack),
        clock_out_of_range().prop_map(FireError::ClockOutOfRange),
        n().prop_map(FireError::NotManual),
        (n(), n()).prop_map(|(transition, field)| FireError::ValueCollides { transition, field }),
        (n(), occurrence()).prop_map(|(transition, occurrence)| FireError::AnswerOccurrence {
            transition,
            occurrence,
        }),
        occurrence().prop_map(FireError::DuplicateOccurrence),
        (n(), subject(), why_not()).prop_map(|(transition, subject, why)| FireError::NotEnabled {
            transition,
            subject,
            why,
        }),
        any::<usize>().prop_map(FireError::NoQuiescence),
    ]
    .boxed()
}

fn fire_values(e: &FireError) -> Vec<String> {
    match e {
        FireError::UnknownTransition(UnknownTransition(t)) | FireError::NotManual(t) => {
            vec![t.clone()]
        }
        FireError::ClockWentBack(ClockWentBack { clock: _, now: _ })
        | FireError::ClockOutOfRange(ClockOutOfRange { now: _, last: _ })
        | FireError::NoQuiescence(_) => vec![],
        FireError::ValueCollides { transition, field } => vec![transition.clone(), field.clone()],
        FireError::AnswerOccurrence {
            transition,
            occurrence,
        } => {
            let mut v = vec![transition.clone()];
            v.extend(occurrence_values(occurrence));
            v
        }
        FireError::DuplicateOccurrence(o) => occurrence_values(o),
        FireError::NotEnabled {
            transition,
            subject,
            why,
        } => {
            let mut v = vec![transition.clone(), subject.to_string()];
            v.extend(why_not_values(why));
            v
        }
    }
}

fn exec_error() -> BoxedStrategy<ExecError> {
    let n = other_name;
    prop_oneof![
        clock_went_back().prop_map(ExecError::ClockWentBack),
        clock_out_of_range().prop_map(ExecError::ClockOutOfRange),
        n().prop_map(ExecError::UndeclaredPlace),
        n().prop_map(ExecError::DerivedPlace),
        n().prop_map(ExecError::ExecutorPlace),
        (n(), word()).prop_map(|(place, field)| ExecError::AnswerField { place, field }),
        n().prop_map(|place| ExecError::PoolFull { place }),
        (n(), n()).prop_map(|(place, subject)| ExecError::NothingToRetract { place, subject }),
        any::<usize>().prop_map(ExecError::NoQuiescence),
        any::<u64>().prop_map(|a| ExecError::UnknownAttempt(AttemptId(a))),
        occurrence().prop_map(ExecError::NotParked),
        (n(), subject()).prop_map(|(transition, subject)| ExecError::RetryWhileRunning {
            transition,
            subject,
        }),
    ]
    .boxed()
}

fn exec_values(e: &ExecError) -> Vec<String> {
    match e {
        ExecError::ClockWentBack(ClockWentBack { clock: _, now: _ })
        | ExecError::ClockOutOfRange(ClockOutOfRange { now: _, last: _ })
        | ExecError::NoQuiescence(_)
        | ExecError::UnknownAttempt(AttemptId(_)) => vec![],
        ExecError::UndeclaredPlace(p)
        | ExecError::DerivedPlace(p)
        | ExecError::ExecutorPlace(p)
        | ExecError::AnswerField { place: p, field: _ }
        | ExecError::PoolFull { place: p } => vec![p.clone()],
        ExecError::NothingToRetract { place, subject } => vec![place.clone(), subject.clone()],
        ExecError::NotParked(o) => occurrence_values(o),
        ExecError::RetryWhileRunning {
            transition,
            subject,
        } => vec![transition.clone(), subject.to_string()],
    }
}

fn derive_error() -> BoxedStrategy<DeriveError> {
    let n = other_name;
    prop_oneof![
        n().prop_map(DeriveError::UndeclaredPlace),
        n().prop_map(DeriveError::StoredPlace),
        (place(), place()).prop_map(|(place, read)| DeriveError::ReadsUndeclared { place, read }),
        (place(), vec(place(), 0..4))
            .prop_map(|(place, cycle)| DeriveError::Cycle { place, cycle }),
        (place(), n(), n()).prop_map(|(place, subject, reason)| DeriveError::AskRowHash {
            place,
            subject,
            reason,
        }),
    ]
    .boxed()
}

fn derive_values(e: &DeriveError) -> Vec<String> {
    match e {
        DeriveError::UndeclaredPlace(p) | DeriveError::StoredPlace(p) => vec![p.clone()],
        DeriveError::ReadsUndeclared { place, read } => vec![place.to_string(), read.to_string()],
        DeriveError::Cycle { place, cycle } => std::iter::once(place)
            .chain(cycle)
            .map(Place::to_string)
            .collect(),
        DeriveError::AskRowHash {
            place,
            subject,
            reason,
        } => vec![place.to_string(), subject.clone(), reason.clone()],
    }
}

fn undo_error() -> BoxedStrategy<UndoError> {
    prop_oneof![
        clock_went_back().prop_map(UndoError::ClockWentBack),
        clock_out_of_range().prop_map(UndoError::ClockOutOfRange),
        occurrence().prop_map(UndoError::UnknownOccurrence),
        occurrence().prop_map(UndoError::NotAnAnswer),
        occurrence().prop_map(UndoError::WorkUnsettled),
        occurrence().prop_map(UndoError::AlreadyUndone),
        any::<usize>().prop_map(UndoError::NoQuiescence),
    ]
    .boxed()
}

fn undo_values(e: &UndoError) -> Vec<String> {
    match e {
        UndoError::ClockWentBack(ClockWentBack { clock: _, now: _ })
        | UndoError::ClockOutOfRange(ClockOutOfRange { now: _, last: _ })
        | UndoError::NoQuiescence(_) => vec![],
        UndoError::UnknownOccurrence(o)
        | UndoError::NotAnAnswer(o)
        | UndoError::WorkUnsettled(o)
        | UndoError::AlreadyUndone(o) => occurrence_values(o),
    }
}

fn net_load_error() -> BoxedStrategy<NetLoadError> {
    let n = other_name;
    prop_oneof![
        (n(), n()).prop_map(|(path, message)| NetLoadError::Unreadable { path, message }),
        (n(), net_file_error()).prop_map(|(at, source)| NetLoadError::Refused { at, source }),
        (n(), n(), n()).prop_map(|(at, transition, reason)| NetLoadError::Logic {
            at,
            transition,
            reason,
        }),
    ]
    .boxed()
}

fn net_load_values(e: &NetLoadError) -> Vec<String> {
    match e {
        NetLoadError::Unreadable { path, message } => vec![path.clone(), message.clone()],
        NetLoadError::Refused { at, source } => {
            let mut v = vec![at.clone()];
            v.extend(net_file_values(source));
            v
        }
        NetLoadError::Logic {
            at,
            transition,
            reason,
        } => vec![at.clone(), transition.clone(), reason.clone()],
    }
}

fn capacity() -> impl Strategy<Value = PlaceCapacity> {
    select(vec![
        PlaceCapacity::Single,
        PlaceCapacity::Fifo,
        PlaceCapacity::Register,
    ])
}

fn section() -> impl Strategy<Value = Section> {
    select(vec![
        Section::Places,
        Section::Transitions,
        Section::Effects,
        Section::Probes,
    ])
}

fn net_file_error() -> BoxedStrategy<NetFileError> {
    let n = other_name;
    prop_oneof![
        n().prop_map(NetFileError::Syntax),
        (n(), capacity())
            .prop_map(|(place, capacity)| NetFileError::PoolNeedsFifo { place, capacity }),
        n().prop_map(|place| NetFileError::EmptyPool { place }),
        n().prop_map(|transition| NetFileError::NoInputArc { transition }),
        (n(), word(), word(), n()).prop_map(|(transition, first, second, place)| {
            NetFileError::InputTwice {
                transition,
                first,
                second,
                place,
            }
        }),
        (section(), n(), word(), n()).prop_map(|(owner, name, field, place)| {
            NetFileError::UndeclaredPlace {
                owner,
                name,
                field,
                place,
            }
        }),
        (n(), n())
            .prop_map(|(transition, effect)| NetFileError::UndeclaredEffect { transition, effect }),
        n().prop_map(|transition| NetFileError::EffectWithoutReceiver { transition }),
        n().prop_map(|transition| NetFileError::ReceiverWithoutEffect { transition }),
        n().prop_map(|transition| NetFileError::KeyedWithoutCountBound { transition }),
        n().prop_map(|transition| NetFileError::RetryNeedsBoth { transition }),
        n().prop_map(|transition| NetFileError::NoTimer { transition }),
        n().prop_map(|transition| NetFileError::ZeroCountBound { transition }),
        (n(), any::<i32>()).prop_map(|(effect, code)| NetFileError::ExitCodeTwice { effect, code }),
        (n(), n()).prop_map(
            |(effect, placeholder)| NetFileError::UnknownLogPlaceholder {
                effect,
                placeholder,
            }
        ),
        n().prop_map(|effect| NetFileError::UnbalancedLogPath { effect }),
        n().prop_map(|effect| NetFileError::LogPathWithoutAttempt { effect }),
        (section(), n()).prop_map(|(owner, name)| NetFileError::EmptyArgv { owner, name }),
    ]
    .boxed()
}

fn net_file_values(e: &NetFileError) -> Vec<String> {
    match e {
        NetFileError::Syntax(s) => vec![s.clone()],
        NetFileError::PoolNeedsFifo { place, capacity: _ } | NetFileError::EmptyPool { place } => {
            vec![place.clone()]
        }
        NetFileError::NoInputArc { transition }
        | NetFileError::EffectWithoutReceiver { transition }
        | NetFileError::ReceiverWithoutEffect { transition }
        | NetFileError::KeyedWithoutCountBound { transition }
        | NetFileError::RetryNeedsBoth { transition }
        | NetFileError::NoTimer { transition }
        | NetFileError::ZeroCountBound { transition } => vec![transition.clone()],
        NetFileError::InputTwice {
            transition,
            first: _,
            second: _,
            place,
        } => vec![transition.clone(), place.clone()],
        NetFileError::UndeclaredPlace {
            owner: _,
            name,
            field: _,
            place,
        } => vec![name.clone(), place.clone()],
        NetFileError::UndeclaredEffect { transition, effect } => {
            vec![transition.clone(), effect.clone()]
        }
        NetFileError::ExitCodeTwice { effect, code: _ }
        | NetFileError::UnbalancedLogPath { effect }
        | NetFileError::LogPathWithoutAttempt { effect } => vec![effect.clone()],
        NetFileError::UnknownLogPlaceholder {
            effect,
            placeholder,
        } => vec![effect.clone(), placeholder.clone()],
        NetFileError::EmptyArgv { owner: _, name } => vec![name.clone()],
    }
}
