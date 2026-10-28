//! The `#[transition]` macro (#167, lane M). The emitted descriptors equal
//! hand-written model descriptors; the generated adapter, fired in the real
//! executor on a random marking, binds the tokens the model picks and maps
//! `Ok`/`Err` to the model outcome.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::TypeHint;
use holon_api::Value;
use holon_macros::input_type;
use holon_macros::place;
use holon_macros::transition;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::FireError;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::PureBody;
use holon_net::exec::Read;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::uft::NativeBody;
use holon_transition_contract::ArcKind;
use holon_transition_contract::AskDecl;
use holon_transition_contract::BodyDecl;
use holon_transition_contract::HlArc;
use holon_transition_contract::NativeRef;
use holon_transition_contract::PlaceName;
use holon_transition_contract::TimerDecl;
use holon_transition_contract::TimerKind;
use holon_transition_contract::TransitionDescriptor;
use holon_transition_contract::TransitionName;
use proptest::prelude::*;

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct A {
    pub n: i64,
}

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct B {
    pub n: i64,
}

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct C {
    pub n: i64,
}

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct D {
    pub n: i64,
}

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub n: i64,
    pub note: String,
}

#[place]
#[derive(Clone, Debug, PartialEq)]
pub struct Seen;

#[input_type]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Ours,
    Theirs,
    Both,
}

fn non_negative(n: i64) -> Result<(), Cause> {
    if n < 0 {
        return Err(Cause::definite(format!("negative n {n}")));
    }
    Ok(())
}

#[transition(manual)]
pub fn take_read(#[take] a: A, #[read] b: B) -> Result<Out, Cause> {
    non_negative(a.n)?;
    Ok(Out {
        n: a.n + b.n,
        note: "take_read".to_string(),
    })
}

#[transition(manual, inhibit = C)]
pub fn two_takes(#[take] a: A, #[take] b: B) -> Result<(C, D), Cause> {
    non_negative(b.n)?;
    Ok((C { n: a.n }, D { n: b.n }))
}

#[transition(
    manual,
    irreversible,
    preview = reads_only_preview,
    inhibit = Out,
    inhibit = Seen,
    retry(after_ms = 500, max_attempts = 3),
    run(after_ms = 9000)
)]
pub fn reads_only(#[read] c: C, #[read] d: D) -> Result<Seen, Cause> {
    non_negative(c.n - d.n)?;
    Ok(Seen)
}

fn reads_only_preview(c: &C, d: &D) -> String {
    format!("{} over {}", c.n, d.n)
}

#[transition(auto, inhibit = Out)]
pub fn auto_copy(#[read] c: C) -> Result<Out, Cause> {
    non_negative(c.n)?;
    Ok(Out {
        n: c.n,
        note: "auto".to_string(),
    })
}

#[transition(manual)]
pub fn drop_a(#[take] a: A) -> Result<(), Cause> {
    non_negative(a.n)
}

#[transition(
    manual,
    persist,
    preview = resolve_preview,
    abandon(after_ms = 100),
    delay(after_ms = 60000)
)]
pub fn resolve(#[take] a: A, #[input] pick: Pick, #[read] b: B) -> Result<A, Cause> {
    let n = match pick {
        Pick::Ours => a.n,
        Pick::Theirs => b.n,
        Pick::Both => a.n + b.n,
    };
    Ok(A { n })
}

fn resolve_preview(a: &A, b: &B) -> String {
    format!("ours {} theirs {}", a.n, b.n)
}

#[transition(manual, occurrence)]
pub fn stamp(#[take] a: A, occurrence: OccurrenceId) -> Result<Out, Cause> {
    let OccurrenceId::Caller(id) = &occurrence else {
        unreachable!("the macro decodes the bound id as a caller id: {occurrence}");
    };
    Ok(Out {
        n: a.n,
        note: id.to_string(),
    })
}

#[transition(manual)]
pub fn rename(#[take] a: A, #[input] name: String) -> Result<Out, Cause> {
    Ok(Out { n: a.n, note: name })
}

fn arc(kind: ArcKind, place: &str) -> HlArc {
    HlArc {
        kind,
        place: PlaceName(place.to_string()),
    }
}

