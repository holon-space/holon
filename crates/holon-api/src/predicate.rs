use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Value;

/// Unified predicate for filtering — used in entity profiles, views, and tests.
///
/// Replaces: `Predicate<T>` trait (traits.rs), `FilterExpr` (render_types.rs),
/// `TestPredicate` (PBT query.rs).
///
/// Evaluatable against a named-value context (row data, UI state, etc.).
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    /// Field/variable is truthy (non-null, non-false, non-empty string)
    Var(String),
    /// Field equals value
    Eq { field: String, value: Value },
    /// Field not equals value
    Ne { field: String, value: Value },
    /// Field > value (numeric, f64-coerced). Fail-shut on type mismatch or missing field.
    Gt { field: String, value: Value },
    /// Field < value (numeric, f64-coerced). Fail-shut on type mismatch or missing field.
    Lt { field: String, value: Value },
    /// Field >= value (numeric, f64-coerced). Fail-shut on type mismatch or missing field.
    Gte { field: String, value: Value },
    /// Field <= value (numeric, f64-coerced). Fail-shut on type mismatch or missing field.
    Lte { field: String, value: Value },
    /// Field is not null
    IsNotNull(String),
    /// Logical NOT
    Not(Box<Predicate>),
    /// All must match
    And(Vec<Predicate>),
    /// Any must match
    Or(Vec<Predicate>),
    /// Always true (no condition)
    Always,
}

impl Predicate {
    /// Evaluate against a named-value context (row data, UI state, etc.).
    pub fn evaluate(&self, context: &HashMap<String, Value>) -> bool {
        match self {
            Predicate::Var(name) => match context.get(name) {
                None | Some(Value::Null) => false,
                Some(Value::Boolean(b)) => *b,
                Some(Value::String(s)) => !s.is_empty(),
                Some(Value::Integer(i)) => *i != 0,
                Some(Value::Float(f)) => *f != 0.0,
                Some(_) => true,
            },
            Predicate::Eq { field, value } => context.get(field).map_or(false, |v| v == value),
            Predicate::Ne { field, value } => match context.get(field) {
                None => !value.is_null(),
                Some(v) => v != value,
            },
            Predicate::Gt { field, value } => compare_f64(context, field, value, |l, r| l > r),
            Predicate::Lt { field, value } => compare_f64(context, field, value, |l, r| l < r),
            Predicate::Gte { field, value } => compare_f64(context, field, value, |l, r| l >= r),
            Predicate::Lte { field, value } => compare_f64(context, field, value, |l, r| l <= r),
            Predicate::IsNotNull(field) => {
                matches!(context.get(field), Some(v) if !v.is_null())
            }
            Predicate::Not(inner) => !inner.evaluate(context),
            Predicate::And(preds) => preds.iter().all(|p| p.evaluate(context)),
            Predicate::Or(preds) => preds.iter().any(|p| p.evaluate(context)),
            Predicate::Always => true,
        }
    }

    /// Convenience: wrap in Not
    pub fn negate(self) -> Self {
        Predicate::Not(Box::new(self))
    }
}

