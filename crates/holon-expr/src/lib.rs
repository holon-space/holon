//! @c4 component
//! @c4 layer Engine
//! Pattern: Interpreter
//!
//! Compiled Rhai expressions — the shared vocabulary between holon-api entity
//! definitions and the holon-engine Petri-net guard evaluator.

use std::collections::BTreeSet;

use rhai::AST;
use rhai::ASTNode;
use rhai::Engine;
use rhai::Expr;
use rhai::Stmt;
use serde::Deserialize;
use serde::Serialize;

/// Hard cap on Rhai VM operations per evaluation. Holon evaluators run
/// expressions that come from VAULT DATA (computed prototype properties,
/// objective terms), so an unbounded engine turns a stored
/// `task_weight: "= while true {}"` into a permanent hang of the calling
/// tool (e.g. the live `rank_tasks` MCP tool). 1M ops is orders of magnitude
/// beyond any legitimate property/objective expression yet bounded in wall
/// time; hitting it aborts the eval with a Rhai `ErrorTooManyOperations`,
/// which every caller already surfaces as an `Err` (fail-loud).
pub const MAX_RHAI_OPERATIONS: u64 = 1_000_000;

/// A `rhai::Engine` with execution bounds set. ALL engines that evaluate
/// user/vault-derived expressions must be built through this — a bare
/// `Engine::new()` is only acceptable for compile-only use.
pub fn bounded_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_RHAI_OPERATIONS);
    engine.set_max_expr_depths(64, 64);
    engine
}

/// A pre-compiled Rhai expression: source kept for debugging, AST for
/// evaluation.
///
/// Serde: serializes as the source string, deserializes by compiling.
/// Deserialization fails loudly if the expression doesn't compile (parse
/// boundary).
#[derive(Clone)]
pub struct CompiledExpr {
    pub source: String,
    pub ast: AST,
    /// The free (unbound) variable names this expression reads — the columns a
    /// row must carry for the expression to evaluate. Derived from the AST at
    /// compile time (see [`free_variables`]), minus names the expression binds
    /// itself (`let`) or explicitly guards (`is_def_var("x")`).
    ///
    /// This is the substrate for *type-aware binding*: at evaluation, a row
    /// missing one of these columns is the STRUCTURALLY-wrong shape for the
    /// expression (a typed non-match), distinguished from a genuine runtime
    /// error on columns that ARE present. It is CONTEXT-FREE — it does not
    /// subtract the names of sibling computed fields (those are resolved by the
    /// evaluator against the live scope, since a referenced name absent from
    /// scope — whether a data column or an unbound prior field — is uniformly
    /// "unbound"). Registered functions and method/property names are never
    /// variables, so they never appear here.
    pub required_columns: BTreeSet<String>,
}

impl std::fmt::Debug for CompiledExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledExpr")
            .field("source", &self.source)
            .finish()
    }
}

impl PartialEq for CompiledExpr {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl std::fmt::Display for CompiledExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

impl Serialize for CompiledExpr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for CompiledExpr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = String::deserialize(deserializer)?;
        Self::compile(&bounded_engine(), &source).map_err(serde::de::Error::custom)
    }
}

impl CompiledExpr {
    pub fn compile(engine: &Engine, source: impl Into<String>) -> Result<Self, String> {
        let raw = source.into();
        // Strip optional leading `=` (org-file convention for Rhai expressions).
        let source = match raw.strip_prefix('=') {
            Some(rest) => rest.trim().to_string(),
            None => raw,
        };
        let ast = engine
            .compile(&source)
            .map_err(|e| format!("Rhai compile error for '{source}': {e}"))?;
        let required_columns = required_columns(&ast);
        Ok(CompiledExpr {
            source,
            ast,
            required_columns,
        })
    }
}

// ---------------------------------------------------------------------------
// AST variable introspection (rhai `internals` feature)
// ---------------------------------------------------------------------------

/// The HARD required-column set of a compiled expression: the free variables it
/// reads, minus names it binds itself (`let`/`const`) and minus names it guards
/// with `is_def_var("x")`.
///
/// A row missing any of these columns is structurally the wrong shape for the
/// expression → the evaluator treats the expression as *unbound* (a typed
/// non-match) WITHOUT invoking rhai, so no "Variable not found" error is ever
/// raised for the expected-heterogeneous case.
///
/// **Soundness / fail-closed.** The `let`-subtraction in [`free_variables`] is
/// flat, not scope-aware, so it is only sound for expressions whose bindings it
/// can see and scope correctly: no closures (whose params bind in a body walked
/// separately and would otherwise leak as required columns — the DANGEROUS
/// silent-skip direction), no `for` loops (whose loop variable is not a
/// `Stmt::Var` and would leak), and no `let` nested inside a block (which could
/// shadow an outer reference and wrongly drop it). For any such
/// **extraction-unsound** expression this returns the EMPTY set, which makes
/// the evaluator ALWAYS evaluate it — a missing variable then surfaces as a
/// LOUD genuine error, never a silent skip. No block-profile flood expression
/// uses these constructs, so the flood fix is unaffected; this only guards
/// latent future authorings. See `extraction_is_unsound`.
pub fn required_columns(ast: &AST) -> BTreeSet<String> {
    if extraction_is_unsound(ast) {
        return BTreeSet::new();
    }
    let free = free_variables(ast);
    let guarded = is_def_var_guards(ast);
    free.difference(&guarded).cloned().collect()
}

