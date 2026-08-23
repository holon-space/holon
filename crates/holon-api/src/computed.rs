//! Shared computed field evaluation via Rhai.
//!
//! Computed fields are pre-compiled Rhai expressions evaluated in topological
//! order — each expression can reference the results of previously evaluated
//! fields. Compilation happens at registration time (TypeRegistry); this module
//! only evaluates.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::OnceLock;

use rhai::Engine as RhaiEngine;
use rhai::Scope;

use crate::Value;
use crate::entity_profile::CompiledComputedField;
use crate::entity_profile::dynamic_to_value;
use crate::entity_profile::value_to_dynamic;

/// Process-global dedup for the LOUD "declared column missing" signal so a
/// per-row render/enrich path emits at most ONE warning per
/// `(context, column)` — a real projection gap is surfaced once, not once per
/// row (which is exactly the flood this whole change removes). Touched only on
/// the rare LOUD path, so no hot-path contention.
fn warned_missing_declared() -> &'static Mutex<HashSet<(String, String)>> {
    static SEEN: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Clear the once-per-`(context, column)` dedup.
///
/// A parity oracle asserts the ABSENCE of these warnings, so it must start from
/// an empty set: the dedup is process-global, and a boot that ran earlier in
/// the same process would otherwise suppress exactly the signal being asserted
/// on.
pub fn reset_missing_declared_warnings() {
    warned_missing_declared().lock().unwrap().clear();
}

/// Surface a "should-be-present-but-missing" declared column ONCE.
///
/// `context` names the seat (computed field name or condition source); `column`
/// is the declared column that the row's projection failed to carry. This is
/// the LOUD half of type-aware binding: the column is part of the entity's
/// declared schema, so its absence is a genuine projection/wiring bug, not the
/// expected heterogeneity a missing property represents.
pub(crate) fn warn_missing_declared_column(context: &str, column: &str) {
    let key = (context.to_string(), column.to_string());
    let mut seen = warned_missing_declared().lock().unwrap();
    if seen.insert(key) {
        tracing::warn!(
            context = context,
            column = column,
            "type-aware binding: DECLARED column absent from row — a computed \
             field / condition requires it but the projection did not carry it. \
             This is a real projection gap (NOT the expected heterogeneity of an \
             optional property). Warned once per (context, column) this process."
        );
    }
}

/// Evaluate pre-compiled computed fields in order, mutating the context in
/// place.
///
/// `fields` must be topologically sorted (use
/// `TypeRegistry::compiled_fields_for()`). Results are added to both the Rhai
/// scope and the context map.
///
/// Evaluation errors are logged and produce `Value::Null` — they do not
/// propagate.
pub fn resolve_computed_fields(
    fields: &[CompiledComputedField],
    context: &mut HashMap<String, Value>,
) {
    if fields.is_empty() {
        return;
    }

    let engine = RhaiEngine::new();
    let mut scope = Scope::new();

    for (k, v) in context.iter() {
        scope.push(k.clone(), value_to_dynamic(v));
    }

    // No declared schema known at this entry point (used by lightweight
    // callers/tests): every missing required column is treated as
    // structurally-absent (silent typed skip).
    let no_declared = BTreeSet::new();
    resolve_computed_fields_with_scope(&engine, &mut scope, fields, context, &no_declared);
}

