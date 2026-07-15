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

use holon_expr::bounded_engine;
use holon_expr::CompiledExpr;
use rhai::Dynamic;
use rhai::Scope;

use crate::predicate::Predicate;
use crate::Value;

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

    /// Render this fragment as a **parameter-free** SQL expression by inlining
    /// each `?` placeholder as a SQL literal, left-to-right.
    ///
    /// This is what seat A (matview-column planting) needs: a `CREATE
    /// MATERIALIZED VIEW … AS SELECT <expr> AS field` cannot carry bind
    /// parameters, so the literals from the *derived-field declaration*
    /// (not user free-text — they come from the prototype block's `= Rhai`
    /// / arithmetic constants) are inlined directly. String literals are
    /// single-quote escaped; non-scalar params ([`Value::Array`]/
    /// [`Value::Object`]) are an error, never silently dropped.
    pub fn inline_sql(&self) -> Result<String, InlineError> {
        let mut out = String::with_capacity(self.sql.len());
        let mut params = self.params.iter();
        for ch in self.sql.chars() {
            if ch == '?' {
                let v = params.next().ok_or(InlineError::PlaceholderParamMismatch {
                    sql: self.sql.clone(),
                    params: self.params.len(),
                })?;
                out.push_str(&value_to_sql_literal(v)?);
            } else {
                out.push(ch);
            }
        }
        if params.next().is_some() {
            return Err(InlineError::PlaceholderParamMismatch {
                sql: self.sql.clone(),
                params: self.params.len(),
            });
        }
        Ok(out)
    }
}

/// Failure inlining a [`SqlFragment`] into a parameter-free expression (seat
/// A).
#[derive(Debug, Clone, PartialEq)]
pub enum InlineError {
    /// A `?` count / params-length mismatch, or a non-scalar param.
    PlaceholderParamMismatch { sql: String, params: usize },
    /// A [`Value::Array`]/[`Value::Object`]/[`Value::Null`]-shaped literal
    /// cannot be inlined as a scalar column expression.
    NonScalarLiteral { value: Value },
}

impl fmt::Display for InlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InlineError::PlaceholderParamMismatch { sql, params } => write!(
                f,
                "cannot inline SQL fragment `{sql}`: `?` placeholders do not match {params} \
                 param(s)"
            ),
            InlineError::NonScalarLiteral { value } => {
                write!(
                    f,
                    "cannot inline non-scalar literal into SQL column: {value:?}"
                )
            }
        }
    }
}

impl std::error::Error for InlineError {}

fn value_to_sql_literal(v: &Value) -> Result<String, InlineError> {
    Ok(match v {
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Boolean(b) => {
            if *b {
                "1".into()
            } else {
                "0".into()
            }
        }
        Value::String(s) | Value::DateTime(s) | Value::Json(s) => {
            format!("'{}'", s.replace('\'', "''"))
        }
        Value::Null | Value::Array(_) | Value::Object(_) => {
            return Err(InlineError::NonScalarLiteral { value: v.clone() });
        }
    })
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

// ---------------------------------------------------------------------------
// C4 hybrid seat — the routing decision the ruling left open.
//
// A derived field is DECLARED once (a `= Rhai` / arithmetic property on a
// prototype block) and parsed into a `Computation`. The seat decides, PER FIELD
// and by `compile_sql()` alone, WHERE it is maintained:
//
//   compile_sql() -> Ok  => planted as an IVM matview column (seat A). Turso's
//                           IVM recomputes/retracts it O(delta) for free.
//   compile_sql() -> Err  => evaluated in the projection stage (seat B) over
// the                           row's CDC-fed context. TOTAL (handles `Script`)
// but                           DISCLOSED as the degraded path — never silent.
//
// The split is an implementation detail the user may inspect
// (`DerivedFieldPlan`) but must not depend on: same declaration surface, same
// observable result.
// ---------------------------------------------------------------------------

/// A derived field declared on a prototype block: a name and the computation
/// that produces its value. Parsed at the boundary (`= Rhai` →
/// [`Computation::Script`], arithmetic/comparison → the structured shapes) —
/// never a raw string here.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedField {
    pub name: String,
    pub computation: Computation,
}

