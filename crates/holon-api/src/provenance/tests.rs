use super::*;

#[test]
fn rule_origin_carries_transition_id() {
    let origin = OpOrigin::Rule {
        transition_id: "rule:delegate-work".to_string(),
    };
    let stamp = ProvenanceStamp::from_origin(&origin, 1_700_000_000_000);
    assert_eq!(stamp.origin, "rule");
    assert_eq!(stamp.transition_id.as_deref(), Some("rule:delegate-work"));
    assert_eq!(stamp.session_id, None);
    assert_eq!(stamp.at_millis, 1_700_000_000_000);
}

#[test]
fn agent_origin_carries_session_and_tool_call() {
    let origin = OpOrigin::Agent {
        session_id: "sess-1".to_string(),
        tool_call_id: "call-42".to_string(),
    };
    let stamp = ProvenanceStamp::from_origin(&origin, 42);
    assert_eq!(stamp.origin, "agent");
    assert_eq!(stamp.session_id.as_deref(), Some("sess-1"));
    assert_eq!(stamp.tool_call_id.as_deref(), Some("call-42"));
    assert_eq!(stamp.transition_id, None);
}

#[test]
fn value_round_trip_is_lossless() {
    for origin in [
        OpOrigin::User,
        OpOrigin::Sync,
        OpOrigin::Ingest,
        OpOrigin::Rule {
            transition_id: "t1".to_string(),
        },
        OpOrigin::Agent {
            session_id: "s".to_string(),
            tool_call_id: "c".to_string(),
        },
    ] {
        let stamp = ProvenanceStamp::from_origin(&origin, 99);
        let parsed = ProvenanceStamp::from_value(&stamp.to_value()).unwrap();
        assert_eq!(stamp, parsed, "round trip for origin {origin:?}");
    }
}

#[test]
fn from_value_fails_loud_on_non_object() {
    let err = ProvenanceStamp::from_value(&Value::String("nope".to_string()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("must be an object"), "got: {err}");
}

#[test]
fn from_value_fails_loud_on_missing_origin() {
    let mut map = HashMap::new();
    map.insert(KEY_AT_MILLIS.to_string(), Value::Integer(1));
    let err = ProvenanceStamp::from_value(&Value::Object(map))
        .unwrap_err()
        .to_string();
    assert!(err.contains(KEY_ORIGIN), "got: {err}");
}
