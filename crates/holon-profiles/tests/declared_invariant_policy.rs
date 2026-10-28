//! A declared invariant's repair policy is judged where the profile is loaded
//! (ruling P-conv.a): only a constant delta on the subject itself, or
//! `disclose`, is accepted. Every other policy is refused with its reason,
//! because two replicas that each repair a merged violation must write the
//! same thing.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_api::Value;
use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_profiles::OnViolation;
use holon_profiles::TypeRegistry;
use holon_profiles::parse_profile_yaml;
use holon_profiles::vault_profiles::VaultProfileDerivation;

const HOLDS: &str = r#"block.task_state == "DONE" => not child(block.task_state != "DONE")"#;

fn profile_with(on_violation: &str) -> String {
    format!(
        "entity_name: block\n\
         invariants:\n  \
           done_parent_has_done_children:\n    \
             holds: '{HOLDS}'\n    \
             on_violation: {on_violation}\n"
    )
}

fn refusal(on_violation: &str) -> String {
    let err = parse_profile_yaml(&profile_with(on_violation))
        .err()
        .unwrap_or_else(|| panic!("the loader accepted the policy `{on_violation}`"));
    format!("{err:#}")
}

#[test]
fn a_repair_that_writes_the_violating_child_is_refused_with_its_reason() {
    let reason = refusal("{ repair: remove_child, target: witness, delete: true }");
    assert!(
        reason.contains("done_parent_has_done_children") && reason.contains("remove_child"),
        "the refusal must name the invariant and the policy: {reason}"
    );
    assert!(
        reason.contains("witness") && reason.contains("replica"),
        "the refusal must say why: the witness differs per replica: {reason}"
    );
}

#[test]
fn a_repair_whose_value_is_computed_is_refused() {
    let reason = refusal("{ repair: copy_child_state, set: { task_state: '= child_state' } }");
    assert!(
        reason.contains("copy_child_state") && reason.contains("constant"),
        "a computed value is not a constant delta: {reason}"
    );
}

#[test]
fn a_repair_that_deletes_the_subject_is_refused() {
    let reason = refusal("{ repair: drop_parent, delete: true }");
    assert!(
        reason.contains("drop_parent") && reason.contains("delet"),
        "a deletion of authored content is not a constant delta: {reason}"
    );
}

#[test]
fn a_repair_that_writes_an_undeclared_field_is_refused() {
    let reason = refusal("{ repair: reopen_parent, set: { taskstate: DOING } }");
    assert!(
        reason.contains("taskstate"),
        "the refusal must name the unknown field: {reason}"
    );
}

#[test]
fn a_repair_that_writes_nothing_is_refused() {
    let reason = refusal("{ repair: noop }");
    assert!(
        reason.contains("noop") && reason.contains("writes nothing"),
        "{reason}"
    );
}

#[test]
fn a_constant_delta_on_the_subject_is_accepted() {
    let profile = parse_profile_yaml(&profile_with(
        "{ repair: reopen_parent, set: { task_state: DOING } }",
    ))
    .expect("a constant delta on the subject itself is convergent");
    let [invariant] = profile.invariants.as_slice() else {
        panic!("expected one invariant, got {:?}", profile.invariants);
    };
    assert_eq!(invariant.name.as_str(), "done_parent_has_done_children");
    assert_eq!(invariant.holds_source, HOLDS);
    let OnViolation::Repair(repair) = &invariant.on_violation else {
        panic!("expected a repair, got {:?}", invariant.on_violation);
    };
    assert_eq!(repair.name, "reopen_parent");
    assert_eq!(
        repair.delta.get("task_state"),
        Some(&Value::String("DOING".to_string()))
    );
}

#[test]
fn disclose_is_accepted() {
    let profile = parse_profile_yaml(&profile_with("disclose")).expect("disclose is allowed");
    assert_eq!(profile.invariants[0].on_violation, OnViolation::Disclose);
}

#[test]
fn an_unknown_policy_keyword_is_refused() {
    let reason = refusal("ignore");
    assert!(reason.contains("ignore"), "{reason}");
}

#[test]
fn an_invariant_that_does_not_parse_is_refused_with_its_name() {
    let yaml = "entity_name: block\n\
                invariants:\n  \
                  broken:\n    \
                    holds: 'block.task_state == '\n    \
                    on_violation: disclose\n";
    let err = format!("{:#}", parse_profile_yaml(yaml).unwrap_err());
    assert!(err.contains("broken"), "{err}");
}

#[test]
fn an_invariant_over_another_relation_than_the_profile_entity_is_refused() {
    let yaml = "entity_name: block\n\
                invariants:\n  \
                  wrong_subject:\n    \
                    holds: 'integration.enabled == 1'\n    \
                    on_violation: disclose\n";
    let err = format!("{:#}", parse_profile_yaml(yaml).unwrap_err());
    assert!(err.contains("wrong_subject"), "{err}");
}

