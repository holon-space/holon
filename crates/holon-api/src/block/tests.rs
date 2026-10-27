use super::*;

#[test]
fn block_schema_has_correct_jsonb_fields() {
    let schema = Block::type_definition();

    assert!(
        schema.field_is_jsonb("properties"),
        "properties should be JSONB"
    );

    assert!(!schema.field_is_jsonb("id"), "id should NOT be JSONB");
    assert!(
        !schema.field_is_jsonb("content"),
        "content should NOT be JSONB"
    );
    assert!(
        !schema.field_is_jsonb("parent_id"),
        "parent_id should NOT be JSONB"
    );
}

/// BUG H1 regression: the file-sync diff base / projection sidecar serde-
/// persists `SnapshotBlock`, which embeds a `Block` whose `tags`/`requires`
/// are `#[serde(skip)]`. Without the `SnapshotBlockWire` DTO this
/// round-trip drops both edge fields, so a cold boot re-emits spurious
/// edge-field writes for every tagged block. Must carry NON-empty
/// `tags` AND `requires`.
#[test]
fn snapshot_block_serde_round_trip_preserves_edge_fields() {
    let expected_tags: Tags = vec!["Inbox".to_string(), "Page".to_string()].into();
    let expected_requires = vec![EntityUri::block("dep1"), EntityUri::block("dep2")];

    let mut block = Block::new_text(
        EntityUri::block("h1"),
        EntityUri::no_parent(),
        "x".to_string(),
    );
    block.tags = expected_tags.clone();
    block.requires = expected_requires.clone();
    let snap = SnapshotBlock {
        block,
        sort_key: "a0".to_string(),
    };

    let bytes = serde_json::to_vec(&snap).expect("serialize SnapshotBlock");
    let back: SnapshotBlock = serde_json::from_slice(&bytes).expect("deserialize SnapshotBlock");

    assert_eq!(back.block.tags, expected_tags);
    assert_eq!(back.block.requires, expected_requires);
    assert_eq!(back.sort_key, "a0");
}
