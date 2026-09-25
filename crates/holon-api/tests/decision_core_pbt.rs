//! The decision core against a reference model: drafts parse exactly when the
//! model finds no violated rule, random command sequences match the model, and
//! an illegal command names one of the rules it violates and changes nothing.

use std::cell::Cell;
use std::collections::BTreeSet;

use chrono::TimeZone;
use chrono::Utc;
use holon_api::decision::AllowedDeciders;
use holon_api::decision::AnswerBody;
use holon_api::decision::AnswerInput;
use holon_api::decision::Answerer;
use holon_api::decision::Change;
use holon_api::decision::Command;
use holon_api::decision::Decider;
use holon_api::decision::Decision;
use holon_api::decision::DecisionDraft;
use holon_api::decision::DecisionError;
use holon_api::decision::DecisionRef;
use holon_api::decision::DraftQuestion;
use holon_api::decision::Effect;
use holon_api::decision::OptionKey;
use holon_api::decision::RawAnswer;
use holon_api::decision::RawRuling;
use holon_api::decision::RawWithdrawal;
use holon_api::decision::RefusedInput;
use holon_api::decision::Rule;
use holon_api::decision::Status;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

type Instant = chrono::DateTime<Utc>;

const ID: &str = "block:sharing-12";

const KEYS: &[&str] = &["a", "b", "c", "d"];
const MALFORMED_KEYS: &[&str] = &["A", "a b", "", "é"];
/// Well-formed, but never one of the options.
const STRANGER: &str = "z";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Person,
    Agent,
    Model,
}

const ANSWERERS: &[(&str, Option<Kind>)] = &[
    ("person:martin", Some(Kind::Person)),
    ("person:eve", Some(Kind::Person)),
    ("agent:orch", Some(Kind::Agent)),
    ("model:jev-1", Some(Kind::Model)),
    ("martin", None),
    ("person:", None),
    ("robot:x", None),
];
const ALLOWED: &[&str] = &["person:martin", "agent:orch"];

/// `(min, max)`, or `None` when the written form is illegal.
type Bounds = Option<(u8, u8)>;

const CHOOSE: &[(Option<&str>, Bounds)] = &[
    (None, Some((1, 1))),
    (Some("1"), Some((1, 1))),
    (Some("2"), Some((2, 2))),
    (Some("0..1"), Some((0, 1))),
    (Some("0..2"), Some((0, 2))),
    (Some("1..3"), Some((1, 3))),
    (Some("0"), None),
    (Some("2..1"), None),
    (Some("1.."), None),
    (Some("x"), None),
    (Some("300"), None),
];

const TIMES: &[(&str, bool)] = &[
    ("2026-09-25T10:04:00Z", true),
    ("2026-09-25T12:04:00.250+02:00", true),
    ("yesterday", false),
];

const REFS: &[(&str, bool)] = &[
    (ID, true),
    ("github-issue:owner/repo/7", true),
    ("block:other-3", true),
    ("nocolon", false),
    ("", false),
    ("block:", false),
    ("sentinel:no_parent", false),
    ("with space:x", false),
];

fn cases(default: u32) -> u32 {
    match std::env::var("PROPTEST_CASES") {
        Ok(n) => n.parse().expect("PROPTEST_CASES is a count"),
        Err(_) => default,
    }
}

const PROBS: &[f64] = &[0.0, 0.2, 0.5, 0.8, 1.0, 1.5, -0.1, f64::NAN];

fn key_pool() -> impl Strategy<Value = String> + Clone {
    prop_oneof![
        6 => prop::sample::select(KEYS).prop_map(str::to_string),
        1 => Just(STRANGER.to_string()),
    ]
}

fn answer_input<K: std::fmt::Debug + Clone + 'static>(
    key: impl Strategy<Value = K> + Clone + 'static,
) -> impl Strategy<Value = AnswerInput<K>> {
    let pair = (key.clone(), prop::sample::select(PROBS));
    prop_oneof![
        prop::collection::vec(key, 0..4).prop_map(AnswerInput::Pick),
        prop::collection::vec(pair.clone(), 0..4).prop_map(AnswerInput::Categorical),
        prop::collection::vec(pair, 0..4).prop_map(AnswerInput::Marginals),
    ]
}

