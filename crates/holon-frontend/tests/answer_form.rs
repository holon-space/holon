//! Answering a `manual` transition's ask (#167 lane U): the shared collector
//! parses every answer kind at the boundary, the form closes only on a
//! settled `Done`, and the menu presents all three D151.a offer states.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::InputContext;
use holon_api::user_transition::InputParseError;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::OccurrenceId;
use holon_api::user_transition::Offer;
use holon_api::user_transition::Offered;
use holon_api::user_transition::PlaceName;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_api::user_transition::UserTransitions;
use holon_frontend::answer::AnswerForm;
use holon_frontend::answer::AnswerOutcome;
use holon_frontend::answer::FormPhase;
use holon_frontend::answer::PARAM_VALUE;
use holon_frontend::answer::answer_descriptor;
use holon_frontend::answer::fire_and_await;
use holon_frontend::command_provider::CommandProvider;
use holon_frontend::command_provider::REFUSED_ITEM_PREFIX;
use holon_frontend::param_collection::CollectStep;
use holon_frontend::param_collection::ParamCollector;
use holon_frontend::popup_menu::PopupItem;
use holon_frontend::popup_menu::PopupProvider;
use holon_frontend::popup_menu::PopupResult;
use proptest::prelude::*;

const EVEN: &str = "Even";
const INBOX: &str = "Inbox";
const INBOX_TOKENS: [&str; 2] = ["a.org", "b.org"];

const DOC: &str = "doc";
const DOC_IDS: [&str; 2] = ["d1", "d2"];
const ROW_KEYS: [&str; 1] = ["r1"];

/// Newtype `Even` parses even integers; place `Inbox` holds two subjects;
/// entity `doc` has two ids; the provider's table has one row key.
struct FakeInput;

impl InputContext for FakeInput {
    fn parse_newtype(&self, type_name: &str, text: &str) -> Result<Value, String> {
        assert_eq!(type_name, EVEN);
        match text.parse::<i64>() {
            Ok(n) if n % 2 == 0 => Ok(Value::Integer(n)),
            _ => Err("expected an even integer".to_string()),
        }
    }

    fn place_holds(&self, place: &str, subject: &str) -> Result<bool, NotImplemented> {
        Ok(place == INBOX && INBOX_TOKENS.contains(&subject))
    }

    fn entity_exists(&self, entity: &EntityName, id: &str) -> Result<bool, NotImplemented> {
        assert_eq!(entity.as_str(), DOC);
        Ok(DOC_IDS.contains(&id))
    }

    fn row_key_exists(&self, key: &str) -> Result<bool, NotImplemented> {
        Ok(ROW_KEYS.contains(&key))
    }
}

fn ask(input: Option<TypeHint>, irreversible: bool, has_preview: bool) -> AskRow {
    AskRow {
        transition: TransitionName("approve".into()),
        subject: SubjectName("a.org".into()),
        hash: AskHash([7; 32]),
        input,
        irreversible,
        has_preview,
    }
}

/// A port whose settlement follows a script, and which records every call.
struct FakePort {
    accept: Result<(), UftError>,
    script: Mutex<VecDeque<Result<AnswerSettlement, UftError>>>,
    answered: Mutex<Vec<AnswerCall>>,
    previews: Mutex<usize>,
}

impl FakePort {
    fn new(accept: Result<(), UftError>, script: Vec<Result<AnswerSettlement, UftError>>) -> Self {
        Self {
            accept,
            script: Mutex::new(script.into()),
            answered: Mutex::new(Vec::new()),
            previews: Mutex::new(0),
        }
    }

    fn answered(&self) -> usize {
        self.answered.lock().unwrap().len()
    }
}

impl UserTransitions for FakePort {
    fn asks(&self) -> Result<Vec<AskRow>, UftError> {
        unreachable!("the form never lists asks")
    }

    fn preview(&self, _: &AskRow) -> Result<String, UftError> {
        *self.previews.lock().unwrap() += 1;
        Ok("deletes 3 blocks".to_string())
    }

    fn answer(&self, call: AnswerCall) -> Result<AnswerId, UftError> {
        let id = call.answer.clone();
        self.answered.lock().unwrap().push(call);
        self.accept.clone().map(|()| id)
    }

    fn settlement(&self, _: &AnswerId) -> Result<AnswerSettlement, UftError> {
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .expect("polled past the end of the settlement script")
    }
}

// ── Collector: every hint parses at the boundary ────────────────────────

