//! C4 A2 — DUAL-EVAL EQUIVALENCE PBT (the correctness artifact the ruling rests
//! on). For every expression BOTH parsers accept, the typed subset parser's
//! `Computation::eval` must equal the full Rhai evaluation.
//!
//! Oracle: **differential** — Rhai (the not-under-test reference engine, reused
//! from production via `holon_expr`) is the ground truth. We generate an
//! abstract subset expression, render it to a single Rhai source string, then
//! feed that ONE string to BOTH the subset parser (→ typed `Computation`) and
//! the Rhai compiler (→ `Computation::Script`). Any disagreement in `eval` over
//! a shared context is a divergence.
//!
//! Staying in the unambiguous interior (see the property-based-testing skill's
//! generator guidance):
//!   * **mixed int/float leaves** — integer leaves render WITHOUT a decimal
//!     (`3`), float leaves WITH (`3.0`), so Rhai's int-vs-float semantics
//!     (truncating integer division, mixed promotion) are exercised. The subset
//!     evaluator mirrors this with type-faithful `Arith` (int op int stays
//!     integer; any float operand promotes). See
//!     `directed_integer_semantics_*`.
//!   * **nonzero literal divisors** only — no `x/0` NaN/inf boundary.
//!   * **integer-valued COMPARISON operands** — `Cmp` (the conditions of `if`)
//!     draws operands from `arb_int_num`, NOT `arb_num`. Rhai's float
//!     comparison operators are relative-epsilon tolerant (NOT strict IEEE;
//!     `rhai/src/func/builtin.rs` `impl_float`), so two float subexpressions
//!     that are mathematically equal but differ by ~1 ULP flip Rhai's `<=`
//!     while the subset (strict, SQL-faithful) does not. That regime's prod
//!     semantics are pending a ruling (see
//!     `directed_float_comparison_epsilon_divergence` and BugFunnel); value
//!     results are unaffected (they cross int/float freely and compare via
//!     `results_equiv`'s relative epsilon).
//!   * **small integral values** in the context — exact ops, and `switch`
//!     scrutinees land on case labels often enough to exercise both hit and
//!     `_`-default arms.
//!
//! The rendered source is fully parenthesised, so Rhai re-parses the exact tree
//! the generator built — evaluation order is identical, not merely equivalent.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
use holon_api::computation::FieldIdent;
use holon_api::expr_parser;
use holon_expr::CompiledExpr;
use holon_expr::bounded_engine;
use proptest::prelude::*;

// Integer-typed and float-typed context variables. Keeping the two kinds
// separate lets `switch` stay MONOMORPHIC (scrutinee and case labels share a
// numeric type) — the only regime where Rhai's type-strict `switch`, SQLite's
// numeric `=`, and `Computation::Case`'s strict equality all agree. Comparisons
// and mixed arithmetic may cross the two kinds freely (all three agree there).
const INT_VARS: [&str; 2] = ["i", "j"];
const FLOAT_VARS: [&str; 2] = ["x", "y"];

#[derive(Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
}

