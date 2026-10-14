//! The lowering of a `manual` HL transition (#167, `uft::lower`) on the real
//! executor: model first, then the lowered net must agree after every op.
//!
//! Twins (`--features naive-twins`, `UFT_LOWER_TWIN=<name>`), each drops one
//! lowering rule: `no-ask-hash`, `no-answer-inhibitor`, `no-expire`,
//! `ask-while-in-flight`, `ask-while-retry-pending`,
//! `ask-while-outcome-unknown`; executor twins:
//! `no-quiescence`, `keep-work-failed`.

mod uft_lower_support;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::ANSWER_ID_KEY;
use holon_api::user_transition::ANSWER_INPUT_KEY;
use holon_api::user_transition::ArcKind;
use holon_api::user_transition::AskDecl;
use holon_api::user_transition::BodyDecl;
use holon_api::user_transition::HlArc;
use holon_api::user_transition::NativeRef;
use holon_api::user_transition::PlaceName;
use holon_api::user_transition::TimerDecl;
use holon_api::user_transition::TimerKind;
use holon_api::user_transition::TransitionDescriptor;
use holon_api::user_transition::TransitionName;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::Executor;
use holon_net::exec::FireError;
use holon_net::exec::HASH_FIELD;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Quiesced;
use holon_net::exec::REASON_FIELD;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::SEEN_FIELD;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::exec::WhyNot;
use holon_net::exec::answer_transition;
use holon_net::exec::answered_place;
use holon_net::exec::asks_place;
use holon_net::exec::expired_place;
use holon_net::uft::Generated;
use holon_net::uft::NativeBody;
use holon_net::uft::NativeParts;
use holon_net::uft::lower;
use proptest::prelude::*;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;
use uft_lower_support::EXPIRED_CHANGED;
use uft_lower_support::EXPIRED_GONE;
use uft_lower_support::Model;
use uft_lower_support::Op;
use uft_lower_support::Ref;
use uft_lower_support::Refusal;
use uft_lower_support::SUBJECTS;
use uft_lower_support::Sit;

const T: &str = "AcceptRemoval";

fn text(s: &str) -> Value {
    Value::String(s.to_owned())
}

fn get(c: &Content, field: &str) -> String {
    match &c[field] {
        Value::String(s) => s.clone(),
        other => panic!("field '{field}' is not text: {other:?}"),
    }
}

fn arc(kind: ArcKind, place: &str) -> HlArc {
    HlArc {
        kind,
        place: PlaceName(place.to_owned()),
    }
}

/// `AcceptRemoval` as the macro or a YAML sidecar states it: WHAT it asks.
fn descriptor() -> TransitionDescriptor {
    TransitionDescriptor {
        name: TransitionName(T.to_owned()),
        arcs: vec![
            arc(ArcKind::Take, "Refused"),
            arc(ArcKind::Read, "Base"),
            arc(ArcKind::Read, "OnDisk"),
            arc(ArcKind::Put, "Removed"),
        ],
        body: BodyDecl::Native {
            symbol: NativeRef("accept_removal".to_owned()),
            effectful: true,
        },
        irreversible: true,
        timers: vec![],
        guard: None,
        ask: Some(AskDecl {
            input: Some(pick()),
            preview: Some(NativeRef("removal_preview".to_owned())),
            persist: false,
        }),
    }
}

fn pick() -> TypeHint {
    TypeHint::Enum {
        type_name: "Pick".to_owned(),
        variants: vec!["Remove".to_owned(), "Keep".to_owned()],
    }
}

fn parts() -> NativeParts {
    NativeParts {
        bind: Arc::new(|r: &mut Reader<'_>, s: &Subject| {
            let why = r.text("Refused", s, "why")?;
            let base = r.text("Base", s, "text")?;
            let ondisk = r.text("OnDisk", s, "text")?;
            Some(Content::from([
                ("why".to_owned(), text(&why)),
                ("base".to_owned(), text(&base)),
                ("ondisk".to_owned(), text(&ondisk)),
            ]))
        }),
        body: NativeBody::Effectful(Receiver::None),
        preview: Some(Arc::new(|s: &Subject, c: &Content| {
            format!(
                "removes {s}: base {} / on disk {}",
                get(c, "base"),
                get(c, "ondisk")
            )
        })),
    }
}

