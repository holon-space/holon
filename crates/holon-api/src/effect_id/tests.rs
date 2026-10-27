use std::sync::Arc;

use super::*;

fn row(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

#[test]
fn id_is_stable_across_calls() {
    let rule = RuleId::new("journals::action::0");
    let key = FiringKey::from_row(&row(&[("name", Value::String("2026-07-10".into()))]));
    let slot = OutputSlot::first();
    let a = deterministic_block_id(&rule, &key, &slot);
    let b = deterministic_block_id(&rule, &key, &slot);
    assert_eq!(a.as_str(), b.as_str());
    assert!(a.as_str().starts_with("block:"));
}

/// Checked-in id vector for a template instance. Every other test (and the
/// keystone) computes its expectation by calling
/// `deterministic_instance_id` itself, so without a literal here a
/// swapped namespace or reordered components would keep every one of
/// them green.
///
/// The second literal is the SAME name under [`HOLON_RULE_NAMESPACE`]: the
/// two namespaces must not mint the same id for the same triple.
#[test]
fn instance_id_matches_its_checked_in_vector() {
    let id = deterministic_instance_id("tpl:weekly", "rule:journals::action::0", "node:2");
    assert_eq!(id.as_str(), "block:3d756849-0c88-5cdd-ba0f-4191b5d2c9cc");
    assert_ne!(id.as_str(), "block:1b4bad95-db12-5da7-bb09-3d4300d2136a");
}

/// The separator keeps the component boundaries: the same characters split
/// differently across the three components must mint different ids.
/// (A component that itself contains `\x1f` can still collide — no caller
/// has one, and the ids are built from vault-authored identifiers.)
#[test]
fn instance_id_depends_on_where_the_components_are_split() {
    let a = deterministic_instance_id("ab", "c", "d");
    let b = deterministic_instance_id("a", "bc", "d");
    let c = deterministic_instance_id("a", "b", "cd");
    assert_ne!(a.as_str(), b.as_str());
    assert_ne!(b.as_str(), c.as_str());
    assert_ne!(a.as_str(), c.as_str());
}

#[test]
fn id_distinct_across_rule_key_slot() {
    let rule_a = RuleId::new("rule-a");
    let rule_b = RuleId::new("rule-b");
    let key_1 = FiringKey::from_row(&row(&[("name", Value::String("2026-07-10".into()))]));
    let key_2 = FiringKey::from_row(&row(&[("name", Value::String("2026-07-11".into()))]));
    let slot_0 = OutputSlot::first();
    let slot_1 = OutputSlot::nth(1);

    let base = deterministic_block_id(&rule_a, &key_1, &slot_0);
    // different rule
    assert_ne!(
        base.as_str(),
        deterministic_block_id(&rule_b, &key_1, &slot_0).as_str()
    );
    // different key
    assert_ne!(
        base.as_str(),
        deterministic_block_id(&rule_a, &key_2, &slot_0).as_str()
    );
    // two slots of one firing → two distinct ids
    assert_ne!(
        base.as_str(),
        deterministic_block_id(&rule_a, &key_1, &slot_1).as_str()
    );
}

#[test]
fn firing_key_is_order_independent_and_type_tagged() {
    let r1 = row(&[
        ("name", Value::String("d".into())),
        ("parent_id", Value::String("block:journals".into())),
    ]);
    let r2 = row(&[
        ("parent_id", Value::String("block:journals".into())),
        ("name", Value::String("d".into())),
    ]);
    assert_eq!(
        FiringKey::from_row(&r1).as_str(),
        FiringKey::from_row(&r2).as_str()
    );
    // Integer 1 and String "1" must not collide.
    let as_int = FiringKey::from_row(&row(&[("v", Value::Integer(1))]));
    let as_str = FiringKey::from_row(&row(&[("v", Value::String("1".into()))]));
    assert_ne!(as_int.as_str(), as_str.as_str());
}

#[test]
fn firing_key_excludes_internal_columns() {
    // CDC delivers _rowid as Integer on the Created path but String on the
    // Updated path; the key must be identical either way (and with no
    // _rowid at all), or the same day's journal gets path-dependent ids.
    let semantic = row(&[("name", Value::String("2026-07-10".into()))]);
    let created_path = row(&[
        ("name", Value::String("2026-07-10".into())),
        ("_rowid", Value::Integer(1)),
    ]);
    let updated_path = row(&[
        ("name", Value::String("2026-07-10".into())),
        ("_rowid", Value::String("1".into())),
    ]);
    assert_eq!(
        FiringKey::from_row(&semantic).as_str(),
        FiringKey::from_row(&created_path).as_str()
    );
    assert_eq!(
        FiringKey::from_row(&created_path).as_str(),
        FiringKey::from_row(&updated_path).as_str()
    );
}

#[test]
fn intent_key_is_stable_and_order_independent() {
    let fp_a = FiringKey::from_row(&row(&[
        ("id", Value::String("t1".into())),
        ("content", Value::String("buy milk".into())),
    ]));
    let fp_b = FiringKey::from_row(&row(&[
        ("content", Value::String("buy milk".into())),
        ("id", Value::String("t1".into())),
    ]));
    let k1 = deterministic_intent_key("todoist", "update-tasks", "t1", &fp_a, None);
    let k2 = deterministic_intent_key("todoist", "update-tasks", "t1", &fp_b, None);
    assert_eq!(k1, k2, "same intent (any param order) → same key");
}

#[test]
fn intent_key_distinct_across_components() {
    let fp = FiringKey::from_row(&row(&[("id", Value::String("t1".into()))]));
    let base = deterministic_intent_key("todoist", "update-tasks", "t1", &fp, None);
    assert_ne!(
        base,
        deterministic_intent_key("gmail", "update-tasks", "t1", &fp, None)
    );
    assert_ne!(
        base,
        deterministic_intent_key("todoist", "add-tasks", "t1", &fp, None)
    );
    assert_ne!(
        base,
        deterministic_intent_key("todoist", "update-tasks", "t2", &fp, None)
    );
    let fp2 = FiringKey::from_row(&row(&[
        ("id", Value::String("t1".into())),
        ("content", Value::String("x".into())),
    ]));
    assert_ne!(
        base,
        deterministic_intent_key("todoist", "update-tasks", "t1", &fp2, None)
    );
}

/// The compose id is a real component of the key: identical params under
/// two submissions must NOT collapse, and one submission must key the same
/// however often it is re-dispatched.
#[test]
fn intent_key_separates_submissions_but_not_retries() {
    let fp = FiringKey::from_row(&row(&[
        ("id", Value::String("sess-1".into())),
        ("message", Value::String("yes".into())),
    ]));
    let first = ComposeId::parse("compose-1").unwrap();
    let second = ComposeId::parse("compose-2").unwrap();

    let k1 = deterministic_intent_key("cc", "send_message", "sess-1", &fp, Some(&first));
    let retry = deterministic_intent_key("cc", "send_message", "sess-1", &fp, Some(&first));
    let k2 = deterministic_intent_key("cc", "send_message", "sess-1", &fp, Some(&second));

    assert_eq!(k1, retry, "a retry of ONE submission keeps its key");
    assert_ne!(k1, k2, "two submissions of identical text are two keys");
    assert_ne!(
        k1,
        deterministic_intent_key("cc", "send_message", "sess-1", &fp, None),
        "a submission-scoped key is distinct from the payload-only key"
    );
}

/// An empty compose id would key exactly like "no compose id", silently
/// restoring the collision. It is refused at the boundary instead.
#[test]
fn an_empty_compose_id_is_refused() {
    assert!(ComposeId::parse("").is_err());
    assert_eq!(ComposeId::parse("x").unwrap().as_str(), "x");
    assert_ne!(
        ComposeId::mint().as_str(),
        ComposeId::mint().as_str(),
        "each mint is a distinct submission"
    );
}
