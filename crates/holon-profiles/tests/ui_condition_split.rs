//! A profile condition is split into a data part (Rhai, evaluated against the
//! row) and a UI part (a `Predicate` with a literal value, evaluated by the
//! frontend). Every conjunct either lands on the side its identifiers say, with
//! the literal the author wrote, or the profile is refused with an error that
//! quotes the conjunct.

use holon_api::Predicate;
use holon_api::Value;
use holon_profiles::parse_entity_profile;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;

const UI_VARIABLES: &[&str] = &[
    "is_focused",
    "is_expanded",
    "view_mode",
    "role",
    "available_width_px",
    "available_height_px",
    "viewport_width_px",
    "viewport_height_px",
    "scale_factor",
];
const DATA_VARIABLES: &[&str] = &["content", "priority", "x100"];
const OPERATORS: &[&str] = &["==", "!=", "<", "<=", ">", ">="];

/// Literal texts a UI comparison accepts, with the value they denote.
fn accepted_literals() -> Vec<(&'static str, Value)> {
    vec![
        ("600", Value::Integer(600)),
        ("0", Value::Integer(0)),
        ("2.5", Value::Float(2.5)),
        ("1e3", Value::Float(1000.0)),
        ("1e-3", Value::Float(0.001)),
        ("true", Value::Boolean(true)),
        ("false", Value::Boolean(false)),
        ("\"table\"", Value::String("table".into())),
        ("\"a<b && c\"", Value::String("a<b && c".into())),
        ("\"say \\\"hi\\\"\"", Value::String("say \"hi\"".into())),
        ("\"a\\\\b\"", Value::String("a\\b".into())),
        ("\"a\\nb\"", Value::String("a\nb".into())),
        ("\"a\\tb\"", Value::String("a\tb".into())),
        ("\"\\x41\"", Value::String("A".into())),
        ("\"\\u00e9\"", Value::String("\u{e9}".into())),
        ("'x'", Value::String("x".into())),
        ("'\\''", Value::String("'".into())),
    ]
}

/// Right sides a UI comparison cannot evaluate as written.
const UNSUPPORTED_RIGHT_SIDES: &[&str] = &[
    "viewport_width_px",
    "100 + 20",
    "0x100",
    "0o7",
    "0b1",
    "1_000",
    "`x`",
    "#\"foo\"#",
];

/// Literal texts whose letters, digits or quotes must not count as identifiers.
const DATA_RIGHT_SIDES: &[&str] = &[
    "0x100",
    "1e3",
    "1_000",
    "`view_mode`",
    "`${content} is_focused`",
    "#\"is_expanded\"#",
    "\"scale_factor\"",
    "7",
];

#[derive(Clone, Debug)]
enum Conj {
    UiCompare {
        var: &'static str,
        op: &'static str,
        text: &'static str,
        value: Value,
    },
    UiFlag {
        var: &'static str,
        negated: bool,
    },
    Data(String),
    Unsupported(String),
}

impl Conj {
    fn text(&self) -> String {
        match self {
            Conj::UiCompare { var, op, text, .. } => format!("{var} {op} {text}"),
            Conj::UiFlag { var, negated } => format!("{}{var}", if *negated { "!" } else { "" }),
            Conj::Data(t) | Conj::Unsupported(t) => t.clone(),
        }
    }

    fn predicate(&self) -> Option<Predicate> {
        match self {
            Conj::UiCompare { var, op, value, .. } => {
                let (field, value) = (var.to_string(), value.clone());
                Some(match *op {
                    "==" => Predicate::Eq { field, value },
                    "!=" => Predicate::Ne { field, value },
                    "<" => Predicate::Lt { field, value },
                    "<=" => Predicate::Lte { field, value },
                    ">" => Predicate::Gt { field, value },
                    ">=" => Predicate::Gte { field, value },
                    other => unreachable!("operator {other}"),
                })
            }
            Conj::UiFlag { var, negated } => {
                let flag = Predicate::Var(var.to_string());
                Some(if *negated {
                    Predicate::Not(Box::new(flag))
                } else {
                    flag
                })
            }
            Conj::Data(_) | Conj::Unsupported(_) => None,
        }
    }
}