/// The model of the boundary parse, written before the implementation.
fn model_parse(hint: &TypeHint, raw: &Value) -> Option<Value> {
    match (hint, raw) {
        (TypeHint::Bool, Value::Boolean(_)) => Some(raw.clone()),
        (TypeHint::String, Value::String(_)) => Some(raw.clone()),
        (TypeHint::Number, Value::Integer(_)) => Some(raw.clone()),
        (TypeHint::Number, Value::String(s)) => s.trim().parse().ok().map(Value::Integer),
        (TypeHint::Enum { variants, .. }, Value::String(s)) => {
            variants.contains(s).then(|| raw.clone())
        }
        (TypeHint::Newtype { .. }, Value::String(s)) => s
            .parse::<i64>()
            .ok()
            .filter(|n| n % 2 == 0)
            .map(Value::Integer),
        (TypeHint::PlaceId { .. }, Value::String(s)) => {
            INBOX_TOKENS.contains(&s.as_str()).then(|| raw.clone())
        }
        (TypeHint::EntityIdOrRoot { .. }, Value::String(s)) if *s == root() => Some(raw.clone()),
        (TypeHint::EntityId { .. } | TypeHint::EntityIdOrRoot { .. }, Value::String(s)) => {
            DOC_IDS.contains(&s.as_str()).then(|| raw.clone())
        }
        (TypeHint::RowKey, Value::String(s)) => ROW_KEYS.contains(&s.as_str()).then(|| raw.clone()),
        _ => None,
    }
}

fn hints() -> impl Strategy<Value = TypeHint> {
    prop_oneof![
        Just(TypeHint::Bool),
        Just(TypeHint::String),
        Just(TypeHint::Number),
        Just(TypeHint::Enum {
            type_name: "Size".into(),
            variants: vec!["S".into(), "M".into()],
        }),
        Just(TypeHint::Newtype {
            type_name: EVEN.into(),
        }),
        Just(TypeHint::PlaceId {
            place: PlaceName(INBOX.into()),
        }),
        Just(TypeHint::EntityId {
            entity_name: DOC.into(),
        }),
        Just(TypeHint::EntityIdOrRoot {
            entity_name: DOC.into(),
        }),
        Just(TypeHint::RowKey),
    ]
}

fn root() -> String {
    EntityUri::no_parent().as_str().to_string()
}

fn raws() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Boolean),
        (-4i64..5).prop_map(Value::Integer),
        prop::sample::select(vec![
            "", "red", "blue", "S", "M", "L", "4", " 7 ", "x", "a.org", "c.org", "d1", "ghost",
            "r1",
        ])
        .prop_map(|s| Value::String(s.to_string())),
        Just(Value::String(root())),
    ]
}

#[test]
fn a_nonexistent_id_is_refused_at_the_parse() {
    for hint in [
        TypeHint::EntityId {
            entity_name: DOC.into(),
        },
        TypeHint::EntityIdOrRoot {
            entity_name: DOC.into(),
        },
        TypeHint::RowKey,
    ] {
        let ask = ask(Some(hint.clone()), false, false);
        let port = FakePort::new(Ok(()), vec![]);
        let mut form = AnswerForm::open(ask, &port);
        form.enter(&Value::String("ghost".into()), &port, &FakeInput);
        assert_eq!(
            port.answered(),
            0,
            "{hint:?}: a nonexistent id fired an answer"
        );
        let FormPhase::Editing { error: Some(error) } = form.phase() else {
            panic!("{hint:?}: the refusal is not shown: {:?}", form.phase());
        };
        assert!(
            error.contains("ghost"),
            "{hint:?}: the refusal does not name the input: {error}"
        );
    }
}

proptest! {
    #[test]
    fn collector_parses_every_hint_or_refuses_without_firing(
        hint in hints(),
        raw in raws(),
    ) {
        let ask = ask(Some(hint.clone()), false, false);
        let descriptor = answer_descriptor(&ask);
        let mut collector = ParamCollector::for_op(&descriptor, &descriptor.bound_params);
        let entered = collector.enter(PARAM_VALUE, &raw, &FakeInput);
        match model_parse(&hint, &raw) {
            Some(typed) => {
                prop_assert!(entered.is_ok(), "{hint:?} refused {raw:?}: {entered:?}");
                let CollectStep::Ready(intent) = collector.current() else {
                    panic!("parsed {raw:?} as {hint:?} but the collector is not ready");
                };
                prop_assert_eq!(intent.params.get(PARAM_VALUE), Some(&typed));
            }
            None => {
                prop_assert!(
                    matches!(entered, Err(InputParseError::Refused { .. })),
                    "{hint:?} accepted {raw:?}: {entered:?}"
                );
                prop_assert!(!matches!(collector.current(), CollectStep::Ready(_)));
            }
        }

        let port = FakePort::new(Ok(()), vec![Ok(AnswerSettlement::Waiting)]);
        let mut form = AnswerForm::open(ask, &port);
        form.enter(&raw, &port, &FakeInput);
        let fired = port.answered();
        match model_parse(&hint, &raw) {
            Some(typed) => {
                prop_assert_eq!(fired, 1);
                prop_assert_eq!(&port.answered.lock().unwrap()[0].value, &Some(typed));
            }
            None => {
                prop_assert_eq!(fired, 0, "a parse error fired an answer");
                let FormPhase::Editing { error: Some(_) } = form.phase() else {
                    panic!("a parse error is not shown: {:?}", form.phase());
                };
            }
        }
    }
}

