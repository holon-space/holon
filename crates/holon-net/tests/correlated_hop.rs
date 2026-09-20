//! A relational hop compiles to a correlated arc, not to guard residue.
//!
//! The arc language reaches one entity: an arc names a place and a refinement
//! evaluated against the subject's own row. A guard that reaches a SECOND
//! entity — `parent(...)`, `child(...)` — therefore had nowhere to land and
//! fell to [`holon_net::GuardResidue`], which the evaluator can only answer
//! `Unknown` for.
//!
//! Built as an AST rather than parsed: the guard TEXT surface refuses a
//! `block.<column>` comparison beside a block predicate, a gap in the text
//! grammar rather than in the arc language.

use holon_net::NetCompileError;
use holon_net::guards::classify_guard;
use holon_pattern::Value;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::OpGuard;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;

/// The shape `MoveGuard`'s machinery-containment clause rests on: reach the
/// parent, then test a cell of it.
#[test]
fn a_parent_hop_conjunct_compiles_to_an_arc_rather_than_residue() {
    let guard = Guard {
        subject: Subject::Block,
        body: Pattern::Parent(Box::new(Pattern::Field {
            field: FieldRef::Column {
                relation: "block".to_string(),
                name: "source_language".to_string(),
            },
            op: CmpOp::Eq,
            rhs: Operand::Lit(Value::String("holon_rule".to_string())),
        })),
    };
    let classified = classify_guard(&guard, "op:block.probe").expect("within the bounds");
    assert!(
        classified.residue.is_empty(),
        "a parent hop must compile to a correlated arc, not to residue: {:?}",
        classified.residue
    );
    assert_eq!(
        classified.modes.len(),
        1,
        "no disjunction, so exactly one firing mode"
    );
    assert_eq!(
        classified.modes[0].hops.len(),
        1,
        "the hop is a correlated group"
    );
}

/// One over-budget USER rule must cost that rule its analyzability and
/// nothing else. A rule block is user content discovered reactively, so a
/// three-deep hop in one block must not make every `derived_net()` call fail.
#[test]
fn an_over_budget_rule_does_not_take_the_whole_net_down() {
    let over_budget = holon_rules::parse_holon_rule(
        "name: over_budget\nwhen: 'parent(parent(parent(has_tag(\"Page\"))))'\n",
    )
    .expect("a three-deep hop is legal guard text");
    let ordinary =
        holon_rules::parse_holon_rule("name: ordinary_rule\nwhen: 'has_tag(\"Page\")'\n")
            .expect("an ordinary rule parses");

    let net = holon_net::derive_net(
        &[],
        &[
            holon_net::RuleSource {
                block_id: "block:over-budget".to_string(),
                acceptance: holon_net::RuleAcceptance::Running(over_budget),
            },
            holon_net::RuleSource {
                block_id: "block:ordinary".to_string(),
                acceptance: holon_net::RuleAcceptance::Running(ordinary),
            },
        ],
    )
    .expect("one over-budget rule must not fail the whole derivation");

    let find = |key: &str| {
        net.transitions
            .iter()
            .find(|t| t.key().as_str() == key)
            .unwrap_or_else(|| panic!("no transition {key} in the net"))
    };

    let bad = find("rule:block:over-budget");
    assert!(
        matches!(
            bad.analyzability,
            holon_net::Analyzability::Unanalyzable { .. }
        ),
        "the over-budget rule is 'cannot say', not absent: {:?}",
        bad.analyzability
    );
    assert_eq!(
        bad.residue.first().map(|r| r.cause),
        Some(holon_net::guards::ResidueCause::HopChainTooLong),
        "and it carries the typed reason so the census can name it"
    );

    let good = find("rule:block:ordinary");
    assert_eq!(
        good.analyzability,
        holon_net::Analyzability::Analyzable,
        "the neighbouring rule is untouched"
    );
}

/// The other side of that line. A `#[require(...)]` on a shipped trait is
/// SOURCE, not user content: nobody's vault can author it and no run-time
/// disclosure can help, so an over-budget one is a programming error and must
/// fail the whole derivation where a build or a test sees it.
#[test]
fn a_trait_declared_guard_over_budget_fails_the_whole_derivation() {
    let mut ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    let victim = ops
        .iter_mut()
        .find(|op| op.name == "set_field")
        .expect("the crud catalog declares set_field");
    victim.guard = OpGuard::parse("parent(parent(parent(has_tag(\"Page\"))))")
        .expect("a three-deep hop is legal guard text");

    // Matched rather than `expect_err`: the Ok side is the whole block
    // catalog, and dumping it would bury the failure it is reporting.
    let error = match holon_net::derive_net(&ops, &[]) {
        Err(error) => error,
        Ok(net) => panic!(
            "an over-budget SHIPPED guard must not degrade quietly; the derivation returned {} \
             transitions instead",
            net.transitions.len()
        ),
    };
    match error {
        NetCompileError::HopChainTooLong {
            transition, depth, ..
        } => {
            assert_eq!(transition, "op:block.set_field", "the error names the op");
            assert_eq!(depth, 3);
        }
        other => panic!("expected the typed bound error, got {other:?}"),
    }
}