/// Mostly the legal entries of a `(value, legal)` table, sometimes any.
fn mostly_legal<T: Copy + std::fmt::Debug + 'static>(
    table: &'static [(&'static str, T)],
    legal: fn(&T) -> bool,
) -> impl Strategy<Value = String> + Clone {
    let legal: Vec<&str> = table
        .iter()
        .filter(|(_, t)| legal(t))
        .map(|(s, _)| *s)
        .collect();
    prop_oneof![
        4 => prop::sample::select(legal),
        1 => prop::sample::select(table).prop_map(|(s, _)| s),
    ]
    .prop_map(str::to_string)
}

fn raw_answerer() -> impl Strategy<Value = String> + Clone {
    mostly_legal(ANSWERERS, Option::is_some)
}

fn raw_time() -> impl Strategy<Value = String> + Clone {
    mostly_legal(TIMES, |ok| *ok)
}

/// Mostly a legal small selection, sometimes any key list.
fn raw_keys() -> impl Strategy<Value = Vec<String>> + Clone {
    prop_oneof![
        3 => prop::sample::subsequence(KEYS, 0..=2)
            .prop_map(|ks| ks.into_iter().map(str::to_string).collect()),
        1 => prop::collection::vec(key_pool(), 0..4),
    ]
}

fn raw_question() -> impl Strategy<Value = DraftQuestion> {
    let option_key = prop_oneof![
        8 => prop::sample::select(KEYS),
        1 => prop::sample::select(MALFORMED_KEYS),
    ];
    let option_keys = prop_oneof![
        3 => prop::sample::subsequence(KEYS, 1..=KEYS.len()),
        1 => prop::collection::vec(option_key, 0..5),
    ];
    let choose = prop_oneof![
        3 => prop::sample::select(CHOOSE.iter().filter(|(_, c)| c.is_some()).copied().collect::<Vec<_>>()),
        1 => prop::sample::select(CHOOSE),
    ];
    (
        "[a-z ?]{0,12}",
        option_keys.prop_flat_map(|keys| {
            let n = keys.len();
            (Just(keys), prop::collection::vec("[A-Za-z ]{0,8}", n))
        }),
        choose,
        prop::option::weighted(0.3, raw_keys()),
        prop::option::weighted(0.3, prop::sample::select(REFS)),
    )
        .prop_map(
            |(question, (keys, labels), (choose, _), recommend, supersedes)| DraftQuestion {
                question,
                options: keys.into_iter().map(str::to_string).zip(labels).collect(),
                choose: choose.map(str::to_string),
                recommend,
                supersedes: supersedes.map(|(r, _)| r.to_string()),
            },
        )
}

fn raw_draft() -> impl Strategy<Value = DecisionDraft> {
    let ruling = (
        prop::option::weighted(0.9, raw_keys()),
        prop::option::weighted(0.9, raw_answerer()),
        prop::option::weighted(0.9, raw_time()),
        prop::option::of("[a-z ]{0,8}"),
    )
        .prop_map(|(chosen, decider, at, note)| RawRuling {
            chosen,
            decider,
            at,
            note,
        });
    let withdrawal = (
        prop::option::weighted(0.9, raw_answerer()),
        prop::option::weighted(0.9, raw_time()),
    )
        .prop_map(|(by, at)| RawWithdrawal { by, at });
    let answer = (
        raw_answerer(),
        raw_time(),
        answer_input(key_pool()),
        prop::option::of("[a-z ]{0,8}"),
    )
        .prop_map(|(by, at, body, rationale)| RawAnswer {
            by,
            at,
            body,
            rationale,
        });
    let refused =
        ("[a-z/ ]{1,10}", "[a-z ]{1,10}").prop_map(|(item, reason)| RefusedInput { item, reason });
    (
        prop::sample::select(
            REFS.iter()
                .filter(|(_, ok)| *ok)
                .copied()
                .collect::<Vec<_>>(),
        ),
        "[a-z0-9#-]{1,8}",
        raw_question(),
        prop::option::weighted(0.4, ruling),
        prop::option::weighted(0.2, withdrawal),
        prop::collection::vec(answer, 0..3),
        prop::collection::vec(refused, 0..2),
    )
        .prop_map(
            |((id, _), label, question, ruling, withdrawn, answers, refused)| DecisionDraft {
                id: DecisionRef::parse(id).expect("REFS marks this id well-formed"),
                label,
                question,
                ruling,
                withdrawn,
                answers,
                refused,
            },
        )
}

