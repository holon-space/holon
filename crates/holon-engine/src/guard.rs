use crate::arc::{PostcondExpr, PrecondSpec};
use crate::value::Value;
use crate::{InputArc, Marking, TokenState};
use rhai::{Engine, Scope};
use std::collections::BTreeMap;

pub use holon_expr::CompiledExpr;

pub struct RhaiEvaluator {
    engine: Engine,
}

impl Default for RhaiEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl RhaiEvaluator {
    pub fn new() -> Self {
        RhaiEvaluator {
            engine: holon_expr::bounded_engine(),
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
    /// Returns Ok(Some(placeholders)) on match, Ok(None) on no match,
    /// Err if a precondition expression is malformed.
    pub fn check_precond(
        &self,
        token: &impl TokenState,
        arc: &InputArc,
        existing_placeholders: &BTreeMap<String, Value>,
    ) -> Result<Option<BTreeMap<String, Value>>, String> {
        if token.token_type() != arc.token_type {
            return Ok(None);
        }
        let mut new_placeholders = BTreeMap::new();
        for (attr, spec) in &arc.precond {
            let token_val = token.get(attr);
            match spec {
                PrecondSpec::Placeholder(name) => {
                    // Placeholder bind: capture value, unifying with any earlier capture
                    let val = token_val.cloned().unwrap_or(Value::Null);
                    let prior = existing_placeholders
                        .get(name)
                        .or_else(|| new_placeholders.get(name));
                    match prior {
                        Some(existing) if *existing != val => return Ok(None),
                        _ => {
                            new_placeholders.insert(name.clone(), val);
                        }
                    }
                }
                PrecondSpec::Comparison { compiled, .. } => {
                    let Some(token_val) = token_val else {
                        return Ok(None);
                    };
                    let mut scope = Scope::new();
                    scope.push("x", token_val.to_rhai_dynamic());
                    match self
                        .engine
                        .eval_ast_with_scope::<bool>(&mut scope, &compiled.ast)
                    {
                        Ok(true) => {}
                        Ok(false) => return Ok(None),
                        Err(e) => {
                            return Err(format!(
                                "precondition '{attr}: {spec}' on arc '{}' failed to evaluate: {e}",
                                arc.bind
                            ))
                        }
                    }
                }
                PrecondSpec::Exact(lit) => {
                    let Some(token_val) = token_val else {
                        return Ok(None);
                    };
                    let matches = match token_val {
                        Value::String(s) => s == lit,
                        Value::Float(f) => lit.parse::<f64>().is_ok_and(|v| (*f - v).abs() < 1e-9),
                        Value::Int(i) => lit.parse::<i64>() == Ok(*i),
                        Value::Bool(b) => lit.parse::<bool>() == Ok(*b),
                        Value::Null => lit == "null",
                    };
                    if !matches {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(new_placeholders))
    }

    /// Find all matching tokens in a place for an input arc.
    /// Returns (token_id, captured_placeholders) per candidate.
    pub fn matching_tokens<M: Marking>(
        &self,
        marking: &M,
        arc: &InputArc,
        already_bound: &[String],
        existing_placeholders: &BTreeMap<String, Value>,
    ) -> Result<Vec<(String, BTreeMap<String, Value>)>, String> {
        let mut matches = Vec::new();
        for token in marking.tokens_of_type(&arc.token_type) {
            if already_bound.contains(&token.id().to_string()) {
                continue;
            }
            if let Some(placeholders) = self.check_precond(token, arc, existing_placeholders)? {
                matches.push((token.id().to_string(), placeholders));
            }
        }
        Ok(matches)
    }

    /// Evaluate a postcondition expression in the context of bound tokens.
    pub fn eval_postcond(
        &self,
        spec: &PostcondExpr,
        bound_tokens: &BTreeMap<String, rhai::Map>,
        placeholders: &BTreeMap<String, Value>,
    ) -> Result<Value, String> {
        let compiled = match spec {
            PostcondExpr::Placeholder(name) => {
                return placeholders
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("unresolved placeholder: {name}"));
            }
            PostcondExpr::Expr(compiled) => compiled,
        };

        let mut scope = Scope::new();
        for (name, map) in bound_tokens {
            scope.push(name.clone(), rhai::Dynamic::from(map.clone()));
        }
        for (k, v) in placeholders {
            scope.push(k.clone(), v.to_rhai_dynamic());
        }

        self.eval_compiled_dynamic(compiled, &mut scope)
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
            rhai::Dynamic::from(
                clock
                    .format("%H")
                    .to_string()
                    .parse::<i64>()
                    .expect("clock.format(\"%H\") always yields a valid hour integer"),
            ),
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
