//! Shared computed field evaluation via Rhai.
//!
//! Computed fields are pre-compiled Rhai expressions evaluated in topological
//! order — each expression can reference the results of previously evaluated fields.
//! Compilation happens at registration time (TypeRegistry); this module only evaluates.

use std::collections::HashMap;

use holon_api::Value;
use rhai::{Engine as RhaiEngine, Scope};

use crate::type_registry::CompiledComputedField;

/// Evaluate pre-compiled computed fields in order, mutating the context in place.
///
/// `fields` must be topologically sorted (use `TypeRegistry::compiled_fields_for()`).
/// Results are added to both the Rhai scope and the context map.
///
/// Evaluation errors are logged and produce `Value::Null` — they do not propagate.
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

    for (name, compiled) in fields {
        let result = engine
            .eval_ast_with_scope::<rhai::Dynamic>(&mut scope, &compiled.ast)
            .unwrap_or_else(|e| {
                tracing::debug!("Computed field '{name}' eval error: {e}");
                rhai::Dynamic::UNIT
            });
        scope.push(name.clone(), result.clone());
        context.insert(name.clone(), dynamic_to_value(&result));
    }
}

/// Evaluate pre-compiled computed fields with an existing Rhai engine and scope.
///
/// Like `resolve_computed_fields` but takes a pre-configured engine and scope,
/// allowing callers (e.g., EntityProfile) to inject custom functions or variables.
pub fn resolve_computed_fields_with_scope(
    engine: &RhaiEngine,
    scope: &mut Scope,
    fields: &[CompiledComputedField],
    context: &mut HashMap<String, Value>,
) {
    for (name, compiled) in fields {
        let result = engine
            .eval_ast_with_scope::<rhai::Dynamic>(scope, &compiled.ast)
            .unwrap_or_else(|e| {
                tracing::debug!("Computed field '{name}' eval error: {e}");
                rhai::Dynamic::UNIT
            });
        scope.push(name.clone(), result.clone());
        context.insert(name.clone(), dynamic_to_value(&result));
    }
}

fn value_to_dynamic(v: &Value) -> rhai::Dynamic {
    match v {
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Integer(i) => rhai::Dynamic::from(*i),
        Value::Float(f) => rhai::Dynamic::from(*f),
        Value::Boolean(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
        Value::DateTime(s) => rhai::Dynamic::from(s.clone()),
        Value::Json(s) => rhai::Dynamic::from(s.clone()),
        Value::Array(_) | Value::Object(_) => rhai::Dynamic::UNIT,
    }
}

fn dynamic_to_value(d: &rhai::Dynamic) -> Value {
    if d.is_unit() {
        Value::Null
    } else if let Some(s) = d.clone().try_cast::<String>() {
        Value::String(s)
    } else if let Some(i) = d.clone().try_cast::<i64>() {
        Value::Integer(i)
    } else if let Some(f) = d.clone().try_cast::<f64>() {
        Value::Float(f)
    } else if let Some(b) = d.clone().try_cast::<bool>() {
        Value::Boolean(b)
    } else {
        Value::String(d.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::CompiledExpr;

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
    fn error_produces_null() {
        let mut ctx = HashMap::new();
        // Expression compiles but fails at eval (references undefined variable)
        let fields = vec![("bad".to_string(), compile("nonexistent_var + 1"))];

        resolve_computed_fields(&fields, &mut ctx);
        assert_eq!(ctx["bad"], Value::Null);
    }
}
