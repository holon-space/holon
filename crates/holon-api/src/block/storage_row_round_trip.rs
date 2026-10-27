use proptest::prelude::*;

use super::*;
use crate::inline_mark::InlineMark;

fn arb_uri() -> impl Strategy<Value = EntityUri> {
    "[a-z0-9]{1,8}".prop_map(|s| EntityUri::block(&s))
}

fn arb_block() -> impl Strategy<Value = Block> {
    (
        (
            arb_uri(),
            prop::option::of(arb_uri()),
            prop::collection::btree_set(
                prop::sample::select(vec!["Page", "decision", "two words", "x"]),
                0..4,
            ),
            prop::collection::vec(arb_uri(), 0..3),
            prop::collection::vec(arb_uri(), 0..3),
            prop::collection::vec(arb_uri(), 0..3),
        ),
        (
            "[a-z ]{0,12}",
            prop::collection::hash_map("[a-z]{1,5}", "[a-z0-9]{0,5}", 0..3),
            0..3u8,
            any::<bool>(),
            any::<bool>(),
            any::<i64>(),
            any::<i64>(),
        ),
    )
        .prop_map(
            |(
                (id, parent, tags, requires, advice_suppressed, contributes_to),
                (content, properties, marks_kind, collapsed, widget_only, created, updated),
            )| {
                let len = content.len();
                let mut block =
                    Block::new_text(id, parent.unwrap_or_else(EntityUri::no_parent), content);
                block.tags = Tags::from_tag_iter(tags.into_iter().map(str::to_string));
                block.requires = requires;
                block.advice_suppressed = advice_suppressed;
                block.contributes_to = contributes_to;
                block.properties = properties
                    .into_iter()
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect();
                block.marks = match marks_kind {
                    0 => None,
                    // The read boundary drops zero-width spans.
                    _ if len == 0 => Some(Vec::new()),
                    1 => Some(Vec::new()),
                    _ => Some(vec![MarkSpan::new(0, len, InlineMark::Bold)]),
                };
                block.collapsed = collapsed;
                block.widget_only = widget_only;
                block.created_at = created;
                block.updated_at = updated;
                block
            },
        )
}

proptest! {
    #[test]
    fn storage_row_parses_back_to_the_same_block(block in arb_block()) {
        let parsed = Block::try_from(block.to_storage_row())
            .map_err(|e| TestCaseError::fail(format!("{e:#}")))?;
        prop_assert_eq!(parsed, block);
    }
}