/// Whether [`free_variables`]'s flat `let`-subtraction can soundly scope this
/// expression's bindings. Returns `true` (unsound → fail-closed to always-eval)
/// when the AST contains a closure/anonymous fn, a `for` loop, or a `let`
/// nested inside a block. See [`required_columns`].
fn extraction_is_unsound(ast: &AST) -> bool {
    // A closure / anonymous fn compiles to an entry in the shared function
    // table; its parameters bind in a body the walk visits separately and would
    // otherwise be collected as free variables (never in scope → silent skip).
    if ast.iter_functions().next().is_some() {
        return true;
    }
    let mut unsound = false;
    ast.walk(&mut |path: &[ASTNode]| {
        match path.last().expect("walk always pushes the current node") {
            // `for i in xs` — the loop variable `i` is an `Ident` in the For
            // node, not a `Stmt::Var`, so it is never subtracted and leaks.
            ASTNode::Stmt(Stmt::For(..)) => unsound = true,
            // A `let` that is not a top-level statement (path deeper than the
            // node itself) sits inside a block and may shadow an outer name that
            // the flat subtraction would then wrongly drop.
            ASTNode::Stmt(Stmt::Var(..)) if path.len() > 1 => unsound = true,
            _ => {}
        }
        !unsound // stop early once unsound
    });
    unsound
}

/// Extract the free (unbound) variable names an expression reads.
///
/// Walks the AST collecting every `Expr::Variable` (non-namespace-qualified)
/// name, then subtracts every `let`/`const`-bound local (`Stmt::Var`).
/// Namespace-qualified paths (`foo::bar`), `this` (`Expr::ThisPtr`), method and
/// property names (`Expr::Property` / `MethodCall`) and function names
/// (`FnCall.name`) are structurally distinct nodes and are never collected — so
/// registered lookup functions never leak in as columns.
fn free_variables(ast: &AST) -> BTreeSet<String> {
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut bound: BTreeSet<String> = BTreeSet::new();

    ast.walk(&mut |path: &[ASTNode]| {
        match path.last().expect("walk always pushes the current node") {
            ASTNode::Expr(Expr::Variable(info, _, _)) => {
                // info: Box<(Option<index>, name, Namespace, hash)>
                if info.2.is_empty() {
                    referenced.insert(info.1.to_string());
                }
            }
            ASTNode::Stmt(Stmt::Var(decl, _, _)) => {
                // decl: Box<(Ident, Expr, Option<index>)>
                bound.insert(decl.0.name.to_string());
            }
            _ => {}
        }
        true
    });

    referenced.difference(&bound).cloned().collect()
}

/// String-literal arguments passed to `is_def_var("x")` — the explicit "this
/// expression copes with x's absence itself" signal (the block profile uses it
/// with `||`/`&&` short-circuits). Such names are EXEMPT from the required set:
/// their absence must not mark the expression unbound.
fn is_def_var_guards(ast: &AST) -> BTreeSet<String> {
    let mut guarded = BTreeSet::new();
    ast.walk(&mut |path: &[ASTNode]| {
        if let ASTNode::Expr(Expr::FnCall(call, _)) = path.last().unwrap() {
            if call.name.as_str() == "is_def_var" {
                if let Some(Expr::StringConstant(s, _)) = call.args.first() {
                    guarded.insert(s.to_string());
                }
            }
        }
        true
    });
    guarded
}

#[cfg(test)]
mod required_columns_tests {
    use super::*;

