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
//!   integer). A non-finite float result (`x/0.0`, `0.0/0.0`, overflow) fails
//!   loud ([`ComputeError::Arithmetic`]), where Rhai yields ±inf or NaN.
//! - a NULL operand → NULL, as in SQL, where Rhai raises on `()`.
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

    /// The SQL scalar function the lowering emits for this operator. SQL's own
    /// operators turn `/0`, overflow and non-finite results into NULL or a
    /// promoted REAL; the function computes [`arith_apply`] and refuses
    /// instead. A database registers it at open.
    pub const fn sql_fn(self) -> &'static str {
        match self {
            ArithOp::Add => "holon_add",
            ArithOp::Sub => "holon_sub",
            ArithOp::Mul => "holon_mul",
            ArithOp::Div => "holon_div",
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
    pub fn apply(self, l: &Value, r: &Value) -> Result<bool, ComputeError> {
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
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct FieldIdent(String);

impl TryFrom<String> for FieldIdent {
    type Error = InvalidFieldIdent;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::parse(&name)
    }
}

impl From<FieldIdent> for String {
    fn from(ident: FieldIdent) -> Self {
        ident.0
    }
}

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

/// The storage class of a declared column, as far as the subset parser needs
/// to distinguish it: `+` over text is [`Computation::Concat`], over numbers
/// [`Computation::Arith`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Numeric,
    Boolean,
}

impl FieldKind {
    /// The kind a declared SQL type belongs to, by SQLite's affinity rules.
    /// `None` for a type with no affinity the parser can act on (BLOB, and
    /// anything unrecognised).
    pub fn of_sql_type(sql_type: &str) -> Option<Self> {
        let t = sql_type.to_ascii_uppercase();
        if t.contains("BOOL") {
            Some(FieldKind::Boolean)
        } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
            Some(FieldKind::Text)
        } else if t.contains("INT")
            || t.contains("REAL")
            || t.contains("FLOA")
            || t.contains("DOUB")
            || t.contains("NUM")
            || t.contains("DEC")
        {
            Some(FieldKind::Numeric)
        } else {
            None
        }
    }
}

/// The declared types of the columns an expression may reference.
///
/// Supplied by the type registry so `+` is resolved from the schema instead of
/// from the operands' syntax — over two undeclared fields the syntax alone
/// cannot tell `a + b` numeric from textual.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldTypes(std::collections::BTreeMap<String, FieldKind>);

impl FieldTypes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, kind: FieldKind) {
        self.0.insert(name.into(), kind);
    }

    pub fn kind(&self, name: &str) -> Option<FieldKind> {
        self.0.get(name).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
    /// it (numeric-aware; see `values_match`), else `else_`.
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
    /// a `Case` with one text arm concatenates a number the other arm supplies
    /// where Rhai adds it
    /// (`case_armed_concat_of_a_number_diverges_from_rhai`). Declaring the
    /// operand columns in [`FieldTypes`] resolves the operator; the three
    /// rendering axes above remain.
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
    /// Rhai's default **checked** integer arithmetic raises as runtime errors —
    /// or a float result that is NaN or infinite.
    Arithmetic {
        detail: String,
    },
    /// A render-DSL call to a name that is neither a value function nor a
    /// widget.
    UnknownFunction {
        name: String,
        call: String,
    },
    /// A call in a value evaluated outside any render, where only the core
    /// value functions exist.
    NotAPlainValueFunction {
        name: String,
        call: String,
    },
    /// A call to `widget` where a value is needed: a widget is only a
    /// widget's child or a template arg. `place` names the arg, once known.
    WidgetInValuePosition {
        widget: String,
        call: String,
        place: Option<String>,
    },
    /// A render-DSL action whose entity part names no entity.
    InvalidActionEntity {
        action: String,
        source: crate::InvalidEntityName,
    },
}