fn model_descriptor(
    name: &str,
    arcs: Vec<HlArc>,
    irreversible: bool,
    timers: Vec<TimerDecl>,
    ask: Option<AskDecl>,
) -> TransitionDescriptor {
    TransitionDescriptor {
        name: TransitionName(name.to_string()),
        arcs,
        body: BodyDecl::Native {
            symbol: NativeRef(format!("transition_macro::{name}")),
            effectful: false,
        },
        irreversible,
        timers,
        guard: None,
        ask,
        undoes: None,
    }
}

fn click() -> Option<AskDecl> {
    Some(AskDecl {
        input: None,
        preview: None,
        persist: false,
    })
}

#[test]
fn emitted_descriptors_equal_the_model() {
    use ArcKind::*;
    let cases = [
        (
            take_read::descriptor(),
            model_descriptor(
                "take_read",
                vec![arc(Take, "A"), arc(Read, "B"), arc(Put, "Out")],
                false,
                vec![],
                click(),
            ),
        ),
        (
            two_takes::descriptor(),
            model_descriptor(
                "two_takes",
                vec![
                    arc(Take, "A"),
                    arc(Take, "B"),
                    arc(Inhibit, "C"),
                    arc(Put, "C"),
                    arc(Put, "D"),
                ],
                false,
                vec![],
                click(),
            ),
        ),
        (
            reads_only::descriptor(),
            model_descriptor(
                "reads_only",
                vec![
                    arc(Read, "C"),
                    arc(Read, "D"),
                    arc(Inhibit, "Out"),
                    arc(Inhibit, "Seen"),
                    arc(Put, "Seen"),
                ],
                true,
                vec![
                    TimerDecl {
                        kind: TimerKind::Retry { max_attempts: 3 },
                        after: Duration::from_millis(500),
                    },
                    TimerDecl {
                        kind: TimerKind::Run,
                        after: Duration::from_millis(9000),
                    },
                ],
                Some(AskDecl {
                    input: None,
                    preview: Some(NativeRef(
                        "transition_macro::reads_only_preview".to_string(),
                    )),
                    persist: false,
                }),
            ),
        ),
        (
            auto_copy::descriptor(),
            model_descriptor(
                "auto_copy",
                vec![arc(Read, "C"), arc(Inhibit, "Out"), arc(Put, "Out")],
                false,
                vec![],
                None,
            ),
        ),
        (
            drop_a::descriptor(),
            model_descriptor("drop_a", vec![arc(Take, "A")], false, vec![], click()),
        ),
        (
            resolve::descriptor(),
            model_descriptor(
                "resolve",
                vec![arc(Take, "A"), arc(Read, "B"), arc(Put, "A")],
                false,
                vec![
                    TimerDecl {
                        kind: TimerKind::Abandon,
                        after: Duration::from_millis(100),
                    },
                    TimerDecl {
                        kind: TimerKind::Delay,
                        after: Duration::from_millis(60000),
                    },
                ],
                Some(AskDecl {
                    input: Some(TypeHint::Enum {
                        type_name: "Pick".to_string(),
                        variants: vec!["Ours".into(), "Theirs".into(), "Both".into()],
                    }),
                    preview: Some(NativeRef("transition_macro::resolve_preview".to_string())),
                    persist: true,
                }),
            ),
        ),
        (
            stamp::descriptor(),
            model_descriptor(
                "stamp",
                vec![arc(Take, "A"), arc(Put, "Out")],
                false,
                vec![],
                click(),
            ),
        ),
        (
            rename::descriptor(),
            model_descriptor(
                "rename",
                vec![arc(Take, "A"), arc(Put, "Out")],
                false,
                vec![],
                Some(AskDecl {
                    input: Some(TypeHint::String),
                    preview: None,
                    persist: false,
                }),
            ),
        ),
    ];
    for (emitted, model) in cases {
        assert_eq!(emitted, model, "descriptor of {}", model.name.0);
    }
}

