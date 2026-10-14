//! The #167 contract: HL-IR, Asks row and Answer call survive serde, and the
//! input boundary refuses bad enum labels and newtype text with a typed error.

use std::time::Duration;

use holon_api::EntityName;
use holon_api::OperationParam;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::*;
use proptest::prelude::*;

struct Cx;

impl InputContext for Cx {
    fn parse_newtype(&self, type_name: &str, text: &str) -> Result<Value, String> {
        assert_eq!(type_name, "FileName");
        if text.is_empty() || text.contains('/') {
            return Err("a file name is not empty and has no '/'".into());
        }
        Ok(Value::String(text.into()))
    }

    fn place_holds(&self, place: &str, subject: &str) -> Result<bool, NotImplemented> {
        Ok(place == "Project" && subject == "p1")
    }

    fn entity_exists(&self, _: &holon_api::EntityName, _: &str) -> Result<bool, NotImplemented> {
        unreachable!("the contract test asks for no entity id")
    }

    fn row_key_exists(&self, _: &str) -> Result<bool, NotImplemented> {
        unreachable!("the contract test asks for no row key")
    }
}

fn pick() -> TypeHint {
    TypeHint::Enum {
        type_name: "Pick".into(),
        variants: vec!["Ours".into(), "Theirs".into(), "Both".into()],
    }
}

fn file_name() -> TypeHint {
    TypeHint::Newtype {
        type_name: "FileName".into(),
    }
}

fn s(text: &str) -> Value {
    Value::String(text.into())
}

#[test]
fn bad_enum_label_is_a_typed_refusal() {
    assert_eq!(parse_input(&pick(), &s("Theirs"), &Cx), Ok(s("Theirs")));
    assert_eq!(
        parse_input(&pick(), &s("Mine"), &Cx),
        Err(InputParseError::Refused {
            type_name: "Pick".into(),
            input: "Mine".into(),
            reason: "expected one of Ours, Theirs, Both".into(),
        })
    );
}

#[test]
fn bad_newtype_text_is_a_typed_refusal() {
    assert_eq!(parse_input(&file_name(), &s("a.org"), &Cx), Ok(s("a.org")));
    assert_eq!(
        parse_input(&file_name(), &s("a/b"), &Cx),
        Err(InputParseError::Refused {
            type_name: "FileName".into(),
            input: "a/b".into(),
            reason: "a file name is not empty and has no '/'".into(),
        })
    );
}

#[test]
fn place_id_outside_its_place_is_refused() {
    let hint = TypeHint::PlaceId {
        place: PlaceName("Project".into()),
    };
    assert_eq!(parse_input(&hint, &s("p1"), &Cx), Ok(s("p1")));
    assert!(matches!(
        parse_input(&hint, &s("p2"), &Cx),
        Err(InputParseError::Refused { type_name, input, .. })
            if type_name == "id of Project" && input == "p2"
    ));
}

#[test]
fn stubs_name_their_lane() {
    assert_eq!(
        Unwired.asks(),
        Err(UftError::NotImplemented(NotImplemented {
            lane: Lane::Integration,
            what: "UserTransitions::asks",
        }))
    );
}

fn name() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9_.#]{0,8}"
}

fn field(type_hint: TypeHint) -> OperationParam {
    OperationParam {
        name: "f".into(),
        type_hint,
        description: String::new(),
    }
}

fn type_hint() -> impl Strategy<Value = Option<TypeHint>> {
    let entity = || "[a-z][a-z0-9_]{0,8}".prop_map(|n| EntityName::new(&n));
    prop_oneof![
        Just(None),
        Just(Some(TypeHint::Bool)),
        Just(Some(TypeHint::String)),
        Just(Some(TypeHint::Number)),
        entity().prop_map(|entity_name| Some(TypeHint::EntityId { entity_name })),
        entity().prop_map(|entity_name| Some(TypeHint::EntityIdOrRoot { entity_name })),
        name().prop_map(|p| Some(TypeHint::FieldValue {
            field_param: p,
            fields: vec![field(TypeHint::RowKey)],
        })),
        Just(Some(TypeHint::RowKey)),
        Just(Some(TypeHint::Object {
            fields: vec![field(TypeHint::Bool)],
        })),
        Just(Some(TypeHint::Expr)),
        Just(Some(TypeHint::Collection)),
        prop::collection::vec(name(), 1..4).prop_map(|variants| Some(TypeHint::Enum {
            type_name: "Pick".into(),
            variants,
        })),
        name().prop_map(|type_name| Some(TypeHint::Newtype { type_name })),
        name().prop_map(|p| Some(TypeHint::PlaceId {
            place: PlaceName(p)
        })),
    ]
}