fn base_net() -> NetDef {
    NetDef::default()
        .place("Refused", Class::Process)
        .place("Base", Class::Data)
        .place("OnDisk", Class::Data)
        .place("Removed", Class::Process)
        .transition(
            TransitionDef::new(
                "Saved",
                Trigger::Manual,
                Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new())),
                Body::Pure(Arc::new(|_: &Subject, _: &Content| Ok(vec![]))),
            )
            .consumes("Refused"),
        )
}

/// `d` with its takes turned into reads: the work leaves its situation intact.
fn read_only(mut d: TransitionDescriptor) -> TransitionDescriptor {
    for a in &mut d.arcs {
        if a.kind == ArcKind::Take {
            a.kind = ArcKind::Read;
        }
    }
    d
}

fn pbt_descriptor(m: &Model) -> TransitionDescriptor {
    if m.read_only {
        read_only(descriptor())
    } else {
        descriptor()
    }
}

#[cfg(feature = "naive-twins")]
fn lowered(d: &TransitionDescriptor) -> NetDef {
    use holon_net::uft::LowerTwin;
    use holon_net::uft::lower_twin;
    let twin = match std::env::var("UFT_LOWER_TWIN").ok().as_deref() {
        None => return lower(base_net(), d, Some(parts())).expect("lowers"),
        Some("no-ask-hash") => LowerTwin::NoAskHash,
        Some("no-answer-inhibitor") => LowerTwin::NoAnswerInhibitor,
        Some("no-expire") => LowerTwin::NoExpire,
        Some("ask-while-in-flight") => LowerTwin::AskWhileInFlight,
        Some("no-quiescence" | "keep-work-failed") => {
            return lower(base_net(), d, Some(parts())).expect("lowers");
        }
        Some("ask-while-retry-pending") => LowerTwin::AskWhileRetryPending,
        Some("ask-while-outcome-unknown") => LowerTwin::AskWhileOutcomeUnknown,
        Some(other) => panic!("UFT_LOWER_TWIN={other} names no twin"),
    };
    lower_twin(base_net(), d, Some(parts()), twin).expect("lowers")
}

#[cfg(not(feature = "naive-twins"))]
fn lowered(d: &TransitionDescriptor) -> NetDef {
    assert!(
        std::env::var("UFT_LOWER_TWIN").is_err(),
        "UFT_LOWER_TWIN needs --features naive-twins"
    );
    lower(base_net(), d, Some(parts())).expect("lowers")
}

/// The executor of the net lowered from `d`; `UFT_LOWER_TWIN=no-quiescence`
/// and `keep-work-failed` select executor twins.
fn executor(d: &TransitionDescriptor) -> Executor {
    let exec = Executor::new(lowered(d)).expect("the lowered net loads");
    #[cfg(feature = "naive-twins")]
    {
        use holon_net::exec::Twin;
        match std::env::var("UFT_LOWER_TWIN").as_deref() {
            Ok("no-quiescence") => return exec.with_twin(Twin::NoQuiescence),
            Ok("keep-work-failed") => return exec.with_twin(Twin::KeepWorkFailed),
            _ => {}
        }
    }
    exec
}

// --------------------------------------------------------------- the SUT

struct Sut {
    exec: Executor,
    /// The model before the op: `apply` receives the model after it.
    model: Model,
    now: Duration,
    requests: BTreeMap<usize, EffectRequest>,
    /// The occurrence each subject has parked with an unknown outcome.
    parked: BTreeMap<usize, OccurrenceId>,
    /// The ask hash each (subject, situation) showed: a pure function of the
    /// situation content.
    hashes: BTreeMap<(usize, Sit), String>,
    /// Answer ids whose work fired.
    fired: BTreeSet<String>,
}

fn counter(tick: u32) -> Content {
    Content::from([("text".to_owned(), text(&tick.to_string()))])
}

fn present(sut: &Sut, place: &str, s: &str) -> bool {
    sut.exec.tokens().token(place, s).is_some()
}