/// The lowering of today's executor (lane L replaces it): one arc per
/// `TransitionDef` arc list, the generated bind and body as fn pointers.
fn lower_today(d: &TransitionDescriptor, bind: Read, body: PureBody) -> TransitionDef {
    let trigger = if d.manual() {
        Trigger::Manual
    } else {
        Trigger::Auto
    };
    d.arcs.iter().fold(
        TransitionDef::new(&d.name.0, trigger, bind, Body::Pure(body)),
        |t, a| match a.kind {
            ArcKind::Take => t.consumes(&a.place.0),
            ArcKind::Read => t.reads(&a.place.0),
            ArcKind::Inhibit => t.inhibits(&a.place.0),
            ArcKind::Put => t,
            other => panic!("the macro emits no {other:?} arc"),
        },
    )
}

const PLACES: [&str; 6] = ["A", "B", "C", "D", "Out", "Seen"];
const SUBJECTS: [&str; 2] = ["s1", "s2"];

/// The marking of the places above: (place, subject) → token content.
type Marking = BTreeMap<(String, String), Content>;

fn content(place: &str, n: i64) -> Content {
    match place {
        "Out" => Content::from([
            ("n".to_string(), Value::Integer(n)),
            ("note".to_string(), Value::String(format!("seed {n}"))),
        ]),
        "Seen" => Content::new(),
        _ => Content::from([("n".to_string(), Value::Integer(n))]),
    }
}

fn n_of(m: &Marking, place: &str, s: &str) -> i64 {
    match m[&(place.to_string(), s.to_string())]["n"] {
        Value::Integer(n) => n,
        ref other => panic!("n of {place} is {other:?}"),
    }
}

/// One catalog transition: the arcs the model reads, and the plain function
/// called on the model's own decoding of the tokens.
struct Case {
    name: &'static str,
    descriptor: fn() -> TransitionDescriptor,
    bind: Read,
    body: PureBody,
    takes: &'static [&'static str],
    reads: &'static [&'static str],
    inhibits: &'static [&'static str],
    model: fn(&Marking, &str) -> ModelOutputs,
}