impl BinOp {
    fn sym(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CmpKind {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpKind {
    fn sym(self) -> &'static str {
        match self {
            CmpKind::Eq => "==",
            CmpKind::Ne => "!=",
            CmpKind::Lt => "<",
            CmpKind::Le => "<=",
            CmpKind::Gt => ">",
            CmpKind::Ge => ">=",
        }
    }
}

#[derive(Debug, Clone)]
struct Cmp {
    op: CmpKind,
    lhs: G,
    rhs: G,
}

/// An abstract subset expression. Only rendered (never evaluated directly): the
/// rendered source is the single canonical form fed to both engines. Integer
/// leaves render WITHOUT a decimal (`3`), float leaves WITH (`3.0`), so Rhai's
/// int-vs-float semantics (integer division, mixed promotion) are exercised.
#[derive(Debug, Clone)]
enum G {
    IntLit(i64),
    FloatLit(u32),
    IntVar(usize),
    FloatVar(usize),
    Bin(BinOp, Box<G>, Box<G>),
    /// `expr / <nonzero int literal>` — int/int is integer division.
    DivInt(Box<G>, i64),
    /// `expr / <nonzero float literal>` — always float division.
    DivFloat(Box<G>, u32),
    /// `if c0 {b0} else if c1 {b1} … else {e}` — 1..=2 conditional branches.
    If(Vec<(Cmp, G)>, Box<G>),
    /// `switch i { m0 => r0, … , _ => e }` — int scrutinee var + int labels.
    SwitchInt(usize, Vec<(i64, G)>, Box<G>),
    /// `switch x { m0.0 => r0, … , _ => e }` — float scrutinee var + float
    /// labels.
    SwitchFloat(usize, Vec<(u32, G)>, Box<G>),
}

fn render(g: &G) -> String {
    match g {
        G::IntLit(n) => format!("{n}"),
        G::FloatLit(n) => format!("{n}.0"),
        G::IntVar(i) => INT_VARS[*i].to_string(),
        G::FloatVar(i) => FLOAT_VARS[*i].to_string(),
        G::Bin(op, l, r) => format!("({} {} {})", render(l), op.sym(), render(r)),
        G::DivInt(l, d) => format!("({} / {d})", render(l)),
        G::DivFloat(l, d) => format!("({} / {d}.0)", render(l)),
        G::If(branches, else_) => {
            let mut out = String::new();
            for (i, (cmp, body)) in branches.iter().enumerate() {
                if i == 0 {
                    out.push_str("if ");
                } else {
                    out.push_str(" else if ");
                }
                out.push_str(&format!(
                    "({} {} {}) {{ {} }}",
                    render(&cmp.lhs),
                    cmp.op.sym(),
                    render(&cmp.rhs),
                    render(body)
                ));
            }
            out.push_str(&format!(" else {{ {} }}", render(else_)));
            out
        }
        G::SwitchInt(v, arms, else_) => render_switch(INT_VARS[*v], arms, else_, false),
        G::SwitchFloat(v, arms, else_) => render_switch(FLOAT_VARS[*v], arms, else_, true),
    }
}

/// Render a switch, deduping labels (both Rhai and the subset parser reject
/// duplicate cases). `float_labels` decides label syntax (`3` vs `3.0`).
fn render_switch<L: Copy + std::fmt::Display + std::hash::Hash + Eq>(
    scrutinee: &str,
    arms: &[(L, G)],
    else_: &G,
    float_labels: bool,
) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut parts = Vec::new();
    for (m, r) in arms {
        if seen.insert(*m) {
            let label = if float_labels {
                format!("{m}.0")
            } else {
                format!("{m}")
            };
            parts.push(format!("{label} => {}", render(r)));
        }
    }
    format!(
        "switch {scrutinee} {{ {}, _ => {} }}",
        parts.join(", "),
        render(else_)
    )
}

fn binop() -> impl Strategy<Value = BinOp> {
    prop_oneof![Just(BinOp::Add), Just(BinOp::Sub), Just(BinOp::Mul)]
}

fn cmpkind() -> impl Strategy<Value = CmpKind> {
    prop_oneof![
        Just(CmpKind::Eq),
        Just(CmpKind::Ne),
        Just(CmpKind::Lt),
        Just(CmpKind::Le),
        Just(CmpKind::Gt),
        Just(CmpKind::Ge),
    ]
}

/// Numeric-only expressions (no `if`/`switch`) of mixed int/float shape. These
/// are the ONLY things the subset grammar admits as arithmetic/comparison
/// operands — a conditional in operand position (`(if … ) / x`) is outside the
/// subset (Rhai accepts it, we fall back). Divisors are nonzero literals so no
/// division-by-zero arises.
fn arb_num() -> impl Strategy<Value = G> {
    let leaf = prop_oneof![
        (0i64..=5).prop_map(G::IntLit),
        (0u32..=5).prop_map(G::FloatLit),
        (0usize..INT_VARS.len()).prop_map(G::IntVar),
        (0usize..FLOAT_VARS.len()).prop_map(G::FloatVar),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            (binop(), inner.clone(), inner.clone()).prop_map(|(op, l, r)| G::Bin(
                op,
                Box::new(l),
                Box::new(r)
            )),
            (inner.clone(), 1i64..=5).prop_map(|(l, d)| G::DivInt(Box::new(l), d)),
            (inner, 1u32..=5).prop_map(|(l, d)| G::DivFloat(Box::new(l), d)),
        ]
    })
}

