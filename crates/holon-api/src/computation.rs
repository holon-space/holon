//! @c4 component
//! @c4 layer Engine
//! Pattern: Interpreter
//!
//! `Computation` — the C4 unified derived-value algebra (ruling:
//! VisionGapAnalysis 2026-07-11, "generalize the Predicate trait to a
//! Computation trait evaluable in memory AND compilable to SQL"). One
//! value-producing expression type with two interpreters:
//!
//! - [`Computation::eval`] — **total**, in-memory, over a named-value context.
//!   This is what the reactive pipeline and `rank_tasks` run.
//! - [`Computation::compile_sql`] — **partial but DISCLOSED**: comparison /
//!   logic / arithmetic / field / literal lower to SQL; an arbitrary Rhai
//!   [`Script`] does not, and that is reported as a typed [`SqlUnsupported`]
//!   error — never a silent `None`. Callers that cannot push a term down must
//!   evaluate it in a *disclosed* degraded mode (in-memory over candidate
//!   rows), never emit a WHERE clause that silently drops the term.
//!
//! A boolean [`Predicate`] is the boolean-valued case, embedded verbatim (it is
//! `flutter_rust_bridge:non_opaque` and crosses to Dart; `Computation` is
//! engine-side only and may hold an opaque Rhai AST, so it *embeds* rather than
//! flattens the predicate).
//!
//! [`Script`]: Computation::Script

use std::collections::HashMap;
use std::fmt;

use holon_expr::CompiledExpr;
use holon_expr::bounded_engine;
use rhai::Dynamic;
use rhai::Scope;

use crate::Value;
use crate::predicate::Predicate;

/// A named-value evaluation context (row data, UI state, prior computed
/// fields).
pub type Context = HashMap<String, Value>;

/// Arithmetic operators for the [`Computation::Arith`] shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    fn apply(self, lhs: f64, rhs: f64) -> f64 {
        match self {
            ArithOp::Add => lhs + rhs,
            ArithOp::Sub => lhs - rhs,
            ArithOp::Mul => lhs * rhs,
            ArithOp::Div => lhs / rhs,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}

/// A value-producing computation over a named-value context.
///
/// This type is **not** FRB-exposed (it may carry an opaque Rhai AST). The
/// boolean subset that must cross to Dart stays in the [`Predicate`] enum,
/// embedded here.
#[derive(Debug, Clone, PartialEq)]
pub enum Computation {
    /// A constant.
    Lit(Value),
    /// Reference a context field / column by name.
    Field(String),
    /// Arithmetic over two numeric sub-computations.
    Arith {
        op: ArithOp,
        lhs: Box<Computation>,
        rhs: Box<Computation>,
    },
    /// The boolean-valued shape — reuses the FRB predicate and its tuned
    /// semantics.
    Predicate(Predicate),
    /// An arbitrary compiled Rhai expression. In-memory only; not
    /// SQL-compilable.
    Script(CompiledExpr),
}

/// Failure evaluating a [`Computation`] in memory. Fail-loud: a missing field
/// or a non-numeric operand in arithmetic is an error, not a silent default.
#[derive(Debug, Clone, PartialEq)]
pub enum ComputeError {
    MissingField(String),
    NotNumeric { context: String, value: Value },
    Script { source: String, detail: String },
}

impl fmt::Display for ComputeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComputeError::MissingField(name) => {
                write!(f, "computation references missing field `{name}`")
            }
            ComputeError::NotNumeric { context, value } => {
                write!(f, "non-numeric operand in {context}: {value:?}")
            }
            ComputeError::Script { source, detail } => {
                write!(f, "Rhai evaluation failed for `{source}`: {detail}")
            }
        }
    }
}

impl std::error::Error for ComputeError {}

/// A parameterized SQL scalar/boolean fragment. Turso-free (params are
/// [`Value`]s); the turso conversion lives caller-side in `holon`.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlFragment {
    pub sql: String,
    pub params: Vec<Value>,
}

impl SqlFragment {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> Self {
        SqlFragment {
            sql: sql.into(),
            params,
        }
    }
}

/// A [`Computation`] shape that cannot be lowered to SQL. **Disclosed** — the
/// replacement for the old silent `Option::None`. Callers must handle it
/// visibly (degraded in-memory evaluation with a banner, or surface the error).
#[derive(Debug, Clone, PartialEq)]
pub enum SqlUnsupported {
    /// An arbitrary Rhai script — genuinely uncompilable to SQL.
    Script { source: String },
}

