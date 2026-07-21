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
//! The rendered source is fully parenthesised, so Rhai re-parses the exact tree
//! the generator built — evaluation order is identical, not merely equivalent.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
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
    #![proptest_config(ProptestConfig::with_cases(512))]

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

        let plan = DerivedFieldPlan::plan(vec![DerivedField::new(*name, comp)]);
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