fn conj() -> impl Strategy<Value = Conj> {
    let ui = prop::sample::select(UI_VARIABLES);
    let op = prop::sample::select(OPERATORS);
    prop_oneof![
        (
            ui.clone(),
            op.clone(),
            prop::sample::select(accepted_literals())
        )
            .prop_map(|(var, op, (text, value))| Conj::UiCompare {
                var,
                op,
                text,
                value
            }),
        (ui.clone(), any::<bool>()).prop_map(|(var, negated)| Conj::UiFlag { var, negated }),
        (
            prop::sample::select(DATA_VARIABLES),
            op.clone(),
            prop::sample::select(DATA_RIGHT_SIDES)
        )
            .prop_map(|(var, op, rhs)| Conj::Data(format!("{var} {op} {rhs}"))),
        (ui.clone(), op.clone()).prop_map(|(var, op)| Conj::Data(format!("{var} {op} content"))),
        (ui, op, prop::sample::select(UNSUPPORTED_RIGHT_SIDES))
            .prop_map(|(var, op, rhs)| Conj::Unsupported(format!("{var} {op} {rhs}"))),
    ]
}

fn check(conjs: &[Conj]) -> Result<(), TestCaseError> {
    let condition = conjs
        .iter()
        .map(Conj::text)
        .collect::<Vec<_>>()
        .join(" && ");
    let yaml = format!(
        "entity_name: block\n\
         variants:\n  \
         - name: matched\n    \
         condition: '{}'\n    \
         render: 'text(col(\"id\"))'\n",
        condition.replace('\'', "''")
    );
    let parsed = parse_entity_profile(&yaml);

    if let Some(bad) = conjs.iter().find(|c| matches!(c, Conj::Unsupported(_))) {
        let error = format!(
            "{:#}",
            parsed
                .err()
                .ok_or_else(|| { TestCaseError::fail(format!("`{condition}` was accepted")) })?
        );
        prop_assert!(
            error.contains(&bad.text()),
            "the error does not quote `{}`: {error}",
            bad.text()
        );
        return Ok(());
    }

    let profile = parsed.map_err(|e| TestCaseError::fail(format!("`{condition}`: {e:#}")))?;
    let variant = &profile.variants[0];
    let ui: Vec<Predicate> = conjs.iter().filter_map(Conj::predicate).collect();
    let expected_ui = match ui.len() {
        0 => Predicate::Always,
        1 => ui.into_iter().next().unwrap(),
        _ => Predicate::And(ui),
    };
    prop_assert_eq!(
        &variant.ui_condition,
        &expected_ui,
        "ui side of `{}`",
        condition
    );
    let data: Vec<String> = conjs
        .iter()
        .filter(|c| matches!(c, Conj::Data(_)))
        .map(Conj::text)
        .collect();
    let expected_data = (!data.is_empty()).then(|| data.join(" && "));
    prop_assert_eq!(
        &variant.data_condition,
        &expected_data,
        "data side of `{}`",
        condition
    );
    Ok(())
}

fn ui_compare(var: &'static str, op: &'static str, literal: usize) -> Conj {
    let (text, value) = accepted_literals()[literal].clone();
    Conj::UiCompare {
        var,
        op,
        text,
        value,
    }
}

fn static_examples() -> Vec<Vec<Conj>> {
    let unsupported = |t: &str| vec![Conj::Unsupported(t.to_string())];
    vec![
        unsupported("available_width_px > viewport_width_px"),
        unsupported("available_width_px > 100 + 20"),
        unsupported("viewport_width_px > 0x100"),
        unsupported("view_mode == `x`"),
        unsupported("view_mode == #\"foo\"#"),
        vec![ui_compare("viewport_width_px", ">", 3)],
        vec![Conj::Data("content == `view_mode`".into())],
        vec![
            Conj::Data("x100 > 0x100".into()),
            ui_compare("view_mode", "==", 7),
        ],
    ]
}

#[test]
fn a_condition_is_split_by_its_identifiers_or_refused_quoting_it() {
    for example in static_examples() {
        check(&example).unwrap_or_else(|e| panic!("static example {example:?}: {e}"));
    }
    TestRunner::new(Config::with_cases(512))
        .run(&prop::collection::vec(conj(), 1..5), |conjs| check(&conjs))
        .unwrap_or_else(|e| panic!("{e}"));
}