fn value_of(v: Option<&str>) -> Value {
    v.map_or(Value::Null, text)
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(m: &Model) -> Sut {
        let mut exec = executor(&pbt_descriptor(m));
        for s in SUBJECTS {
            for (place, content) in [
                ("Refused", Content::from([("why".to_owned(), text("0"))])),
                ("Base", counter(0)),
                ("OnDisk", counter(0)),
            ] {
                let q = exec
                    .write(place, s, content, Duration::ZERO)
                    .expect("the lowered net quiesces");
                assert!(q.requests.is_empty(), "no answer, so no work");
            }
        }
        let mut sut = Sut {
            exec,
            model: m.clone(),
            now: Duration::ZERO,
            requests: BTreeMap::new(),
            parked: BTreeMap::new(),
            hashes: BTreeMap::new(),
            fired: BTreeSet::new(),
        };
        compare(&mut sut, m);
        sut
    }

    fn apply(mut sut: Sut, m: &Model, op: Op) -> Sut {
        sut.now += Duration::from_millis(1);
        let now = sut.now;
        let pre = sut.model.clone();
        let mut mid = pre.clone();
        mid.apply_op(&op);
        let tick = pre.next;
        let op_text = format!("{op:?}");
        let quiesced =
            |q: Result<Quiesced<()>, ExecError>| q.expect("the lowered net quiesces").requests;
        let requests = match op {
            Op::ChangeBase(s) => quiesced(sut.exec.write("Base", SUBJECTS[s], counter(tick), now)),
            Op::ChangeOnDisk(s) => {
                quiesced(sut.exec.write("OnDisk", SUBJECTS[s], counter(tick), now))
            }
            Op::Reappear(s) => quiesced(sut.exec.write(
                "Refused",
                SUBJECTS[s],
                Content::from([("why".to_owned(), text(&tick.to_string()))]),
                now,
            )),
            Op::Saved(s) => {
                let got = sut.exec.fire(
                    "Saved",
                    SUBJECTS[s],
                    OccurrenceId::caller(&format!("op{tick}")),
                    Content::new(),
                    now,
                );
                assert_eq!(
                    got.is_ok(),
                    pre.subj[s].refused.is_some(),
                    "model divergence: Saved on {} gave {got:?}",
                    SUBJECTS[s]
                );
                got.map_or_else(|_| Vec::new(), |q| q.requests)
            }
            Op::Answer { s, value, seen } => answer(&mut sut, &pre, s, value, seen, tick, now),
            Op::Step => sut.exec.step(now).expect("the lowered net quiesces"),
            Op::Complete { s, ok } => {
                let req = sut
                    .requests
                    .remove(&s)
                    .expect("precondition: a work request is in flight");
                let result = if ok {
                    EffectResult::Done(vec![Output::new(
                        "Removed",
                        &req.subject,
                        req.input.clone(),
                    )])
                } else {
                    EffectResult::Failed(Cause::definite("disk full"))
                };
                sut.exec
                    .complete(req.attempt, result, now)
                    .expect("the attempt is known")
                    .requests
            }
            Op::Lost(s) => {
                let req = sut
                    .requests
                    .remove(&s)
                    .expect("precondition: a work request is in flight");
                let requests = sut
                    .exec
                    .complete(
                        req.attempt,
                        EffectResult::Unknown(Cause::definite("lost")),
                        now,
                    )
                    .expect("the attempt is known")
                    .requests;
                sut.parked.insert(s, req.occurrence);
                requests
            }
            Op::Resolve { s, verify } => {
                let occurrence = sut
                    .parked
                    .remove(&s)
                    .expect("precondition: an occurrence is parked");
                let resolution = if verify {
                    Resolution::Verify(vec![Output::new(
                        "Removed",
                        &Subject::new(SUBJECTS[s]),
                        Content::new(),
                    )])
                } else {
                    Resolution::Drop
                };
                sut.exec
                    .resolve(&occurrence, resolution, now)
                    .expect("the occurrence is parked")
                    .requests
            }
        };
        no_work_failed_stands(&sut, &op_text);
        no_answered_stands(&sut, &op_text);
        started(&mut sut, &mid, m, requests);
        compare(&mut sut, m);
        sut.model = m.clone();
        sut
    }
}

/// P-quiescent, `Answered` clause: the work or the expiry has taken it.
fn no_answered_stands(sut: &Sut, op: &str) {
    stands_nowhere(sut, &answered_place(T).to_string(), op);
}

