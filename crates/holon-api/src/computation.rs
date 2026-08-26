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
//! ## Numeric semantics (type-faithful, mirrors the repo's Rhai engine)
//!
//! `eval` keeps `Integer` and `Float` distinct through arithmetic, matching
//! Rhai's default **checked** integer arithmetic (verified against
//! `bounded_engine`, rhai 1.x, no `unchecked` feature):
//!
//! - `int op int` → `int` — including **integer division** (`5 / 2 = 2`, `-5 /
//!   2 = -2`, truncating toward zero). Overflow and integer division-by-zero
//!   **fail loud** ([`ComputeError::Arithmetic`]), exactly as Rhai raises
//!   `Addition overflow` / `Division by zero`.
//! - mixed `int`/`float` (either operand float) → `float` (Rhai promotes the
//!   integer). Float arithmetic is IEEE and NOT checked: `x/0.0` → ±inf,
//!   `0.0/0.0` → NaN, same as Rhai.
//!
//! Equality has TWO faces, because Rhai's `==` and `switch` disagree on
//! cross-type numerics (both verified against the engine):
//!
//! - [`Computation::Compare`] `==`/`!=` and the ordering ops are **numeric**
//!   (`5 == 5.0` is true) — matching Rhai `==` AND SQLite `=`.
//! - [`Computation::Case`] (the `switch` shape) is **type-strict** (`switch 2 {
//!   2.0 => … }` does NOT match) — matching Rhai `switch`. Its SQL lowering
//!   uses SQLite's numeric `=`, so eval and SQL agree only when a switch's
//!   scrutinee and case labels share a numeric type; the A2 domain keeps them
//!   same-type.
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