#[test]
fn every_valid_policy_shape_loads() {
    let reopen = || {
        OnViolation::Repair(holon_profiles::ConstantRepair {
            name: "reopen_parent".to_string(),
            delta: [("task_state".to_string(), Value::String("DOING".to_string()))].into(),
        })
    };
    let cases = [
        ("disclose", OnViolation::Disclose),
        ("'disclose'", OnViolation::Disclose),
        (
            "{ repair: reopen_parent, set: { task_state: DOING } }",
            reopen(),
        ),
        (
            "{ repair: reopen_parent, target: subject, set: { task_state: DOING }, delete: false }",
            reopen(),
        ),
    ];
    for (policy, expected) in cases {
        let profile = parse_profile_yaml(&profile_with(policy))
            .unwrap_or_else(|e| panic!("`{policy}` must load: {e:#}"));
        assert_eq!(profile.invariants[0].on_violation, expected, "`{policy}`");
    }
}

#[test]
fn every_malformed_policy_shape_is_refused_by_name() {
    let cases = [
        (
            "{ repiar: reopen_parent, set: { task_state: DOING } }",
            "unknown field `repiar`",
        ),
        ("{ set: { task_state: DOING } }", "missing field `repair`"),
        (
            "{ repair: reopen_parent, set: { task_state: DOING }, extra: 1 }",
            "unknown field `extra`",
        ),
        (
            "{ repair: reopen_parent, target: wtiness }",
            "unknown variant `wtiness`",
        ),
        ("{ repair: [reopen_parent] }", "sequence"),
        ("5", "integer `5`, expected an on_violation policy"),
        ("[disclose]", "positional `on_violation` list is refused"),
        (
            "[reopen_parent, subject, { task_state: DOING }, false]",
            "positional `on_violation` list is refused: write it keyed, as `{ repair: <name>, \
             target: subject, set: { <field>: <constant> }, delete: false }`",
        ),
    ];
    let wrong: Vec<String> = cases
        .into_iter()
        .filter_map(
            |(policy, needle)| match parse_profile_yaml(&profile_with(policy)) {
                Ok(profile) => Some(format!("`{policy}` loaded as {:?}", profile.invariants)),
                Err(e) if !format!("{e:#}").contains(needle) => Some(format!(
                    "`{policy}`: refusal must contain `{needle}`: {e:#}"
                )),
                Err(_) => None,
            },
        )
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The vault refuses the profile file that spells a policy positionally and
/// discloses it by file, while the keyed file next to it takes effect.
#[test]
fn a_positional_policy_refuses_its_file_and_spares_the_keyed_one() {
    let registry = Arc::new(TypeRegistry::new());
    registry
        .register(holon_api::block::Block::type_definition())
        .expect("register block");
    let bus = Arc::new(ConditionBus::new());
    let mut derivation =
        VaultProfileDerivation::new(registry.vault_profile_admitter(), Arc::clone(&bus));
    let keyed = "vault/.holon/types/keyed.yaml";
    let positional = "vault/.holon/types/positional.yaml";
    let rows: BTreeMap<String, Arc<String>> = [
        (
            keyed,
            profile_with("{ repair: reopen_parent, set: { task_state: DOING } }")
                .replace("done_parent_has_done_children", "reopens_parent"),
        ),
        (
            positional,
            profile_with("[reopen_parent, subject, { task_state: DOING }, false]"),
        ),
    ]
    .into_iter()
    .map(|(id, content)| (id.to_string(), Arc::new(content)))
    .collect();
    derivation.derive(&rows);

    let declared: Vec<String> = registry
        .declared_invariants("block")
        .iter()
        .map(|i| i.name.to_string())
        .collect();
    assert_eq!(declared, ["reopens_parent"]);
    let refusals: Vec<(String, String)> = bus
        .current()
        .into_iter()
        .map(|c| match c.reason {
            ConditionKind::ProfileRefused { error } => (c.subject, error),
            other => panic!("unexpected condition {other:?}"),
        })
        .collect();
    let [(subject, error)] = refusals.as_slice() else {
        panic!("exactly the positional file must be disclosed: {refusals:?}");
    };
    assert_eq!(subject, positional);
    for needle in [
        "invariants.done_parent_has_done_children",
        "positional `on_violation` list is refused",
        "{ repair: <name>, target: subject, set: { <field>: <constant> }, delete: false }",
    ] {
        assert!(
            error.contains(needle),
            "the disclosure must contain `{needle}`: {error}"
        );
    }
}