fn compare_f64(
    context: &HashMap<String, Value>,
    field: &str,
    rhs: &Value,
    cmp: impl Fn(f64, f64) -> bool,
) -> bool {
    let Some(lhs_value) = context.get(field) else {
        return false;
    };
    let (Some(lhs), Some(rhs)) = (lhs_value.as_f64(), rhs.as_f64()) else {
        return false;
    };
    cmp(lhs, rhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn always_is_true() {
        assert!(Predicate::Always.evaluate(&HashMap::new()));
    }

    #[test]
    fn var_truthy() {
        assert!(Predicate::Var("x".into()).evaluate(&ctx(&[("x", Value::Boolean(true))])));
        assert!(!Predicate::Var("x".into()).evaluate(&ctx(&[("x", Value::Boolean(false))])));
        assert!(!Predicate::Var("x".into()).evaluate(&ctx(&[("x", Value::Null)])));
        assert!(!Predicate::Var("missing".into()).evaluate(&HashMap::new()));
        assert!(Predicate::Var("x".into()).evaluate(&ctx(&[("x", Value::String("yes".into()))])));
        assert!(!Predicate::Var("x".into()).evaluate(&ctx(&[("x", Value::String("".into()))])));
    }

    #[test]
    fn eq_ne() {
        let c = ctx(&[("status", Value::String("active".into()))]);
        assert!(Predicate::Eq {
            field: "status".into(),
            value: Value::String("active".into())
        }
        .evaluate(&c));
        assert!(!Predicate::Eq {
            field: "status".into(),
            value: Value::String("inactive".into())
        }
        .evaluate(&c));
        assert!(Predicate::Ne {
            field: "status".into(),
            value: Value::String("inactive".into())
        }
        .evaluate(&c));
        assert!(!Predicate::Ne {
            field: "status".into(),
            value: Value::String("active".into())
        }
        .evaluate(&c));
    }

    #[test]
    fn is_not_null() {
        let c = ctx(&[("x", Value::Integer(1))]);
        assert!(Predicate::IsNotNull("x".into()).evaluate(&c));
        assert!(!Predicate::IsNotNull("missing".into()).evaluate(&c));
        let c2 = ctx(&[("x", Value::Null)]);
        assert!(!Predicate::IsNotNull("x".into()).evaluate(&c2));
    }

    #[test]
    fn not_predicate() {
        assert!(!Predicate::Not(Box::new(Predicate::Always)).evaluate(&HashMap::new()));
    }

    #[test]
    fn and_or() {
        let c = ctx(&[("a", Value::Boolean(true)), ("b", Value::Boolean(false))]);
        let and = Predicate::And(vec![Predicate::Var("a".into()), Predicate::Var("b".into())]);
        assert!(!and.evaluate(&c));
        let or = Predicate::Or(vec![Predicate::Var("a".into()), Predicate::Var("b".into())]);
        assert!(or.evaluate(&c));
    }

    #[test]
    fn gt_lt_gte_lte_numeric_coercion() {
        let c = ctx(&[
            ("width_px", Value::Float(456.0)),
            ("count", Value::Integer(3)),
        ]);

        // Lt: float field < int rhs, coerced to f64.
        assert!(Predicate::Lt {
            field: "width_px".into(),
            value: Value::Integer(500)
        }
        .evaluate(&c));
        assert!(!Predicate::Lt {
            field: "width_px".into(),
            value: Value::Integer(400)
        }
        .evaluate(&c));

        // Gt: int field > float rhs, coerced to f64.
        assert!(Predicate::Gt {
            field: "count".into(),
            value: Value::Float(2.5)
        }
        .evaluate(&c));
        assert!(!Predicate::Gt {
            field: "count".into(),
            value: Value::Float(3.0)
        }
        .evaluate(&c));

        // Gte / Lte with equal values.
        assert!(Predicate::Gte {
            field: "count".into(),
            value: Value::Integer(3)
        }
        .evaluate(&c));
        assert!(Predicate::Lte {
            field: "count".into(),
            value: Value::Integer(3)
        }
        .evaluate(&c));
    }

    #[test]
    fn gt_lt_fail_shut_on_missing_or_type_mismatch() {
        // Missing field — fail-shut (false).
        assert!(!Predicate::Lt {
            field: "absent".into(),
            value: Value::Integer(9)
        }
        .evaluate(&HashMap::new()));

        // Type mismatch (string cannot coerce to f64) — fail-shut.
        let c = ctx(&[("x", Value::String("hello".into()))]);
        assert!(!Predicate::Gt {
            field: "x".into(),
            value: Value::Integer(1)
        }
        .evaluate(&c));
    }

    #[test]
    fn and_combines_data_eq_and_ui_lt() {
        // Mixed data-side Eq + UI-side Lt — the pattern split_condition produces.
        let c = ctx(&[
            ("task_state", Value::String("done".into())),
            ("available_width_px", Value::Float(480.0)),
        ]);
        let pred = Predicate::And(vec![
            Predicate::Eq {
                field: "task_state".into(),
                value: Value::String("done".into()),
            },
            Predicate::Lt {
                field: "available_width_px".into(),
                value: Value::Integer(600),
            },
        ]);
        assert!(pred.evaluate(&c));

        // Same pattern, width fails.
        let c_wide = ctx(&[
            ("task_state", Value::String("done".into())),
            ("available_width_px", Value::Float(1024.0)),
        ]);
        assert!(!pred.evaluate(&c_wide));
    }

    #[test]
    fn ne_null_semantics() {
        // Missing field != non-null value → true (SQL-like)
        assert!(Predicate::Ne {
            field: "x".into(),
            value: Value::Integer(1)
        }
        .evaluate(&HashMap::new()));
        // Missing field != NULL → false (NULL != NULL is false in SQL)
        assert!(!Predicate::Ne {
            field: "x".into(),
            value: Value::Null
        }
        .evaluate(&HashMap::new()));
    }

    #[test]
    fn serde_roundtrip() {
        let pred = Predicate::And(vec![
            Predicate::Var("is_focused".into()),
            Predicate::Eq {
                field: "view_mode".into(),
                value: Value::String("table".into()),
            },
        ]);
        let json = serde_json::to_string(&pred).unwrap();
        let parsed: Predicate = serde_json::from_str(&json).unwrap();
        assert_eq!(pred, parsed);
    }
}