    fn req(src: &str) -> BTreeSet<String> {
        CompiledExpr::compile(&bounded_engine(), src)
            .unwrap()
            .required_columns
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn headline_case_extracts_exactly_the_two_columns() {
        assert_eq!(
            req("tags.contains(\"x\") && task_state == \"done\""),
            set(&["tags", "task_state"])
        );
    }

    #[test]
    fn method_and_property_names_are_not_columns() {
        assert_eq!(req("tags.contains(\"y\")"), set(&["tags"]));
        assert_eq!(req("tags.len > 0"), set(&["tags"]));
    }

    #[test]
    fn function_names_are_not_columns_but_args_are() {
        // `entity_by_id` is a registered lookup fn (register_entity_lookups);
        // its NAME is not a column, its ARG is.
        assert_eq!(req("entity_by_id(parent_id)"), set(&["parent_id"]));
    }

    #[test]
    fn arithmetic_and_operators() {
        assert_eq!(req("priority * 10 + weight"), set(&["priority", "weight"]));
    }

    #[test]
    fn conditionals_pick_up_all_branches() {
        assert_eq!(
            req("if entity_type == \"task\" { due } else { created }"),
            set(&["entity_type", "due", "created"])
        );
    }

    #[test]
    fn constants_and_strings_contribute_nothing() {
        assert!(req("\"literal\" == \"other\" && 42 > 1").is_empty());
    }

    #[test]
    fn local_let_bindings_are_excluded() {
        assert_eq!(req("let x = 5; x + tags.len"), set(&["tags"]));
    }

    #[test]
    fn nested_method_args_are_captured() {
        assert_eq!(
            req("tags.contains(task_state)"),
            set(&["tags", "task_state"])
        );
    }

    #[test]
    fn index_access_root_is_captured() {
        assert_eq!(req("props[key]"), set(&["props", "key"]));
    }

    #[test]
    fn leading_equals_prefix_is_stripped_before_analysis() {
        // Org-file `=` prefix must not change the extracted columns.
        assert_eq!(req("= priority * 2"), set(&["priority"]));
    }

    // --- Real block_profile.yaml expressions (assets/default/types) ---

    #[test]
    fn is_task_requires_only_task_state() {
        assert_eq!(req("task_state != ()"), set(&["task_state"]));
    }

    #[test]
    fn is_source_requires_content_type() {
        assert_eq!(req("content_type == \"source\""), set(&["content_type"]));
    }

    #[test]
    fn is_page_row_requires_only_tags() {
        assert_eq!(
            req("tags != () && tags.contains(\"\\\"Page\\\"\")"),
            set(&["tags"])
        );
    }

    #[test]
    fn is_holon_source_requires_source_language_not_the_prior_field() {
        // is_source is a sibling computed field, NOT a column — but required_columns
        // is context-free and lists every referenced name; the EVALUATOR resolves
        // is_source against the live scope. Here we only assert the column
        // source_language is present and no method/keyword leaks in.
        let cols = req(
            "is_source && (source_language == \"holon_prql\" || source_language == \"render\")",
        );
        assert!(cols.contains("source_language"));
        assert!(cols.contains("is_source"));
        assert_eq!(cols.len(), 2);
    }

    #[test]
    fn is_def_var_guarded_column_is_exempt() {
        // embedded_page condition: `role` is is_def_var-guarded AND read bare;
        // the guard exempts it, is_page_row remains (a sibling field name).
        assert_eq!(
            req("is_page_row && (!is_def_var(\"role\") || role != \"page_title\")"),
            set(&["is_page_row"])
        );
    }

    #[test]
    fn todo_states_document_id_is_guarded_away() {
        assert!(req(
            "if is_def_var(\"document_id\") && document_id != () { document(document_id) } else { () }"
        )
        .is_empty());
    }

    // --- Adversarial extraction cases (verifier-found latent bugs) ---
    //
    // These constructs are EXTRACTION-UNSOUND for the flat let-subtraction, so
    // `required_columns` fail-closes to the empty set. Empty required => the
    // evaluator always evaluates => a missing variable is LOUD, never silently
    // skipped. The critical property is that a closure PARAMETER must NEVER end
    // up as a required column (that would classify the whole expression unbound
    // and silently skip it — the dangerous direction).

    #[test]
    fn closure_param_never_becomes_a_required_column() {
        // BUG A: `|t| …` — `t` (never a scope column) must NOT be required.
        // Fail-closed => empty => always evaluated (loud on a real missing col).
        let cols = req("tags.filter(|t| t != \"\").len > 0");
        assert!(
            !cols.contains("t"),
            "closure param t must never be a required column (silent-skip trap): {cols:?}"
        );
        assert!(
            cols.is_empty(),
            "closure expr fail-closes to always-eval: {cols:?}"
        );
    }

    #[test]
    fn for_loop_variable_never_becomes_a_required_column() {
        let cols = req("let s = 0; for i in items { s += i }; s > items.len");
        assert!(
            !cols.contains("i"),
            "for-loop var i must not be required: {cols:?}"
        );
        assert!(
            cols.is_empty(),
            "for-loop expr fail-closes to always-eval: {cols:?}"
        );
    }

    #[test]
    fn shadowed_outer_variable_is_not_silently_dropped() {
        // `a` is free (outer) AND rebound by a nested `let a`. The flat
        // subtraction would drop the outer `a`; fail-closed avoids the wrong
        // (silent) answer by evaluating — a missing outer `a` is then loud.
        let cols = req("a + (if true { let a = 1; a } else { 0 })");
        assert!(
            cols.is_empty(),
            "nested-let shadow fail-closes to always-eval: {cols:?}"
        );
    }

    #[test]
    fn string_interpolation_columns_are_extracted() {
        assert_eq!(
            req("`hello ${name} ${task_state}`"),
            set(&["name", "task_state"])
        );
    }

    #[test]
    fn namespaced_call_target_is_not_a_column() {
        // `foo::bar` is namespace-qualified — not a scope variable; only the arg.
        assert_eq!(req("foo::bar(x)"), set(&["x"]));
    }

    #[test]
    fn top_level_let_stays_sound_not_fail_closed() {
        // A single top-level `let` is soundly scoped (not nested) — extraction
        // stays precise, so this must NOT fail-close to empty.
        assert_eq!(req("let x = 5; x + tags.len"), set(&["tags"]));
    }
}