/// P-quiescent, `WorkFailed` clause: `ReleaseFailed` or `ClearFailed` has
/// taken it.
fn no_work_failed_stands(sut: &Sut, op: &str) {
    stands_nowhere(sut, &Generated::WorkFailed.name(T), op);
}

fn stands_nowhere(sut: &Sut, place: &str, op: &str) {
    for s in SUBJECTS {
        assert!(
            !present(sut, place, s),
            "P-quiescent: {place} stands on {s} after {op}"
        );
    }
}

fn answer(
    sut: &mut Sut,
    pre: &Model,
    s: usize,
    value: Option<&'static str>,
    seen: uft_lower_support::Seen,
    tick: u32,
    now: Duration,
) -> Vec<EffectRequest> {
    let subject = SUBJECTS[s];
    let seen_sit = pre.seen_sit(s, seen).expect("precondition");
    let seen = sut.hashes[&(s, seen_sit)].clone();
    let input = Content::from([
        (ANSWER_ID_KEY.to_owned(), text(&tick.to_string())),
        (SEEN_FIELD.to_owned(), text(&seen)),
        (ANSWER_INPUT_KEY.to_owned(), value_of(value)),
    ]);
    let got = match sut.exec.fire(
        &answer_transition(T),
        subject,
        OccurrenceId::caller(&tick.to_string()),
        input,
        now,
    ) {
        Ok(q) => Ok(q.requests),
        Err(FireError::NotEnabled {
            why: WhyNot::Inhibited(p),
            ..
        }) if p.as_str() == Generated::WorkFailed.name(T) => Err(Refusal::NoLongerApplies),
        Err(FireError::NotEnabled {
            why: WhyNot::Inhibited(_),
            ..
        }) => Err(Refusal::AlreadyAnswered),
        Err(FireError::NotEnabled {
            why: WhyNot::Guard, ..
        }) => Err(Refusal::NoLongerApplies),
        Err(e) => panic!("Answer refused for an unplanned reason: {e}"),
    };
    let requests = got.as_ref().map_or_else(|_| Vec::new(), Clone::clone);
    match (got.map(|_| ()), pre.answer_result(s)) {
        (Ok(()), Err(Refusal::AlreadyAnswered)) => {
            panic!("P3 a second answer on {subject} was accepted: two Answered for one ask")
        }
        (Err(e), Ok(())) if pre.subj[s].last_failed => {
            panic!("P4 a re-answer on {subject} after a failure is refused: {e:?}")
        }
        (got, want) => assert_eq!(
            got, want,
            "the answer on {subject} (value {value:?}) got the wrong reply"
        ),
    }
    requests
}

/// The work requests an op returned against the model's quiesce after the
/// op (`pre`: the model after the op alone).
fn started(sut: &mut Sut, pre: &Model, m: &Model, requests: Vec<EffectRequest>) {
    let mut started = Vec::new();
    for req in requests {
        assert_eq!(&*req.transition, T, "only the work has an effect");
        let s = SUBJECTS
            .iter()
            .position(|x| *x == req.subject.as_str())
            .expect("a known subject");
        let ans = pre.subj[s]
            .answered
            .clone()
            .unwrap_or_else(|| panic!("the work fired on {} with no answer", req.subject));
        let answer = get(&req.input, ANSWER_ID_KEY);
        assert_eq!(answer, ans.id.to_string(), "the work took another answer");
        let sit = pre.subj[s]
            .sit()
            .expect("P1 the work fires on a changed binding: the situation is gone");
        assert_eq!(
            sit, ans.seen,
            "P1 the work fires on a changed binding: answer {answer} saw {:?}, the work binds {sit:?}",
            ans.seen
        );
        assert_eq!(
            get(&req.input, SEEN_FIELD),
            sut.hashes[&(s, sit)],
            "P1 the work binds a situation whose hash differs from the one seen"
        );
        assert_eq!(
            req.input[ANSWER_INPUT_KEY],
            value_of(ans.value),
            "the work binds another value than the answer's"
        );
        assert!(
            sut.fired.insert(answer.clone()),
            "the work fired twice on answer {answer}"
        );
        started.push(ans.id);
        sut.requests.insert(s, req);
    }
    let want = pre.clone().step();
    for id in &want {
        if started.contains(id) {
            continue;
        }
        let ans = m
            .subj
            .iter()
            .find_map(|sj| sj.flight.as_ref().filter(|f| f.ans.id == *id))
            .expect("the model started it");
        assert!(
            !ans.ans.after_failure,
            "P4 a re-answer ({id}) after a failure did not fire"
        );
        panic!("model divergence: the work did not fire for answer {id}");
    }
    assert_eq!(started, want, "model divergence: started work");
    for s in SUBJECTS {
        if sut.exec.tokens().token(&answered_place(T), s).is_none() {
            continue;
        }
        let why = sut.exec.why_not(T, s);
        assert!(
            !matches!(why, Some(WhyNot::Guard | WhyNot::Needs { .. })),
            "P2 an Answered on {s} outlives its ask: {why:?}"
        );
        panic!(
            "P-enabled: an Answered on {s} matches its situation at quiescence, but the work is not enabled: {why:?}"
        );
    }
}

