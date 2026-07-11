//! Runtime-definable advice rules (ADR 0022).
//!
//! An advice rule is a vault block (`source_language =
//! 'holon_advice_rule_yaml'`) discovered by the entity-profile scan pattern,
//! parsed at the boundary into a closed typed model, and compiled by the engine
//! into ONE anchor-denormalized materialized view per rule
//! (`advice_rule_{slug}`) via `reconcile_named_view`.
//!
//! This crate owns the rule *core*: discovery, parse, SQL lowering, and matview
//! synthesis. It does NOT own rendering/weaving (the renderer reads
//! `advice_rule_{slug}` filtered to the current anchor — a separate concern).

pub mod discovery;
pub mod holon_rule;
pub mod lowering;
pub mod reconcile_plan;
pub mod rule;
pub mod status;
pub mod synthesis;

pub use discovery::ADVICE_RULE_SOURCE_LANGUAGE;
pub use discovery::DiscoveredRule;
pub use discovery::GET_ADVICE_RULES_SQL;
pub use discovery::is_advice_rule_block;
pub use discovery::parse_discovered_rule;
pub use holon_rule::Emit;
pub use holon_rule::HolonRule;
pub use holon_rule::HolonRuleParseError;
pub use holon_rule::NameTemplate;
pub use holon_rule::Place;
pub use holon_rule::RuleName;
pub use holon_rule::TemplateSegment;
pub use holon_rule::parse_holon_rule;
pub use lowering::LoweringError;
pub use reconcile_plan::AdviceReconcilerState;
pub use reconcile_plan::ReconcilePlan;
pub use reconcile_plan::RuleEvent;
pub use reconcile_plan::StatusOutcome;
pub use rule::AdviceRule;
pub use rule::AdviceRuleParseError;
pub use rule::AnchorSelector;
pub use rule::BoundedK;
pub use rule::BoundedN;
pub use rule::PropEqSpec;
pub use rule::RuleSlug;
pub use rule::ScoringTemplate;
pub use rule::TagOverlapRecencySpec;
pub use rule::parse_advice_rule;
pub use status::AdviceRuleStatus;
pub use status::AdviceRuleStatusHandle;
pub use synthesis::AdviceSynthesisError;
pub use synthesis::ReconcileOutcome;
pub use synthesis::SynthesizedMatview;
pub use synthesis::reconcile_advice_rule;
pub use synthesis::synthesize_matview;

/// The bundled lessons-for-tasks rule (ADR 0022 v1 cut). Shipped here as a
/// crate asset for the DDL snapshot test; final vault seeding (as an org block
/// under `assets/default/`) is wired in Increment F Step 6 — see the report's
/// open items.
pub const BUNDLED_LESSONS_FOR_TASKS_YAML: &str = include_str!("../assets/lessons_for_tasks.yaml");

#[cfg(test)]
mod tests {
    use super::*;

    fn task_lessons_rule() -> AdviceRule {
        parse_advice_rule(BUNDLED_LESSONS_FOR_TASKS_YAML).expect("bundled rule parses")
    }

    #[test]
    fn bundled_rule_round_trips() {
        let rule = task_lessons_rule();
        assert_eq!(rule.name.as_str(), "lessons_for_tasks");
        assert!(rule.active);
        assert_eq!(rule.k.get(), 5);
        assert_eq!(rule.anchor, AnchorSelector::HasTag("task".to_string()));
        let ScoringTemplate::TagOverlapRecency(spec) = &rule.candidates;
        assert_eq!(spec.source, AnchorSelector::HasTag("lesson".to_string()));
    }

    #[test]
    fn parse_refuses_unknown_field() {
        let yaml = "name: r\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\nbogus: 1\n";
        let err = parse_advice_rule(yaml).expect_err("unknown field must be refused");
        assert!(matches!(err, AdviceRuleParseError::Yaml { .. }));
        assert!(err.to_string().contains("bogus") || err.to_string().contains("unknown"));
    }

    #[test]
    fn parse_refuses_k_over_cap() {
        let yaml = "name: r\nactive: true\nk: 11\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\n";
        let err = parse_advice_rule(yaml).expect_err("k=11 must be refused");
        assert!(err.to_string().contains("1..=10"), "got: {err}");
    }

