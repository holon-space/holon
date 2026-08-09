//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Macro-expansion tests for `holon-macros`.

// Test crate for holon-macros
// This allows us to test the macro expansion since proc macros can't be used in their own crate
#![allow(clippy::manual_range_contains)] // expanded operations_trait macro emits `>=`/`<=` checks

use async_trait::async_trait;
use holon_core::Result;
use holon_core::UndoAction;
use holon_macros::require;

/// `#[require]` fixtures for the ADR 0031 guard retarget.
///
/// Every guard is RELATIONAL (P6=A): a predicate over the state the op touches,
/// never over its parameters — parameter validity is the typed params' job, and
/// a parameter predicate binds nothing to iterate.
#[holon_macros::operations_trait]
#[async_trait]
pub trait TestTrait<T>: Send + Sync
where
    T: Send + Sync + 'static,
{
    /// Delete an item by ID. Clock-driven: a builtin makes the guard iterate
    /// the clock relation, so this exercises `Subject` inference through the
    /// macro.
    #[require("not block_exists(\"Journals/{today}\")")]
    // ALLOW(unused_param): test fixture — id is part of the trait shape
    async fn delete(&self, _id: &str) -> Result<UndoAction>;

    /// Set a boolean flag. Block-driven single leaf.
    #[require("has_tag(\"flaggable\")")]
    // ALLOW(unused_param): test fixture — the trait shape carries both params
    async fn set_flag(&self, _id: &str, _value: bool) -> Result<UndoAction>;

    /// Set priority. Two attributes conjoin — the composition escape hatch for
    /// the 80-character literal lint — into `page_under_non_page`.
    #[require("has_tag(\"Page\")")]
    #[require("parent(not has_tag(\"Page\"))")]
    // ALLOW(unused_param): test fixture — the trait shape carries both params
    async fn set_priority(&self, _id: &str, _priority: i64) -> Result<UndoAction>;

    /// Method without precondition.
    // ALLOW(unused_param): test fixture — id is part of the trait shape
    async fn no_precondition(&self, _id: &str) -> Result<UndoAction>;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::pattern::Binding;
    use holon_api::pattern::BuiltinRef;
    use holon_api::pattern::CurrentSchema;
    use holon_api::pattern::Guard;
    use holon_api::pattern::InMemoryWorld;
    use holon_api::pattern::OpGuard;
    use holon_api::pattern::PathPattern;
    use holon_api::pattern::PathSegment;
    use holon_api::pattern::Pattern;
    use holon_api::pattern::Subject;
    use holon_api::pattern::WorldBlock;

    use super::*;

    fn ops() -> Vec<holon_api::OperationDescriptor> {
        __operations_test_trait::test_trait("test-entity", "test", "test_table", "id")
    }

    fn guard_of(name: &str) -> Guard {
        ops()
            .iter()
            .find(|op| op.name == name)
            .unwrap_or_else(|| panic!("op {name} exists"))
            .guard
            .guard()
            .unwrap_or_else(|| panic!("op {name} declares a guard"))
            .clone()
    }

    fn block(id: &str, name: &str, parent: Option<&str>, tags: &[&str]) -> WorldBlock {
        WorldBlock {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent.map(str::to_string),
            properties: HashMap::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// The macro emits the guard the Pattern parser produces — declarative
    /// data, parsed at expansion time, not a closure.
    #[test]
    fn require_emits_the_parsed_guard_ast() {
        assert_eq!(
            guard_of("delete"),
            Guard {
                subject: Subject::Clock,
                body: Pattern::Not(Box::new(Pattern::BlockExists(PathPattern {
                    segments: vec![
                        PathSegment::Lit("Journals".to_string()),
                        PathSegment::Builtin(BuiltinRef::Today),
                    ],
                }))),
            },
            "a builtin makes the guard clock-driven"
        );
        assert_eq!(
            guard_of("set_flag"),
            Guard {
                subject: Subject::Block,
                body: Pattern::HasTag("flaggable".to_string()),
            }
        );
    }

    /// Several `#[require]`s conjoin into one `And`.
    #[test]
    fn multiple_requires_conjoin_into_one_guard() {
        assert_eq!(
            guard_of("set_priority"),
            Guard {
                subject: Subject::Block,
                body: Pattern::And(vec![
                    Pattern::HasTag("Page".to_string()),
                    Pattern::Parent(Box::new(Pattern::Not(Box::new(Pattern::HasTag(
                        "Page".to_string()
                    ))))),
                ]),
            }
        );
    }

    /// No `#[require]` is an explicit stated fact, not an absence.
    #[test]
    fn no_require_declares_op_guard_none() {
        let op = ops();
        let op = op.iter().find(|op| op.name == "no_precondition").unwrap();
        assert_eq!(op.guard, OpGuard::None);
        assert!(op.guard.guard().is_none());
    }

    /// The emitted guard evaluates: `page_under_non_page` binds exactly the
    /// page whose parent is not a page.
    #[test]
    fn emitted_guard_evaluates_over_a_world() {
        let g = guard_of("set_priority");
        let world = InMemoryWorld::new(
            vec![
                block("root", "Root", None, &["Page"]),
                block("ok", "OK", Some("root"), &["Page"]),
                block("plain", "Plain", Some("root"), &[]),
                block("bad", "Bad", Some("plain"), &["Page"]),
            ],
            "2026-08-10",
        );
        assert_eq!(
            g.evaluate(&world).bindings,
            vec![Binding::Block("bad".to_string())],
            "only the page under a non-page parent is bound"
        );
    }

    /// The clock-driven guard re-fires on day rollover, and compiles to SQL
    /// that reads the clock relation rather than `date('now')`.
    #[test]
    fn emitted_clock_guard_evaluates_and_compiles() {
        let g = guard_of("delete");
        let journals = vec![block("j", "Journals", None, &[])];
        assert!(
            g.evaluate(&InMemoryWorld::new(journals.clone(), "2026-08-10"))
                .enabled(),
            "no journal today ⇒ enabled"
        );

        let mut with_today = journals;
        with_today.push(block("d", "2026-08-10", Some("j"), &[]));
        assert!(
            !g.evaluate(&InMemoryWorld::new(with_today, "2026-08-10"))
                .enabled(),
            "today's journal exists ⇒ disabled"
        );

        let sql = g.to_sql(&CurrentSchema);
        assert!(sql.contains("FROM clock c"), "{sql}");
        assert!(!sql.contains("date('now')"), "{sql}");
    }

    /// The descriptor's guard is plain data: it round-trips through serde,
    /// which is what makes the catalog loadable by a second consumer.
    #[test]
    fn emitted_guard_is_serializable_data() {
        let op = ops();
        let op = op.iter().find(|op| op.name == "set_priority").unwrap();
        let json = serde_json::to_string(op).expect("serialize");
        let back: holon_api::OperationDescriptor =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, op);
    }
}