fn answerer_kind(raw: &str) -> Option<Kind> {
    ANSWERERS
        .iter()
        .find(|(s, _)| *s == raw)
        .expect("answerer strings come from ANSWERERS")
        .1
}

fn time_ok(raw: &str) -> bool {
    TIMES
        .iter()
        .find(|(s, _)| *s == raw)
        .expect("times come from TIMES")
        .1
}

/// Rules a key set violates against `options` and a valid `choose`.
fn selection_violations(
    keys: &[String],
    options: &[String],
    choose: Option<(u8, u8)>,
    out: &mut BTreeSet<Rule>,
) {
    let mut seen = BTreeSet::new();
    for k in keys {
        if !options.contains(k) || !seen.insert(k) {
            out.insert(Rule::Dc2);
        }
    }
    if let Some((min, max)) = choose {
        if keys.len() < usize::from(min) || keys.len() > usize::from(max) {
            out.insert(Rule::Dc3);
        }
    }
}

fn distribution_violations(
    pairs: &[(String, f64)],
    options: &[String],
    categorical: bool,
    choose: Option<(u8, u8)>,
    out: &mut BTreeSet<Rule>,
) {
    if categorical && choose.is_some_and(|(_, max)| max != 1) {
        out.insert(Rule::Dc5);
    }
    let keys: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();
    selection_violations(&keys, options, None, out);
    if pairs.iter().any(|(_, p)| !(0.0..=1.0).contains(p)) {
        out.insert(Rule::Dc5);
    }
    if categorical && pairs.iter().map(|(_, p)| p).sum::<f64>() > 1.0 {
        out.insert(Rule::Dc5);
    }
}

fn question_violations(id: &str, q: &DraftQuestion) -> (BTreeSet<Rule>, Option<(u8, u8)>) {
    let mut v = BTreeSet::new();
    let options: Vec<String> = q.options.iter().map(|(k, _)| k.clone()).collect();
    let distinct: BTreeSet<&String> = options.iter().collect();
    if options.is_empty()
        || distinct.len() != options.len()
        || options.iter().any(|k| MALFORMED_KEYS.contains(&k.as_str()))
    {
        v.insert(Rule::Dc1);
    }
    let choose = CHOOSE
        .iter()
        .find(|(s, _)| *s == q.choose.as_deref())
        .expect("choose comes from CHOOSE")
        .1
        .filter(|(_, max)| usize::from(*max) <= options.len());
    if choose.is_none() {
        v.insert(Rule::Dc3);
    }
    if let Some(r) = &q.recommend {
        selection_violations(r, &options, choose, &mut v);
    }
    if let Some(s) = &q.supersedes {
        let ok = REFS
            .iter()
            .find(|(r, _)| r == s)
            .expect("refs come from REFS")
            .1;
        if !ok || s == id {
            v.insert(Rule::Dc7);
        }
    }
    (v, choose)
}