/// The marking and the derived asks against the model, after every op.
fn compare(sut: &mut Sut, m: &Model) {
    for (i, s) in SUBJECTS.iter().enumerate() {
        let sj = &m.subj[i];
        let row = |what: &str, got: bool, want: bool| {
            assert_eq!(got, want, "model divergence on {s}: {what}");
        };
        row("Refused", present(sut, "Refused", s), sj.refused.is_some());
        let answered = sut.exec.tokens().token(&answered_place(T), s);
        row("Answered", answered.is_some(), sj.answered.is_some());
        if let (Some(t), Some(ans)) = (answered, &sj.answered) {
            assert_eq!(get(&t.content, ANSWER_ID_KEY), ans.id.to_string());
            assert_eq!(t.content[ANSWER_INPUT_KEY], value_of(ans.value));
        }
        row(
            "work running",
            present(sut, Place::running(T).as_str(), s),
            sj.flight.is_some(),
        );
        row(
            "work parked with an unknown outcome",
            present(sut, Place::outcome_unknown(T).as_str(), s),
            sj.parked.is_some(),
        );
        row(
            "WorkFailed",
            present(sut, &Generated::WorkFailed.name(T), s),
            sj.failed_port,
        );
        row("Removed", present(sut, "Removed", s), sj.removed);
        row(
            "failure mark",
            sut.exec.mark(T, s).is_some(),
            sj.mark.is_some(),
        );
        let ask = sut.exec.derive(&asks_place(T), &Subject::new(s));
        if ask.is_some() {
            let why = sut.exec.why_not(&answer_transition(T), s);
            assert!(
                why.is_none(),
                "P5 an offered ask on {s} has a dead Answer: {why:?}"
            );
        }
        row("ask", ask.is_some(), sj.offered().is_some());
        if let (Some(ask), Some(sit)) = (ask, sj.offered()) {
            let hash = get(&ask, HASH_FIELD);
            let known = sut.hashes.entry((i, sit)).or_insert_with(|| hash.clone());
            assert_eq!(
                *known, hash,
                "the ask hash of {s} moved although its situation {sit:?} did not"
            );
            assert_eq!(
                get(&ask, "preview"),
                format!("removes {s}: base {} / on disk {}", sit.1, sit.2)
            );
        }
    }
    let expired_place = expired_place(T);
    let got: BTreeMap<u32, (String, String)> = sut
        .exec
        .tokens()
        .subjects(&expired_place)
        .into_iter()
        .map(|id| {
            let t = sut
                .exec
                .tokens()
                .token(&expired_place, &id)
                .expect("listed");
            (
                id.parse().expect("an answer id"),
                (get(&t.content, "why"), get(&t.content, REASON_FIELD)),
            )
        })
        .collect();
    let want: BTreeMap<u32, (String, String)> = m
        .expired
        .iter()
        .map(|(id, why)| {
            let reason = match *why {
                EXPIRED_GONE => "ask_gone",
                EXPIRED_CHANGED => "situation_changed",
                other => panic!("unknown expiry '{other}'"),
            };
            (*id, (why.to_string(), reason.to_owned()))
        })
        .collect();
    assert_eq!(got, want, "the disclosed ExpireAnswer firings");
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 20_000,
        .. ProptestConfig::default()
    })]

    #[test]
    fn lowered_manual_transition(sequential 1..40 => Test);

    /// P-quiescent runs in `apply`; this name carries it.
    #[test]
    fn p_quiescent_no_answered_or_work_failed_stands(sequential 1..40 => Test);
}

