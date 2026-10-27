use super::*;

fn uri(s: &str) -> EntityUri {
    EntityUri::parse_owned(s.to_string()).unwrap()
}

#[test]
fn block_content_accessors_and_display() {
    let text = BlockContent::text("hi");
    assert_eq!(text.as_text(), Some("hi"));
    assert!(text.as_source().is_none());
    assert_eq!(text.to_plain_text(), "hi");
    assert_eq!(format!("{text}"), "hi");

    let src = BlockContent::source("python", "print(1)");
    assert_eq!(src.as_text(), None);
    let sb = src.as_source().expect("source variant");
    assert_eq!(sb.language.as_deref(), Some("python"));
    assert_eq!(sb.source, "print(1)");
    assert_eq!(src.to_plain_text(), "print(1)");
    assert_eq!(format!("{src}"), "[python] print(1)");

    let named = SourceBlock::new("holon_prql", "from blocks")
        .with_name("q1")
        .with_header_arg("results", "table");
    assert!(named.is_prql());
    assert_eq!(named.name.as_deref(), Some("q1"));
    assert_eq!(
        named.get_header_arg("results"),
        Some(&Value::String("table".to_string()))
    );
    assert_eq!(named.get_header_arg("nope"), None);
    assert!(!SourceBlock::new("python", "x").is_prql());
}

#[test]
fn new_rich_carries_parent_id() {
    let rich = Block::new_rich(
        uri("block:r1"),
        uri("block:parent"),
        "rich text",
        Vec::<MarkSpan>::new(),
    );
    assert_eq!(rich.id, uri("block:r1"));
    // Constructor must carry the passed parent_id (not drop it to default).
    assert_eq!(rich.parent_id, uri("block:parent"));
    assert_ne!(rich.parent_id, EntityUri::no_parent());
    assert_eq!(rich.content, "rich text");
}

#[test]
fn block_predicates_titles_and_mime() {
    let mut b = Block::new_text(uri("block:b1"), EntityUri::no_parent(), "line1\nline2");
    assert_eq!(b.title(), "line1");
    assert_eq!(b.content_text(), "line1\nline2");
    assert!(!b.is_page());
    b.set_page(true);
    assert!(b.is_page());
    assert!(b.tags.contains(PAGE_TAG));
    b.set_page(false);
    assert!(!b.is_page());

    let src = Block::new_source(uri("block:s1"), uri("block:b1"), "holon_prql", "from x");
    // Constructor must carry the passed parent_id (not drop it to default).
    assert_eq!(src.parent_id, uri("block:b1"));
    assert_ne!(src.parent_id, EntityUri::no_parent());
    assert!(src.is_source_block());
    assert!(src.is_prql_block());
    assert!(!b.is_source_block());
    assert!(!b.is_prql_block());
    let py = Block::new_source(uri("block:s2"), uri("block:b1"), "python", "x");
    assert!(!py.is_prql_block());

    let img = Block::new_image(uri("block:i1"), uri("block:b1"), "attachments/pic.png");
    // Constructor must carry the passed id (not drop it to a random default).
    assert_eq!(img.id, uri("block:i1"));
    assert_eq!(img.parent_id, uri("block:b1"));
    assert!(img.is_image_block());
    assert_eq!(img.image_mime(), Some("image/png"));
    let jpg = Block::new_image(uri("block:i2"), uri("block:b1"), "a/b.JPG");
    // extension matching is exact-case; unknown ext falls back
    let jpg_lower = Block::new_image(uri("block:i3"), uri("block:b1"), "a/b.jpg");
    assert_eq!(jpg_lower.image_mime(), Some("image/jpeg"));
    let weird = Block::new_image(uri("block:i4"), uri("block:b1"), "a/b.xyz");
    assert_eq!(weird.image_mime(), Some("application/octet-stream"));
    assert_eq!(b.image_mime(), None);
    let _ = jpg;

    // Properties round-trip.
    let mut p = Block::new_text(uri("block:p1"), EntityUri::no_parent(), "x");
    p.set_property("k", "v");
    assert_eq!(p.get_property("k"), Some(Value::String("v".to_string())));
    assert_eq!(p.get_property_str("k"), Some("v".to_string()));
    assert_eq!(p.get_property_str("missing"), None);
    assert_eq!(p.properties_map().len(), 1);
    p.set_properties_map(HashMap::new());
    assert!(p.properties_map().is_empty());

    let meta = BlockMetadata {
        created_at: 11,
        updated_at: 22,
    };
    p.set_metadata(meta.clone());
    assert_eq!(p.metadata(), meta);
}