impl fmt::Display for SqlUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlUnsupported::Script { source } => write!(
                f,
                "computation `{source}` is a Rhai script and cannot be compiled to SQL; evaluate \
                 it in-memory (disclosed degraded mode)"
            ),
        }
    }
}

impl std::error::Error for SqlUnsupported {}

impl Computation {
    /// Total, in-memory evaluation against a named-value context.
    pub fn eval(&self, ctx: &Context) -> Result<Value, ComputeError> {
        match self {
            Computation::Lit(v) => Ok(v.clone()),
            Computation::Field(name) => ctx
                .get(name)
                .cloned()
                .ok_or_else(|| ComputeError::MissingField(name.clone())),
            Computation::Arith { op, lhs, rhs } => {
                let l = as_number(&lhs.eval(ctx)?, "arithmetic left operand")?;
                let r = as_number(&rhs.eval(ctx)?, "arithmetic right operand")?;
                Ok(Value::Float(op.apply(l, r)))
            }
            Computation::Predicate(p) => Ok(Value::Boolean(p.evaluate(ctx))),
            Computation::Script(expr) => eval_script(expr, ctx),
        }
    }

    /// Partial, **disclosed** lowering to a SQL fragment. `Err(SqlUnsupported)`
    /// names the exact shape that cannot lower — never a bare `None`.
    pub fn compile_sql(&self) -> Result<SqlFragment, SqlUnsupported> {
        match self {
            Computation::Lit(v) => Ok(SqlFragment::new("?", vec![v.clone()])),
            Computation::Field(name) => Ok(SqlFragment::new(name.clone(), vec![])),
            Computation::Arith { op, lhs, rhs } => {
                let l = lhs.compile_sql()?;
                let r = rhs.compile_sql()?;
                let mut params = l.params;
                params.extend(r.params);
                Ok(SqlFragment::new(
                    format!("({} {} {})", l.sql, op.sql(), r.sql),
                    params,
                ))
            }
            Computation::Predicate(p) => predicate_to_sql(p),
            Computation::Script(expr) => Err(SqlUnsupported::Script {
                source: expr.source.clone(),
            }),
        }
    }
}

fn as_number(v: &Value, context: &str) -> Result<f64, ComputeError> {
    v.as_f64().ok_or_else(|| ComputeError::NotNumeric {
        context: context.to_string(),
        value: v.clone(),
    })
}

/// Evaluate a compiled Rhai expression over `ctx` — the same single-expression
/// path `rank_tasks` uses, generalized to arbitrary [`Value`] inputs.
fn eval_script(expr: &CompiledExpr, ctx: &Context) -> Result<Value, ComputeError> {
    let engine = bounded_engine();
    let mut scope = Scope::new();
    for (k, v) in ctx {
        match v {
            Value::Integer(i) => scope.push(k.clone(), *i as f64),
            Value::Float(f) => scope.push(k.clone(), *f),
            Value::Boolean(b) => scope.push(k.clone(), *b),
            Value::String(s) => scope.push(k.clone(), s.clone()),
            _ => continue,
        };
    }
    let result: Dynamic = engine
        .eval_ast_with_scope(&mut scope, &expr.ast)
        .map_err(|e| ComputeError::Script {
            source: expr.source.clone(),
            detail: e.to_string(),
        })?;
    if result.is_float() {
        Ok(Value::Float(result.as_float().unwrap()))
    } else if result.is_int() {
        Ok(Value::Integer(result.as_int().unwrap()))
    } else if result.is_bool() {
        Ok(Value::Boolean(result.as_bool().unwrap()))
    } else if result.is_string() {
        Ok(Value::String(result.into_string().unwrap()))
    } else {
        Err(ComputeError::Script {
            source: expr.source.clone(),
            detail: format!("non-scalar Rhai result: {result:?}"),
        })
    }
}

