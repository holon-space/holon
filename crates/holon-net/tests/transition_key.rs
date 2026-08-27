//! A transition's identity must survive a recompile: reports name transitions
//! by key, so deriving the same sources in a different order must yield equal
//! reports, and two sources claiming one key must be refused.

use holon_api::OperationDescriptor;
use holon_net::RuleAcceptance;
use holon_net::RuleSource;
use holon_net::TransitionKey;
use holon_net::derive_net;
use holon_rules::parse_holon_rule;

fn block_catalog() -> Vec<OperationDescriptor> {
    let mut ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    ops.extend(holon_core::__operations_block_operations::block_operations(
        "block", "block", "block", "id",
    ));
    ops
}

fn rule(block_id: &str, name: &str, page: &str) -> RuleSource {
    let rule = parse_holon_rule(&format!(
        r#"
name: {name}
when: 'not block_exists("{page}/{{today}}")'
emit:
  place: page({page})
  name: "{{today}}"
"#
    ))
    .expect("the rule parses");
    RuleSource {
        block_id: block_id.to_string(),
        acceptance: RuleAcceptance::Running(rule),
    }
}

fn rules() -> Vec<RuleSource> {
    vec![
        rule("block:rule-journal", "daily_journal", "journals"),
        rule("block:rule-standup", "daily_standup", "standups"),
    ]
}

/// The reports are a function of the declared sources, not of the order
/// `all_providers()` happened to register them in.
#[test]
fn reports_do_not_depend_on_the_derivation_order() {
    let ops = block_catalog();
    let rules = rules();

    let forward = derive_net(&ops, &rules).expect("forward order compiles");

    let mut reversed_ops = ops.clone();
    reversed_ops.reverse();
    let mut reversed_rules = rules.clone();
    reversed_rules.reverse();
    let reversed = derive_net(&reversed_ops, &reversed_rules).expect("reverse order compiles");

    assert_eq!(
        holon_net::conflicts(&forward),
        holon_net::conflicts(&reversed),
        "the conflict report must name transitions by a derivation-order-independent identity"
    );
    assert_eq!(
        holon_net::cycles(&forward),
        holon_net::cycles(&reversed),
        "the cycle report must name transitions by a derivation-order-independent identity"
    );
}

/// Two providers claiming the same `entity.op` is a loud failure, never a
/// silent last-wins or a net carrying two transitions under one identity.
#[test]
fn two_descriptors_with_the_same_entity_and_op_are_refused() {
    let ops = block_catalog();
    let set_field = ops
        .iter()
        .find(|op| op.name == "set_field")
        .expect("the catalog advertises set_field");
    let duplicate = set_field.clone();

    let outcome = derive_net(&[set_field.clone(), duplicate], &[]);
    let measured = outcome.as_ref().map(|net| {
        net.transitions
            .iter()
            .map(|t| t.source.clone())
            .collect::<Vec<_>>()
    });
    assert!(
        outcome.is_err(),
        "duplicate (entity, op) must be an error; today: {measured:?}"
    );
}

/// `classify_for_net` refuses a dotted non-marker entity at the catalog, but
/// `derive_net` is a public entry point of its own: a descriptor that reaches
/// it anyway must come back as a typed error naming the offender, never as a
/// panic through the library boundary.
#[test]
fn a_descriptor_with_a_dotted_entity_is_a_typed_error() {
    let ops = block_catalog();
    let mut dotted = ops
        .iter()
        .find(|op| op.name == "set_field")
        .expect("the catalog advertises set_field")
        .clone();
    dotted.entity_name = holon_api::EntityName::new("orgmode.import");

    let err = derive_net(&[dotted], &[]).expect_err("a dotted entity has no transition key");
    assert!(
        matches!(
            &err,
            holon_net::NetCompileError::UnkeyableOperation { op, source }
                if op == "set_field"
                    && source == &holon_net::NetError::DottedEntityName {
                        entity: "orgmode.import".to_string(),
                    }
        ),
        "{err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("set_field") && message.contains("orgmode.import"),
        "the error must name both halves of the offending pair: {message}"
    );
}

/// Two rules sharing a display name are distinct transitions — the rule's
/// identity is its block, not its name.
#[test]
fn two_rules_sharing_a_name_are_distinct_transitions() {
    let same_name = vec![
        rule("block:rule-a", "daily_journal", "journals"),
        rule("block:rule-b", "daily_journal", "standups"),
    ];
    let net = derive_net(&[], &same_name).expect("distinct blocks compile");
    assert_eq!(net.transitions.len(), 2);
}