// ------------------------------------------------- structural checks

#[test]
fn ask_row_carries_the_declared_input_type() {
    let mut exec = executor(&descriptor());
    assert!(situation(&mut exec).is_empty(), "no answer, so no work");
    let rows = exec.asks();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].transition.0, T);
    assert_eq!(rows[0].subject.0, "a");
    assert_eq!(rows[0].input, Some(pick()));
    assert!(rows[0].irreversible && rows[0].has_preview);
}

#[test]
fn generated_names_map_back_to_their_transition() {
    for g in Generated::ALL {
        assert_eq!(Generated::parse(&g.name(T)), Some((g, T)));
    }
    assert_eq!(Generated::Asks.name(T), asks_place(T).to_string());
    assert_eq!(Generated::Answered.name(T), answered_place(T).to_string());
    assert_eq!(Generated::Answer.name(T), answer_transition(T).to_string());
    assert_eq!(Generated::Expired.name(T), expired_place(T).to_string());
    assert_eq!(Generated::parse("Refused"), None);
}

#[test]
fn an_auto_descriptor_lowers_one_to_one() {
    let mut d = descriptor();
    d.ask = None;
    let mut p = parts();
    p.preview = None;
    let mut exec = Executor::new(lower(base_net(), &d, Some(p)).expect("lowers")).expect("loads");
    let requests = situation(&mut exec);
    assert_eq!(requests.len(), 1);
    assert_eq!(&*requests[0].transition, T);
    assert_eq!(
        get(&requests[0].input, "base"),
        "0",
        "the native bind runs as the work bind"
    );
}

#[test]
fn a_descriptor_that_cannot_lower_is_a_load_error_naming_it() {
    let refused = |d: TransitionDescriptor, p: Option<NativeParts>| {
        let e = lower(base_net(), &d, p).err().expect("refused");
        assert_eq!(e.transition, T, "{e}");
        e.reason
    };
    let mut persist = descriptor();
    persist.ask.as_mut().expect("manual").persist = true;
    assert!(refused(persist, Some(parts())).contains("persistent"));

    let mut delay = descriptor();
    delay.timers.push(TimerDecl {
        kind: TimerKind::Delay,
        after: Duration::from_secs(1),
    });
    assert!(refused(delay, Some(parts())).contains("Delay"));

    assert!(refused(descriptor(), None).contains("native parts"));

    let mut pure = parts();
    pure.body = NativeBody::Pure(Arc::new(|_: &Subject, _: &Content| Ok(vec![])));
    assert!(refused(descriptor(), Some(pure)).contains("effectful"));

    let mut port = descriptor();
    port.arcs.push(arc(ArcKind::OnFailure, "Removed"));
    assert!(refused(port, Some(parts())).contains("failure port"));
}

#[test]
fn a_pure_work_body_reads_the_answer_value_and_id_under_the_adapter_keys() {
    let mut d = descriptor();
    d.body = BodyDecl::Native {
        symbol: NativeRef("accept_removal".to_owned()),
        effectful: false,
    };
    let mut p = parts();
    p.body = NativeBody::Pure(Arc::new(|s: &Subject, input: &Content| {
        Ok(vec![Output::new("Removed", s, input.clone())])
    }));
    let mut exec = Executor::new(lower(base_net(), &d, Some(p)).expect("lowers")).expect("loads");
    assert!(situation(&mut exec).is_empty(), "no answer, so no work");
    let ask = exec
        .derive(&asks_place(T), &Subject::new("a"))
        .expect("asks");
    let pure = exec
        .fire(
            &answer_transition(T),
            "a",
            OccurrenceId::caller("1"),
            Content::from([
                (ANSWER_ID_KEY.to_owned(), text("1")),
                (SEEN_FIELD.to_owned(), text(&get(&ask, HASH_FIELD))),
                (ANSWER_INPUT_KEY.to_owned(), text("Remove")),
            ]),
            Duration::ZERO,
        )
        .expect("answered");
    assert!(pure.requests.is_empty(), "a pure work makes no request");
    let removed = &exec
        .tokens()
        .token("Removed", "a")
        .expect("the work ran")
        .content;
    assert_eq!(removed[ANSWER_INPUT_KEY], text("Remove"));
    assert_eq!(removed[ANSWER_ID_KEY], text("1"));
}

