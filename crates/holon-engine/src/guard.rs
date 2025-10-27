use crate::value::Value;
use crate::{InputArc, Marking, TokenState};
use rhai::{Engine, Scope, AST};
use std::collections::BTreeMap;

/// A pre-compiled Rhai expression: source kept for debugging, AST for evaluation.
#[derive(Clone)]
pub struct CompiledExpr {
    pub source: String,
    pub ast: AST,
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

impl CompiledExpr {
    pub fn compile(engine: &Engine, source: impl Into<String>) -> Result<Self, String> {
        let source = source.into();
        let ast = engine
            .compile(&source)
            .map_err(|e| format!("Rhai compile error for '{source}': {e}"))?;
        Ok(CompiledExpr { source, ast })
    }
}

pub struct RhaiEvaluator {
    engine: Engine,
}

impl RhaiEvaluator {
    pub fn new() -> Self {
        RhaiEvaluator {
            engine: Engine::new(),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn compile_expression(&self, source: impl Into<String>) -> Result<CompiledExpr, String> {
        CompiledExpr::compile(&self.engine, source)
    }

    pub fn eval_compiled_expr(
        &self,
        compiled: &CompiledExpr,
        scope: &mut Scope,
    ) -> Result<f64, String> {
        self.engine
            .eval_ast_with_scope::<rhai::Dynamic>(scope, &compiled.ast)
            .map_err(|e| format!("eval error for '{}': {e}", compiled.source))
            .and_then(|d| {
                if let Ok(f) = d.as_float() {
                    Ok(f)
                } else if let Ok(i) = d.as_int() {
                    Ok(i as f64)
                } else {
                    Err(format!(
                        "expression '{}' did not return a number: {d:?}",
                        compiled.source
                    ))
                }
            })
    }

    pub fn eval_compiled_bool(
        &self,
        compiled: &CompiledExpr,
        scope: &mut Scope,
    ) -> Result<bool, String> {
        self.engine
            .eval_ast_with_scope::<bool>(scope, &compiled.ast)
            .map_err(|e| format!("constraint eval error for '{}': {e}", compiled.source))
    }

    pub fn eval_compiled_dynamic(
        &self,
        compiled: &CompiledExpr,
        scope: &mut Scope,
    ) -> Result<Value, String> {
        self.engine
            .eval_ast_with_scope::<rhai::Dynamic>(scope, &compiled.ast)
            .map(Value::from)
            .map_err(|e| format!("eval error for '{}': {e}", compiled.source))
    }

    pub fn token_to_map(token: &impl TokenState) -> rhai::Map {
        let mut map = rhai::Map::new();
        map.insert(
            "token_type".into(),
            rhai::Dynamic::from(token.token_type().to_string()),
        );
        for (k, v) in token.attrs() {
            map.insert(k.clone().into(), v.to_rhai_dynamic());
        }
        map
    }

    /// Check if a token matches all preconditions on an input arc.
    /// Returns Some(placeholders) if it matches, None otherwise.
    pub fn check_precond(
        &self,
        token: &impl TokenState,
        arc: &InputArc,
        existing_placeholders: &BTreeMap<String, Value>,
    ) -> Option<BTreeMap<String, Value>> {
        if token.token_type() != arc.token_type {
            return None;
        }
        let mut new_placeholders = BTreeMap::new();
        for (attr, spec) in &arc.precond {
            let token_val = token.get(attr);
            if spec.starts_with('$') {
                // Placeholder bind: capture value
                if let Some(val) = token_val {
                    new_placeholders.insert(spec.clone(), val.clone());
                } else {
                    new_placeholders.insert(spec.clone(), Value::Null);
                }
            } else if spec.starts_with(">=")
                || spec.starts_with("<=")
                || spec.starts_with('>')
                || spec.starts_with('<')
                || spec.starts_with("==")
                || spec.starts_with("!=")
            {
                // Rhai comparison expression
                let token_val = token_val?;
                let rhai_val = token_val.to_rhai_dynamic();
                let expr = format!("x {spec}");
                let mut scope = Scope::new();
                scope.push("x", rhai_val);
                for (k, v) in existing_placeholders {
                    scope.push(k.clone(), v.to_rhai_dynamic());
                }
                match self.engine.eval_with_scope::<bool>(&mut scope, &expr) {
                    Ok(true) => {}
                    _ => return None,
                }
            } else {
                // Exact match
                let token_val = token_val?;
                let matches = match token_val {
                    Value::String(s) => s == spec,
                    Value::Float(f) => spec.parse::<f64>().map_or(false, |v| (*f - v).abs() < 1e-9),
                    Value::Int(i) => spec.parse::<i64>().map_or(false, |v| *i == v),
                    Value::Bool(b) => spec.parse::<bool>().map_or(false, |v| *b == v),
                    Value::Null => spec == "null",
                };
                if !matches {
                    return None;
                }
            }
        }
        Some(new_placeholders)
    }

    /// Find the first matching token in a place for an input arc.
    /// Returns (token_id, captured_placeholders).
    pub fn find_matching_token<M: Marking>(
        &self,
        marking: &M,
        arc: &InputArc,
        already_bound: &[String],
        existing_placeholders: &BTreeMap<String, Value>,
    ) -> Option<(String, BTreeMap<String, Value>)> {
        let candidates = marking.tokens_of_type(&arc.token_type);
        for token in candidates {
            if already_bound.contains(&token.id().to_string()) {
                continue;
            }
            if let Some(placeholders) = self.check_precond(token, arc, existing_placeholders) {
                return Some((token.id().to_string(), placeholders));
            }
        }
        None
    }

    /// Evaluate a postcondition expression in the context of bound tokens.
    pub fn eval_postcond(
        &self,
        expr: &str,
        bound_tokens: &BTreeMap<String, rhai::Map>,
        placeholders: &BTreeMap<String, Value>,
    ) -> Result<Value, String> {
        if expr.starts_with('$') {
            // Placeholder reference
            return placeholders
                .get(expr)
                .cloned()
                .ok_or_else(|| format!("unresolved placeholder: {expr}"));
        }

        let mut scope = Scope::new();
        for (name, map) in bound_tokens {
            scope.push(name.clone(), rhai::Dynamic::from(map.clone()));
        }
        for (k, v) in placeholders {
            scope.push(k.clone(), v.to_rhai_dynamic());
        }

        self.engine
            .eval_with_scope::<rhai::Dynamic>(&mut scope, expr)
            .map(Value::from)
            .map_err(|e| format!("postcond eval error: {e}"))
    }

    /// Build a Rhai scope with all tokens registered by their id.
    pub fn build_marking_scope<M: Marking>(marking: &M) -> Scope<'static> {
        let mut scope = Scope::new();
        for token in marking.tokens() {
            let map = Self::token_to_map(token);
            scope.push(token.id().to_string(), rhai::Dynamic::from(map));
        }

        let clock = marking.clock();
        let mut clock_map = rhai::Map::new();
        clock_map.insert(
            "hour".into(),
            rhai::Dynamic::from(clock.format("%H").to_string().parse::<i64>().unwrap_or(0)),
        );
        clock_map.insert(
            "weekday".into(),
            rhai::Dynamic::from(clock.format("%A").to_string()),
        );
        scope.push("clock", rhai::Dynamic::from(clock_map));

        scope
    }

    pub fn eval_expr(&self, expr: &str, scope: &mut Scope) -> Result<f64, String> {
        self.engine
            .eval_with_scope::<rhai::Dynamic>(scope, expr)
            .map_err(|e| format!("eval error: {e}"))
            .and_then(|d| {
                if let Ok(f) = d.as_float() {
                    Ok(f)
                } else if let Ok(i) = d.as_int() {
                    Ok(i as f64)
                } else {
                    Err(format!("expression did not return a number: {d:?}"))
                }
            })
    }

    pub fn eval_bool(&self, expr: &str, scope: &mut Scope) -> Result<bool, String> {
        self.engine
            .eval_with_scope::<bool>(scope, expr)
            .map_err(|e| format!("constraint eval error: {e}"))
    }
}
