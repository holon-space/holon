//! `hardcoded_table_name` — flags string literals whose value matches a
//! known holon matview/junction-table name. Those names must be referenced
//! via the `BLOCK_WRITE_TABLE` / `BLOCK_READ_TABLE` (etc.) constants so that
//! when a table is promoted to a matview, every construction site moves
//! together.
//!
//! Maps directly to the May 2026 LoroSyncController regression:
//! `loro_module.rs:120` constructed an `SqlOperationProvider` with
//! `table_name = "block"` after `block` had been promoted to a matview,
//! silently producing write-rejected operations.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_ast;
extern crate rustc_hir;

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_ast::LitKind;
use rustc_hir::Expr;
use rustc_hir::ExprKind;
use rustc_lint::LateContext;
use rustc_lint::LateLintPass;

dylint_linting::declare_late_lint! {
    /// ### What it does
    ///
    /// Flags `&str` / `String` literals whose value is exactly one of the
    /// holon matview or junction-table names. These should be referenced
    /// through the constants in `crates/holon/src/storage/block_table_names.rs`
    /// (`BLOCK_WRITE_TABLE`, `BLOCK_READ_TABLE`, …).
    ///
    /// ### Why is this bad?
    ///
    /// When a table is promoted to a matview (or renamed), Cargo doesn't
    /// help us — string literals are invisible to the type checker. The
    /// May 2026 regression had `SqlOperationProvider::new(..., "block")`
    /// silently rejecting writes after `block` became a matview. Going
    /// through `BLOCK_WRITE_TABLE` makes the breakage compile-time.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// SqlOperationProvider::new(deps, "block".to_string())  // <-- bug bait
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use holon::storage::block_table_names::BLOCK_WRITE_TABLE;
    /// SqlOperationProvider::new(deps, BLOCK_WRITE_TABLE.to_string())
    /// ```
    ///
    /// To suppress: `#[allow(hardcoded_table_name)]` (rare — at the
    /// definition of the table-name constants themselves, or in a test
    /// that's deliberately constructing raw SQL).
    pub HARDCODED_TABLE_NAME,
    Warn,
    "string literal is a known table/matview name; use the typed constant"
}

/// Names that must come from constants. Mirror
/// `crates/holon/src/storage/block_table_names.rs`.
const KNOWN_TABLE_NAMES: &[&str] = &[
    "block",
    "block_raw",
    "block_tags",
    "block_requires",
    "focus_roots",
];

impl<'tcx> LateLintPass<'tcx> for HardcodedTableName {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let ExprKind::Lit(lit) = expr.kind else {
            return;
        };
        let LitKind::Str(sym, _) = lit.node else {
            return;
        };
        let s = sym.as_str();
        if !KNOWN_TABLE_NAMES.contains(&s) {
            return;
        }
        span_lint_and_help(
            cx,
            HARDCODED_TABLE_NAME,
            expr.span,
            format!("string literal `\"{s}\"` is a known holon matview/junction name"),
            None,
            "reference it via the typed constant in \
             `crates/holon/src/storage/block_table_names.rs` (`BLOCK_WRITE_TABLE`, \
             `BLOCK_READ_TABLE`, …) so a future rename or matview-promotion is a compile error, \
             not a silent runtime write-rejection. See devlog/MEMORY: LoroSyncController May 2026.",
        );
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
