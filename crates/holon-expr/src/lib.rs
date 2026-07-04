//! @c4 component
//! @c4 layer Engine
//! Pattern: Interpreter
//!
//! Compiled Rhai expressions — the shared vocabulary between holon-api entity definitions and the holon-engine Petri-net guard evaluator.

use rhai::{Engine, AST};
use serde::{Deserialize, Serialize};

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

/// A pre-compiled Rhai expression: source kept for debugging, AST for evaluation.
///
/// Serde: serializes as the source string, deserializes by compiling.
/// Deserialization fails loudly if the expression doesn't compile (parse boundary).
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
        Ok(CompiledExpr { source, ast })
    }
}