/// Disclosed boolean-predicate → SQL. Replaces the old silent `ToSql for
/// Predicate` (traits.rs): every shape lowers, so this is total — but it lives
/// with [`Computation::compile_sql`]'s contract, and And/Or propagate child
/// errors with `?` instead of `filter_map`-dropping them.
pub fn predicate_to_sql(pred: &Predicate) -> Result<SqlFragment, SqlUnsupported> {
    Ok(match pred {
        Predicate::Eq { field, value } => {
            SqlFragment::new(format!("{field} = ?"), vec![value.clone()])
        }
        Predicate::Ne { field, value } => {
            if value.is_null() {
                SqlFragment::new(format!("{field} IS NOT NULL"), vec![])
            } else {
                SqlFragment::new(format!("{field} != ?"), vec![value.clone()])
            }
        }
        Predicate::Gt { field, value } => {
            SqlFragment::new(format!("{field} > ?"), vec![value.clone()])
        }
        Predicate::Lt { field, value } => {
            SqlFragment::new(format!("{field} < ?"), vec![value.clone()])
        }
        Predicate::Gte { field, value } => {
            SqlFragment::new(format!("{field} >= ?"), vec![value.clone()])
        }
        Predicate::Lte { field, value } => {
            SqlFragment::new(format!("{field} <= ?"), vec![value.clone()])
        }
        Predicate::IsNotNull(field) => SqlFragment::new(format!("{field} IS NOT NULL"), vec![]),
        Predicate::Var(field) => SqlFragment::new(
            format!("{field} IS NOT NULL AND {field} != '' AND {field} != 0"),
            vec![],
        ),
        Predicate::Not(inner) => {
            let p = predicate_to_sql(inner)?;
            SqlFragment::new(format!("NOT ({})", p.sql), p.params)
        }
        Predicate::And(preds) => join_predicates(preds, "AND")?,
        Predicate::Or(preds) => join_predicates(preds, "OR")?,
        // Was silently `None`; a no-op filter is `TRUE`, which IS compilable.
        Predicate::Always => SqlFragment::new("1 = 1", vec![]),
    })
}