impl ComputeError {
    /// `self`, placed in `place` when it is a widget call no enclosing arg
    /// has named yet.
    pub fn placed_in(self, place: impl FnOnce() -> String) -> Self {
        match self {
            ComputeError::WidgetInValuePosition {
                widget,
                call,
                place: None,
            } => ComputeError::WidgetInValuePosition {
                widget,
                call,
                place: Some(place()),
            },
            other => other,
        }
    }
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
            ComputeError::UnknownFunction { name, call } => write!(
                f,
                "unknown function `{name}` in `{call}`: neither a value function nor a widget"
            ),
            ComputeError::NotAPlainValueFunction { name, call } => write!(
                f,
                "`{name}` cannot be called in `{call}`: outside a render, only the value \
                 functions {} can be called",
                crate::render_eval::CORE_VALUE_FN_NAMES.join(", ")
            ),
            ComputeError::WidgetInValuePosition {
                widget,
                call,
                place,
            } => write!(
                f,
                "widget `{widget}` cannot stand in {}, which needs a value: a widget is only \
                 a widget's child or a template arg (in `{call}`)",
                place.as_deref().unwrap_or("an operand or condition")
            ),
            ComputeError::InvalidActionEntity { action, source } => {
                write!(f, "action `{action}`: {source}")
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
        let mut params = self.params.iter().enumerate();
        for ch in self.sql.chars() {
            if ch == '?' {
                let (i, v) = params.next().ok_or(InlineError::PlaceholderParamMismatch {
                    sql: self.sql.clone(),
                    params: self.params.len(),
                })?;
                out.push_str(&value_to_sql_literal(v, i + 1, &self.sql)?);
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
    NonFiniteFloat {
        value: f64,
        placeholder: usize,
        sql: String,
    },
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
            InlineError::NonFiniteFloat {
                value,
                placeholder,
                sql,
            } => {
                write!(
                    f,
                    "cannot inline non-finite float ({value}) for placeholder {placeholder} of \
                     `{sql}` as a SQL literal; SQLite has no inf/NaN literal"
                )
            }
        }
    }
}

impl std::error::Error for InlineError {}

/// A negative number is spelled as a CAST of its text: SQL has no negative
/// literal, only unary minus, which the matview planner refuses, and `-`
/// applied to `9223372036854775808` is a REAL.
fn value_to_sql_literal(v: &Value, placeholder: usize, sql: &str) -> Result<String, InlineError> {
    Ok(match v {
        Value::Integer(i) if *i < 0 => format!("CAST('{i}' AS INTEGER)"),
        Value::Integer(i) => i.to_string(),
        // `{:?}` for f64 always renders a decimal point or exponent, so the
        // literal keeps REAL affinity in SQLite (`3.0`, not `3` which would take
        // integer division). Non-finite floats have no SQL literal — fail loud.
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(InlineError::NonFiniteFloat {
                    value: *f,
                    placeholder,
                    sql: sql.to_string(),
                });
            }
            if f.is_sign_negative() {
                format!("CAST('{f:?}' AS REAL)")
            } else {
                format!("{f:?}")
            }
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
            Computation::Concat { lhs, rhs } => concat_apply(&lhs.eval(ctx)?, &rhs.eval(ctx)?),
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
                    format!("{}({}, {})", op.sql_fn(), l.sql, r.sql),
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

    /// Refuse an arithmetic operand, anywhere in the tree, whose declared
    /// storage class is not numeric. [`Self::result_kind`] answers for the
    /// node it is asked about only; an arithmetic node under a comparison or a
    /// branch is checked here. A [`Computation::Script`] is opaque.
    pub fn check_arith_operands(&self, types: &FieldTypes) -> Result<(), ResultKindUnknown> {
        match self {
            Computation::Arith { lhs, rhs, .. } => {
                for operand in [lhs, rhs] {
                    if let Ok(kind @ (FieldKind::Text | FieldKind::Boolean)) =
                        operand.result_kind(types)
                    {
                        return Err(ResultKindUnknown::NonNumericOperand {
                            operand: format!("{operand:?}"),
                            kind,
                        });
                    }
                    operand.check_arith_operands(types)?;
                }
                Ok(())
            }
            Computation::Compare { lhs, rhs, .. }
            | Computation::Concat { lhs, rhs }
            | Computation::And { lhs, rhs } => {
                lhs.check_arith_operands(types)?;
                rhs.check_arith_operands(types)
            }
            Computation::Case {
                scrutinee,
                branches,
                else_,
            } => {
                scrutinee.check_arith_operands(types)?;
                for (match_value, result) in branches {
                    match_value.check_arith_operands(types)?;
                    result.check_arith_operands(types)?;
                }
                else_.check_arith_operands(types)
            }
            Computation::Lit(_)
            | Computation::Field(_)
            | Computation::IsDefined(_)
            | Computation::Predicate(_)
            | Computation::Script(_) => Ok(()),
        }
    }

    /// The [`FieldKind`] this computation produces, inferred from the shape and
    /// the declared types of the columns it reads.
    ///
    /// Every seat that must know a computed field's storage class reads it from
    /// HERE: the planted column's declared SQL type and the capability question
    /// "can this home persist a computed field of that kind" are two views of
    /// one answer, and deriving them separately is how they come to disagree.
    ///
    /// [`Computation::Script`] has no inferable kind — it is an opaque Rhai
    /// AST. A `computed_persisted` declaration can never carry one
    /// ([`crate::ComputedSpec::parse`] refuses it), so the error is reachable
    /// only from a `computed_live` caller, which is told so by name.
    pub fn result_kind(&self, types: &FieldTypes) -> Result<FieldKind, ResultKindUnknown> {
        Ok(match self {
            Computation::Lit(v) => match kind_of_literal(v) {
                Some(kind) => kind,
                // A bare `Null` names no storage class. Inside a `Case` arm it
                // is absorbed below; standing alone it is a field whose type
                // nothing can state.
                None => return Err(ResultKindUnknown::NullLiteral),
            },
            Computation::Field(name) => types
                .kind(name)
                .ok_or_else(|| ResultKindUnknown::UndeclaredField(name.clone()))?,
            Computation::Arith { lhs, rhs, .. } => {
                for operand in [lhs, rhs] {
                    match operand.result_kind(types) {
                        Ok(FieldKind::Numeric) | Err(ResultKindUnknown::NullLiteral) => {}
                        Ok(kind) => {
                            return Err(ResultKindUnknown::NonNumericOperand {
                                operand: format!("{operand:?}"),
                                kind,
                            });
                        }
                        Err(e) => return Err(e),
                    }
                }
                FieldKind::Numeric
            }
            Computation::Concat { .. } => FieldKind::Text,
            Computation::Compare { .. }
            | Computation::And { .. }
            | Computation::IsDefined(_)
            | Computation::Predicate(_) => FieldKind::Boolean,
            Computation::Case {
                branches, else_, ..
            } => {
                // The scrutinee is compared, never returned, so it does not
                // contribute. `Null` arms are absorbed: `if c { name } else { ()
                // }` is a nullable text column, not a type disagreement.
                let mut kind = None;
                for arm in branches.iter().map(|(_, result)| result).chain([&**else_]) {
                    let arm_kind = match arm.result_kind(types) {
                        Ok(k) => k,
                        Err(ResultKindUnknown::NullLiteral) => continue,
                        Err(e) => return Err(e),
                    };
                    match kind {
                        None => kind = Some(arm_kind),
                        Some(seen) if seen == arm_kind => {}
                        Some(seen) => {
                            return Err(ResultKindUnknown::MixedCaseArms {
                                first: seen,
                                second: arm_kind,
                            });
                        }
                    }
                }
                kind.ok_or(ResultKindUnknown::NullLiteral)?
            }
            Computation::Script(expr) => {
                return Err(ResultKindUnknown::Script {
                    source: expr.source.clone(),
                });
            }
        })
    }
}

/// The storage class of a literal. `None` for [`Value::Null`], which names
/// none, and for the shapes no planted column carries.
fn kind_of_literal(value: &Value) -> Option<FieldKind> {
    match value {
        // DateTime and Json are carried as TEXT by the SQL leg, which is what
        // the storage class asks about.
        Value::String(_) | Value::DateTime(_) | Value::Json(_) => Some(FieldKind::Text),
        Value::Integer(_) | Value::Float(_) => Some(FieldKind::Numeric),
        Value::Boolean(_) => Some(FieldKind::Boolean),
        Value::Null | Value::Array(_) | Value::Object(_) | Value::Removed(_) => None,
    }
}

/// Why [`Computation::result_kind`] could not name a storage class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultKindUnknown {
    /// The expression is a bare `()` / `null`, or a `Case` all of whose arms
    /// are.
    NullLiteral,
    /// A column the declaration reads is absent from [`FieldTypes`].
    UndeclaredField(String),
    /// Two `Case` arms produce different storage classes, so the column has no
    /// single one.
    MixedCaseArms { first: FieldKind, second: FieldKind },
    /// An opaque Rhai expression.
    Script { source: String },
    /// An arithmetic operand of another storage class. The storage has no
    /// boolean, so SQL would compute over `1`/`0` where `eval` refuses.
    NonNumericOperand { operand: String, kind: FieldKind },
}

impl fmt::Display for ResultKindUnknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullLiteral => write!(
                f,
                "the expression yields only `()`, which names no storage class"
            ),
            Self::UndeclaredField(name) => write!(
                f,
                "`{name}` is not among the declared columns, so its storage class is unknown"
            ),
            Self::MixedCaseArms { first, second } => write!(
                f,
                "the branches disagree on the storage class: {first:?} and {second:?}"
            ),
            Self::Script { source } => write!(
                f,
                "`{source}` is an opaque Rhai expression, which has no inferable storage class"
            ),
            Self::NonNumericOperand { operand, kind } => write!(
                f,
                "arithmetic takes numbers, but the operand {operand} is {kind:?}"
            ),
        }
    }
}