#[test]
fn block_content_roundtrip_and_depth() {
    let rich = Block::new_rich(uri("block:r1"), EntityUri::no_parent(), "txt", vec![]);
    assert!(matches!(
        rich.to_block_content(),
        BlockContent::RichText { ref text, .. } if text == "txt"
    ));

    let mut b = Block::new_text(uri("block:t1"), EntityUri::no_parent(), "old");
    b.set_block_content(BlockContent::source("python", "code"));
    assert!(b.is_source_block());
    assert_eq!(b.content_text(), "code");

    let from = Block::from_block_content(
        uri("block:f1"),
        EntityUri::no_parent(),
        BlockContent::text("abc"),
    );
    assert_eq!(from.content_text(), "abc");
    assert_eq!(from.to_block_content(), BlockContent::text("abc"));

    // depth chain: b3 -> b2 -> b1 (root)
    let b1 = Block::new_text(uri("block:b1"), EntityUri::no_parent(), "1");
    let b2 = Block::new_text(uri("block:b2"), uri("block:b1"), "2");
    let b3 = Block::new_text(uri("block:b3"), uri("block:b2"), "3");
    let lookup: HashMap<&str, &Block> = [("b1", &b1), ("b2", &b2), ("b3", &b3)]
        .into_iter()
        .collect();
    assert_eq!(b1.depth_from(|id| lookup.get(id).copied()), 0);
    assert_eq!(b2.depth_from(|id| lookup.get(id).copied()), 1);
    assert_eq!(b3.depth_from(|id| lookup.get(id).copied()), 2);
}

#[test]
fn blocks_by_document_groups_descendants_under_page() {
    let mut page = Block::new_text(uri("block:pg"), EntityUri::no_parent(), "Page");
    page.set_page(true);
    let c1 = Block::new_text(uri("block:c1"), uri("block:pg"), "c1");
    let c2 = Block::new_text(uri("block:c2"), uri("block:c1"), "c2");

    let grouped = blocks_by_document(&[page.clone(), c1.clone(), c2.clone()]);
    let entry = grouped
        .iter()
        .find(|(id, _)| id.as_str() == page.id.as_str())
        .expect("page entry present");
    let mut ids: Vec<&str> = entry.1.iter().map(|b| b.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["block:c1", "block:c2"]);
}