fn asked(exec: &Executor) -> String {
    get(
        &exec
            .derive(&asks_place(T), &Subject::new("a"))
            .expect("asks"),
        HASH_FIELD,
    )
}

fn answer_as(exec: &mut Executor, id: &str, seen: &str) -> Vec<EffectRequest> {
    exec.fire(
        &answer_transition(T),
        "a",
        OccurrenceId::caller(id),
        Content::from([
            (ANSWER_ID_KEY.to_owned(), text(id)),
            (SEEN_FIELD.to_owned(), text(seen)),
            (ANSWER_INPUT_KEY.to_owned(), text("Remove")),
        ]),
        Duration::ZERO,
    )
    .unwrap_or_else(|e| panic!("answer {id} refused: {e:?}"))
    .requests
}

/// Subject `a` asked for `AcceptRemoval` lowered from `d`.
fn asking(d: &TransitionDescriptor) -> Executor {
    let mut exec = executor(d);
    assert!(situation(&mut exec).is_empty(), "no answer, so no work");
    exec
}

/// Writes the situation of subject `a`; returns the work requests of the
/// last write.
fn situation(exec: &mut Executor) -> Vec<EffectRequest> {
    let mut requests = Vec::new();
    for (place, content) in [
        ("Refused", Content::from([("why".to_owned(), text("0"))])),
        ("Base", counter(0)),
        ("OnDisk", counter(0)),
    ] {
        requests = exec
            .write(place, "a", content, Duration::ZERO)
            .expect("quiesces")
            .requests;
    }
    requests
}

/// Answers `id` on the current ask and returns the one work request.
fn answer_and_start(exec: &mut Executor, id: &str) -> EffectRequest {
    let seen = asked(exec);
    let [req] = <[_; 1]>::try_from(answer_as(exec, id, &seen))
        .unwrap_or_else(|reqs| panic!("the work fires once on answer {id}: {reqs:?}"));
    req
}

/// `OnDisk` of `a` changes; no work starts.
fn on_disk_changes(exec: &mut Executor) {
    let q = exec
        .write("OnDisk", "a", counter(1), Duration::ZERO)
        .expect("quiesces");
    assert!(q.requests.is_empty(), "{:?}", q.requests);
}

fn work_answers(requests: &[EffectRequest]) -> Vec<String> {
    requests
        .iter()
        .map(|r| get(&r.input, ANSWER_ID_KEY))
        .collect()
}

/// A dropped outcome-unknown puts `WorkFailed` after the work consumed its
/// `Answered`; it must neither swallow a later answer nor hide its expiry.
#[test]
fn a_dropped_failure_swallows_no_later_answer() {
    let mut exec = asking(&descriptor());
    let req = answer_and_start(&mut exec, "1");
    let lost = exec
        .complete(
            req.attempt,
            EffectResult::Unknown(Cause::definite("lost")),
            Duration::ZERO,
        )
        .expect("the attempt is known");
    assert!(lost.requests.is_empty());
    let dropped = exec
        .resolve(&req.occurrence, Resolution::Drop, Duration::ZERO)
        .expect("parked");
    assert!(dropped.requests.is_empty());

    let reappeared = exec
        .write(
            "Refused",
            "a",
            Content::from([("why".to_owned(), text("0"))]),
            Duration::ZERO,
        )
        .expect("quiesces");
    assert!(reappeared.requests.is_empty());
    let stale = asked(&exec);
    on_disk_changes(&mut exec);
    assert!(answer_as(&mut exec, "2", &stale).is_empty());
    assert!(
        exec.tokens().token(&expired_place(T), "2").is_some(),
        "WorkFailed of the dropped answer 1 swallowed the stale answer 2: no Expired; WorkFailed: {:?}",
        exec.tokens().token(&Generated::WorkFailed.name(T), "a")
    );

    let fresh = asked(&exec);
    assert_eq!(
        work_answers(&answer_as(&mut exec, "3", &fresh)),
        ["3"],
        "answer 3 did not reach the work"
    );
}

