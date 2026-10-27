use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

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
    /// Field > value (numeric, f64-coerced). Fail-shut on type mismatch or
    /// missing field.
    Gt { field: String, value: Value },
    /// Field < value (numeric, f64-coerced). Fail-shut on type mismatch or
    /// missing field.
    Lt { field: String, value: Value },
    /// Field >= value (numeric, f64-coerced). Fail-shut on type mismatch or
    /// missing field.
    Gte { field: String, value: Value },
    /// Field <= value (numeric, f64-coerced). Fail-shut on type mismatch or
    /// missing field.
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
            Predicate::Eq { field, value } => context.get(field) == Some(value),
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
mod tests;