#[test]
fn text_kinds_ask_for_text_entry() {
    for hint in [
        TypeHint::String,
        TypeHint::Number,
        TypeHint::Newtype {
            type_name: EVEN.into(),
        },
        TypeHint::PlaceId {
            place: PlaceName(INBOX.into()),
        },
        TypeHint::EntityId {
            entity_name: "doc".into(),
        },
    ] {
        let ask = ask(Some(hint.clone()), false, false);
        let descriptor = answer_descriptor(&ask);
        let collector = ParamCollector::for_op(&descriptor, &descriptor.bound_params);
        assert!(
            matches!(collector.current(), CollectStep::Enter { ref param_name, .. } if param_name == PARAM_VALUE),
            "{hint:?}: {:?}",
            collector.current()
        );
    }
}

#[test]
fn enum_variants_are_choices() {
    let ask = ask(
        Some(TypeHint::Enum {
            type_name: "Size".into(),
            variants: vec!["S".into(), "M".into()],
        }),
        false,
        false,
    );
    let descriptor = answer_descriptor(&ask);
    let collector = ParamCollector::for_op(&descriptor, &descriptor.bound_params);
    let CollectStep::Collect { choices, .. } = collector.current() else {
        panic!("enum is not a choice: {:?}", collector.current());
    };
    let labels: Vec<_> = choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["S", "M"]);
}

// ── Hook: fire-and-await closes only on a settled Done ──────────────────

fn settlement(terminal: u8) -> Result<AnswerSettlement, UftError> {
    let work = OccurrenceId::caller("w1");
    match terminal {
        0 => Ok(AnswerSettlement::Done { work }),
        1 => Ok(AnswerSettlement::Failed {
            work,
            cause: "disk full".into(),
        }),
        2 => Ok(AnswerSettlement::Expired {
            reason: ExpireReason::AskGone,
        }),
        3 => Ok(AnswerSettlement::Expired {
            reason: ExpireReason::SituationChanged,
        }),
        4 => Ok(AnswerSettlement::Undone),
        _ => Err(UftError::UnknownAnswer(AnswerId("x".into()))),
    }
}

fn expected(terminal: u8) -> AnswerOutcome {
    match terminal {
        0 => AnswerOutcome::Done,
        1 => AnswerOutcome::Failed {
            cause: "disk full".into(),
        },
        2 => AnswerOutcome::Expired(ExpireReason::AskGone),
        3 => AnswerOutcome::Expired(ExpireReason::SituationChanged),
        4 => AnswerOutcome::Undone,
        _ => AnswerOutcome::Refused(UftError::UnknownAnswer(AnswerId("x".into()))),
    }
}

proptest! {
    #[test]
    fn form_stays_open_until_the_answer_settles(
        pending in prop::collection::vec(any::<bool>(), 0..5),
        terminal in 0u8..6,
    ) {
        let mut script: Vec<_> = pending
            .iter()
            .map(|working| {
                Ok(if *working {
                    AnswerSettlement::Working { work: OccurrenceId::caller("w1") }
                } else {
                    AnswerSettlement::Waiting
                })
            })
            .collect();
        script.push(settlement(terminal));

        let port = FakePort::new(Ok(()), script.clone());
        let mut form = AnswerForm::open(ask(None, false, false), &port);
        form.submit(&port);
        for _ in &pending {
            prop_assert!(!form.is_closed(), "closed before settled");
            prop_assert!(matches!(form.phase(), FormPhase::Awaiting(_)), "{:?}", form.phase());
            form.poll(&port);
        }
        prop_assert!(!form.is_closed(), "closed before settled");
        form.poll(&port);
        let FormPhase::Settled(outcome) = form.phase() else {
            panic!("not settled after the terminal settlement: {:?}", form.phase());
        };
        prop_assert_eq!(outcome, &expected(terminal));
        prop_assert_eq!(form.is_closed(), terminal == 0, "only Done closes the form");

        let port = FakePort::new(Ok(()), script);
        let call = AnswerCall {
            transition: TransitionName("approve".into()),
            subject: SubjectName("a.org".into()),
            seen: AskHash([7; 32]),
            answer: AnswerId("a1".into()),
            value: None,
        };
        let outcome = futures::executor::block_on(fire_and_await(&port, call, || async {}));
        prop_assert_eq!(outcome, expected(terminal));
    }
}