/// Comparison operators for the [`Computation::Compare`] shape (a
/// boolean-valued comparison between two sub-computations — unlike
/// [`Predicate`], both sides are arbitrary expressions, so `field > field` is
/// expressible).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// Evaluate the comparison. `Eq`/`Ne` use numeric-aware value equality;
    /// the ordering operators require numeric operands (fail-loud otherwise).
    fn apply(self, l: &Value, r: &Value) -> Result<bool, ComputeError> {
        match self {
            CmpOp::Eq => Ok(values_match(l, r)),
            CmpOp::Ne => Ok(!values_match(l, r)),
            CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                let x = as_number(l, "comparison left operand")?;
                let y = as_number(r, "comparison right operand")?;
                Ok(match self {
                    CmpOp::Lt => x < y,
                    CmpOp::Le => x <= y,
                    CmpOp::Gt => x > y,
                    CmpOp::Ge => x >= y,
                    CmpOp::Eq | CmpOp::Ne => unreachable!(),
                })
            }
        }
    }

    fn sql(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// Numeric-aware value equality: two numbers compare by `f64`, everything else
/// by structural [`Value`] equality. Used ONLY by [`CmpOp`] `Eq`/`Ne`,
/// mirroring Rhai's coercing `==` (`5 == 5.0` is true) and SQLite's numeric
/// `=`.
///
/// [`Computation::Case`] deliberately does NOT use this — Rhai `switch` is
/// type-strict, so Case matches with structural `Value` equality.
fn values_match(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// A context key / column name that is a bare SQL identifier: non-empty,
/// `[A-Za-z_][A-Za-z0-9_]*`.
///
/// The constraint is in the type because the name reaches planted matview DDL
/// in **identifier position** (`(<name> IS NOT NULL)`), where quoting is not an
/// option — a definedness test is over a column, and quoting would turn it into
/// a comparison against a string literal. Anything that is not an identifier is
/// therefore rejected where a raw string first appears, not re-checked at the
/// SQL boundary.
///
/// The grammar is exactly the one the subset tokenizer already enforces for a
/// bare field reference, so `is_def_var("x")` and `x` accept the same names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldIdent(String);

impl FieldIdent {
    pub fn parse(name: &str) -> Result<Self, InvalidFieldIdent> {
        let mut chars = name.chars();
        let valid = match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        };
        if valid {
            Ok(FieldIdent(name.to_string()))
        } else {
            Err(InvalidFieldIdent {
                name: name.to_string(),
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FieldIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A name that is not a bare SQL identifier and so cannot become a
/// [`FieldIdent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidFieldIdent {
    pub name: String,
}

impl fmt::Display for InvalidFieldIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a bare identifier ([A-Za-z_][A-Za-z0-9_]*), so it cannot name a column",
            self.name
        )
    }
}

impl std::error::Error for InvalidFieldIdent {}

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
    /// A boolean comparison between two sub-computations (`lhs op rhs`).
    /// Distinct from [`Predicate`], which compares a named field to a literal
    /// `Value`; here both sides are expressions, so `field > field` and
    /// `expr <= 0` are expressible. Produced by the A2 subset parser for the
    /// conditions of `if`.
    Compare {
        op: CmpOp,
        lhs: Box<Computation>,
        rhs: Box<Computation>,
    },
    /// A multi-way conditional with **equality-match on a scrutinee** (the
    /// `switch`/`if` shape): evaluate `scrutinee`, then take the `result` of
    /// the first `(match_value, result)` branch whose `match_value` equals
    /// it (numeric-aware; see [`values_match`]), else `else_`.
    ///
    /// - `switch x { a => r1, b => r2, _ => e }` maps directly: `scrutinee =
    ///   x`, branches `[(a, r1), (b, r2)]`, `else_ = e`.
    /// - `if c1 { r1 } else if c2 { r2 } else { e }` maps with `scrutinee =
    ///   Lit(Boolean(true))` and branches `[(c1, r1), (c2, r2)]` — each
    ///   `match_value` is the boolean condition.
    ///
    /// **SQL lowering deliberately targets nested `iif(...)`, NOT `CASE
    /// WHEN`**: the Turso fork's IVM matview planner rejects `CASE` at DDL
    /// ("Cannot convert LogicalExpr to AST Expr: Case"), but accepts `iif`
    /// (proven in `holon-turso/tests/json_extract_matview_spike.rs`). Affinity
    /// note: `iif(scrutinee = match_value, …)` relies on SQLite's numeric
    /// comparison, matching the in-memory `values_match`.
    Case {
        scrutinee: Box<Computation>,
        branches: Vec<(Computation, Computation)>,
        else_: Box<Computation>,
    },
    /// Text concatenation of two sub-computations (`lhs || rhs` in SQL). A
    /// shape of its own rather than an [`ArithOp::Add`] overload: `Add`'s
    /// `apply` is `f64`-typed by contract, and widening it would make every
    /// numeric operand silently stringifiable.
    ///
    /// Rendering follows **SQLite, not Rhai** — the planted column is what
    /// production reads. Three axes diverge from Rhai, each pinned by a named
    /// test in `derived_field_dual_eval_pbt.rs`:
    ///
    /// | operand | here | Rhai |
    /// |---|---|---|
    /// | [`Value::Null`] | propagates ⇒ `Null` | raises |
    /// | whole float `1.0` | `"1.0"` (`{:?}`) | `"1"` (`Display`) |
    /// | [`Value::Boolean`] | `"1"` / `"0"` | `"true"` / `"false"` |
    ///
    /// A fourth divergence is about *which* operator is chosen, not rendering:
    /// `+` is `Concat` by syntax, so a `Case` with one text arm concatenates a
    /// number the other arm supplies where Rhai adds it
    /// (`case_armed_concat_of_a_number_diverges_from_rhai`). All four need the
    /// declared field types to resolve — the I3-1 gap.
    Concat {
        lhs: Box<Computation>,
        rhs: Box<Computation>,
    },
    /// Short-circuiting boolean conjunction (`lhs && rhs`). Short-circuit is
    /// load-bearing, not an optimization: `is_def_var("role") && role != ()`
    /// must not evaluate `role` when it is absent, or the guard raises the
    /// very [`ComputeError::MissingField`] it exists to prevent.
    ///
    /// `eval` raises [`ComputeError::WrongType`] on a non-boolean operand, but
    /// the lowering `(l AND r)` is read truthily by SQLite — so the subset
    /// parser refuses an operand that is not boolean by syntax, keeping the two
    /// seats from disagreeing
    /// (`a_non_boolean_and_operand_is_refused_by_both_seats`).
    And {
        lhs: Box<Computation>,
        rhs: Box<Computation>,
    },
    /// `is_def_var("name")` — is `name` bound in the context?
    ///
    /// `eval` is key-presence, matching Rhai exactly (a key bound to
    /// [`Value::Null`] is *defined*). SQL lowers to `name IS NOT NULL`, because
    /// a row has no notion of an absent column — NULL is the row world's only
    /// spelling of "not there". The two therefore disagree on exactly one
    /// state: a context key present with a `Null` value. The idiom this shape
    /// serves conjoins it with `name != ()`, under which both legs agree; the
    /// isolated divergence is pinned by
    /// `is_defined_eval_and_sql_diverge_on_null` in
    /// `holon-turso/tests/derived_field_eval_vs_sql.rs`.
    ///
    /// The name is a [`FieldIdent`], not a `String`: it lands in identifier
    /// position in planted DDL, where it cannot be quoted or parameterised.
    IsDefined(FieldIdent),
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
    NotNumeric {
        context: String,
        value: Value,
    },
    /// An operand of the wrong type for a non-arithmetic shape (a non-boolean
    /// under `&&`, a non-scalar under `||`).
    WrongType {
        context: String,
        expected: &'static str,
        value: Value,
    },
    Script {
        source: String,
        detail: String,
    },
    /// Integer overflow or integer division-by-zero — the same conditions
    /// Rhai's default **checked** integer arithmetic raises as runtime errors.
    /// Float arithmetic is NOT checked (mirrors Rhai): `x/0.0` yields ±inf and
    /// `0.0/0.0` NaN at eval time (a non-finite value is only rejected later,
    /// at SQL-plant time).
    Arithmetic {
        detail: String,
    },
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
            ComputeError::WrongType {
                context,
                expected,
                value,
            } => {
                write!(f, "{context} expects {expected}, found {value:?}")
            }
            ComputeError::Script { source, detail } => {
                write!(f, "Rhai evaluation failed for `{source}`: {detail}")
            }
            ComputeError::Arithmetic { detail } => {
                write!(f, "arithmetic error: {detail}")
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
    /// A non-finite float (`±inf`/`NaN`) has no SQLite literal syntax; planting
    /// one would silently corrupt the column, so it is a loud planning error.
    NonFiniteFloat { value: f64 },
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
            InlineError::NonFiniteFloat { value } => {
                write!(
                    f,
                    "cannot inline non-finite float ({value}) as a SQL literal; SQLite has no \
                     inf/NaN literal"
                )
            }
        }
    }
}

