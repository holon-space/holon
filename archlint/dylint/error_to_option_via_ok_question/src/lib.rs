//! `error_to_option_via_ok_question` — flags `expr.ok()?` where `expr` has
//! type `Result<_, _>` and the enclosing function returns `Option<_>`.
//!
//! This is the type-aware partner to `result_to_none`. `result_to_none`
//! catches the explicit `match`-arm shape; this lint catches the chained
//! `.ok()?` shape that desugars into the same "drop the error to None"
//! anti-pattern. CLAUDE.md global: *"DO NOT returning null or None in case
//! of an error — DO throw an exception / return an Err / Failure"*.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_hir;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::ty::is_type_diagnostic_item;
use rustc_hir::{Expr, ExprKind, FnRetTy, MatchSource};
use rustc_lint::{LateContext, LateLintPass};
use rustc_span::sym;

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Flags `expr.ok()?` (and equivalent desugared `?` on `Option<T>` derived
    /// from a `Result<T, _>` via `.ok()`) where the enclosing function returns
    /// `Option<_>`.
    ///
    /// ### Why is this bad?
    ///
    /// `Result.ok()` discards the error, then `?` on the resulting `Option`
    /// propagates `None`. The caller can't distinguish "missing" from "failed
    /// for reason X". Per CLAUDE.md: *"DO NOT return null or None in case of
    /// an error."*
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// fn parse_first_int(s: &str) -> Option<i32> {
    ///     let n = s.parse::<i32>().ok()?;  // <-- error vanished here
    ///     Some(n + 1)
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// fn parse_first_int(s: &str) -> Result<i32, std::num::ParseIntError> {
    ///     let n = s.parse::<i32>()?;
    ///     Ok(n + 1)
    /// }
    /// ```
    ///
    /// To suppress: `#[allow(error_to_option_via_ok_question)]`.
    pub ERROR_TO_OPTION_VIA_OK_QUESTION,
    Warn,
    "`expr.ok()?` in an Option-returning fn silently drops the error"
}

/// Walk up the HIR until we hit the enclosing fn body and grab its return type.
fn enclosing_fn_returns_option(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    let parent_def_id = cx.tcx.hir_enclosing_body_owner(expr.hir_id);
    let node = cx.tcx.hir_node_by_def_id(parent_def_id);
    let Some(decl) = node.fn_decl() else {
        return false;
    };
    let FnRetTy::Return(ret_ty) = decl.output else {
        return false;
    };
    let rustc_hir::TyKind::Path(qpath) = ret_ty.kind else {
        return false;
    };
    let Some(def_id) = cx.qpath_res(&qpath, ret_ty.hir_id).opt_def_id() else {
        return false;
    };
    cx.tcx.is_diagnostic_item(sym::Option, def_id)
}

impl<'tcx> LateLintPass<'tcx> for ErrorToOptionViaOkQuestion {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        // The `?` operator desugars to `match` with MatchSource::TryDesugar.
        // The scrutinee of that match is the expression `?` was applied to.
        let ExprKind::Match(scrutinee, _, MatchSource::TryDesugar(_)) = expr.kind else {
            return;
        };
        // Inside the desugar, the scrutinee has the shape `Try::branch(<inner>)`
        // — we want to inspect <inner>. clippy_utils handles this:
        let inner = peel_try_desugar(scrutinee);
        // Look for `<receiver>.ok()` where the call resolves to `Result::ok`.
        let ExprKind::MethodCall(seg, receiver, args, _) = inner.kind else {
            return;
        };
        if seg.ident.as_str() != "ok" || !args.is_empty() {
            return;
        }
        let receiver_ty = cx.typeck_results().expr_ty(receiver);
        if !is_type_diagnostic_item(cx, receiver_ty, sym::Result) {
            return;
        }
        if !enclosing_fn_returns_option(cx, expr) {
            return;
        }
        span_lint_and_help(
            cx,
            ERROR_TO_OPTION_VIA_OK_QUESTION,
            expr.span,
            "`Result::ok()?` silently drops the error and propagates `None`",
            None,
            "change the enclosing fn's return type to `Result<_, _>` and \
             propagate the error with `?`, or wrap the error in a richer \
             enum that this fn's caller can distinguish from a real `None`. \
             CLAUDE.md: \"DO NOT return null or None in case of an error\".",
        );
    }
}

/// In rustc HIR, `expr?` desugars to a match on `Try::branch(scrutinee)`.
/// Strip the `Try::branch(...)` call so we can inspect the original expression.
fn peel_try_desugar<'tcx>(expr: &'tcx Expr<'tcx>) -> &'tcx Expr<'tcx> {
    if let ExprKind::Call(_, [inner]) = expr.kind {
        return inner;
    }
    expr
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