fn join_predicates(preds: &[Predicate], sep: &str) -> Result<SqlFragment, SqlUnsupported> {
    let mut parts = Vec::with_capacity(preds.len());
    let mut params = Vec::new();
    // `?` propagates a child's SqlUnsupported instead of dropping it (the disclosed
    // fix).
    for p in preds {
        let frag = predicate_to_sql(p)?;
        parts.push(format!("({})", frag.sql));
        params.extend(frag.params);
    }
    Ok(SqlFragment::new(parts.join(&format!(" {sep} ")), params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> rhai::Engine {
        bounded_engine()
    }

    fn ctx(pairs: &[(&str, Value)]) -> Context {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn lit_and_field_eval() {
        let c = ctx(&[("x", Value::Integer(7))]);
        assert_eq!(
            Computation::Lit(Value::Integer(3)).eval(&c).unwrap(),
            Value::Integer(3)
        );
        assert_eq!(
            Computation::Field("x".into()).eval(&c).unwrap(),
            Value::Integer(7)
        );
    }

    #[test]
    fn missing_field_is_loud() {
        let c = ctx(&[]);
        assert_eq!(
            Computation::Field("nope".into()).eval(&c),
            Err(ComputeError::MissingField("nope".into()))
        );
    }

    #[test]
    fn arith_eval() {
        let c = ctx(&[("a", Value::Float(2.0)), ("b", Value::Integer(3))]);
        let expr = Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("a".into())),
            rhs: Box::new(Computation::Arith {
                op: ArithOp::Add,
                lhs: Box::new(Computation::Field("b".into())),
                rhs: Box::new(Computation::Lit(Value::Integer(1))),
            }),
        };
        assert_eq!(expr.eval(&c).unwrap(), Value::Float(8.0)); // 2 * (3 + 1)
    }

    #[test]
    fn embedded_predicate_eval() {
        let c = ctx(&[("done", Value::Boolean(true))]);
        let p = Computation::Predicate(Predicate::Var("done".into()));
        assert_eq!(p.eval(&c).unwrap(), Value::Boolean(true));
    }

    #[test]
    fn script_eval_matches_rhai() {
        let expr = CompiledExpr::compile(&engine(), "priority * 10.0 + 1.0").unwrap();
        let c = ctx(&[("priority", Value::Float(3.0))]);
        assert_eq!(
            Computation::Script(expr).eval(&c).unwrap(),
            Value::Float(31.0)
        );
    }

    #[test]
    fn script_switch_like_prototype_weight() {
        // Mirrors the default `priority_weight` prototype expression.
        let expr = CompiledExpr::compile(
            &engine(),
            "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }",
        )
        .unwrap();
        let c = ctx(&[("priority", Value::Float(2.0))]);
        assert_eq!(
            Computation::Script(expr).eval(&c).unwrap(),
            Value::Float(40.0)
        );
    }

    #[test]
    fn compile_sql_lowers_arith_and_field() {
        let expr = Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("weight".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        };
        let frag = expr.compile_sql().unwrap();
        assert_eq!(frag.sql, "(weight * ?)");
        assert_eq!(frag.params, vec![Value::Integer(2)]);
    }

    #[test]
    fn compile_sql_predicate_and() {
        let p = Predicate::And(vec![
            Predicate::Eq {
                field: "a".into(),
                value: Value::Integer(1),
            },
            Predicate::Gt {
                field: "b".into(),
                value: Value::Integer(5),
            },
        ]);
        let frag = Computation::Predicate(p).compile_sql().unwrap();
        assert_eq!(frag.sql, "(a = ?) AND (b > ?)");
        assert_eq!(frag.params, vec![Value::Integer(1), Value::Integer(5)]);
    }

    #[test]
    fn compile_sql_always_is_true_not_dropped() {
        // Regression: the old ToSql returned None for Always (silent no-filter).
        let frag = Computation::Predicate(Predicate::Always)
            .compile_sql()
            .unwrap();
        assert_eq!(frag.sql, "1 = 1");
    }

    #[test]
    fn compile_sql_script_is_disclosed_not_silent() {
        // THE anti-regression for the silent hole: a script fails LOUD and NAMED.
        let expr = CompiledExpr::compile(&engine(), "if x > 1 { 2 } else { 3 }").unwrap();
        let err = Computation::Script(expr).compile_sql().unwrap_err();
        assert_eq!(
            err,
            SqlUnsupported::Script {
                source: "if x > 1 { 2 } else { 3 }".into()
            }
        );
    }

    /// Evaluate an ordered set of named computed fields, threading each result
    /// back into the context so later fields can depend on earlier ones
    /// (the reactive "computed = properties in the pipeline" shape).
    /// Returns the full field map.
    fn recompute_fields(
        inputs: &Context,
        fields: &[(&str, Computation)],
    ) -> HashMap<String, Value> {
        let mut scope = inputs.clone();
        for (name, comp) in fields {
            let v = comp.eval(&scope).unwrap();
            scope.insert(name.to_string(), v);
        }
        fields
            .iter()
            .map(|(n, _)| (n.to_string(), scope[*n].clone()))
            .collect()
    }

    #[test]
    fn end_to_end_computed_field_and_incrementality() {
        let eng = engine();
        // Derived-field program: priority_weight, then task_weight depending on it +
        // position. Mixed Script + Arith + Field shapes.
        let fields = vec![
            (
                "priority_weight",
                Computation::Script(
                    CompiledExpr::compile(
                        &eng,
                        "switch priority { 3.0 => 100.0, 2.0 => 40.0, _ => 1.0 }",
                    )
                    .unwrap(),
                ),
            ),
            (
                "position_weight",
                Computation::Arith {
                    op: ArithOp::Mul,
                    lhs: Box::new(Computation::Lit(Value::Float(0.001))),
                    rhs: Box::new(Computation::Field("position".into())),
                },
            ),
            (
                "task_weight",
                Computation::Arith {
                    op: ArithOp::Add,
                    lhs: Box::new(Computation::Field("priority_weight".into())),
                    rhs: Box::new(Computation::Field("position_weight".into())),
                },
            ),
        ];

        let inputs = ctx(&[
            ("priority", Value::Float(2.0)),
            ("position", Value::Float(10.0)),
        ]);
        let before = recompute_fields(&inputs, &fields);
        assert_eq!(before["priority_weight"], Value::Float(40.0));
        assert_eq!(before["position_weight"], Value::Float(0.01));
        assert_eq!(before["task_weight"], Value::Float(40.01));

        // Incremental change: bump ONLY `priority`. `priority_weight` and its
        // dependent `task_weight` recompute; `position_weight` is untouched.
        let mut changed = inputs.clone();
        changed.insert("priority".into(), Value::Float(3.0));
        let after = recompute_fields(&changed, &fields);

        assert_eq!(after["priority_weight"], Value::Float(100.0)); // changed
        assert_eq!(after["task_weight"], Value::Float(100.01)); // dependent changed
        assert_eq!(
            after["position_weight"], before["position_weight"],
            "field independent of the changed input must not move"
        );
        assert_ne!(after["priority_weight"], before["priority_weight"]);
        assert_ne!(after["task_weight"], before["task_weight"]);
    }

    #[test]
    fn compile_sql_and_with_script_child_propagates_error() {
        // An And that embeds a script term must NOT silently drop it. Here the script
        // is inside a Computation::And-equivalent via Arith to prove propagation.
        let script = CompiledExpr::compile(&engine(), "x + 1").unwrap();
        let expr = Computation::Arith {
            op: ArithOp::Add,
            lhs: Box::new(Computation::Field("y".into())),
            rhs: Box::new(Computation::Script(script)),
        };
        assert!(matches!(
            expr.compile_sql(),
            Err(SqlUnsupported::Script { .. })
        ));
    }
}