impl std::error::Error for InlineError {}

fn value_to_sql_literal(v: &Value) -> Result<String, InlineError> {
    Ok(match v {
        Value::Integer(i) => i.to_string(),
        // `{:?}` for f64 always renders a decimal point or exponent, so the
        // literal keeps REAL affinity in SQLite (`3.0`, not `3` which would take
        // integer division). Non-finite floats have no SQL literal — fail loud.
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(InlineError::NonFiniteFloat { value: *f });
            }
            format!("{f:?}")
        }
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
        Value::Removed(_) | Value::Null | Value::Array(_) | Value::Object(_) => {
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
    /// A column name that is not a bare identifier. It would land unquoted in
    /// identifier position, so it is refused rather than emitted.
    NonIdentifierColumn { detail: String },
}

impl fmt::Display for SqlUnsupported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlUnsupported::Script { source } => write!(
                f,
                "computation `{source}` is a Rhai script and cannot be compiled to SQL; evaluate \
                 it in-memory (disclosed degraded mode)"
            ),
            SqlUnsupported::NonIdentifierColumn { detail } => {
                write!(f, "cannot compile a column reference to SQL: {detail}")
            }
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
                let l = lhs.eval(ctx)?;
                let r = rhs.eval(ctx)?;
                arith_apply(*op, &l, &r)
            }
            Computation::Compare { op, lhs, rhs } => {
                let l = lhs.eval(ctx)?;
                let r = rhs.eval(ctx)?;
                Ok(Value::Boolean(op.apply(&l, &r)?))
            }
            Computation::Case {
                scrutinee,
                branches,
                else_,
            } => {
                // Type-strict equality (structural `Value`) — mirrors Rhai's
                // type-sensitive `switch`, NOT the numeric `==`.
                let s = scrutinee.eval(ctx)?;
                for (match_value, result) in branches {
                    if s == match_value.eval(ctx)? {
                        return result.eval(ctx);
                    }
                }
                else_.eval(ctx)
            }
            Computation::Concat { lhs, rhs } => {
                let l = lhs.eval(ctx)?;
                let r = rhs.eval(ctx)?;
                if l == Value::Null || r == Value::Null {
                    return Ok(Value::Null);
                }
                Ok(Value::String(format!(
                    "{}{}",
                    concat_text(&l, "`||` left operand")?,
                    concat_text(&r, "`||` right operand")?
                )))
            }
            Computation::And { lhs, rhs } => {
                if !as_bool(&lhs.eval(ctx)?, "`&&` left operand")? {
                    return Ok(Value::Boolean(false));
                }
                Ok(Value::Boolean(as_bool(
                    &rhs.eval(ctx)?,
                    "`&&` right operand",
                )?))
            }
            Computation::IsDefined(name) => Ok(Value::Boolean(ctx.contains_key(name.as_str()))),
            Computation::Predicate(p) => Ok(Value::Boolean(p.evaluate(ctx))),
            Computation::Script(expr) => eval_script(expr, ctx),
        }
    }

    /// The set of context-field names this computation reads. `Script` is
    /// opaque to structural walking (a compiled Rhai AST) and contributes
    /// nothing here — callers that need Script dependencies must scan
    /// [`CompiledExpr::source`] themselves. Used for topological ordering of
    /// dependent derived fields.
    pub fn referenced_fields(&self) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        self.collect_fields(&mut out);
        out
    }

    fn collect_fields(&self, out: &mut std::collections::BTreeSet<String>) {
        match self {
            Computation::Lit(_) | Computation::Script(_) => {}
            Computation::Field(name) => {
                out.insert(name.clone());
            }
            Computation::Arith { lhs, rhs, .. }
            | Computation::Compare { lhs, rhs, .. }
            | Computation::Concat { lhs, rhs }
            | Computation::And { lhs, rhs } => {
                lhs.collect_fields(out);
                rhs.collect_fields(out);
            }
            Computation::IsDefined(name) => {
                out.insert(name.as_str().to_string());
            }
            Computation::Case {
                scrutinee,
                branches,
                else_,
            } => {
                scrutinee.collect_fields(out);
                for (match_value, result) in branches {
                    match_value.collect_fields(out);
                    result.collect_fields(out);
                }
                else_.collect_fields(out);
            }
            Computation::Predicate(p) => collect_predicate_fields(p, out),
        }
    }

    /// Partial, **disclosed** lowering to a SQL fragment. `Err(SqlUnsupported)`
    /// names the exact shape that cannot lower — never a bare `None`.
    pub fn compile_sql(&self) -> Result<SqlFragment, SqlUnsupported> {
        match self {
            // `NULL` is emitted as a keyword, not a bound param: a param can
            // neither be inlined into a matview column expression nor compared
            // with `=`.
            Computation::Lit(Value::Null) => Ok(SqlFragment::new("NULL", vec![])),
            Computation::Lit(v) => Ok(SqlFragment::new("?", vec![v.clone()])),
            // The subset tokenizer only ever builds `Field` from an identifier,
            // but the variant carries a raw `String` a programmatic caller does
            // not inherit that constraint from — and this is identifier
            // position, so a non-identifier would be injected verbatim.
            Computation::Field(name) => {
                FieldIdent::parse(name).map_err(|e| SqlUnsupported::NonIdentifierColumn {
                    detail: e.to_string(),
                })?;
                Ok(SqlFragment::new(name.clone(), vec![]))
            }
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
            // `x = NULL` is NULL in SQL, never true — an `=`/`!=` against the
            // unit literal must lower to the null-test operators or the whole
            // guard silently evaluates to NULL.
            Computation::Compare {
                op: op @ (CmpOp::Eq | CmpOp::Ne),
                lhs,
                rhs,
            } if **lhs == Computation::Lit(Value::Null)
                || **rhs == Computation::Lit(Value::Null) =>
            {
                let other = if **lhs == Computation::Lit(Value::Null) {
                    rhs
                } else {
                    lhs
                };
                let o = other.compile_sql()?;
                let test = if matches!(op, CmpOp::Eq) {
                    "IS NULL"
                } else {
                    "IS NOT NULL"
                };
                Ok(SqlFragment::new(format!("({} {})", o.sql, test), o.params))
            }
            Computation::Compare { op, lhs, rhs } => {
                let l = lhs.compile_sql()?;
                let r = rhs.compile_sql()?;
                let mut params = l.params;
                params.extend(r.params);
                Ok(SqlFragment::new(
                    format!("({} {} {})", l.sql, op.sql(), r.sql),
                    params,
                ))
            }
            Computation::Concat { lhs, rhs } => {
                let l = lhs.compile_sql()?;
                let r = rhs.compile_sql()?;
                let mut params = l.params;
                params.extend(r.params);
                Ok(SqlFragment::new(
                    format!("({} || {})", l.sql, r.sql),
                    params,
                ))
            }
            Computation::And { lhs, rhs } => {
                let l = lhs.compile_sql()?;
                let r = rhs.compile_sql()?;
                let mut params = l.params;
                params.extend(r.params);
                Ok(SqlFragment::new(
                    format!("({} AND {})", l.sql, r.sql),
                    params,
                ))
            }
            // `name` is a `FieldIdent`, so it is an identifier by construction —
            // the only reason this interpolation is safe.
            Computation::IsDefined(name) => {
                Ok(SqlFragment::new(format!("({name} IS NOT NULL)"), vec![]))
            }
            Computation::Case {
                scrutinee,
                branches,
                else_,
            } => {
                let s = scrutinee.compile_sql()?;
                let e = else_.compile_sql()?;
                build_iif_chain(&s, branches, &e)
            }
            Computation::Predicate(p) => predicate_to_sql(p),
            Computation::Script(expr) => Err(SqlUnsupported::Script {
                source: expr.source.clone(),
            }),
        }
    }
}