impl std::error::Error for ResultKindUnknown {}

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

pub(crate) fn as_bool(v: &Value, context: &str) -> Result<bool, ComputeError> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => Err(ComputeError::WrongType {
            context: context.to_string(),
            expected: "a boolean",
            value: other.clone(),
        }),
    }
}

/// [`Computation::Concat`] over two evaluated operands: a NULL operand gives
/// NULL.
pub fn concat_apply(l: &Value, r: &Value) -> Result<Value, ComputeError> {
    if *l == Value::Null || *r == Value::Null {
        return Ok(Value::Null);
    }
    Ok(Value::String(format!(
        "{}{}",
        concat_text(l, "`||` left operand")?,
        concat_text(r, "`||` right operand")?
    )))
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
/// promotes to a float result, which must be finite, and a NULL operand gives
/// NULL. See the module header.
///
/// The planted SQL column computes this same function ([`ArithOp::sql_fn`]),
/// so both seats give one value or one refusal.
pub fn arith_apply(op: ArithOp, lhs: &Value, rhs: &Value) -> Result<Value, ComputeError> {
    if *lhs == Value::Null || *rhs == Value::Null {
        return Ok(Value::Null);
    }
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
    let operand = |v: &Value, side: &str| {
        v.as_f64().ok_or_else(|| ComputeError::NotNumeric {
            context: format!("the {side} operand of `{lhs:?} {} {rhs:?}`", op.sql()),
            value: v.clone(),
        })
    };
    let a = operand(lhs, "left")?;
    let b = operand(rhs, "right")?;
    finite_float(op.apply(a, b), || format!("{a:?} {} {b:?}", op.sql()))
}

pub(crate) fn finite_float(f: f64, expr: impl FnOnce() -> String) -> Result<Value, ComputeError> {
    if f.is_finite() {
        Ok(Value::Float(f))
    } else {
        Err(ComputeError::Arithmetic {
            detail: format!("non-finite float {f} from {}", expr()),
        })
    }
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
        finite_float(result.as_float().unwrap(), || expr.source.clone())
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
    pub name: FieldIdent,
    pub computation: Computation,
}

impl DerivedField {
    pub fn new(name: FieldIdent, computation: Computation) -> Self {
        DerivedField { name, computation }
    }
}

/// A derived field that lowered to SQL and can be planted as a matview column.
/// `sql` is the inlined, parameter-free column expression.
#[derive(Debug, Clone, PartialEq)]
pub struct PlantedColumn {
    pub name: FieldIdent,
    pub sql: String,
}

impl PlantedColumn {
    /// The `(<expr>) AS "<name>"` select-list entry for this column.
    ///
    /// The alias is quoted, matching `TypeDefinition::to_create_table_sql`, so
    /// a column named with a SQL keyword (`order`, `end`) is expressible. The
    /// expression is parenthesised so a bare compound `sql` keeps its
    /// precedence against the `AS`.
    pub fn select_expr(&self) -> String {
        format!("({}) AS \"{}\"", self.sql, self.name)
    }
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
            ctx.insert(field.name.to_string(), value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