/// The rendered key is the join key an MCP client correlates a report entry
/// with a transition by. Changing either form breaks that wire contract.
#[test]
fn the_key_serializes_as_its_rendered_string() {
    let op = TransitionKey::operation("block", "set_field").expect("a dotless entity keys");
    let rule = TransitionKey::rule("block:rule-daily-journal");
    assert_eq!(op.to_string(), "op:block.set_field");
    assert_eq!(rule.to_string(), "rule:block:rule-daily-journal");
    assert_eq!(
        serde_json::to_string(&op).unwrap(),
        r#""op:block.set_field""#
    );
    assert_eq!(
        serde_json::to_string(&rule).unwrap(),
        r#""rule:block:rule-daily-journal""#
    );
    let back: TransitionKey = serde_json::from_str(r#""op:block.set_field""#).unwrap();
    assert_eq!(back, op);
}

/// A report is a payload an MCP client reads: it must carry the rendered keys
/// verbatim and resolve back through the net it came from.
#[test]
fn reports_carry_the_rendered_keys_and_resolve_against_the_net() {
    let net = derive_net(&block_catalog(), &rules()).expect("the catalog and rules compile");
    let report = holon_net::conflicts(&net);
    let json = serde_json::to_string(&report).expect("the report serializes");
    assert!(
        json.contains(r#""op:block.set_field""#),
        "the report names set_field by its rendered key: {json}"
    );
    assert!(
        json.contains(r#""rule:block:rule-journal""#),
        "the report names the journal rule by its rendered key: {json}"
    );

    for key in report
        .contentions
        .iter()
        .flat_map(|c| c.writers.iter().chain(&c.readers))
        .chain(&report.unanalyzable)
    {
        assert!(
            net.transition(key).is_some(),
            "{key} must resolve against the net it was derived from"
        );
    }

    let back: holon_net::ConflictReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, report);
}

/// One block cannot host two rules — a repeated block id is the same
/// refusal as a repeated `entity.op`.
#[test]
fn two_rules_from_one_block_are_refused() {
    let same_block = vec![
        rule("block:rule-a", "daily_journal", "journals"),
        rule("block:rule-a", "daily_standup", "standups"),
    ];
    assert!(
        derive_net(&[], &same_block).is_err(),
        "one block hosts at most one rule"
    );
}

/// A block the watcher could not parse is declared automation whose
/// declaration cannot be read: it enters the net `active: false` and
/// `Unanalyzable`, never as an absence (ADR 0032 fail-closed).
#[test]
fn an_unparseable_rule_block_is_inactive_and_unanalyzable() {
    let opaque = RuleSource {
        block_id: "block:rule-broken".to_string(),
        acceptance: RuleAcceptance::Opaque {
            reason: "parse failed: mapping values are not allowed here".to_string(),
        },
    };
    let net = derive_net(&[], &[opaque]).expect("an opaque rule still compiles");

    let key = TransitionKey::rule("block:rule-broken");
    let transition = net
        .transition(&key)
        .expect("a rule the watcher refused must still be a transition");

    assert!(
        matches!(
            &transition.source,
            holon_net::TransitionSource::Rule { active: false, .. }
        ),
        "a refused rule is not active; got {:?}",
        transition.source
    );
    assert_eq!(
        transition.analyzability,
        holon_net::Analyzability::Unanalyzable {
            undeclared: vec![
                holon_net::UndeclaredHalf::Arcs,
                holon_net::UndeclaredHalf::MarkingDelta,
            ],
        },
        "no parsed rule means neither half is declared",
    );
    assert!(
        transition.arcs.is_empty(),
        "an unreadable declaration yields no arcs, and the Unanalyzable slot is what says so",
    );
}

/// A rule the watcher parsed but parked keeps its full arc declaration — only
/// `active` records that it does not fire.
#[test]
fn a_parked_rule_stays_analyzable_but_inactive() {
    let running = rule("block:rule-journal", "daily_journal", "journals");
    let parked = RuleSource {
        block_id: "block:rule-parked".to_string(),
        acceptance: RuleAcceptance::Parked {
            rule: match rule("block:rule-parked", "parked_journal", "parked").acceptance {
                RuleAcceptance::Running(rule) => rule,
                other => panic!("the fixture builds a running rule; got {other:?}"),
            },
            reason: "block-subject operate rules are not reactively wired".to_string(),
        },
    };
    let net = derive_net(&[], &[running, parked]).expect("both rules compile");

    let parked = net
        .transition(&TransitionKey::rule("block:rule-parked"))
        .expect("the parked rule is in the net");
    assert_eq!(parked.analyzability, holon_net::Analyzability::Analyzable);
    assert!(
        !parked.arcs.is_empty(),
        "a parked rule's declaration is still readable, so it still has arcs",
    );
    assert!(matches!(
        &parked.source,
        holon_net::TransitionSource::Rule { active: false, .. }
    ));

    let running = net
        .transition(&TransitionKey::rule("block:rule-journal"))
        .expect("the running rule is in the net");
    assert!(matches!(
        &running.source,
        holon_net::TransitionSource::Rule { active: true, .. }
    ));
}