/// Lower a [`Computation::Case`] to a nested `iif(...)` chain. Each branch
/// becomes `iif(<scrutinee> = <match_value>, <result>, <rest>)`; the innermost
/// `<rest>` is the else expression. `iif` is used instead of SQL `CASE` because
/// the Turso fork's IVM planner rejects `CASE` in a matview SELECT (spike:
/// `json_extract_matview_spike.rs`). The scrutinee fragment is re-emitted per
/// branch, so its params repeat in left-to-right placeholder order — consistent
/// with [`SqlFragment::inline_sql`].
fn build_iif_chain(
    scrutinee: &SqlFragment,
    branches: &[(Computation, Computation)],
    else_frag: &SqlFragment,
) -> Result<SqlFragment, SqlUnsupported> {
    match branches.split_first() {
        None => Ok(else_frag.clone()),
        Some(((match_value, result), rest)) => {
            let mv = match_value.compile_sql()?;
            let res = result.compile_sql()?;
            let inner = build_iif_chain(scrutinee, rest, else_frag)?;
            let mut params = scrutinee.params.clone();
            params.extend(mv.params);
            params.extend(res.params);
            params.extend(inner.params);
            Ok(SqlFragment::new(
                format!(
                    "iif({} = {}, {}, {})",
                    scrutinee.sql, mv.sql, res.sql, inner.sql
                ),
                params,
            ))
        }
    }
}

