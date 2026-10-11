//! A declared invariant's repair policy is judged where the profile is loaded
//! (ruling P-conv.a): only a constant delta on the subject itself, or
//! `disclose`, is accepted. Every other policy is refused with its reason,
//! because two replicas that each repair a merged violation must write the
//! same thing.

use holon_api::Value;
use holon_profiles::OnViolation;
use holon_profiles::parse_profile_yaml;

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