fn draft_violations(d: &DecisionDraft) -> BTreeSet<Rule> {
    let (mut v, choose) = question_violations(d.id.uri().as_str(), &d.question);
    let options: Vec<String> = d.question.options.iter().map(|(k, _)| k.clone()).collect();
    if d.ruling.is_some() && d.withdrawn.is_some() {
        v.insert(Rule::Dc4);
    }
    if let Some(r) = &d.ruling {
        if r.chosen.is_none() || r.decider.is_none() || r.at.is_none() {
            v.insert(Rule::Dc4);
        }
        match r.decider.as_deref().map(answerer_kind) {
            Some(None) => {
                v.insert(Rule::Syntax);
            }
            Some(Some(Kind::Model)) => {
                v.insert(Rule::Dc8);
            }
            _ => {}
        }
        if r.at.as_deref().is_some_and(|t| !time_ok(t)) {
            v.insert(Rule::Syntax);
        }
        if let Some(chosen) = &r.chosen {
            selection_violations(chosen, &options, choose, &mut v);
        }
    }
    if let Some(w) = &d.withdrawn {
        if w.by.is_none() || w.at.is_none() {
            v.insert(Rule::Dc4);
        }
        if w.by.as_deref().is_some_and(|b| answerer_kind(b).is_none())
            || w.at.as_deref().is_some_and(|t| !time_ok(t))
        {
            v.insert(Rule::Syntax);
        }
    }
    for a in &d.answers {
        if answerer_kind(&a.by).is_none() || !time_ok(&a.at) {
            v.insert(Rule::Syntax);
        }
        body_violations(&a.body, &options, choose, &mut v);
    }
    v
}

fn body_violations(
    body: &AnswerInput<String>,
    options: &[String],
    choose: Option<(u8, u8)>,
    v: &mut BTreeSet<Rule>,
) {
    match body {
        AnswerInput::Pick(keys) => selection_violations(keys, options, choose, v),
        AnswerInput::Categorical(p) => distribution_violations(p, options, true, choose, v),
        AnswerInput::Marginals(p) => distribution_violations(p, options, false, choose, v),
    }
}

fn selection_strings<'a>(keys: impl Iterator<Item = &'a OptionKey>) -> BTreeSet<String> {
    keys.map(|k| k.as_str().to_string()).collect()
}

fn instant(raw: &str) -> Instant {
    chrono::DateTime::parse_from_rfc3339(raw)
        .expect("valid in TIMES")
        .with_timezone(&Utc)
}