/// Collect the field names referenced by a [`Predicate`] into `out`.
fn collect_predicate_fields(pred: &Predicate, out: &mut std::collections::BTreeSet<String>) {
    match pred {
        Predicate::Eq { field, .. }
        | Predicate::Ne { field, .. }
        | Predicate::Gt { field, .. }
        | Predicate::Lt { field, .. }
        | Predicate::Gte { field, .. }
        | Predicate::Lte { field, .. }
        | Predicate::IsNotNull(field)
        | Predicate::Var(field) => {
            out.insert(field.clone());
        }
        Predicate::Not(inner) => collect_predicate_fields(inner, out),
        Predicate::And(preds) | Predicate::Or(preds) => {
            for p in preds {
                collect_predicate_fields(p, out);
            }
        }
        Predicate::Always => {}
    }
}

fn as_bool(v: &Value, context: &str) -> Result<bool, ComputeError> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => Err(ComputeError::WrongType {
            context: context.to_string(),
            expected: "a boolean",
            value: other.clone(),
        }),
    }
}

/// The canonical value→text rendering for [`Computation::Concat`], chosen to
/// match what SQLite's `||` produces for the same value, so the planted column
/// and the in-memory evaluation agree.
///
/// `Float` uses `{:?}` (shortest round-tripping form, always with a decimal
/// point) — the same rendering [`value_to_sql_literal`] plants. That agrees
/// with SQLite over ordinary magnitudes; the two exponent spellings drift at
/// extremes (`1e20` vs SQLite's `1.0e+20`), which is why the eval-vs-SQL matrix
/// pins the agreeing range explicitly.
fn concat_text(v: &Value, context: &str) -> Result<String, ComputeError> {
    Ok(match v {
        Value::String(s) | Value::DateTime(s) | Value::Json(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}"),
        Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) | Value::Removed(_) => {
            return Err(ComputeError::WrongType {
                context: context.to_string(),
                expected: "a scalar value",
                value: v.clone(),
            });
        }
    })
}