// No `_` arm: a new variant does not compile until it gets an index here and a
// strategy in `type_hint`.
fn variant_index(h: &TypeHint) -> usize {
    match h {
        TypeHint::Bool => 0,
        TypeHint::String => 1,
        TypeHint::Number => 2,
        TypeHint::EntityId { .. } => 3,
        TypeHint::EntityIdOrRoot { .. } => 4,
        TypeHint::FieldValue { .. } => 5,
        TypeHint::RowKey => 6,
        TypeHint::Object { .. } => 7,
        TypeHint::Expr => 8,
        TypeHint::Collection => 9,
        TypeHint::Enum { .. } => 10,
        TypeHint::Newtype { .. } => 11,
        TypeHint::PlaceId { .. } => 12,
    }
}

#[test]
fn type_hint_generator_draws_every_variant() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = type_hint();
    let drawn: std::collections::BTreeSet<usize> = (0..500)
        .filter_map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .map(|h| variant_index(&h))
        .collect();
    assert_eq!(drawn, (0..13).collect());
}

fn arc() -> impl Strategy<Value = HlArc> {
    (
        prop_oneof![
            Just(ArcKind::Take),
            Just(ArcKind::Read),
            Just(ArcKind::Inhibit),
            Just(ArcKind::Put),
            Just(ArcKind::OnFailure),
            Just(ArcKind::OnTimeout),
        ],
        name(),
    )
        .prop_map(|(kind, place)| HlArc {
            kind,
            place: PlaceName(place),
        })
}

fn timer() -> impl Strategy<Value = TimerDecl> {
    (
        prop_oneof![
            (1u32..5).prop_map(|max_attempts| TimerKind::Retry { max_attempts }),
            Just(TimerKind::Run),
            Just(TimerKind::Abandon),
            Just(TimerKind::Delay),
        ],
        0u64..1_000_000,
    )
        .prop_map(|(kind, ms)| TimerDecl {
            kind,
            after: Duration::from_millis(ms),
        })
}

fn tool_arg() -> impl Strategy<Value = ToolArg> {
    prop_oneof![
        Just(ToolArg::Answer),
        name().prop_map(|t| ToolArg::Literal(s(&t))),
        any::<i64>().prop_map(|n| ToolArg::Literal(Value::Integer(n))),
        (name(), name()).prop_map(|(place, field)| ToolArg::TokenField {
            place: PlaceName(place),
            field,
        }),
    ]
}

fn descriptor() -> impl Strategy<Value = TransitionDescriptor> {
    let ask = prop::option::of(
        (type_hint(), prop::option::of(name()), any::<bool>()).prop_map(
            |(input, preview, persist)| AskDecl {
                input,
                preview: preview.map(NativeRef),
                persist,
            },
        ),
    );
    let body = prop_oneof![
        (name(), any::<bool>()).prop_map(|(symbol, effectful)| BodyDecl::Native {
            symbol: NativeRef(symbol),
            effectful,
        }),
        name().prop_map(|e| BodyDecl::Expr(ExprSlot::new(e))),
        (
            name(),
            prop::collection::btree_map(name(), tool_arg(), 0..3)
        )
            .prop_map(|(tool, args)| BodyDecl::Tool { tool, args }),
    ];
    (
        name(),
        prop::collection::vec(arc(), 0..5),
        body,
        any::<bool>(),
        prop::collection::vec(timer(), 0..3),
        prop::option::of(name().prop_map(ExprSlot::new)),
        ask,
        prop::option::of(name().prop_map(TransitionName)),
    )
        .prop_map(
            |(name, arcs, body, irreversible, timers, guard, ask, undoes)| TransitionDescriptor {
                name: TransitionName(name),
                arcs,
                body,
                irreversible,
                timers,
                guard,
                ask,
                undoes,
            },
        )
}

fn ask_row() -> impl Strategy<Value = AskRow> {
    (
        name(),
        name(),
        any::<[u8; 32]>(),
        type_hint(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(t, subject, hash, input, irreversible, has_preview)| AskRow {
                transition: TransitionName(t),
                subject: SubjectName(subject),
                hash: AskHash(hash),
                input,
                irreversible,
                has_preview,
            },
        )
}

fn answer_call() -> impl Strategy<Value = AnswerCall> {
    (
        name(),
        name(),
        any::<[u8; 32]>(),
        name(),
        prop::option::of(prop_oneof![
            name().prop_map(Value::String),
            any::<i64>().prop_map(Value::Integer),
            any::<bool>().prop_map(Value::Boolean),
        ]),
    )
        .prop_map(|(t, subject, seen, answer, value)| AnswerCall {
            transition: TransitionName(t),
            subject: SubjectName(subject),
            seen: AskHash(seen),
            answer: AnswerId(answer),
            value,
        })
}

fn round_trips<T>(value: &T) -> Result<(), TestCaseError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    prop_assert_eq!(&back, value, "json: {}", json);
    Ok(())
}

proptest! {
    #[test]
    fn transition_descriptor_round_trips(d in descriptor()) {
        round_trips(&d)?;
    }

    #[test]
    fn ask_row_round_trips(r in ask_row()) {
        round_trips(&r)?;
    }

    #[test]
    fn answer_call_round_trips(c in answer_call()) {
        round_trips(&c)?;
    }
}