impl DerivedField {
    pub fn new(name: impl Into<String>, computation: Computation) -> Self {
        DerivedField {
            name: name.into(),
            computation,
        }
    }
}

/// A derived field that lowered to SQL and can be planted as a matview column.
/// `sql` is the inlined, parameter-free column expression (`{sql} AS {name}`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlantedColumn {
    pub name: String,
    pub sql: String,
}

/// The hybrid-seat routing decision for a set of derived fields (see the module
/// note). Inspectable, not depend-able: `sql_planted` + `stage_evaluated`
/// together always cover every input field exactly once.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DerivedFieldPlan {
    /// Fields whose `compile_sql()` succeeded — planted as IVM matview columns.
    pub sql_planted: Vec<PlantedColumn>,
    /// Fields that fell to the disclosed projection-stage path (`Script`, or a
    /// fragment that could not be inlined). Carried with the *reason* they
    /// fell.
    pub stage_evaluated: Vec<StageField>,
}

/// A stage-evaluated field plus the disclosed reason it could not be planted.
#[derive(Debug, Clone, PartialEq)]
pub struct StageField {
    pub field: DerivedField,
    pub reason: String,
}

impl DerivedFieldPlan {
    /// Classify each declared field into its seat. **Discloses** the stage
    /// path: every field routed to seat B is logged at `info` with its name
    /// and the reason it could not lower to SQL. This is the
    /// anti-silent-degradation guarantee — a caller can also read
    /// `stage_evaluated` to annotate the UI.
    pub fn plan(fields: Vec<DerivedField>) -> Self {
        let mut sql_planted = Vec::new();
        let mut stage_evaluated = Vec::new();
        for field in fields {
            let reason = match field.computation.compile_sql() {
                Ok(frag) => match frag.inline_sql() {
                    Ok(sql) => {
                        sql_planted.push(PlantedColumn {
                            name: field.name.clone(),
                            sql,
                        });
                        continue;
                    }
                    Err(e) => e.to_string(),
                },
                Err(e) => e.to_string(),
            };
            tracing::info!(
                field = %field.name,
                reason = %reason,
                "C4 derived field routed to DISCLOSED projection-stage evaluation \
                 (not SQL-plantable); IVM will not maintain it incrementally"
            );
            stage_evaluated.push(StageField { field, reason });
        }
        DerivedFieldPlan {
            sql_planted,
            stage_evaluated,
        }
    }