/// Ruling 2.a: a failed answer is released, never expired, even when the
/// situation changes before the next step.
#[test]
fn a_failed_answer_does_not_expire() {
    let mut exec = asking(&descriptor());
    let req = answer_and_start(&mut exec, "1");
    let failed = exec
        .complete(
            req.attempt,
            EffectResult::Failed(Cause::definite("disk full")),
            Duration::ZERO,
        )
        .expect("the attempt is known");
    assert!(failed.requests.is_empty());
    on_disk_changes(&mut exec);
    assert!(
        exec.tokens().token(&expired_place(T), "1").is_none(),
        "the failed answer 1 expired instead of being released"
    );
    let fresh = asked(&exec);
    assert_eq!(
        work_answers(&answer_as(&mut exec, "2", &fresh)),
        ["2"],
        "answer 2 did not reach the work"
    );
}

/// A work that only reads its places leaves the situation intact while it
/// runs; the ask must still hide until the run ends.
#[test]
fn a_read_only_work_in_flight_hides_its_ask() {
    let mut exec = asking(&read_only(descriptor()));
    let req = answer_and_start(&mut exec, "1");
    assert_eq!(
        exec.derive(&asks_place(T), &Subject::new("a")),
        None,
        "the ask shows while answer 1 runs"
    );
    let done = exec
        .complete(req.attempt, EffectResult::Done(vec![]), Duration::ZERO)
        .expect("the attempt is known");
    assert!(done.requests.is_empty());
    assert!(
        exec.derive(&asks_place(T), &Subject::new("a")).is_some(),
        "the ask returns once the run ends"
    );
}

/// EX-unknown.a: a parked unknown outcome is no failure — the run stays
/// unsettled, so the ask of a read-only work must stay hidden until the user
/// resolves it.
#[test]
fn a_parked_unknown_outcome_hides_its_ask() {
    let mut exec = asking(&read_only(descriptor()));
    let req = answer_and_start(&mut exec, "1");
    let lost = exec
        .complete(
            req.attempt,
            EffectResult::Unknown(Cause::definite("lost")),
            Duration::ZERO,
        )
        .expect("the attempt is known");
    assert!(lost.requests.is_empty());
    assert_eq!(
        exec.derive(&asks_place(T), &Subject::new("a")),
        None,
        "the ask shows while answer 1 is parked with an unknown outcome"
    );
    assert!(
        exec.asks().is_empty(),
        "the parked occurrence still offers an Asks row: {:?}",
        exec.asks()
    );
    let verified = exec
        .resolve(&req.occurrence, Resolution::Verify(vec![]), Duration::ZERO)
        .expect("the occurrence is parked");
    assert!(verified.requests.is_empty());
    assert!(
        exec.derive(&asks_place(T), &Subject::new("a")).is_some(),
        "the ask does not return once the parked occurrence resolved"
    );
}

/// An answer that expires during a retry backoff leaves no busy token, but
/// (T, s) is not idle while the retry is pending: the ask must stay hidden.
#[test]
fn an_ask_hides_while_a_retry_is_pending() {
    let mut d = descriptor();
    d.timers.push(TimerDecl {
        kind: TimerKind::Retry { max_attempts: 3 },
        after: Duration::from_secs(10),
    });
    let mut exec = asking(&d);
    let req = answer_and_start(&mut exec, "1");
    let failed = exec
        .complete(
            req.attempt,
            EffectResult::Failed(Cause::retryable("busy")),
            Duration::ZERO,
        )
        .expect("the attempt is known");
    assert!(failed.requests.is_empty());
    on_disk_changes(&mut exec);
    assert!(
        exec.tokens().token(&expired_place(T), "1").is_some(),
        "answer 1 did not expire during the backoff"
    );
    assert!(
        matches!(exec.why_not(T, "a"), Some(WhyNot::RetryPending { .. })),
        "the retry is not pending: {:?}",
        exec.why_not(T, "a")
    );
    assert_eq!(
        exec.derive(&asks_place(T), &Subject::new("a")),
        None,
        "the ask shows while a retry is pending"
    );
}