#[test]
fn a_refused_answer_is_shown_not_closed() {
    let port = FakePort::new(Err(UftError::AlreadyAnswered), vec![]);
    let mut form = AnswerForm::open(ask(None, false, false), &port);
    form.submit(&port);
    let FormPhase::Settled(outcome) = form.phase() else {
        panic!("{:?}", form.phase());
    };
    assert_eq!(outcome, &AnswerOutcome::Refused(UftError::AlreadyAnswered));
    assert!(!form.is_closed());
    assert!(outcome.text().contains("already"), "{}", outcome.text());
}

#[test]
fn expired_says_the_question_no_longer_applies() {
    let text = AnswerOutcome::Expired(ExpireReason::AskGone).text();
    assert!(text.contains("no longer applies"), "{text}");
}

// ── Irreversible: preview on open, confirm before firing ────────────────

#[test]
fn irreversible_ask_confirms_with_the_preview_before_firing() {
    let port = FakePort::new(Ok(()), vec![Ok(AnswerSettlement::Waiting)]);
    let mut form = AnswerForm::open(ask(None, true, true), &port);
    assert_eq!(
        *port.previews.lock().unwrap(),
        1,
        "the preview loads on open"
    );
    assert_eq!(form.preview(), Some(&Ok("deletes 3 blocks".to_string())));

    form.submit(&port);
    assert!(matches!(form.phase(), FormPhase::Confirming { .. }));
    assert_eq!(
        port.answered(),
        0,
        "an irreversible answer fired unconfirmed"
    );

    form.cancel();
    assert!(matches!(form.phase(), FormPhase::Editing { error: None }));
    form.submit(&port);
    form.confirm(&port);
    assert_eq!(port.answered(), 1);
    assert!(matches!(form.phase(), FormPhase::Awaiting(_)));
}

#[test]
fn a_reversible_ask_loads_no_preview_it_lacks() {
    let port = FakePort::new(Ok(()), vec![]);
    let form = AnswerForm::open(ask(None, false, false), &port);
    assert_eq!(*port.previews.lock().unwrap(), 0);
    assert_eq!(form.preview(), None);
}

// ── Offers: the three D151.a states in the menu model ───────────────────

#[test]
fn menu_presents_enabled_disabled_with_reason_and_absent() {
    let offered = |name: &str, offer: Offer| {
        let mut descriptor = answer_descriptor(&ask(None, false, false));
        descriptor.name = name.into();
        Offered { descriptor, offer }
    };
    let offers = vec![
        offered("enabled_op", Offer::Enabled),
        offered(
            "refused_op",
            Offer::Refused {
                reason: "already answered".into(),
            },
        ),
        offered(
            "unknown_op",
            Offer::Unknown {
                why: "marking not loaded".into(),
            },
        ),
    ];
    let item = |id: &str| PopupItem {
        id: id.into(),
        label: id.to_uppercase(),
        icon: None,
    };
    let items = vec![
        item("enabled_op"),
        item("refused_op"),
        item("unknown_op"),
        item("absent_op"),
    ];

    let shown = CommandProvider::apply_offers(items, &offers);
    let ids: Vec<&str> = shown.iter().map(|i| i.id.as_str()).collect();
    let refused_id = format!("{REFUSED_ITEM_PREFIX}refused_op");
    assert!(
        ids.contains(&refused_id.as_str()),
        "disabled-with-reason not shown: {ids:?}"
    );
    assert_eq!(
        ids,
        ["enabled_op", refused_id.as_str(), "unknown_op"],
        "absent_op must be absent"
    );
    let refused = &shown[1];
    assert!(
        refused.label.contains("already answered"),
        "the reason is not on the entry: {}",
        refused.label
    );

    let provider = CommandProvider::new(vec![], HashMap::new()).with_offers(offers);
    match provider.on_select(refused, "") {
        PopupResult::Failed { message, .. } => assert_eq!(message, "already answered"),
        other => panic!("a disabled entry dispatched: {other:?}"),
    }
}