/// The legal draft's content, as the parsed decision must show it.
fn assert_matches_draft(d: &Decision, draft: &DecisionDraft) {
    assert_eq!(d.id(), &draft.id);
    assert_eq!(d.label(), draft.label);
    assert_eq!(d.question(), draft.question.question);
    let options: Vec<(String, String)> = d
        .options()
        .iter()
        .map(|o| (o.key.as_str().to_string(), o.label.clone()))
        .collect();
    assert_eq!(options, draft.question.options);
    let (min, max) = CHOOSE
        .iter()
        .find(|(s, _)| *s == draft.question.choose.as_deref())
        .and_then(|(_, c)| *c)
        .expect("a legal draft has a valid choose");
    assert_eq!((d.choose().min(), d.choose().max()), (min, max));
    assert_eq!(
        d.recommend().map(|s| selection_strings(s.keys())),
        draft
            .question
            .recommend
            .as_ref()
            .map(|r| r.iter().cloned().collect())
    );
    assert_eq!(
        d.supersedes().map(|s| s.to_string()),
        draft.question.supersedes
    );
    match (d.status(), &draft.ruling, &draft.withdrawn) {
        (Status::Open, None, None) => {}
        (Status::Decided(r), Some(raw), None) => {
            assert_eq!(
                selection_strings(r.chosen().keys()),
                raw.chosen.iter().flatten().cloned().collect()
            );
            assert_eq!(Some(r.decider().to_string()), raw.decider);
            assert_eq!(r.at(), instant(raw.at.as_deref().unwrap()));
            assert_eq!(r.note(), raw.note.as_deref());
        }
        (Status::Withdrawn { by, at }, None, Some(raw)) => {
            assert_eq!(Some(by.to_string()), raw.by);
            assert_eq!(*at, instant(raw.at.as_deref().unwrap()));
        }
        (s, r, w) => panic!("status {s:?} does not match ruling {r:?} / withdrawal {w:?}"),
    }
    assert_eq!(d.answers().len(), draft.answers.len());
    for (a, raw) in d.answers().iter().zip(&draft.answers) {
        assert_eq!(a.by().to_string(), raw.by);
        assert_eq!(a.at(), instant(&raw.at));
        assert_eq!(a.rationale(), raw.rationale.as_deref());
        match (a.body(), &raw.body) {
            (AnswerBody::Pick(s), AnswerInput::Pick(k)) => {
                assert_eq!(selection_strings(s.keys()), k.iter().cloned().collect())
            }
            (AnswerBody::Categorical(p), AnswerInput::Categorical(k))
            | (AnswerBody::Marginals(p), AnswerInput::Marginals(k)) => {
                let got: Vec<(String, f64)> = p
                    .iter()
                    .map(|(k, p)| (k.as_str().to_string(), p.get()))
                    .collect();
                let mut want = k.clone();
                want.sort_by(|x, y| x.0.cmp(&y.0));
                assert_eq!(got, want);
            }
            (a, r) => panic!("answer body {a:?} does not match {r:?}"),
        }
    }
    assert_eq!(d.refused(), draft.refused);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(512)))]

    #[test]
    fn a_draft_parses_exactly_when_it_violates_no_rule(draft in raw_draft()) {
        let expected = draft_violations(&draft);
        match Decision::parse(draft.clone()) {
            Ok(d) => {
                prop_assert!(expected.is_empty(), "parsed, but the model expects {expected:?}: {draft:?}");
                assert_matches_draft(&d, &draft);
                prop_assert_eq!(Decision::parse(d.to_draft()), Ok(d));
            }
            Err(e) => prop_assert!(
                expected.contains(&e.rule()),
                "refused with {e} ({:?}); the model expects {expected:?}: {draft:?}",
                e.rule()
            ),
        }
    }

    #[test]
    fn ask_opens_exactly_the_legal_questions(q in raw_question()) {
        let id = DecisionRef::parse(ID).unwrap();
        let (expected, _) = question_violations(ID, &q);
        match Decision::ask(id.clone(), "sharing-12".into(), q.clone()) {
            Ok(d) => {
                prop_assert!(expected.is_empty(), "asked, but the model expects {expected:?}");
                prop_assert_eq!(d.status(), &Status::Open);
                prop_assert!(d.answers().is_empty());
                prop_assert_eq!(d.id(), &id);
                prop_assert_eq!(Decision::parse(d.to_draft()), Ok(d));
            }
            Err(e) => prop_assert!(
                expected.contains(&e.rule()),
                "refused with {e}; the model expects {expected:?}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ModelStatus {
    Open,
    Decided {
        chosen: BTreeSet<String>,
        decider: String,
        at: Instant,
    },
    Withdrawn {
        by: String,
        at: Instant,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Model {
    options: Vec<String>,
    choose: (u8, u8),
    status: ModelStatus,
    answers: Vec<(String, Instant)>,
}

impl Model {
    fn project(d: &Decision) -> Model {
        let status = match d.status() {
            Status::Open => ModelStatus::Open,
            Status::Decided(r) => ModelStatus::Decided {
                chosen: selection_strings(r.chosen().keys()),
                decider: r.decider().to_string(),
                at: r.at(),
            },
            Status::Withdrawn { by, at } => ModelStatus::Withdrawn {
                by: by.to_string(),
                at: *at,
            },
        };
        Model {
            options: d
                .options()
                .iter()
                .map(|o| o.key.as_str().to_string())
                .collect(),
            choose: (d.choose().min(), d.choose().max()),
            status,
            answers: d
                .answers()
                .iter()
                .map(|a| (a.by().to_string(), a.at()))
                .collect(),
        }
    }

    fn violations(&self, c: &Command) -> BTreeSet<Rule> {
        let mut v = BTreeSet::new();
        let strings = |keys: &[OptionKey]| -> Vec<String> {
            keys.iter().map(|k| k.as_str().to_string()).collect()
        };
        match c {
            Command::Answer { body, .. } => {
                let body = match body {
                    AnswerInput::Pick(k) => AnswerInput::Pick(strings(k)),
                    AnswerInput::Categorical(p) => AnswerInput::Categorical(
                        p.iter()
                            .map(|(k, p)| (k.as_str().to_string(), *p))
                            .collect(),
                    ),
                    AnswerInput::Marginals(p) => AnswerInput::Marginals(
                        p.iter()
                            .map(|(k, p)| (k.as_str().to_string(), *p))
                            .collect(),
                    ),
                };
                body_violations(&body, &self.options, Some(self.choose), &mut v);
            }
            Command::Decide {
                chosen, decider, ..
            } => {
                if matches!(self.status, ModelStatus::Withdrawn { .. }) {
                    v.insert(Rule::Dc4);
                }
                if !ALLOWED.contains(&decider.to_string().as_str()) {
                    v.insert(Rule::Dc8);
                }
                selection_violations(&strings(chosen), &self.options, Some(self.choose), &mut v);
            }
            Command::Withdraw { .. } => {
                if self.status != ModelStatus::Open {
                    v.insert(Rule::Dc4);
                }
            }
        }
        v
    }

    fn step(&mut self, c: &Command, now: Instant) {
        match c {
            Command::Answer { by, .. } => self.answers.push((by.to_string(), now)),
            Command::Decide {
                chosen, decider, ..
            } => {
                self.status = ModelStatus::Decided {
                    chosen: selection_strings(chosen.iter()),
                    decider: decider.to_string(),
                    at: now,
                }
            }
            Command::Withdraw { by } => {
                self.status = ModelStatus::Withdrawn {
                    by: by.to_string(),
                    at: now,
                }
            }
        }
    }
}

fn legal_question() -> impl Strategy<Value = DraftQuestion> {
    prop::sample::subsequence(KEYS, 1..=KEYS.len())
        .prop_flat_map(|keys| {
            let n = keys.len();
            let chooses: Vec<Option<&str>> = CHOOSE
                .iter()
                .filter(|(_, c)| c.is_some_and(|(_, max)| usize::from(max) <= n))
                .map(|(s, _)| *s)
                .collect();
            (Just(keys), prop::sample::select(chooses))
        })
        .prop_map(|(keys, choose)| DraftQuestion {
            question: "Delete the fork branch?".into(),
            options: keys
                .iter()
                .map(|k| (k.to_string(), format!("option {k}")))
                .collect(),
            choose: choose.map(str::to_string),
            recommend: None,
            supersedes: None,
        })
}

fn command() -> impl Strategy<Value = Command> {
    let answerer = prop::sample::select(
        ANSWERERS
            .iter()
            .filter(|(_, k)| k.is_some())
            .map(|(s, _)| *s)
            .collect::<Vec<_>>(),
    )
    .prop_map(|s| Answerer::parse(s).expect("ANSWERERS marks it well-formed"));
    let key = key_pool().prop_map(|k| OptionKey::parse(&k).expect("pool keys are well-formed"));
    prop_oneof![
        3 => (answerer.clone(), answer_input(key.clone()), prop::option::of("[a-z ]{0,8}"))
            .prop_map(|(by, body, rationale)| Command::Answer { by, body, rationale }),
        3 => (prop::collection::vec(key, 0..4), answerer.clone(), prop::option::of("[a-z ]{0,8}"))
            .prop_map(|(chosen, decider, note)| Command::Decide { chosen, decider, note }),
        1 => answerer.prop_map(|by| Command::Withdraw { by }),
    ]
}

fn allowed() -> AllowedDeciders {
    AllowedDeciders::new(
        ALLOWED.iter().map(|s| {
            Decider::try_from(Answerer::parse(s).unwrap()).expect("ALLOWED holds no model")
        }),
    )
}

/// Changes `apply` made for other states: earlier states of the decision
/// under test, and states of a second decision.
struct NotFromHere(Vec<Change>);

impl NotFromHere {
    /// Each is refused with `Rule::Stale`; returns how many were tried.
    fn assert_refused_by(&self, d: &Decision) -> Result<usize, TestCaseError> {
        for c in &self.0 {
            match d.commit(c.clone()) {
                Err(e) => prop_assert_eq!(e.rule(), Rule::Stale, "{}", e),
                Ok(_) => prop_assert!(false, "{c:?} committed to {d:?}"),
            }
        }
        Ok(self.0.len())
    }
}

fn apply_all(
    mut d: Decision,
    commands: Vec<Command>,
    t0: Instant,
    deciders: &AllowedDeciders,
) -> Vec<Change> {
    let mut made = Vec::new();
    for (i, c) in commands.into_iter().enumerate() {
        if let Ok(change) = d.apply(c, t0 + chrono::Duration::seconds(i as i64), deciders) {
            d = d.commit(change.clone()).expect("apply made it for d");
            made.push(change);
        }
    }
    made
}

#[test]
fn command_sequences_match_the_model() {
    let refused = Cell::new(0usize);
    let strategy = (
        legal_question(),
        prop::collection::vec(command(), 0..30),
        legal_question(),
        prop::collection::vec(command(), 0..8),
    );
    TestRunner::new(ProptestConfig::with_cases(cases(256)))
        .run(&strategy, |(q, commands, other_q, other_commands)| {
            let deciders = allowed();
            let t0 = Utc.with_ymd_and_hms(2026, 9, 25, 10, 0, 0).unwrap();
            let other = Decision::ask(
                DecisionRef::parse("block:other-3").unwrap(),
                "other-3".into(),
                other_q,
            )
            .expect("legal_question is legal");
            let mut not_from_here = NotFromHere(apply_all(other, other_commands, t0, &deciders));
            let mut d = Decision::ask(DecisionRef::parse(ID).unwrap(), "sharing-12".into(), q)
                .expect("legal_question is legal");
            let mut model = Model::project(&d);
            for (i, c) in commands.into_iter().enumerate() {
                let now = t0 + chrono::Duration::seconds(i as i64);
                let expected = model.violations(&c);
                let before = d.clone();
                match d.apply(c.clone(), now, &deciders) {
                    Ok(change) => {
                        prop_assert!(
                            expected.is_empty(),
                            "{c:?} applied, but the model expects {expected:?}"
                        );
                        prop_assert_eq!(change.decision(), d.id());
                        match (change.effect(), &c, &model.status) {
                            (Effect::Answered(a), Command::Answer { by, rationale, .. }, _) => {
                                prop_assert_eq!(a.by(), by);
                                prop_assert_eq!(a.at(), now);
                                prop_assert_eq!(a.rationale(), rationale.as_deref());
                            }
                            (
                                Effect::Decided { ruling, replaces },
                                Command::Decide { note, .. },
                                prior,
                            ) => {
                                prop_assert_eq!(ruling.note(), note.as_deref());
                                let replaced = replaces.as_ref().map(|r| ModelStatus::Decided {
                                    chosen: selection_strings(r.chosen().keys()),
                                    decider: r.decider().to_string(),
                                    at: r.at(),
                                });
                                let prior_ruling = matches!(prior, ModelStatus::Decided { .. })
                                    .then(|| prior.clone());
                                prop_assert_eq!(
                                    replaced,
                                    prior_ruling,
                                    "DC9: a re-decide carries the ruling it replaces"
                                );
                            }
                            (Effect::Withdrawn { at, .. }, Command::Withdraw { .. }, _) => {
                                prop_assert_eq!(*at, now)
                            }
                            (e, c, _) => prop_assert!(false, "{c:?} produced {e:?}"),
                        }
                        refused.set(refused.get() + not_from_here.assert_refused_by(&d)?);
                        d = d.commit(change.clone()).expect("apply made it for d");
                        not_from_here.0.push(change);
                        model.step(&c, now);
                        prop_assert_eq!(Model::project(&d), model.clone());
                        prop_assert_eq!(Decision::parse(d.to_draft()), Ok(d.clone()));
                    }
                    Err(e) => {
                        prop_assert!(
                            expected.contains(&e.rule()),
                            "{c:?} refused with {e} ({:?}); the model expects {expected:?}",
                            e.rule()
                        );
                        prop_assert_eq!(&d, &before);
                    }
                }
            }
            refused.set(refused.get() + not_from_here.assert_refused_by(&d)?);
            Ok(())
        })
        .unwrap();
    eprintln!(
        "changes not made for the committed state, all refused: {}",
        refused.get()
    );
    assert!(refused.get() > 0, "no foreign or stale change was tried");
}

#[test]
fn a_model_is_never_a_decider() {
    let model = Answerer::parse("model:jev-1").unwrap();
    assert_eq!(Decider::try_from(model).unwrap_err().rule(), Rule::Dc8);
}

#[test]
fn a_decision_ref_is_scheme_qualified() {
    for (raw, ok) in REFS {
        let parsed = DecisionRef::parse(raw);
        assert_eq!(parsed.is_ok(), *ok, "{raw:?}: {parsed:?}");
        if let Err(e) = parsed {
            assert_eq!(e.rule(), Rule::Dc7, "{raw:?}");
        }
    }
    assert_eq!(
        DecisionRef::parse("github-issue:owner/repo/7")
            .unwrap()
            .uri()
            .scheme(),
        "github-issue"
    );
}

fn asked(options: &[&str], choose: &str) -> Decision {
    let q = DraftQuestion {
        question: "q".into(),
        options: options
            .iter()
            .map(|k| (k.to_string(), k.to_string()))
            .collect(),
        choose: Some(choose.into()),
        recommend: None,
        supersedes: None,
    };
    Decision::ask(DecisionRef::parse(ID).unwrap(), "l".into(), q).unwrap()
}

fn t0() -> Instant {
    Utc.with_ymd_and_hms(2026, 9, 25, 10, 0, 0).unwrap()
}

fn decide(keys: &[&str]) -> Command {
    Command::Decide {
        chosen: keys.iter().map(|k| OptionKey::parse(k).unwrap()).collect(),
        decider: Answerer::parse("person:martin").unwrap(),
        note: None,
    }
}

#[test]
fn a_change_from_another_decision_is_stale() {
    let a = asked(&["a", "b"], "1");
    let b = Decision::parse(DecisionDraft {
        id: DecisionRef::parse("block:other-3").unwrap(),
        ..a.to_draft()
    })
    .unwrap();
    let change = a.apply(decide(&["a"]), t0(), &allowed()).unwrap();
    let e = b.commit(change).unwrap_err();
    assert!(matches!(e, DecisionError::StaleChange(_)), "{e:?}");
}

#[test]
fn a_withdrawn_decision_takes_no_earlier_ruling() {
    let d = asked(&["a", "b"], "1");
    let ruling = d.apply(decide(&["a"]), t0(), &allowed()).unwrap();
    let by = Answerer::parse("person:martin").unwrap();
    let withdrawn = d
        .commit(d.apply(Command::Withdraw { by }, t0(), &allowed()).unwrap())
        .unwrap();
    let e = withdrawn.commit(ruling).unwrap_err();
    assert!(matches!(e, DecisionError::StaleChange(_)), "{e:?}");
}

#[test]
fn a_decided_decision_takes_no_earlier_withdrawal() {
    let d = asked(&["a", "b"], "1");
    let by = Answerer::parse("person:martin").unwrap();
    let withdrawal = d.apply(Command::Withdraw { by }, t0(), &allowed()).unwrap();
    let decided = d
        .commit(d.apply(decide(&["a"]), t0(), &allowed()).unwrap())
        .unwrap();
    let e = decided.commit(withdrawal).unwrap_err();
    assert!(matches!(e, DecisionError::StaleChange(_)), "{e:?}");
}

#[test]
fn a_change_survives_rereading_the_same_state() {
    let d = asked(&["a", "b"], "1");
    let change = d.apply(decide(&["a"]), t0(), &allowed()).unwrap();
    let reread = Decision::parse(d.to_draft()).unwrap();
    assert!(matches!(
        reread.commit(change).unwrap().status(),
        Status::Decided(_)
    ));
}