    #[test]
    fn parse_refuses_reserved_raw_query() {
        let yaml = "name: r\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\nraw_query:\n  sql: \
                    SELECT 1\n";
        let err = parse_advice_rule(yaml).expect_err("reserved raw_query must be refused");
        assert!(err.to_string().contains("reserved"), "got: {err}");
    }

    #[test]
    fn parse_allows_null_reserved_raw_query() {
        let yaml = "name: r\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\nraw_query: null\n";
        parse_advice_rule(yaml).expect("null raw_query is unset, allowed");
    }

    #[test]
    fn parse_accepts_retrieval_width_n() {
        let yaml = "name: r\nactive: true\nk: 3\nn: 20\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\n";
        let rule = parse_advice_rule(yaml).expect("k=3, n=20 must parse");
        assert_eq!(rule.n().get(), 20);
        assert_eq!(rule.k.get(), 3);
    }

    #[test]
    fn parse_refuses_n_over_cap() {
        let yaml = "name: r\nactive: true\nk: 3\nn: 51\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\n";
        let err = parse_advice_rule(yaml).expect_err("n=51 must be refused");
        assert!(err.to_string().contains("1..=50"), "got: {err}");
    }

    #[test]
    fn parse_refuses_k_over_n() {
        let yaml = "name: r\nactive: true\nk: 8\nn: 5\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\n";
        let err = parse_advice_rule(yaml).expect_err("k=8 > n=5 must be refused");
        assert!(matches!(
            err,
            AdviceRuleParseError::KGreaterThanN { k: 8, n: 5 }
        ));
        let msg = err.to_string();
        assert!(msg.contains("k=8") && msg.contains("n=5"), "got: {msg}");
    }

    #[test]
    fn parse_absent_n_defaults_to_k() {
        let rule = task_lessons_rule();
        assert_eq!(rule.n().get(), rule.k.get(), "absent n resolves to k");
        assert_eq!(rule.n().get(), 5);
    }

    #[test]
    fn parse_refuses_reserved_rerank() {
        let yaml = "name: r\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\nrerank:\n  profile: \
                    bge-reranker\n";
        let err = parse_advice_rule(yaml).expect_err("reserved rerank must be refused");
        assert!(err.to_string().contains("reserved"), "got: {err}");
    }

    #[test]
    fn parse_allows_null_reserved_rerank() {
        let yaml = "name: r\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\nrerank: null\n";
        parse_advice_rule(yaml).expect("null rerank is unset, allowed");
    }

    #[test]
    fn parse_refuses_bad_slug() {
        let yaml = "name: Bad-Slug\nactive: true\nk: 3\nanchor:\n  has_tag: task\ncandidates:\n  \
                    tag_overlap_recency:\n    source:\n      has_tag: lesson\n";
        let err = parse_advice_rule(yaml).expect_err("uppercase/hyphen slug must be refused");
        assert!(err.to_string().contains("slug"), "got: {err}");
    }

    fn lower(sel: &AnchorSelector, id_expr: &str, raw_alias: &str) -> lowering::SqlFragments {
        let mut seq = 0usize;
        sel.lower(id_expr, raw_alias, &mut seq).unwrap()
    }

    #[test]
    fn anchor_has_tag_lowers_to_inner_join() {
        let frags = lower(
            &AnchorSelector::HasTag("task".to_string()),
            "atg.block_id",
            "a",
        );
        assert_eq!(
            frags.joins,
            vec![
                "JOIN block_tags ht0 ON ht0.block_id = atg.block_id AND ht0.tag = 'task'"
                    .to_string()
            ]
        );
        assert!(
            frags.predicates.is_empty(),
            "tag membership is a join, not a WHERE"
        );
        assert!(
            !frags.needs_block_raw,
            "has_tag reads block_tags, not block_raw"
        );
    }

    #[test]
    fn anchor_entity_lowers_to_block_type_predicate() {
        let frags = lower(
            &AnchorSelector::Entity(holon_api::EntityName::new("note")),
            "atg.block_id",
            "a",
        );
        assert_eq!(frags.predicates, vec!["a.block_type = 'note'".to_string()]);
        assert!(frags.joins.is_empty());
        assert!(frags.needs_block_raw, "entity reads block_raw.block_type");
    }

