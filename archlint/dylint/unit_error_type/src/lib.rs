//! `unit_error_type` — flags `Result<T, ()>`. The unit error carries no
//! diagnostic information, violating the CLAUDE.md project rule
//! *"enrich the error message with information"*. Use a real error type
//! (`anyhow::Error`, a domain-specific enum, `Box<dyn std::error::Error>`).

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_hir;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::{AmbigArg, FnRetTy, Ty, TyKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_span::sym;

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Flags type expressions of the shape `Result<_, ()>` — including
    /// function return types, `let` bindings, and type aliases.
    ///
    /// ### Why is this bad?
    ///
    /// CLAUDE.md (project): *"NEVER swallow errors!! Use Result and **enrich
    /// the error message with information**."* `Result<_, ()>` is a Result
    /// whose error variant carries zero information — equivalent to `Option`
    /// for the caller and worse for debugging.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// fn parse(s: &str) -> Result<i32, ()> { ... }   // no info on failure
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// fn parse(s: &str) -> Result<i32, ParseIntError> { ... }
    /// // or `anyhow::Result<i32>`, or a domain-specific enum
    /// ```
    ///
    /// To suppress: `#[allow(unit_error_type)]`.
    pub UNIT_ERROR_TYPE,
    Warn,
    "`Result<_, ()>` carries no error information"
}

fn is_unit_ty(ty: &Ty<'_>) -> bool {
    matches!(ty.kind, TyKind::Tup(args) if args.is_empty())
}

fn check_result_ty(cx: &LateContext<'_>, hir_ty: &Ty<'_>) {
    let TyKind::Path(qpath) = hir_ty.kind else {
        return;
    };
    let Some(def_id) = cx.qpath_res(&qpath, hir_ty.hir_id).opt_def_id() else {
        return;
    };
    if !cx.tcx.is_diagnostic_item(sym::Result, def_id) {
        return;
    }
    let rustc_hir::QPath::Resolved(_, path) = qpath else {
        return;
    };
    let Some(seg) = path.segments.last() else {
        return;
    };
    let Some(args) = seg.args else { return };
    // Result<T, E> — second generic arg is the error type.
    let err_ty = args
        .args
        .iter()
        .filter_map(|a| match a {
            rustc_hir::GenericArg::Type(t) => Some(t.as_unambig_ty()),
            _ => None,
        })
        .nth(1);
    let Some(err_ty) = err_ty else { return };
    if !is_unit_ty(err_ty) {
        return;
    }
    span_lint_and_help(
        cx,
        UNIT_ERROR_TYPE,
        hir_ty.span,
        "`Result<_, ()>` discards all error information",
        None,
        "use a real error type — a domain enum, `anyhow::Error`, or \
         `Box<dyn std::error::Error>` — so failure modes are diagnosable.",
    );
}

impl<'tcx> LateLintPass<'tcx> for UnitErrorType {
    fn check_ty(&mut self, cx: &LateContext<'tcx>, ty: &'tcx Ty<'tcx, AmbigArg>) {
        check_result_ty(cx, ty.as_unambig_ty());
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        _: rustc_hir::intravisit::FnKind<'tcx>,
        decl: &'tcx rustc_hir::FnDecl<'tcx>,
        _: &'tcx rustc_hir::Body<'tcx>,
        _: rustc_span::Span,
        _: rustc_hir::def_id::LocalDefId,
    ) {
        if let FnRetTy::Return(ret_ty) = decl.output {
            check_result_ty(cx, ret_ty);
        }
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