/// Integer-VALUED numeric expressions: integer leaves and the integer-closed
/// operators (`+`/`-`/`*` and truncating integer division). No floats, no
/// `DivFloat` — so every value is an exact `i64` and comparisons over these are
/// exact in ALL THREE engines (subset, SQLite, Rhai).
///
/// Used ONLY for comparison operands (`Cmp`), NOT for arithmetic value
/// positions. Rhai's float comparison operators are relative-epsilon tolerant
/// (see [`directed_float_comparison_epsilon_divergence`]), so comparing two
/// float subexpressions that differ by ~1 ULP flips Rhai's result but not the
/// subset's (strict, SQL-faithful) — a divergence in a regime whose prod
/// semantics are pending a ruling. Restricting comparison operands to exact
/// integers keeps the differential property inside the interior where the
/// engines provably agree, WITHOUT weakening the oracle for value results
/// (those still cross int/float freely and compare via `results_equiv`).
fn arb_int_num() -> impl Strategy<Value = G> {
    let leaf = prop_oneof![
        (0i64..=5).prop_map(G::IntLit),
        (0usize..INT_VARS.len()).prop_map(G::IntVar),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            (binop(), inner.clone(), inner.clone()).prop_map(|(op, l, r)| G::Bin(
                op,
                Box::new(l),
                Box::new(r)
            )),
            (inner, 1i64..=5).prop_map(|(l, d)| G::DivInt(Box::new(l), d)),
        ]
    })
}

/// Any subset expression: a numeric expr, or an `if`/`switch` whose *value
/// positions* (branch bodies, arm results, else) recurse into any expression —
/// so conditionals nest — while conditions and arithmetic operands stay
/// numeric-only. Each `switch` is monomorphic (int scrutinee+int labels, or
/// float+float).
fn arb_expr() -> impl Strategy<Value = G> {
    arb_num().prop_recursive(3, 40, 4, |value| {
        // Comparison operands are integer-VALUED (exact in all three engines);
        // see `arb_int_num`. Rhai's epsilon-tolerant float comparison would
        // otherwise flip near-ULP-tie float comparisons (pending ruling).
        let cmp = (cmpkind(), arb_int_num(), arb_int_num()).prop_map(|(op, lhs, rhs)| Cmp {
            op,
            lhs,
            rhs,
        });
        prop_oneof![
            (
                prop::collection::vec((cmp, value.clone()), 1..=2),
                value.clone()
            )
                .prop_map(|(branches, e)| G::If(branches, Box::new(e))),
            (
                0usize..INT_VARS.len(),
                prop::collection::vec((1i64..=4, value.clone()), 1..=3),
                value.clone()
            )
                .prop_map(|(v, arms, e)| G::SwitchInt(v, arms, Box::new(e))),
            (
                0usize..FLOAT_VARS.len(),
                prop::collection::vec((1u32..=4, value.clone()), 1..=3),
                value
            )
                .prop_map(|(v, arms, e)| G::SwitchFloat(v, arms, Box::new(e))),
        ]
    })
}

/// Build a shared context binding `i,j` to Integers and `x,y` to Floats.
fn build_ctx(vals: [u32; 4]) -> HashMap<String, Value> {
    let mut ctx = HashMap::new();
    ctx.insert("i".to_string(), Value::Integer((vals[0] % 6) as i64));
    ctx.insert("j".to_string(), Value::Integer((vals[1] % 6) as i64));
    ctx.insert("x".to_string(), Value::Float((vals[2] % 6) as f64));
    ctx.insert("y".to_string(), Value::Float((vals[3] % 6) as f64));
    ctx
}

/// Compare two `eval` results. Both must be `Ok`; numeric values compare within
/// a tight relative epsilon (identical IEEE ops, so this is near-exact),
/// booleans exactly.
fn results_equiv(
    a: &Result<Value, holon_api::computation::ComputeError>,
    b: &Result<Value, holon_api::computation::ComputeError>,
) -> bool {
    match (a, b) {
        (Ok(x), Ok(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(fx), Some(fy)) => (fx - fy).abs() <= 1e-9 * (1.0 + fx.abs().max(fy.abs())),
            _ => x == y,
        },
        _ => false,
    }
}