type ModelOutputs = Result<Vec<(&'static str, Content)>, Cause>;

fn out(o: Out) -> (&'static str, Content) {
    (
        "Out",
        Content::from([
            ("n".to_string(), Value::Integer(o.n)),
            ("note".to_string(), Value::String(o.note)),
        ]),
    )
}

fn catalog() -> Vec<Case> {
    vec![
        Case {
            name: "take_read",
            descriptor: take_read::descriptor,
            bind: Arc::new(take_read::bind),
            body: Arc::new(take_read::body),
            takes: &["A"],
            reads: &["B"],
            inhibits: &[],
            model: |m, s| {
                take_read(A { n: n_of(m, "A", s) }, B { n: n_of(m, "B", s) }).map(|o| vec![out(o)])
            },
        },
        Case {
            name: "two_takes",
            descriptor: two_takes::descriptor,
            bind: Arc::new(two_takes::bind),
            body: Arc::new(two_takes::body),
            takes: &["A", "B"],
            reads: &[],
            inhibits: &["C"],
            model: |m, s| {
                two_takes(A { n: n_of(m, "A", s) }, B { n: n_of(m, "B", s) })
                    .map(|(c, d)| vec![("C", content("C", c.n)), ("D", content("D", d.n))])
            },
        },
        Case {
            name: "reads_only",
            descriptor: reads_only::descriptor,
            bind: Arc::new(reads_only::bind),
            body: Arc::new(reads_only::body),
            takes: &[],
            reads: &["C", "D"],
            inhibits: &["Out", "Seen"],
            model: |m, s| {
                reads_only(C { n: n_of(m, "C", s) }, D { n: n_of(m, "D", s) })
                    .map(|Seen| vec![("Seen", Content::new())])
            },
        },
        Case {
            name: "auto_copy",
            descriptor: auto_copy::descriptor,
            bind: Arc::new(auto_copy::bind),
            body: Arc::new(auto_copy::body),
            takes: &[],
            reads: &["C"],
            inhibits: &["Out"],
            model: |m, s| auto_copy(C { n: n_of(m, "C", s) }).map(|o| vec![out(o)]),
        },
        Case {
            name: "drop_a",
            descriptor: drop_a::descriptor,
            bind: Arc::new(drop_a::bind),
            body: Arc::new(drop_a::body),
            takes: &["A"],
            reads: &[],
            inhibits: &[],
            model: |m, s| drop_a(A { n: n_of(m, "A", s) }).map(|()| vec![]),
        },
    ]
}

fn marking_of(exec: &Executor) -> Marking {
    let mut m = Marking::new();
    for p in PLACES {
        for s in SUBJECTS {
            if let Some(t) = exec.tokens().token(p, s) {
                m.insert((p.to_string(), s.to_string()), t.content.clone());
            }
        }
    }
    m
}

/// The model of one firing of `case` for `s`: `None` when not enabled, else
/// the marking after it and the settlement.
fn model_fire(case: &Case, m: &Marking, s: &str) -> Option<(Marking, Settled)> {
    let has = |p: &str| m.contains_key(&(p.to_string(), s.to_string()));
    let enabled = case.takes.iter().chain(case.reads).all(|p| has(p))
        && !case.inhibits.iter().any(|p| has(p));
    if !enabled {
        return None;
    }
    Some(match (case.model)(m, s) {
        Err(cause) => (m.clone(), Settled::Failed(cause)),
        Ok(outputs) => {
            let mut after = m.clone();
            for p in case.takes {
                after.remove(&(p.to_string(), s.to_string()));
            }
            for (p, c) in outputs {
                after.insert((p.to_string(), s.to_string()), c);
            }
            (after, Settled::Done)
        }
    })
}

fn assert_reads_kept(case: &Case, before: &Marking, after: &Marking) {
    for p in case.reads {
        for s in SUBJECTS {
            let key = (p.to_string(), s.to_string());
            assert!(
                !before.contains_key(&key) || after.contains_key(&key),
                "read token consumed: {p} of {s} by {}",
                case.name
            );
        }
    }
}

fn marking_strategy() -> impl Strategy<Value = Marking> {
    proptest::collection::vec(
        proptest::option::of(-3i64..10),
        PLACES.len() * SUBJECTS.len(),
    )
    .prop_map(|cells| {
        let mut m = Marking::new();
        for (i, cell) in cells.into_iter().enumerate() {
            let (p, s) = (PLACES[i / SUBJECTS.len()], SUBJECTS[i % SUBJECTS.len()]);
            if let Some(n) = cell {
                m.insert((p.to_string(), s.to_string()), content(p, n));
            }
        }
        m
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn adapter_binds_and_maps_like_the_model(
        which in 0usize..5,
        marking in marking_strategy(),
    ) {
        let cases = catalog();
        let case = &cases[which];
        let d = (case.descriptor)();
        let net = PLACES
            .iter()
            .fold(NetDef::default(), |net, p| net.place(p, Class::Process))
            .transition(lower_today(&d, case.bind.clone(), case.body.clone()));
        let mut exec = Executor::new(net).expect("the catalog net loads");
        let now = Duration::ZERO;
        for ((p, s), c) in &marking {
            let q = exec
                .write(p, s, c.clone(), now)
                .expect("the catalog net quiesces");
            prop_assert!(q.requests.is_empty(), "a pure catalog net asks for no effect");
        }
        let mut expected = marking.clone();
        if d.manual() {
            let fired = exec.fire(case.name, "s1", OccurrenceId::caller("o1"), Content::new(), now);
            match model_fire(case, &marking, "s1") {
                None => prop_assert!(
                    matches!(fired, Err(FireError::NotEnabled { .. })),
                    "{} fired where the model is not enabled: {fired:?}",
                    case.name
                ),
                Some((after, settled)) => {
                    prop_assert!(fired.is_ok(), "{} refused: {fired:?}", case.name);
                    prop_assert_eq!(
                        exec.settled(&OccurrenceId::caller("o1")).expect("known occurrence"),
                        Some(settled)
                    );
                    expected = after;
                }
            }
        } else {
            prop_assert_eq!(exec.step(now).expect("the auto net quiesces"), vec![]);
            for s in SUBJECTS {
                if let Some((after, _)) = model_fire(case, &expected, s) {
                    expected = after;
                }
            }
        }
        let actual = marking_of(&exec);
        assert_reads_kept(case, &marking, &actual);
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn input_reaches_the_function(a in -5i64..5, b in -5i64..5, pick in 0usize..3) {
        let label = ["Ours", "Theirs", "Both"][pick];
        let s = Subject::new("s");
        let c = Content::from([
            ("a".to_string(), object(&[("n", Value::Integer(a))])),
            ("b".to_string(), object(&[("n", Value::Integer(b))])),
            ("@input".to_string(), Value::String(label.to_string())),
        ]);
        let n = [a, b, a + b][pick];
        prop_assert_eq!(
            resolve::body(&s, &c),
            Ok(vec![Output::new("A", &s, content("A", n))])
        );
    }
}

fn object(fields: &[(&str, Value)]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

#[test]
fn a_bad_or_missing_input_is_a_definite_failure() {
    let s = Subject::new("s");
    let mut c = Content::from([
        ("a".to_string(), object(&[("n", Value::Integer(1))])),
        ("b".to_string(), object(&[("n", Value::Integer(2))])),
    ]);
    let missing = resolve::body(&s, &c).expect_err("no input bound");
    assert!(!missing.retryable);
    assert!(missing.text.contains("@input"), "{}", missing.text);
    c.insert("@input".to_string(), Value::String("Mine".to_string()));
    let bad = resolve::body(&s, &c).expect_err("not a Pick");
    assert!(bad.text.contains("`Mine` is not a Pick"), "{}", bad.text);
}

#[test]
fn a_string_input_binds_through_try_from() {
    let s = Subject::new("s");
    let c = Content::from([
        ("a".to_string(), object(&[("n", Value::Integer(4))])),
        ("@input".to_string(), Value::String("renamed".to_string())),
    ]);
    assert_eq!(
        rename::body(&s, &c),
        Ok(vec![Output::new(
            "Out",
            &s,
            Content::from([
                ("n".to_string(), Value::Integer(4)),
                ("note".to_string(), Value::String("renamed".to_string())),
            ])
        )])
    );
}

#[test]
fn the_occurrence_is_the_last_argument() {
    let s = Subject::new("s");
    let c = Content::from([
        ("a".to_string(), object(&[("n", Value::Integer(7))])),
        ("@occurrence".to_string(), Value::String("o-9".to_string())),
    ]);
    assert_eq!(
        stamp::body(&s, &c),
        Ok(vec![Output::new(
            "Out",
            &s,
            Content::from([
                ("n".to_string(), Value::Integer(7)),
                ("note".to_string(), Value::String("o-9".to_string())),
            ])
        )])
    );
}

#[test]
fn native_parts_carry_the_same_bind_body_and_preview() {
    let s = Subject::new("s");
    let c = Content::from([
        ("c".to_string(), object(&[("n", Value::Integer(5))])),
        ("d".to_string(), object(&[("n", Value::Integer(2))])),
    ]);
    let parts = reads_only::native_parts();
    let NativeBody::Pure(body) = &parts.body else {
        panic!("a plain function is a pure body");
    };
    assert_eq!(body(&s, &c), reads_only::body(&s, &c));
    let preview = parts.preview.expect("reads_only declares a preview");
    assert_eq!(preview(&s, &c), "5 over 2");
    assert!(take_read::native_parts().preview.is_none());
}

#[test]
fn a_token_without_its_field_fails_loud() {
    let s = Subject::new("s");
    let c = Content::from([
        ("a".to_string(), object(&[])),
        ("b".to_string(), object(&[("n", Value::Integer(2))])),
    ]);
    let cause = take_read::body(&s, &c).expect_err("A has no n");
    assert!(cause.text.contains("parameter `a`"), "{}", cause.text);
}

#[test]
fn the_function_stays_callable_as_written() {
    assert_eq!(
        take_read(A { n: 1 }, B { n: 2 }),
        Ok(Out {
            n: 3,
            note: "take_read".to_string()
        })
    );
    assert_eq!(A::PLACE, "A");
}