    #[test]
    fn anchor_prop_eq_lowers_to_json_extract() {
        let frags = lower(
            &AnchorSelector::PropEq(PropEqSpec {
                key: "status".to_string(),
                value: "open".to_string(),
            }),
            "ctg.block_id",
            "c",
        );
        assert_eq!(
            frags.predicates,
            vec!["json_extract(c.properties, '$.status') = 'open'".to_string()]
        );
        assert!(frags.needs_block_raw);
    }

    #[test]
    fn anchor_and_composes_joins_and_predicates_with_unique_aliases() {
        let sel = AnchorSelector::And(vec![
            AnchorSelector::HasTag("task".to_string()),
            AnchorSelector::HasTag("urgent".to_string()),
            AnchorSelector::Entity(holon_api::EntityName::new("note")),
        ]);
        let frags = lower(&sel, "atg.block_id", "a");
        assert_eq!(
            frags.joins,
            vec![
                "JOIN block_tags ht0 ON ht0.block_id = atg.block_id AND ht0.tag = 'task'"
                    .to_string(),
                "JOIN block_tags ht1 ON ht1.block_id = atg.block_id AND ht1.tag = 'urgent'"
                    .to_string(),
            ]
        );
        assert_eq!(frags.predicates, vec!["a.block_type = 'note'".to_string()]);
        assert!(frags.needs_block_raw);
    }

    #[test]
    fn lowering_refuses_injection_in_tag() {
        let mut seq = 0usize;
        let err = AnchorSelector::HasTag("x' OR '1'='1".to_string())
            .lower("atg.block_id", "a", &mut seq)
            .expect_err("quote must be refused");
        assert!(matches!(err, LoweringError::UnsafeValue { .. }));
    }

    #[test]
    fn lowering_refuses_empty_and() {
        let mut seq = 0usize;
        assert_eq!(
            AnchorSelector::And(vec![]).lower("atg.block_id", "a", &mut seq),
            Err(LoweringError::EmptyAnd)
        );
    }

    #[test]
    fn lowering_refuses_bad_prop_key() {
        let mut seq = 0usize;
        let err = AnchorSelector::PropEq(PropEqSpec {
            key: "a.b; DROP".to_string(),
            value: "x".to_string(),
        })
        .lower("atg.block_id", "a", &mut seq)
        .expect_err("bad key must be refused");
        assert!(matches!(err, LoweringError::UnsafeKey { .. }));
    }

    /// Snapshot of the synthesized DDL for the bundled rule. If this changes,
    /// the IVM shape changed — update deliberately and re-check against a
    /// live matview.
    #[test]
    fn bundled_rule_ddl_snapshot() {
        let synth = synthesize_matview(&task_lessons_rule()).expect("synthesize");
        assert_eq!(synth.view_name, "advice_rule_lessons_for_tasks");
        let expected = "SELECT\n    atg.block_id AS anchor_id,\n    ctg.block_id AS lesson_id,\n    COUNT(*) \
             AS shared_tag_count\nFROM block_tags atg\nJOIN block_tags ctg ON ctg.tag = atg.tag \
             AND ctg.block_id <> atg.block_id\nJOIN block_tags ht0 ON ht0.block_id = atg.block_id \
             AND ht0.tag = 'task'\nJOIN block_tags ht1 ON ht1.block_id = ctg.block_id AND ht1.tag \
             = 'lesson'\nGROUP BY atg.block_id, ctg.block_id";
        assert_eq!(synth.select_sql, expected);
    }

    #[test]
    fn discovery_marker_matches() {
        assert!(is_advice_rule_block(Some("holon_advice_rule_yaml")));
        assert!(!is_advice_rule_block(Some("holon_entity_profile_yaml")));
        assert!(!is_advice_rule_block(None));
    }

    #[test]
    fn discovered_rule_carries_error() {
        let bad = parse_discovered_rule("block:1", "name: 9\nnot valid rule");
        assert!(bad.result.is_err());
        assert_eq!(bad.block_id, "block:1");
    }
}