/// DIRECTED regression for the **float-comparison epsilon divergence** — the
/// reason `Cmp` operands are restricted to integer-valued arithmetic (see
/// [`arb_int_num`] and the module header).
///
/// Rhai's `<`/`<=`/`>`/`>=`/`==`/`!=` on floats are **relative-epsilon
/// tolerant** (`rhai-1.25.1/src/func/builtin.rs` `impl_float`: e.g. `<=` is
/// `(y - x)/max > -FLOAT::EPSILON`), NOT strict IEEE. The subset evaluator
/// ([`crate::computation::CmpOp::apply`] / `values_match`) is strict — matching
/// SQLite, the SQL-lowering target that seat A exists to mirror. So when two
/// float operands are mathematically equal but computed via different rounding
/// paths (differ by ~1 ULP), the two engines pick different branches.
///
/// This case is FROZEN as the canonical demonstration: both engines compute the
/// two operands to **bit-identical** f64 (LHS `0.4`, RHS `0.4 - 1 ULP`), yet
/// Rhai's `<=` reports `true` (within epsilon) while the subset (and SQLite)
/// reports `false`. Neither arithmetic side is wrong; the divergence lives
/// entirely in Rhai's non-strict comparison operator. Prod semantics for this
/// float-comparison regime are PENDING A RULING (see BugFunnel; whether prod
/// `bounded_engine()` Rhai should be made strict to match SQL-lowered derived
/// fields). Until then the equivalence property deliberately does not assert
/// agreement here.
#[test]
fn directed_float_comparison_epsilon_divergence() {
    let engine = bounded_engine();
    let mut ctx = HashMap::new();
    ctx.insert("x".to_string(), Value::Float(0.0));
    ctx.insert("y".to_string(), Value::Float(2.0));

    let src = "(((y / 5) / 1.0) <= (((x + 2) - (4 / 5.0)) / 3))";
    let subset = expr_parser::parse(src).unwrap().eval(&ctx).unwrap();
    let rhai = Computation::Script(CompiledExpr::compile(&engine, src).unwrap())
        .eval(&ctx)
        .unwrap();

    // Subset (== SQLite) is strict IEEE: 0.4 <= (0.4 - 1 ULP) is false.
    assert_eq!(
        subset,
        Value::Boolean(false),
        "subset must be strict IEEE (SQL-faithful)"
    );
    // Rhai is epsilon-tolerant: treats the two near-equal floats as equal, so
    // `<=` holds. This is the documented, out-of-spec-for-our-DSL behavior.
    assert_eq!(
        rhai,
        Value::Boolean(true),
        "Rhai comparison is relative-epsilon tolerant (FLOAT::EPSILON)"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn subset_eval_equals_rhai(expr in arb_expr(), vals in any::<[u32; 4]>()) {
        let src = render(&expr);
        let ctx = build_ctx(vals);

        let comp = expr_parser::parse(&src)
            .unwrap_or_else(|e| panic!("subset parser rejected its own render `{src}`: {e}"));
        let engine = bounded_engine();
        let script = CompiledExpr::compile(&engine, &src)
            .unwrap_or_else(|e| panic!("Rhai rejected `{src}`: {e}"));

        let subset_val = comp.eval(&ctx);
        let rhai_val = Computation::Script(script).eval(&ctx);

        prop_assert!(
            results_equiv(&subset_val, &rhai_val),
            "divergence on `{src}` ctx={ctx:?}: subset={subset_val:?} rhai={rhai_val:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// I3-0: string literals, `Concat`, `&&`, `is_def_var`, `()`.
// ---------------------------------------------------------------------------

const STR_VARS: [&str; 2] = ["s", "t"];

/// A text-valued expression: a leading string LITERAL followed by a
/// left-associated chain of further literals and string-bound context vars.
///
/// The leading literal is not decoration — it is the interior boundary. The
/// subset decides `+` syntactically (`is_string_typed`), so text-ness has to
/// enter the chain at the leftmost operand and propagate rightwards through the
/// left-associated `Concat`s. A var-only prefix (`s + t`) is arithmetic to the
/// subset and concatenation to Rhai; that divergence is real and pinned by
/// `concat_of_two_untyped_fields_diverges_from_rhai`, so the generator stays
/// out of it rather than rediscovering it 512 times.
///
/// Mixed-type and NULL operands are likewise not generated — see
/// `directed_concat_null_and_coercion_semantics`.
fn arb_str_expr() -> impl Strategy<Value = String> {
    let lit = prop_oneof![
        Just("\"\"".to_string()),
        Just("\" — \"".to_string()),
        Just("\"x\"".to_string()),
        Just("\"a b\"".to_string()),
    ];
    let leaf = prop_oneof![
        lit.clone(),
        (0usize..STR_VARS.len()).prop_map(|i| STR_VARS[i].to_string()),
    ];
    (lit, prop::collection::vec(leaf, 0..4)).prop_map(|(head, rest)| {
        rest.into_iter()
            .fold(head, |acc, leaf| format!("({acc} + {leaf})"))
    })
}

fn str_ctx() -> HashMap<String, Value> {
    HashMap::from([
        ("s".to_string(), Value::String("Ada".to_string())),
        ("t".to_string(), Value::String("Löve — ✓".to_string())),
    ])
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Concatenation over the agreed interior: subset `Concat` == Rhai `+`,
    /// multibyte content included.
    #[test]
    fn subset_concat_equals_rhai(src in arb_str_expr()) {
        let ctx = str_ctx();
        let comp = expr_parser::parse(&src)
            .unwrap_or_else(|e| panic!("subset parser rejected `{src}`: {e}"));
        prop_assert!(
            !matches!(comp, Computation::Script(_)),
            "`{src}` must be a typed computation"
        );
        let engine = bounded_engine();
        let script = CompiledExpr::compile(&engine, &src)
            .unwrap_or_else(|e| panic!("Rhai rejected `{src}`: {e}"));

        let subset_val = comp.eval(&ctx);
        let rhai_val = Computation::Script(script).eval(&ctx);
        prop_assert_eq!(
            format!("{subset_val:?}"),
            format!("{rhai_val:?}"),
            "concat divergence on `{}`", src
        );
    }
}

/// The two regimes where `Concat` deliberately does NOT follow Rhai, because
/// its lowering target is SQLite `||` and the planted column is what production
/// reads.
///
/// 1. **NULL propagates.** `x || NULL` is NULL in SQL; Rhai's `+` on `()`
///    raises. `Concat` returns [`Value::Null`] — SQL-faithful.
/// 2. **Numeric operands are coerced to text**, using the same rendering
///    `SqlFragment::inline_sql` plants (`{:?}` for floats, so `1.0` keeps its
///    decimal point exactly as SQLite prints a REAL). Rhai renders an f64 with
///    `Display` (`1`), which is the divergence this test freezes.
///
/// The SQL half of both claims is checked against a real engine in
/// `holon-turso/tests/derived_field_eval_vs_sql.rs`.
#[test]
fn directed_concat_null_and_coercion_semantics() {
    let cases: &[(&str, HashMap<String, Value>, Value)] = &[
        (
            r#"role + " — " + email"#,
            HashMap::from([
                ("role".to_string(), Value::Null),
                ("email".to_string(), Value::String("a@b".into())),
            ]),
            Value::Null,
        ),
        (
            r#""n=" + n"#,
            HashMap::from([("n".to_string(), Value::Integer(1))]),
            Value::String("n=1".into()),
        ),
        (
            r#""f=" + f"#,
            HashMap::from([("f".to_string(), Value::Float(1.0))]),
            Value::String("f=1.0".into()),
        ),
        (
            r#""f=" + f"#,
            HashMap::from([("f".to_string(), Value::Float(2.5))]),
            Value::String("f=2.5".into()),
        ),
        (
            r#""b=" + b"#,
            HashMap::from([("b".to_string(), Value::Boolean(true))]),
            Value::String("b=1".into()),
        ),
    ];
    for (src, ctx, expected) in cases {
        let got = expr_parser::parse(src)
            .unwrap_or_else(|e| panic!("must parse `{src}`: {e}"))
            .eval(ctx)
            .unwrap_or_else(|e| panic!("`{src}` must evaluate: {e}"));
        assert_eq!(&got, expected, "`{src}` over {ctx:?}");
    }
}

/// `&&` must short-circuit, or the definedness guard raises the very
/// `MissingField` it exists to prevent. This is the load-bearing property of
/// the acceptance case, not a nicety.
#[test]
fn logical_and_short_circuits_like_rhai() {
    let src = r#"is_def_var("role") && role != ()"#;
    let comp = expr_parser::parse(src).unwrap();
    let engine = bounded_engine();
    let script = Computation::Script(CompiledExpr::compile(&engine, src).unwrap());

    let absent: HashMap<String, Value> = HashMap::new();
    assert_eq!(comp.eval(&absent).unwrap(), Value::Boolean(false));
    assert_eq!(script.eval(&absent).unwrap(), Value::Boolean(false));

    let null = HashMap::from([("role".to_string(), Value::Null)]);
    assert_eq!(comp.eval(&null).unwrap(), Value::Boolean(false));

    let present = HashMap::from([("role".to_string(), Value::String("CTO".into()))]);
    assert_eq!(comp.eval(&present).unwrap(), Value::Boolean(true));
    assert_eq!(script.eval(&present).unwrap(), Value::Boolean(true));
}

/// The acceptance case, evaluated over the three `role` states, against Rhai.
#[test]
fn display_name_matches_rhai_on_every_role_state() {
    let src = r#"if is_def_var("role") && role != () { role + " — " + email } else { email }"#;
    let comp = expr_parser::parse(src).unwrap();
    let engine = bounded_engine();
    let script = Computation::Script(CompiledExpr::compile(&engine, src).unwrap());

    let email = Value::String("ada@x".to_string());
    let absent = HashMap::from([("email".to_string(), email.clone())]);
    let present = HashMap::from([
        ("email".to_string(), email.clone()),
        ("role".to_string(), Value::String("CTO".into())),
    ]);
    for ctx in [&absent, &present] {
        assert_eq!(
            comp.eval(ctx).unwrap(),
            script.eval(ctx).unwrap(),
            "display_name divergence over {ctx:?}"
        );
    }
    assert_eq!(comp.eval(&absent).unwrap(), email);
    assert_eq!(
        comp.eval(&present).unwrap(),
        Value::String("CTO — ada@x".to_string())
    );
    // Rhai binds no `()`-valued variable through a `Scope` of `Value`s, so the
    // null state has no Rhai counterpart — the subset leg alone pins it.
    let null_role = HashMap::from([
        ("email".to_string(), email.clone()),
        ("role".to_string(), Value::Null),
    ]);
    assert_eq!(comp.eval(&null_role).unwrap(), email);
}

/// Widening the grammar moves shipped expressions from seat B to seat A, so the
/// ones that newly parse must be checked, not assumed.
///
/// `block_profile.yaml`'s `bullet_shape` is the one shipped computed field the
/// I3-0 shapes newly admit (`!= ()`, `&&`, string-literal results); the rest
/// still reject on `||`, `.`-method calls or non-`is_def_var` call forms and
/// keep falling back to Rhai. Its three `collapsed` states must agree with
/// Rhai.
#[test]
fn newly_parseable_shipped_bullet_shape_matches_rhai() {
    let src = r#"if collapsed != () && collapsed != 0 { "orgmode" } else { "circle" }"#;
    let comp = expr_parser::parse(src).expect("now in the subset");
    let script = Computation::Script(CompiledExpr::compile(&bounded_engine(), src).unwrap());
    comp.compile_sql().expect("and it must lower to SQL");

    for (collapsed, expected) in [
        (Value::Integer(1), "orgmode"),
        (Value::Integer(0), "circle"),
        (Value::Null, "circle"),
    ] {
        let ctx = HashMap::from([("collapsed".to_string(), collapsed.clone())]);
        assert_eq!(
            comp.eval(&ctx).unwrap(),
            Value::String(expected.to_string()),
            "bullet_shape over collapsed={collapsed:?}"
        );
        // Rhai binds no unit through a `Value` scope, so it checks the two
        // numeric states.
        if collapsed != Value::Null {
            assert_eq!(comp.eval(&ctx).unwrap(), script.eval(&ctx).unwrap());
        }
    }
}

/// KNOWN LIMITATION, found by the differential oracle (counterexample `s + s`):
/// `+` over two operands that are neither provably text nor provably numeric
/// stays arithmetic and fails LOUD, where Rhai — which knows the runtime types
/// — concatenates.
///
/// The subset cannot decide this without the declared column types, and this
/// lane deliberately does not route the type registry through `Computation`
/// (that is I3-1). Until it does, a text-joining field must carry a string
/// literal — every realistic display field carries a separator anyway. Failing
/// loud is third in the error-handling order; silently taking SQLite's
/// `+`-on-TEXT coercion (which yields `0`) would be fourth.
#[test]
fn concat_of_two_untyped_fields_diverges_from_rhai() {
    let comp = expr_parser::parse("a + b").unwrap();
    assert!(matches!(comp, Computation::Arith { .. }));
    let ctx = HashMap::from([
        ("a".to_string(), Value::String("x".into())),
        ("b".to_string(), Value::String("y".into())),
    ]);
    let err = comp
        .eval(&ctx)
        .expect_err("must fail loud, not concatenate");
    assert!(
        matches!(err, holon_api::computation::ComputeError::NotNumeric { .. }),
        "expected NotNumeric, got {err:?}"
    );
    // Rhai, knowing the runtime types, succeeds — the divergence, frozen.
    let rhai = Computation::Script(CompiledExpr::compile(&bounded_engine(), "a + b").unwrap())
        .eval(&ctx)
        .expect("Rhai concatenates");
    assert_eq!(rhai, Value::String("xy".to_string()));
}

/// KNOWN LIMITATION, the MIRROR of
/// `concat_of_two_untyped_fields_diverges_from_rhai`: where that one has syntax
/// say numeric and a string flow (loud `NotNumeric`), here syntax says TEXT and
/// a number flows — and the subset silently concatenates.
///
/// `is_string_typed` reads a `Case` as text when any arm is a string literal,
/// so `(if c {"x"} else {y}) + z` is `Concat` regardless of which arm the
/// scrutinee actually selects. With `c = false` the value that flows is `y`, a
/// number, and `Concat` renders it: `"12"`, where Rhai — which sees the runtime
/// types — adds to `3`. The SQL leg agrees with the subset, not with Rhai
/// (`iif(0,'x',1) || 2` is `'12'`), so this is a subset-vs-Rhai divergence, not
/// an eval-vs-SQL one: the two seats stay consistent with each other.
///
/// Not fixable without the declared field types — the same I3-1 gap. It is
/// pinned rather than made loud because a `Concat` over a number is
/// well-defined in the seat that matters (the planted column); only the Rhai
/// reading differs.
#[test]
fn case_armed_concat_of_a_number_diverges_from_rhai() {
    let src = r#"(if c { "x" } else { y }) + z"#;
    let comp = expr_parser::parse(src).unwrap();
    assert!(matches!(comp, Computation::Concat { .. }));
    let ctx = HashMap::from([
        ("c".to_string(), Value::Boolean(false)),
        ("y".to_string(), Value::Integer(1)),
        ("z".to_string(), Value::Integer(2)),
    ]);
    assert_eq!(
        comp.eval(&ctx).expect("subset concatenates"),
        Value::String("12".to_string())
    );
    let rhai = Computation::Script(CompiledExpr::compile(&bounded_engine(), src).unwrap())
        .eval(&ctx)
        .expect("Rhai adds");
    assert_eq!(rhai, Value::Integer(3));
}

/// The THIRD Rhai-divergence axis for `Concat` (after NULL and float): a
/// boolean operand renders SQL-style `1`/`0`, not Rhai's `true`/`false`.
///
/// `concat_text` is deliberately SQLite-faithful — the planted column is what
/// production reads, and SQLite has no boolean type — so this follows from the
/// same rule that decided the NULL and float axes. Pinned against Rhai here so
/// the axis is not silent; pinned against `eval` in
/// `directed_concat_null_and_coercion_semantics`.
#[test]
fn concat_of_a_boolean_renders_sql_style_not_rhai_style() {
    let src = r#"("a" + b) + c"#;
    let comp = expr_parser::parse(src).unwrap();
    let ctx = HashMap::from([
        ("b".to_string(), Value::Integer(1)),
        ("c".to_string(), Value::Boolean(false)),
    ]);
    assert_eq!(
        comp.eval(&ctx).expect("subset concatenates"),
        Value::String("a10".to_string()),
        "boolean renders as SQLite's 0"
    );
    let rhai = Computation::Script(CompiledExpr::compile(&bounded_engine(), src).unwrap())
        .eval(&ctx)
        .expect("Rhai concatenates");
    assert_eq!(rhai, Value::String("a1false".to_string()));
}

/// F4 — the two seats must AGREE on a non-boolean `&&` operand, and they do so
/// by BOTH refusing it.
///
/// `eval` raises `WrongType`; the lowering `(… AND n)` would be read truthily
/// by SQLite (`1 AND 5` is `1`), so seat A raising where seat B yields a value
/// is exactly the eval/SQL disagreement the dual oracle exists to forbid. An
/// operand with no boolean evidence — neither its syntax nor a declared
/// BOOLEAN column type — therefore leaves the subset and falls back to Rhai,
/// which raises on it too.
#[test]
fn a_non_boolean_and_operand_is_refused_by_both_seats() {
    let src = r#"is_def_var("n") && n"#;
    let err = expr_parser::parse(src).expect_err("must leave the subset");
    assert!(
        err.message.contains("is not boolean"),
        "error must name the constraint: {}",
        err.message
    );

    // Seat B, the fallback: Rhai compiles it but raises at evaluation, so no
    // seat silently produces a value.
    let ctx = HashMap::from([("n".to_string(), Value::Integer(5))]);
    let rhai =
        Computation::Script(CompiledExpr::compile(&bounded_engine(), src).unwrap()).eval(&ctx);
    assert!(rhai.is_err(), "Rhai must raise too, got {rhai:?}");

    // The boolean-by-syntax shapes the acceptance case relies on still parse.
    for ok in [
        r#"is_def_var("role") && role != ()"#,
        "collapsed != () && collapsed != 0",
        r#"a == 1 && b == 2 && c == 3"#,
    ] {
        expr_parser::parse(ok).unwrap_or_else(|e| panic!("`{ok}` must stay in the subset: {e}"));
    }
}

/// The verifier's two refutation counterexamples, pinned as directed regression
/// cases, plus mixed and whole-float division. Confirms subset eval == Rhai for
/// the integer-semantics class the float-only generator structurally missed.
#[test]
fn directed_integer_semantics_regressions() {
    let engine = bounded_engine();
    // (source, empty-ctx). These are literal-only expressions.
    let cases = [
        "5 / 2",
        "9 / 4 + 1",
        "5 / 2.0",
        "3.0 / 2.0",
        "-5 / 2",
        "7 / 2 * 3",
    ];
    let ctx: HashMap<String, Value> = HashMap::new();
    for src in cases {
        let comp =
            expr_parser::parse(src).unwrap_or_else(|e| panic!("subset must parse `{src}`: {e}"));
        let script = CompiledExpr::compile(&engine, src)
            .unwrap_or_else(|e| panic!("Rhai must compile `{src}`: {e}"));
        let subset = comp.eval(&ctx);
        let rhai = Computation::Script(script).eval(&ctx);
        assert!(
            results_equiv(&subset, &rhai),
            "`{src}`: subset={subset:?} rhai={rhai:?}"
        );
    }
    // Pin the exact refuted values so a regression is unmistakable.
    assert_eq!(
        expr_parser::parse("5 / 2").unwrap().eval(&ctx).unwrap(),
        Value::Integer(2)
    );
    assert_eq!(
        expr_parser::parse("3.0 / 2.0").unwrap().eval(&ctx).unwrap(),
        Value::Float(1.5)
    );
}

// ---------------------------------------------------------------------------
// Flagship: the default petri computed props must now PLANT (seat A).
// ---------------------------------------------------------------------------

/// The four default petri computed-prop expressions (verbatim, sans leading
/// `=`).
const DEFAULT_PETRI_PROPS: &[(&str, &str)] = &[
    (
        "priority_weight",
        "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }",
    ),
    (
        "urgency_weight",
        "if days_to_deadline > deadline_buffer_days { 0.0 } else if days_to_deadline <= 0.0 { \
         deadline_penalty } else { deadline_penalty * (1.0 - days_to_deadline / \
         deadline_buffer_days) }",
    ),
    ("position_weight", "0.001 * (max_position - position)"),
    (
        "task_weight",
        "priority_weight * (1.0 + urgency_weight) + position_weight",
    ),
];

#[test]
fn default_petri_props_parse_and_plant_to_seat_a() {
    for (name, src) in DEFAULT_PETRI_PROPS {
        let comp = expr_parser::parse(src)
            .unwrap_or_else(|e| panic!("{name}: subset parser must accept default prop: {e}"));
        assert!(
            !matches!(comp, Computation::Script(_)),
            "{name} must be a TYPED computation, not a Rhai Script"
        );
        comp.compile_sql()
            .unwrap_or_else(|e| panic!("{name}: compile_sql must succeed: {e}"));

        let plan = DerivedFieldPlan::plan(vec![DerivedField::new(
            FieldIdent::parse(name).expect("identifier"),
            comp,
        )]);
        assert_eq!(
            plan.sql_planted.len(),
            1,
            "{name} must be SQL-planted (seat A); stage={:?}",
            plan.stage_evaluated
        );
        assert!(
            plan.stage_evaluated.is_empty(),
            "{name} must NOT fall to the projection stage"
        );
    }
}

#[test]
fn flagship_switch_and_if_defaults_match_rhai() {
    // The two conditional flagship props evaluated both ways over sample inputs.
    let engine = bounded_engine();
    let cases: &[(&str, HashMap<String, Value>)] = &[
        (
            "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }",
            HashMap::from([("priority".to_string(), Value::Float(2.0))]),
        ),
        (
            "if days_to_deadline > deadline_buffer_days { 0.0 } else if days_to_deadline <= 0.0 { \
             deadline_penalty } else { deadline_penalty * (1.0 - days_to_deadline / \
             deadline_buffer_days) }",
            HashMap::from([
                ("days_to_deadline".to_string(), Value::Float(1.0)),
                ("deadline_buffer_days".to_string(), Value::Float(3.0)),
                ("deadline_penalty".to_string(), Value::Float(200.0)),
            ]),
        ),
    ];
    for (src, ctx) in cases {
        let comp = expr_parser::parse(src).unwrap();
        let script = CompiledExpr::compile(&engine, *src).unwrap();
        let subset = comp.eval(ctx);
        let rhai = Computation::Script(script).eval(ctx);
        assert!(
            results_equiv(&subset, &rhai),
            "flagship `{src}`: subset={subset:?} rhai={rhai:?}"
        );
    }
}
