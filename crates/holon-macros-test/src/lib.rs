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

    /// Method without precondition. Also the `TransitionArcs::Undeclared`
    /// fixture: no `#[reads]`/`#[emits]` at all.
    // ALLOW(unused_param): test fixture — id is part of the trait shape
    async fn no_precondition(&self, _id: &str) -> Result<UndoAction>;

    /// Arc fixture: reads two places, writes one, and declares one place
    /// EXCLUDED with its reason. `#[affects]` is declared alongside so the
    /// `emits ⊇ affects` consistency lock (OQ-2=A) has something to bite on.
    #[holon_macros::affects("parent_id")]
    #[holon_macros::reads("block.parent_id", "block.tags")]
    #[holon_macros::emits("block.parent_id")]
    #[holon_macros::emits(excluded("block.sort_key", "the ordering authority mints order keys"))]
    // ALLOW(unused_param): test fixture — the trait shape carries both params
    async fn move_it(&self, _id: &str, _parent: &str) -> Result<UndoAction>;

    /// Arc fixture: a genuinely read-only declaration. `#[emits()]` is empty on
    /// purpose — "writes nothing", which is a different claim from
    /// `Undeclared`.
    #[holon_macros::reads("clock.today")]
    #[holon_macros::emits()]
    // ALLOW(unused_param): test fixture — id is part of the trait shape
    async fn peek(&self, _id: &str) -> Result<UndoAction>;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::arcs::ArcEmit;
    use holon_api::arcs::ArcPlace;
    use holon_api::arcs::ArcRelation;
    use holon_api::arcs::TransitionArcs;
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

    /// The emitted guard carries the developer's own literal, and the
    /// conjunction case carries the JOINED text — a refusal quoting one of two
    /// `#[require]`s would misdescribe what refused.
    #[test]
    fn require_emits_the_source_literal_joined() {
        let source_of = |name: &str| {
            ops()
                .iter()
                .find(|op| op.name == name)
                .unwrap_or_else(|| panic!("op {name} exists"))
                .guard
                .source()
                .unwrap_or_else(|| panic!("op {name} declares a guard"))
                .to_string()
        };
        assert_eq!(source_of("set_flag"), "has_tag(\"flaggable\")");
        assert_eq!(
            source_of("set_priority"),
            "has_tag(\"Page\") and parent(not has_tag(\"Page\"))"
        );
    }

    fn arcs_of(name: &str) -> TransitionArcs {
        ops()
            .iter()
            .find(|op| op.name == name)
            .unwrap_or_else(|| panic!("op {name} exists"))
            .arcs
            .clone()
    }

    /// The macro emits the PARSED arcs — typed places, not the strings the
    /// developer wrote.
    #[test]
    fn reads_and_emits_are_parsed_into_typed_places() {
        let place = |relation, field: &str| ArcPlace {
            relation,
            field: field.to_string(),
        };
        assert_eq!(
            arcs_of("move_it"),
            TransitionArcs::Declared {
                reads: vec![
                    place(ArcRelation::block(), "parent_id"),
                    place(ArcRelation::block(), "tags"),
                ],
                emits: vec![
                    ArcEmit::Writes(place(ArcRelation::block(), "parent_id")),
                    ArcEmit::Excluded {
                        place: place(ArcRelation::block(), "sort_key"),
                        reason: "the ordering authority mints order keys".to_string(),
                    },
                ],
            }
        );
        assert_eq!(
            arcs_of("move_it").written_places(),
            vec![&place(ArcRelation::block(), "parent_id")],
            "an EXCLUDED place is declared, not written"
        );
    }

    /// No `#[reads]`/`#[emits]` yields the fail-closed `Undeclared`, which is
    /// distinguishable from a declared empty write set.
    #[test]
    fn absent_arcs_are_undeclared_and_empty_arcs_are_not() {
        assert_eq!(arcs_of("no_precondition"), TransitionArcs::Undeclared);
        assert_eq!(arcs_of("no_precondition").emits(), None);
        assert_eq!(
            arcs_of("peek"),
            TransitionArcs::Declared {
                reads: vec![ArcPlace {
                    relation: ArcRelation::clock(),
                    field: "today".to_string(),
                }],
                emits: vec![],
            }
        );
        assert_eq!(
            arcs_of("peek").emits(),
            Some(&[][..]),
            "\"writes nothing\" is a statement; Undeclared is the refusal to make one"
        );
    }

    /// OQ-2=(A): `#[emits]` must cover every `#[affects]` field. The lock lives
    /// over the real catalog in `holon-app`; this is the mechanism check that
    /// it can actually bite — `affects` names bare block fields, so the
    /// mapping is `field ↦ block.field`.
    #[test]
    fn declared_emits_cover_the_declared_affects() {
        let op = ops();
        let op = op.iter().find(|op| op.name == "move_it").expect("move_it");
        let written: Vec<String> = op
            .arcs
            .written_places()
            .iter()
            .map(|p| p.to_string())
            .collect();
        for field in &op.affected_fields {
            assert!(
                written.contains(&format!("block.{field}")),
                "#[affects({field:?})] is not covered by #[emits]; declared writes: {written:?}"
            );
        }
        assert!(
            !op.affected_fields.is_empty(),
            "the lock is not vacuous here"
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
            g.evaluate(&world)
                .expect("a clock guard evaluates")
                .bindings,
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
                .expect("a clock guard evaluates")
                .enabled(),
            "no journal today ⇒ enabled"
        );

        let mut with_today = journals;
        with_today.push(block("d", "2026-08-10", Some("j"), &[]));
        assert!(
            !g.evaluate(&InMemoryWorld::new(with_today, "2026-08-10"))
                .expect("a clock guard evaluates")
                .enabled(),
            "today's journal exists ⇒ disabled"
        );

        let sql = g.to_sql(&CurrentSchema).expect("a clock guard compiles");
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