fn as_number(v: &Value, context: &str) -> Result<f64, ComputeError> {
    v.as_f64().ok_or_else(|| ComputeError::NotNumeric {
        context: context.to_string(),
        value: v.clone(),
    })
}

/// Type-faithful arithmetic mirroring Rhai: `int op int` stays integer
/// (checked; overflow / integer-div-by-zero fail loud), any float operand
/// promotes to a float result (IEEE, unchecked). See the module header.
fn arith_apply(op: ArithOp, lhs: &Value, rhs: &Value) -> Result<Value, ComputeError> {
    if let (Value::Integer(a), Value::Integer(b)) = (lhs, rhs) {
        let (a, b) = (*a, *b);
        let checked = match op {
            ArithOp::Add => a.checked_add(b),
            ArithOp::Sub => a.checked_sub(b),
            ArithOp::Mul => a.checked_mul(b),
            // checked_div also rejects i64::MIN / -1 overflow.
            ArithOp::Div => a.checked_div(b),
        };
        return checked.map(Value::Integer).ok_or_else(|| {
            let detail = if op == ArithOp::Div && b == 0 {
                format!("integer division by zero: {a} / 0")
            } else {
                format!("integer overflow: {a} {} {b}", op.sql())
            };
            ComputeError::Arithmetic { detail }
        });
    }
    let a = as_number(lhs, "arithmetic left operand")?;
    let b = as_number(rhs, "arithmetic right operand")?;
    Ok(Value::Float(op.apply(a, b)))
}

