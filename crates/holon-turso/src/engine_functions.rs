//! Registry of engine-provided SQL functions available to `holon_sql` queries.
//!
//! Martin's ruling (docs/Proposals/FtsRegistry-2026-07-11.md): engine functions
//! are declared ONCE, by shape:
//!
//! - **Scalar / Predicate** functions resolve through the Turso fork's `Func`
//!   enum path (`turso_core::function`, e.g. `fts_match` / `fts_score`). Holon
//!   passes `holon_sql` through verbatim, so these are usable in query blocks
//!   as soon as the engine build enables them — this registry is the holon-side
//!   single source of truth for what exists (name, arity, shape) and for the
//!   ADR 0024 guard classification (`dual_evaluable`).
//! - **Set-valued** (relation-returning) functions — e.g. a future
//!   `similar(block, k)` over a sparse-vector index — resolve via the
//!   matview/TVF declaration path instead: the function names a materialized
//!   relation the engine maintains, not a row-at-a-time callable. No set-valued
//!   function ships yet; [`FunctionShape::SetValued`] documents the slot.
//!
//! Insert-only by convention (like `CapMap`): declarations are appended, never
//! mutated or removed at runtime.

/// How a function participates in a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionShape {
    /// Returns one value per row (e.g. `fts_score`, `fts_highlight`).
    Scalar,
    /// Scalar returning a boolean, intended for `WHERE` (e.g. `fts_match`).
    Predicate,
    /// Returns a relation; resolved via the matview/TVF path, NOT the Func
    /// enum. Declaring one requires wiring a maintained materialized relation
    /// (see `matview_manager`); until then this variant is a documented stub.
    SetValued,
}

/// Argument-count contract. FTS functions are variadic over indexed columns:
/// `fts_match(col1, .., colN, query)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    Exact(usize),
    AtLeast(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineFunctionDecl {
    pub name: &'static str,
    pub arity: Arity,
    pub shape: FunctionShape,
    /// ADR 0024 Pattern guards are dual-evaluated (SQL + in-memory). Functions
    /// that only exist engine-side (the whole fts_* family — they consult a
    /// Tantivy index living in the database) CANNOT be evaluated in memory and
    /// must be classified SQL-only by the guard planner.
    pub dual_evaluable: bool,
}

/// Every engine-provided function available to `holon_sql`, beyond the SQLite
/// builtin surface. Appended-to only.
pub const ENGINE_FUNCTIONS: &[EngineFunctionDecl] = &[
    EngineFunctionDecl {
        name: "fts_match",
        // (col1, .., colN, query)
        arity: Arity::AtLeast(2),
        shape: FunctionShape::Predicate,
        dual_evaluable: false,
    },
    EngineFunctionDecl {
        name: "fts_score",
        // (col1, .., colN, query)
        arity: Arity::AtLeast(2),
        shape: FunctionShape::Scalar,
        dual_evaluable: false,
    },
    EngineFunctionDecl {
        name: "fts_highlight",
        // (col1, .., colN, before_tag, after_tag, query)
        arity: Arity::AtLeast(4),
        shape: FunctionShape::Scalar,
        dual_evaluable: false,
    },
];

/// Look up an engine function by (case-insensitive) name.
pub fn engine_function(name: &str) -> Option<&'static EngineFunctionDecl> {
    ENGINE_FUNCTIONS
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_family_is_declared_with_ruled_shapes() {
        assert_eq!(
            engine_function("fts_match").unwrap().shape,
            FunctionShape::Predicate
        );
        assert_eq!(
            engine_function("FTS_SCORE").unwrap().shape,
            FunctionShape::Scalar
        );
        assert_eq!(
            engine_function("fts_highlight").unwrap().shape,
            FunctionShape::Scalar
        );
        assert!(engine_function("similar").is_none());
    }

    #[test]
    fn fts_family_is_sql_only_for_guards() {
        assert!(ENGINE_FUNCTIONS.iter().all(|d| !d.dual_evaluable));
    }
}