#[test]
fn try_from_row_is_strict_about_required_columns() {
    fn base_row() -> crate::StorageEntity {
        [
            ("id", Value::String("block:b1".to_string())),
            ("parent_id", Value::Null),
            ("content", Value::String("hello".to_string())),
            ("content_type", Value::String("text".to_string())),
            ("created_at", Value::Integer(1)),
            ("updated_at", Value::Integer(2)),
            ("tags", Value::Array(vec![Value::String("a".to_string())])),
            ("requires", Value::Array(vec![])),
            ("advice_suppressed", Value::Array(vec![])),
            ("contributes_to", Value::Array(vec![])),
            ("collapsed", Value::Integer(0)),
            ("widget_only", Value::Integer(0)),
            ("block_type", Value::Null),
        ]
        .into_iter()
        .map(|(k, v)| (std::sync::Arc::<str>::from(k), v))
        .collect()
    }

    let ok = Block::try_from(base_row()).expect("valid row parses");
    assert_eq!(ok.content, "hello");
    assert_eq!(ok.created_at, 1);
    assert_eq!(ok.updated_at, 2);
    assert!(ok.tags.contains("a"));
    assert!(ok.requires.is_empty());
    assert!(ok.advice_suppressed.is_empty());
    assert!(!ok.collapsed);
    assert!(!ok.widget_only);
    assert_eq!(ok.block_type, None);
    assert!(ok.parent_id.as_block_id().is_none());

    let mut typed = base_row();
    typed.insert(
        std::sync::Arc::<str>::from("block_type"),
        Value::String("note".to_string()),
    );
    assert_eq!(
        Block::try_from(typed).expect("typed row parses").block_type,
        Some(EntityName::new("note"))
    );
    let mut no_block_type = base_row();
    no_block_type.remove("block_type");
    let err = Block::try_from(no_block_type).unwrap_err().to_string();
    assert!(err.contains("block_type"), "got: {err}");
    let mut legacy = base_row();
    legacy.insert(
        std::sync::Arc::<str>::from("block_type"),
        Value::String("text".to_string()),
    );
    assert_eq!(
        Block::try_from(legacy)
            .expect("legacy row parses")
            .block_type,
        None
    );
    let mut bad_block_type = base_row();
    bad_block_type.insert(std::sync::Arc::<str>::from("block_type"), Value::Integer(5));
    assert!(Block::try_from(bad_block_type).is_err());

    // `collapsed` is stored as SQLite INTEGER 0/1 (turso_value_to_value
    // never produces Value::Boolean on read) — a folded row must parse true.
    let mut folded = base_row();
    folded.insert(std::sync::Arc::<str>::from("collapsed"), Value::Integer(1));
    assert!(
        Block::try_from(folded)
            .expect("folded row parses")
            .collapsed
    );

    // Absent collapsed column defaults to expanded (unlike tags/requires,
    // there's no cross-reader COALESCE contract for this new scalar
    // column — see `optional_bool`).
    let mut no_collapsed = base_row();
    no_collapsed.remove("collapsed");
    assert!(
        !Block::try_from(no_collapsed)
            .expect("parses without collapsed")
            .collapsed
    );

    // `widget_only` mirrors `collapsed`: INTEGER 0/1 on read, absent
    // column defaults to false (see `optional_bool`).
    let mut widget = base_row();
    widget.insert(
        std::sync::Arc::<str>::from("widget_only"),
        Value::Integer(1),
    );
    assert!(
        Block::try_from(widget)
            .expect("widget_only row parses")
            .widget_only
    );

    let mut no_widget = base_row();
    no_widget.remove("widget_only");
    assert!(
        !Block::try_from(no_widget)
            .expect("parses without widget_only")
            .widget_only
    );

    // Absent advice_suppressed column = broken projection, must error.
    let mut no_advice = base_row();
    no_advice.remove("advice_suppressed");
    let err = Block::try_from(no_advice).unwrap_err().to_string();
    assert!(err.contains("advice_suppressed"), "got: {err}");

    // Absent tags column = broken projection, must error mentioning the column.
    let mut no_tags = base_row();
    no_tags.remove("tags");
    let err = Block::try_from(no_tags).unwrap_err().to_string();
    assert!(err.contains("tags"), "got: {err}");

    // Non-string array element must error.
    let mut bad_elem = base_row();
    bad_elem.insert(
        std::sync::Arc::<str>::from("tags"),
        Value::Array(vec![Value::Integer(1)]),
    );
    let err = Block::try_from(bad_elem).unwrap_err().to_string();
    assert!(err.contains("non-string"), "got: {err}");

    // JSON-string form is accepted.
    let mut json_tags = base_row();
    json_tags.insert(
        std::sync::Arc::<str>::from("tags"),
        Value::String("[\"x\",\"y\"]".to_string()),
    );
    let parsed = Block::try_from(json_tags).expect("json tags parse");
    assert!(parsed.tags.contains("x") && parsed.tags.contains("y"));

    // Wrong-typed created_at must error.
    let mut bad_ts = base_row();
    bad_ts.insert(
        std::sync::Arc::<str>::from("created_at"),
        Value::Boolean(true),
    );
    let err = Block::try_from(bad_ts).unwrap_err().to_string();
    assert!(err.contains("created_at"), "got: {err}");

    // Missing content must error.
    let mut no_content = base_row();
    no_content.remove("content");
    let err = Block::try_from(no_content).unwrap_err().to_string();
    assert!(err.contains("content"), "got: {err}");
}