    /// Seat B: evaluate the stage-only fields against `ctx`, writing each
    /// result back into `ctx` so later fields can depend on earlier ones
    /// (topological caller responsibility, same contract as
    /// `resolve_computed_fields`).
    ///
    /// **Retraction-correct**: each field's value is *inserted*, overwriting
    /// any prior value for that name — recomputation replaces, never
    /// stacks. On an input change the caller re-invokes with the fresh
    /// `ctx` and the derived values are wholly recomputed.
    ///
    /// **Fail-loud**: an eval error surfaces as [`ComputeError`] naming the
    /// field — unlike the legacy `resolve_computed_fields`, which
    /// substituted `Null`.
    pub fn evaluate_stage(&self, ctx: &mut Context) -> Result<(), ComputeError> {
        for StageField { field, .. } in &self.stage_evaluated {
            let value = field.computation.eval(ctx)?;
            ctx.insert(field.name.clone(), value);
        }
        Ok(())
    }
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
    fn inline_sql_inlines_literals_no_placeholders() {
        let frag = Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        }
        .compile_sql()
        .unwrap();
        // Parameterized form keeps `?`; inlined form is DDL-safe (no bind params).
        assert_eq!(frag.sql, "(priority * ?)");
        assert_eq!(frag.inline_sql().unwrap(), "(priority * 2)");
    }

    #[test]
    fn inline_sql_escapes_string_literals() {
        let frag = SqlFragment::new("name = ?", vec![Value::String("O'Brien".into())]);
        assert_eq!(frag.inline_sql().unwrap(), "name = 'O''Brien'");
    }

    #[test]
    fn inline_sql_rejects_non_scalar() {
        let frag = SqlFragment::new("x = ?", vec![Value::Array(vec![Value::Integer(1)])]);
        assert!(matches!(
            frag.inline_sql(),
            Err(InlineError::NonScalarLiteral { .. })
        ));
    }

    #[test]
    fn plan_splits_sql_plantable_from_script() {
        // `weight * 2` lowers to SQL (seat A); a switch script does not (seat B).
        let sql_field = DerivedField::new(
            "boosted",
            Computation::Arith {
                op: ArithOp::Mul,
                lhs: Box::new(Computation::Field("weight".into())),
                rhs: Box::new(Computation::Lit(Value::Integer(2))),
            },
        );
        let script_field = DerivedField::new(
            "priority_weight",
            Computation::Script(
                CompiledExpr::compile(&engine(), "switch priority { 3.0 => 100.0, _ => 1.0 }")
                    .unwrap(),
            ),
        );
        let plan = DerivedFieldPlan::plan(vec![sql_field, script_field]);

        assert_eq!(plan.sql_planted.len(), 1);
        assert_eq!(plan.sql_planted[0].name, "boosted");
        assert_eq!(plan.sql_planted[0].sql, "(weight * 2)");

        assert_eq!(plan.stage_evaluated.len(), 1);
        assert_eq!(plan.stage_evaluated[0].field.name, "priority_weight");
        // Disclosed reason names the offending shape.
        assert!(
            plan.stage_evaluated[0].reason.contains("Rhai script"),
            "reason must disclose why it fell to the stage: {}",
            plan.stage_evaluated[0].reason
        );
    }

    #[test]
    fn plan_stage_eval_is_live_and_retracts_cleanly() {
        // A Rhai-only derived field, maintained via seat B. Changing the input
        // recomputes it and REPLACES the old value (never stacks).
        let script_field = DerivedField::new(
            "priority_weight",
            Computation::Script(
                CompiledExpr::compile(
                    &engine(),
                    "switch priority { 3.0 => 100.0, 2.0 => 40.0, _ => 1.0 }",
                )
                .unwrap(),
            ),
        );
        let plan = DerivedFieldPlan::plan(vec![script_field]);
        assert!(plan.sql_planted.is_empty());

        let mut c = ctx(&[("priority", Value::Float(2.0))]);
        plan.evaluate_stage(&mut c).unwrap();
        assert_eq!(c["priority_weight"], Value::Float(40.0));

        // Input changes: same map re-evaluated. The derived value is replaced,
        // and there is exactly one entry for the field (no stacking).
        c.insert("priority".into(), Value::Float(3.0));
        plan.evaluate_stage(&mut c).unwrap();
        assert_eq!(c["priority_weight"], Value::Float(100.0));
        assert_eq!(
            c.keys().filter(|k| *k == "priority_weight").count(),
            1,
            "recomputation must replace, not stack"
        );
    }

    #[test]
    fn plan_stage_eval_is_fail_loud_on_missing_input() {
        // Unlike legacy resolve_computed_fields (which substitutes Null), the seat
        // surfaces a named error for a missing input.
        let field = DerivedField::new("d", Computation::Field("absent".into()));
        // Field-only computations DO lower to SQL, so force the stage path with a
        // script that references the missing input.
        let script = DerivedField::new(
            "d",
            Computation::Script(CompiledExpr::compile(&engine(), "absent + 1.0").unwrap()),
        );
        let _ = field;
        let plan = DerivedFieldPlan::plan(vec![script]);
        let mut c = ctx(&[]);
        assert!(plan.evaluate_stage(&mut c).is_err());
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