/// Evaluate a compiled Rhai expression over `ctx` — the same single-expression
/// path `rank_tasks` uses, generalized to arbitrary [`Value`] inputs.
fn eval_script(expr: &CompiledExpr, ctx: &Context) -> Result<Value, ComputeError> {
    let engine = bounded_engine();
    let mut scope = Scope::new();
    for (k, v) in ctx {
        match v {
            // Push integers as Rhai INT (i64), NOT coerced to f64: coercion made
            // the Script (seat B) path disagree with the typed (seat A) path on
            // integer semantics — `switch j { 1 => … }` is type-strict, so a
            // silently-float `j` would miss. Type-faithful marshalling keeps the
            // two seats observably identical.
            Value::Integer(i) => scope.push(k.clone(), *i),
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
    // Every arm below interpolates `field` into identifier position, where it
    // can be neither quoted nor parameterised. `Predicate` is FRB-exposed and
    // carries a raw `String`, so the constraint cannot live in its type without
    // crossing the bridge; it is enforced here instead — the one place a
    // predicate becomes SQL text. `Not`/`And`/`Or` recurse through this same
    // check.
    if let Some(field) = predicate_column(pred) {
        FieldIdent::parse(field).map_err(|e| SqlUnsupported::NonIdentifierColumn {
            detail: e.to_string(),
        })?;
    }
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

/// The column a predicate names directly, if it names one. The composite shapes
/// return `None` — their children are checked when they recurse.
fn predicate_column(pred: &Predicate) -> Option<&str> {
    match pred {
        Predicate::Eq { field, .. }
        | Predicate::Ne { field, .. }
        | Predicate::Gt { field, .. }
        | Predicate::Lt { field, .. }
        | Predicate::Gte { field, .. }
        | Predicate::Lte { field, .. }
        | Predicate::IsNotNull(field)
        | Predicate::Var(field) => Some(field),
        Predicate::Not(_) | Predicate::And(_) | Predicate::Or(_) | Predicate::Always => None,
    }
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

    fn f(name: &str) -> Box<Computation> {
        Box::new(Computation::Field(name.into()))
    }

    fn lit(x: f64) -> Box<Computation> {
        Box::new(Computation::Lit(Value::Float(x)))
    }

    #[test]
    fn compare_eval_and_sql() {
        let c = ctx(&[("a", Value::Float(3.0)), ("b", Value::Float(5.0))]);
        let gt = Computation::Compare {
            op: CmpOp::Gt,
            lhs: f("a"),
            rhs: f("b"),
        };
        assert_eq!(gt.eval(&c).unwrap(), Value::Boolean(false));
        assert_eq!(gt.compile_sql().unwrap().inline_sql().unwrap(), "(a > b)");
    }

    #[test]
    fn case_switch_eval_matches_first_equal_branch() {
        // switch priority { 3.0 => 100.0, 2.0 => 40.0, _ => 1.0 }
        let case = Computation::Case {
            scrutinee: f("priority"),
            branches: vec![
                (Computation::Lit(Value::Float(3.0)), *lit(100.0)),
                (Computation::Lit(Value::Float(2.0)), *lit(40.0)),
            ],
            else_: lit(1.0),
        };
        assert_eq!(
            case.eval(&ctx(&[("priority", Value::Float(2.0))])).unwrap(),
            Value::Float(40.0)
        );
        assert_eq!(
            case.eval(&ctx(&[("priority", Value::Float(9.0))])).unwrap(),
            Value::Float(1.0) // falls to else
        );
    }

    #[test]
    fn case_lowers_to_nested_iif_not_case_when() {
        // The spike-mandated lowering: CASE is rejected by the fork IVM, iif is not.
        let case = Computation::Case {
            scrutinee: f("priority"),
            branches: vec![
                (Computation::Lit(Value::Float(3.0)), *lit(100.0)),
                (Computation::Lit(Value::Float(2.0)), *lit(40.0)),
            ],
            else_: lit(1.0),
        };
        let sql = case.compile_sql().unwrap().inline_sql().unwrap();
        assert_eq!(
            sql,
            "iif(priority = 3.0, 100.0, iif(priority = 2.0, 40.0, 1.0))"
        );
        assert!(!sql.contains("CASE"), "must NOT emit SQL CASE");
    }

    #[test]
    fn case_if_shape_uses_boolean_scrutinee() {
        // if a > b { 10.0 } else { 20.0 }  == Case over scrutinee `true`.
        let case = Computation::Case {
            scrutinee: Box::new(Computation::Lit(Value::Boolean(true))),
            branches: vec![(
                Computation::Compare {
                    op: CmpOp::Gt,
                    lhs: f("a"),
                    rhs: f("b"),
                },
                *lit(10.0),
            )],
            else_: lit(20.0),
        };
        assert_eq!(
            case.eval(&ctx(&[("a", Value::Float(7.0)), ("b", Value::Float(1.0))]))
                .unwrap(),
            Value::Float(10.0)
        );
        assert_eq!(
            case.compile_sql().unwrap().inline_sql().unwrap(),
            "iif(1 = (a > b), 10.0, 20.0)"
        );
    }

    #[test]
    fn case_referenced_fields_walks_all_arms() {
        let case = Computation::Case {
            scrutinee: f("s"),
            branches: vec![(*f("m"), *f("r"))],
            else_: f("e"),
        };
        let fields = case.referenced_fields();
        assert!(["s", "m", "r", "e"].iter().all(|n| fields.contains(*n)));
    }

    fn ilit(i: i64) -> Box<Computation> {
        Box::new(Computation::Lit(Value::Integer(i)))
    }

    fn arith(op: ArithOp, l: Box<Computation>, r: Box<Computation>) -> Computation {
        Computation::Arith { op, lhs: l, rhs: r }
    }

    #[test]
    fn int_arithmetic_is_type_faithful_integer_division() {
        // 5 / 2 = 2 (integer division), matching Rhai; NOT 2.5.
        let e = arith(ArithOp::Div, ilit(5), ilit(2));
        assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Integer(2));
        // 9 / 4 + 1 = 2 + 1 = 3.
        let e = arith(
            ArithOp::Add,
            Box::new(arith(ArithOp::Div, ilit(9), ilit(4))),
            ilit(1),
        );
        assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Integer(3));
    }

    #[test]
    fn mixed_int_float_arithmetic_promotes_to_float() {
        // 5 / 2.0 = 2.5 (mixed promotes), matching Rhai.
        let e = arith(ArithOp::Div, ilit(5), lit(2.0));
        assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Float(2.5));
    }

    #[test]
    fn integer_division_by_zero_is_fail_loud() {
        let e = arith(ArithOp::Div, ilit(5), ilit(0));
        assert!(matches!(
            e.eval(&ctx(&[])),
            Err(ComputeError::Arithmetic { .. })
        ));
    }

    #[test]
    fn integer_overflow_is_fail_loud() {
        let e = arith(ArithOp::Add, ilit(i64::MAX), ilit(1));
        assert!(matches!(
            e.eval(&ctx(&[])),
            Err(ComputeError::Arithmetic { .. })
        ));
    }

    #[test]
    fn whole_float_literal_plants_with_decimal_point() {
        // 3.0 / 2.0 must plant as `(3.0 / 2.0)` (=1.5), NOT `(3 / 2)` (=1 in
        // SQLite integer division). This is the eval-vs-SQL refutation fix.
        let e = arith(ArithOp::Div, lit(3.0), lit(2.0));
        assert_eq!(
            e.compile_sql().unwrap().inline_sql().unwrap(),
            "(3.0 / 2.0)"
        );
    }

    #[test]
    fn case_switch_is_type_strict_like_rhai() {
        // switch (int 2) { 2.0 => 100, _ => 1 } does NOT match the float case.
        let case = Computation::Case {
            scrutinee: ilit(2),
            branches: vec![(Computation::Lit(Value::Float(2.0)), *ilit(100))],
            else_: ilit(1),
        };
        assert_eq!(case.eval(&ctx(&[])).unwrap(), Value::Integer(1));
    }

    #[test]
    fn non_finite_float_literal_is_a_loud_plant_error() {
        let frag = SqlFragment::new("x = ?", vec![Value::Float(f64::INFINITY)]);
        assert!(matches!(
            frag.inline_sql(),
            Err(InlineError::NonFiniteFloat { .. })
        ));
        let nan = SqlFragment::new("x = ?", vec![Value::Float(f64::NAN)]);
        assert!(matches!(
            nan.inline_sql(),
            Err(InlineError::NonFiniteFloat { .. })
        ));
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
