use super::*;

#[test]
fn private_fields_are_unrepresentable() {
    for (field, private) in holon_pattern::schema::BLOCK.private_fields() {
        assert_eq!(
            BlockWriteField::parse(field),
            Err(BlockWriteFieldError::Private {
                field: field.to_string(),
                route: private.route,
            })
        );
    }
}

#[test]
fn the_legacy_untyped_block_type_is_refused() {
    let refused = BlockWriteField::BlockType
        .parse_value(crate::Value::String("text".to_string()))
        .expect_err("\"text\" reads as untyped, so writing it must be refused");
    assert!(
        refused.contains("block_type") && refused.contains("\"text\""),
        "{refused}"
    );
}

#[test]
fn order_keys_are_unrepresentable() {
    assert_eq!(
        BlockWriteField::parse("after_block_id"),
        Err(BlockWriteFieldError::OrderKey("after_block_id".to_string()))
    );
}

#[test]
fn storage_internal_fields_are_rejected() {
    for field in [
        "id",
        "depth",
        "completed",
        "created_at",
        "updated_at",
        "_change_origin",
    ] {
        assert_eq!(
            BlockWriteField::parse(field),
            Err(BlockWriteFieldError::StorageInternal(field.to_string())),
            "{field} must be rejected"
        );
    }
    assert_eq!(
        BlockWriteField::parse("_expected_content"),
        Err(BlockWriteFieldError::StorageInternal(
            "_expected_content".to_string()
        ))
    );
}

/// The refusal must be actionable: it names the parameter, the offending
/// name, and the per-property route that replaces it (ruling D126.a).
#[test]
fn the_whole_properties_bag_is_unrepresentable() {
    let field = holon_pattern::schema::BLOCK
        .field("properties")
        .expect("the block schema declares the bag")
        .name;
    let err = BlockWriteField::parse(field)
        .expect_err("a whole-bag set_field carries no per-property kind and must be refused");
    assert_eq!(err, BlockWriteFieldError::WholeBag(field.to_string()));

    let msg = err.to_string();
    assert!(
        msg.contains("set_field(\"properties\")"),
        "the refusal must name the operation and the offending name as the caller spelled \
             them, got: {msg}"
    );
    assert!(
        msg.contains("'field' parameter"),
        "the refusal must name the offending PARAMETER, got: {msg}"
    );
    assert!(
        msg.contains("property key") && msg.contains("one"),
        "the refusal must name the per-PROPERTY route a caller switches to, got: {msg}"
    );
}

/// The order-rekey control key must not be writable as a property in VALUE
/// position. `set_field(field="_order_rekeys", value=<object>)` would
/// otherwise land the control key inside `properties` where the SQL
/// provider's key-position filter cannot strip it, and the Loro→SQL
/// projection would re-inject it as a live params key on the next flush.
#[test]
fn operation_control_keys_are_rejected() {
    // Reference the consts, not the literals — the routing/expected prefixes
    // are archlint-guarded string forms.
    for field in [
        crate::entity::ORDER_REKEYS_PARAM,
        crate::entity::ROUTING_DOC_URI_KEY,
        crate::entity::POSITION_AFTER_BLOCK_ID_PARAM,
    ] {
        assert!(
            BlockWriteField::parse(field).is_err(),
            "{field} is operation-control metadata and must be refused at the intent boundary"
        );
    }
}

#[test]
fn empty_field_is_rejected() {
    assert_eq!(BlockWriteField::parse(""), Err(BlockWriteFieldError::Empty));
}

#[test]
fn known_fields_parse_to_their_variant() {
    assert_eq!(
        BlockWriteField::parse("content"),
        Ok(BlockWriteField::Content)
    );
    assert_eq!(BlockWriteField::parse("tags"), Ok(BlockWriteField::Tags));
    assert_eq!(
        BlockWriteField::parse("task_state"),
        Ok(BlockWriteField::TaskState)
    );
}

#[test]
fn unknown_names_parse_as_validated_properties() {
    match BlockWriteField::parse("DEADLINE") {
        Ok(BlockWriteField::Property(key)) => assert_eq!(key.as_str(), "DEADLINE"),
        other => panic!("expected Property, got {other:?}"),
    }
}

#[test]
fn as_str_round_trips() {
    for field in ["content", "marks", "task_state", "DEADLINE"] {
        assert_eq!(BlockWriteField::parse(field).unwrap().as_str(), field);
    }
}