/// Evaluate pre-compiled computed fields with an existing Rhai engine and
/// scope.
///
/// Like `resolve_computed_fields` but takes a pre-configured engine and scope,
/// allowing callers (e.g., EntityProfile) to inject custom functions or
/// variables.
///
/// **Type-aware binding (C4 caller contract).** This is the production enrich
/// seat (`ui_watcher::enrich_row` → `resolve_computed_only`) and the render
/// seat (`EntityProfile::build_scope`). Each field carries its
/// `required_columns` (derived from its AST at compile time). Before
/// evaluating, we compare those against the live `scope`:
///
/// - **Structurally unbound** (a required column is absent from scope — e.g.
///   `task_state` on a non-task block, or a sibling computed field that was
///   itself unbound): the field is the wrong shape for this heterogeneous row.
///   We do NOT invoke Rhai (so NO "Variable not found" error is ever raised)
///   and do NOT push the field into scope (absence = the typed "unbound" value,
///   which propagates cleanly to dependents and to variant conditions). Its
///   output value defaults to `Null` via `extract_computed_values`. This is
///   SILENT for optional columns and LOUD (once) for columns in the entity's
///   `declared_columns` — a declared column missing is a real projection gap.
/// - **All required columns present**: evaluate. A failure now is a genuine
///   runtime error on columns that ARE present (a type mismatch, a bad lookup)
///   — surfaced at `warn` (disclosed degraded) and substituted with `Null`.
///
/// NOTE: this path keeps its **caller-provided engine** deliberately — that
/// engine carries the custom entity-lookup Rhai functions registered by the
/// profile resolver (`register_entity_lookups`), which the default
/// `bounded_engine()` behind `Computation::eval` lacks.
///
/// `declared_columns` is the entity's declared schema (TypeDefinition
/// persistent field names). Pass an empty set when no schema is known (every
/// miss is then treated as structurally-absent → silent).
pub fn resolve_computed_fields_with_scope(
    engine: &RhaiEngine,
    scope: &mut Scope,
    fields: &[CompiledComputedField],
    context: &mut HashMap<String, Value>,
    declared_columns: &BTreeSet<String>,
) {
    for (name, compiled) in fields {
        let missing: Vec<&String> = compiled
            .required_columns
            .iter()
            .filter(|col| !scope.contains(col))
            .collect();

        if !missing.is_empty() {
            // Structurally unbound. Disclose only the declared-but-missing
            // columns (real projection gaps); the rest are expected
            // heterogeneity (optional properties / unbound sibling fields).
            for col in &missing {
                if declared_columns.contains(*col) {
                    warn_missing_declared_column(name, col);
                }
            }
            // Do NOT push to scope — absence is the typed "unbound" value, which
            // propagates to dependent fields and variant conditions (they see it
            // missing, not as a `unit` that would type-error). Record Null in the
            // OUTPUT context so consumers that read the context map keep the
            // field's shape (the render path reads scope via
            // `extract_computed_values`, which applies the same Null default).
            context.insert(name.clone(), Value::Null);
            continue;
        }

        let result = match engine.eval_ast_with_scope::<rhai::Dynamic>(scope, &compiled.ast) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    field = %name,
                    error = %e,
                    "C4 enrich: computed field eval failed on PRESENT columns — genuine \
                     runtime error (type mismatch / bad lookup), DISCLOSED degraded mode, \
                     substituting Null"
                );
                rhai::Dynamic::UNIT
            }
        };
        scope.push(name.clone(), result.clone());
        context.insert(name.clone(), dynamic_to_value(&result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompiledExpr;

    fn compile(expr: &str) -> CompiledExpr {
        let engine = RhaiEngine::new();
        CompiledExpr::compile(&engine, expr).unwrap()
    }

    #[test]
    fn basic_computed_field() {
        let mut ctx = HashMap::new();
        ctx.insert("priority".to_string(), Value::Integer(3));

        let fields = vec![("priority_score".to_string(), compile("priority * 10"))];

        resolve_computed_fields(&fields, &mut ctx);
        assert_eq!(ctx["priority_score"], Value::Integer(30));
    }

    #[test]
    fn chained_computed_fields() {
        let mut ctx = HashMap::new();
        ctx.insert("base".to_string(), Value::Float(2.0));

        let fields = vec![
            ("doubled".to_string(), compile("base * 2.0")),
            ("quadrupled".to_string(), compile("doubled * 2.0")),
        ];

        resolve_computed_fields(&fields, &mut ctx);
        assert_eq!(ctx["doubled"], Value::Float(4.0));
        assert_eq!(ctx["quadrupled"], Value::Float(8.0));
    }

    #[test]
    fn missing_required_column_is_unbound_null_not_evaluated() {
        // Type-aware binding: a required column absent from scope makes the field
        // UNBOUND — rhai is never invoked (no "Variable not found"), output is Null.
        let mut ctx = HashMap::new();
        let fields = vec![("bad".to_string(), compile("nonexistent_var + 1"))];

        resolve_computed_fields(&fields, &mut ctx);
        assert_eq!(ctx["bad"], Value::Null);
    }

    #[test]
    fn unbound_field_does_not_poison_a_dependent_boolean_field() {
        // The Seat-B cascade root: `is_page_row` unbound (tags absent) must NOT
        // become a `unit` that type-errors a dependent `&&`. It must stay
        // unbound so the dependent is ALSO unbound (no genuine eval error).
        let engine = RhaiEngine::new();
        let mut scope = Scope::new();
        let mut ctx = HashMap::new();
        let fields = vec![
            ("is_page_row".to_string(), compile("tags != ()")),
            (
                "embedded".to_string(),
                compile("is_page_row && content_type == \"text\""),
            ),
        ];
        let declared = BTreeSet::new();
        resolve_computed_fields_with_scope(&engine, &mut scope, &fields, &mut ctx, &declared);
        // Both unbound → Null, and crucially NO panic / no genuine error.
        assert_eq!(ctx.get("is_page_row"), Some(&Value::Null));
        assert_eq!(ctx.get("embedded"), Some(&Value::Null));
        // Neither was pushed into scope (absence = unbound).
        assert!(!scope.contains("is_page_row"));
        assert!(!scope.contains("embedded"));
    }

    #[test]
    fn present_columns_still_evaluate_and_a_prior_field_binds_dependents() {
        let engine = RhaiEngine::new();
        let mut scope = Scope::new();
        scope.push(
            "content_type".to_string(),
            value_to_dynamic(&Value::String("text".into())),
        );
        let mut ctx = HashMap::new();
        let fields = vec![
            (
                "is_source".to_string(),
                compile("content_type == \"source\""),
            ),
            ("shown".to_string(), compile("!is_source")),
        ];
        let declared = BTreeSet::new();
        resolve_computed_fields_with_scope(&engine, &mut scope, &fields, &mut ctx, &declared);
        assert_eq!(ctx["is_source"], Value::Boolean(false));
        assert_eq!(ctx["shown"], Value::Boolean(true));
    }
}
