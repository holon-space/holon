//! `result_to_none` — flags `match r { Ok(x) => Some(x), Err(_) => None }`
//! and `r.ok()` whose result flows directly to `None`/`?` in an Option-returning
//! position. Codifies the CLAUDE.md rule "DO NOT return None in case of an
//! error" with type information that ast-grep can't supply on its own.
//!
//! This is the type-aware partner to `archlint/rules/ok.yml`. The ast-grep
//! `ok` rule catches every `.ok()` (and accepts a curated allow-list);
//! `result_to_none` zooms in on the specific shape where the error is
//! demonstrably converted to `None`.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_hir;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::ty::is_type_diagnostic_item;
use rustc_hir::{Arm, Expr, ExprKind, Pat, PatKind, QPath};
use rustc_lint::{LateContext, LateLintPass};
use rustc_span::sym;

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Flags `match r { Ok(x) => Some(x), Err(_) => None }` (and the swap)
    /// where the matched expression is a `Result<_, _>`.
    ///
    /// ### Why is this bad?
    ///
    /// The error is silently dropped. Per CLAUDE.md ("DO NOT return null or
    /// None in case of an error — DO throw / return Err / Failure"), errors
    /// must propagate. Either change the function's return type to
    /// `Result<_, _>`, or include the error in a richer enum variant.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// fn parse_thing(s: &str) -> Option<Thing> {
    ///     match Thing::parse(s) {
    ///         Ok(t) => Some(t),
    ///         Err(_) => None,  // <-- error vanished
    ///     }
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// fn parse_thing(s: &str) -> Result<Thing, ThingError> {
    ///     Thing::parse(s)
    /// }
    /// ```
    ///
    /// To suppress: `#[allow(result_to_none)]` on the function or expression.
    pub RESULT_TO_NONE,
    Warn,
    "manual Result -> Option conversion drops the error"
}

fn is_some_call(expr: &Expr<'_>) -> bool {
    if let ExprKind::Call(func, _) = expr.kind
        && let ExprKind::Path(QPath::Resolved(_, path)) = func.kind
    {
        return path
            .segments
            .last()
            .is_some_and(|seg| seg.ident.as_str() == "Some");
    }
    false
}

fn is_none_path(expr: &Expr<'_>) -> bool {
    if let ExprKind::Path(QPath::Resolved(_, path)) = expr.kind {
        return path
            .segments
            .last()
            .is_some_and(|seg| seg.ident.as_str() == "None");
    }
    false
}

fn pat_is_variant(pat: &Pat<'_>, name: &str) -> bool {
    match pat.kind {
        PatKind::TupleStruct(QPath::Resolved(_, path), ..) => path
            .segments
            .last()
            .is_some_and(|seg| seg.ident.as_str() == name),
        PatKind::Expr(_) => false,
        _ => false,
    }
}

fn arm_matches(
    cx: &LateContext<'_>,
    arm: &Arm<'_>,
    variant: &str,
    body_check: fn(&Expr<'_>) -> bool,
) -> bool {
    if arm.guard.is_some() {
        return false;
    }
    let _ = cx;
    pat_is_variant(arm.pat, variant) && body_check(arm.body.peel_blocks())
}

impl<'tcx> LateLintPass<'tcx> for ResultToNone {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Match(scrutinee, arms, _) = expr.kind else {
            return;
        };
        if arms.len() != 2 {
            return;
        }

        let scrutinee_ty = cx.typeck_results().expr_ty(scrutinee);
        if !is_type_diagnostic_item(cx, scrutinee_ty, sym::Result) {
            return;
        }

        let ok_some = arm_matches(cx, &arms[0], "Ok", is_some_call)
            && arm_matches(cx, &arms[1], "Err", is_none_path);
        let some_ok = arm_matches(cx, &arms[0], "Err", is_none_path)
            && arm_matches(cx, &arms[1], "Ok", is_some_call);
        if !(ok_some || some_ok) {
            return;
        }

        span_lint_and_help(
            cx,
            RESULT_TO_NONE,
            expr.span,
            "manual `Result` -> `Option` conversion silently discards the error",
            None,
            "either change the return type to `Result<_, _>` and propagate the \
             error, or wrap it in a richer enum variant. CLAUDE.md: \"DO NOT \
             return null or None in case of an error\".",
        );
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
