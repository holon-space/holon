use super::*;

fn sample() -> ProposalRecord {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String("block:x".to_string()));
    params.insert("content".to_string(), Value::String("hello".to_string()));
    ProposalRecord::pending(EntityName::new("block"), "create", params)
}

#[test]
fn value_round_trip_is_lossless() {
    let record = sample();
    let parsed = ProposalRecord::from_value(&record.to_value()).unwrap();
    assert_eq!(record, parsed);
}

#[test]
fn resolved_round_trip_carries_resolver() {
    let record = sample().resolved(ProposalStatus::Accepted, Value::String("user".to_string()));
    let parsed = ProposalRecord::from_value(&record.to_value()).unwrap();
    assert_eq!(parsed.status, ProposalStatus::Accepted);
    assert_eq!(parsed.resolved_by, Some(Value::String("user".to_string())));
}

#[test]
fn from_value_fails_loud_on_missing_op() {
    let mut map = HashMap::new();
    map.insert(KEY_STATUS.to_string(), Value::String("pending".to_string()));
    map.insert(KEY_ENTITY.to_string(), Value::String("block".to_string()));
    map.insert(KEY_PARAMS.to_string(), Value::Object(HashMap::new()));
    let err = ProposalRecord::from_value(&Value::Object(map))
        .unwrap_err()
        .to_string();
    assert!(err.contains(KEY_OP), "got: {err}");
}

#[test]
fn from_value_fails_loud_on_an_unnameable_entity() {
    let mut value = sample().to_value();
    let Value::Object(map) = &mut value else {
        unreachable!("a proposal record is an object")
    };
    map.insert(KEY_ENTITY.to_string(), Value::String("a b".to_string()));
    let err = ProposalRecord::from_value(&value).unwrap_err().to_string();
    assert!(err.contains(KEY_ENTITY), "got: {err}");
    assert!(err.contains("is not a valid entity name"), "got: {err}");
}

#[test]
fn unknown_status_fails_loud() {
    let err = ProposalStatus::parse("maybe").unwrap_err().to_string();
    assert!(err.contains("maybe"), "got: {err}");
}
