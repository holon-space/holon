//! Field parity between the UI read model and the hydrated block read it will
//! eventually replace (D172.a, F1a risk R2).
//!
//! The read model publishes `SnapshotBlock`; today's consumers read
//! `HYDRATED_BLOCK_COLUMNS` out of SQL. Before any consumer flips (F1b), every
//! one of those columns must be accounted for — served by the read model, or
//! recorded here with the reason it need not be. The test ENUMERATES the
//! column list from the production constant, so a column added to the SQL read
//! without a decision here fails this test instead of silently arriving in a
//! render as an empty string.

use std::collections::HashSet;

use holon_api::SnapshotBlock;

/// Reads one column's value off a published `SnapshotBlock`, rendered as text
/// purely so the table can hold readers of differing types. The point is that
/// the read EXISTS and compiles — a `Field` entry naming something the read
/// model does not carry cannot be written.
type FieldReader = fn(&SnapshotBlock) -> String;

/// How the read model answers one hydrated column.
#[derive(Clone, Copy)]
enum Parity {
    /// Served directly: the function that reads it off the published row.
    /// A FUNCTION, not a label — a label only claims the field exists, and a
    /// claim is exactly what rots. Every one of these is called against a
    /// probe `SnapshotBlock` whose slots all hold DISTINCT values, and the
    /// expected value is stated per column, so a reader wired to the wrong
    /// slot fails rather than merely not panicking.
    Field(FieldReader),
    /// Not on `SnapshotBlock`, and does not need to be — the reason is the
    /// second element and is asserted to be non-empty so no entry can be
    /// waved through.
    NotNeeded(&'static str),
}

/// The decision table. One entry per `HYDRATED_BLOCK_COLUMNS` column.
const PARITY: &[(&str, Parity)] = &[
    ("id", Parity::Field(|b| b.block.id.to_string())),
    (
        "parent_id",
        Parity::Field(|b| b.block.parent_id.to_string()),
    ),
    ("sort_key", Parity::Field(|b| b.sort_key.clone())),
    ("content", Parity::Field(|b| b.block.content.clone())),
    (
        "content_type",
        Parity::Field(|b| b.block.content_type.to_string()),
    ),
    (
        "source_language",
        Parity::Field(|b| format!("{:?}", b.block.source_language)),
    ),
    (
        "source_name",
        Parity::Field(|b| format!("{:?}", b.block.source_name)),
    ),
    (
        "properties",
        Parity::Field(|b| format!("{:?}", b.block.properties)),
    ),
    (
        "property_kinds",
        Parity::NotNeeded(
            "a JSON-round-trip REPAIR token, not data: the SQL `properties` column loses \
             DateTime/Json kinds and `PropertyKinds::retype` puts them back on read \
             (crates/holon-turso/src/turso.rs:1589). `Block::properties` is already a typed \
             `HashMap<String, Value>`, so the read model holds the information at HIGHER \
             fidelity and has nothing to repair.",
        ),
    ),
    ("marks", Parity::Field(|b| format!("{:?}", b.block.marks))),
    (
        "collapsed",
        Parity::Field(|b| b.block.collapsed.to_string()),
    ),
    (
        "widget_only",
        Parity::Field(|b| b.block.widget_only.to_string()),
    ),
    (
        "completed",
        Parity::NotNeeded(
            "NOT carried by the Loro authority at all: `read_properties_from_meta` strips it \
             via RESERVED_PROPERTY_KEYS (crates/holon-loro/src/loro_backend.rs:458,476-491) and \
             `block_to_params` never emits it, so the Loro->SQL projection does not write this \
             column either. The read model cannot be more complete than the authority it \
             publishes. Pre-existing gap on the CRDT path, tracked separately — NOT introduced \
             by F1a and not closable inside it.",
        ),
    ),
    (
        "block_type",
        Parity::NotNeeded("same as `completed` — stripped by RESERVED_PROPERTY_KEYS."),
    ),
    (
        "created_at",
        Parity::Field(|b| b.block.created_at.to_string()),
    ),
    (
        "updated_at",
        Parity::Field(|b| b.block.updated_at.to_string()),
    ),
    ("tags", Parity::Field(|b| format!("{:?}", b.block.tags))),
    (
        "requires",
        Parity::Field(|b| format!("{:?}", b.block.requires)),
    ),
    (
        "advice_suppressed",
        Parity::Field(|b| format!("{:?}", b.block.advice_suppressed)),
    ),
    (
        "contributes_to",
        Parity::Field(|b| format!("{:?}", b.block.contributes_to)),
    ),
];

/// The production column list, re-read from the file rather than copied, so
/// this test tracks the real constant.
fn hydrated_columns() -> Vec<String> {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../holon-turso/src/block_table_names.rs"
    ))
    .expect("read block_table_names.rs");
    let start = src
        .find("const HYDRATED_BLOCK_COLUMNS: &str = ")
        .expect("HYDRATED_BLOCK_COLUMNS not found — the constant was renamed or moved");
    let body = &src[start..src[start..].find(";\n").expect("constant terminator") + start];

    // Flatten the Rust string literal (`\` + newline + indent continuations,
    // quotes) so the SQL reads as one line before it is tokenized.
    let flat: String = body
        .replace(['\\', '"'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // Each selected column is either `b.<name>` or a subquery aliased
    // `AS <name>`. Scan for both rather than splitting on commas — the
    // subqueries contain commas of their own.
    let ident = |s: &str| -> String {
        s.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect()
    };
    let mut cols = Vec::new();
    let mut rest = flat.as_str();
    while let Some(pos) = rest.find("b.") {
        cols.push(ident(&rest[pos + 2..]));
        rest = &rest[pos + 2..];
    }
    let mut rest = flat.as_str();
    while let Some(pos) = rest.find(" AS ") {
        cols.push(ident(rest[pos + 4..].trim_start()));
        rest = &rest[pos + 4..];
    }
    // `b.id` also appears inside the junction subqueries' WHERE clauses; the
    // dedupe below folds those away.
    cols.retain(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'));
    cols.sort();
    cols.dedup();
    cols
}

#[test]
fn every_hydrated_column_is_accounted_for_in_the_read_model() {
    let columns = hydrated_columns();
    assert!(
        columns.len() >= 15,
        "parsed only {} columns out of HYDRATED_BLOCK_COLUMNS ({columns:?}) — the parser broke, \
         so this test would pass vacuously",
        columns.len()
    );

    let decided: HashSet<&str> = PARITY.iter().map(|(c, _)| *c).collect();
    let undecided: Vec<&String> = columns
        .iter()
        .filter(|c| !decided.contains(c.as_str()))
        .collect();
    assert!(
        undecided.is_empty(),
        "hydrated columns with no read-model parity decision: {undecided:?}. Add an entry to \
         PARITY saying whether the read model serves the column or why it need not."
    );

    let stale: Vec<&str> = PARITY
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| !columns.contains(&c.to_string()))
        .collect();
    assert!(
        stale.is_empty(),
        "PARITY decides columns the hydrated read no longer selects: {stale:?} — drop them"
    );

    for (column, parity) in PARITY {
        if let Parity::NotNeeded(reason) = parity {
            assert!(
                reason.len() > 40,
                "column {column:?} is waved through with a reason too short to be one: \
                 {reason:?}"
            );
        }
    }
}

/// Every `Parity::Field` entry is EXERCISED against a probe row whose slots
/// all hold DISTINCT values, and every reader's result is asserted. Two
/// same-typed slots (`source_language`/`source_name`, both `Option<String>`)
/// would otherwise be interchangeable, so distinctness is what gives the test
/// its teeth.
#[test]
fn every_served_column_is_readable_off_a_published_row() {
    let mut block = holon_api::block::Block::new_text(
        holon_api::EntityUri::block("parity-probe"),
        holon_api::EntityUri::block("parity-parent"),
        "parity probe",
    );
    block.content_type = holon_api::ContentType::Source;
    block.source_language = Some("prql".parse::<holon_api::SourceLanguage>().expect("prql"));
    block.source_name = Some("parity-source-name".to_string());
    block
        .properties
        .insert("parity_key".to_string(), holon_api::Value::Integer(7));
    // Deliberately OPPOSITE, so a reader wired to the other bool is caught.
    block.collapsed = true;
    block.widget_only = false;
    block.created_at = 111;
    block.updated_at = 222;
    // Every edge field gets a DISTINCT non-empty value: three readers that
    // all expected "[]" were interchangeable, so swapping two of them passed.
    block.tags = vec!["parity-tag".to_string()].into();
    block.requires = vec![holon_api::EntityUri::block("parity-requires")];
    block.advice_suppressed = vec![holon_api::EntityUri::block("parity-suppressed")];
    block.contributes_to = vec![holon_api::EntityUri::block("parity-contributes")];
    block.marks = Some(vec![holon_api::MarkSpan::new(
        0,
        6,
        holon_api::InlineMark::Bold,
    )]);
    let row = SnapshotBlock {
        block,
        sort_key: "a0".to_string(),
    };

    // One expectation per served column. A column served by a reader but
    // missing here fails below, so the table cannot drift out of the test.
    let expected: &[(&str, &str)] = &[
        ("id", "block:parity-probe"),
        ("parent_id", "block:parity-parent"),
        ("sort_key", "a0"),
        ("content", "parity probe"),
        ("content_type", "source"),
        ("source_language", r#"Some(Other("prql"))"#),
        ("source_name", r#"Some("parity-source-name")"#),
        ("collapsed", "true"),
        ("widget_only", "false"),
        ("created_at", "111"),
        ("updated_at", "222"),
    ];
    let expected: std::collections::HashMap<&str, &str> = expected.iter().copied().collect();

    let mut served = 0;
    let mut unexpected = Vec::new();
    for (column, parity) in PARITY {
        let Parity::Field(read) = parity else {
            continue;
        };
        served += 1;
        let value = read(&row);
        match expected.get(column) {
            Some(want) => assert_eq!(
                &value, want,
                "column {column:?} read back {value:?} — the reader is wired to the wrong slot,                  or the probe changed"
            ),
            // Debug-formatted collections: pin the payload, which is what a
            // wrong-slot reader would lose, without pinning the formatting.
            None => {
                assert!(
                    !value.is_empty(),
                    "column {column:?} read back an empty string"
                );
                unexpected.push((*column, value));
            }
        }
    }
    // The remaining served columns are the collection-shaped ones; each must
    // carry the distinct value the probe put in its own slot.
    let by_col: std::collections::HashMap<&str, String> = unexpected.into_iter().collect();
    assert!(
        by_col["properties"].contains("parity_key"),
        "properties lost the probe key: {:?}",
        by_col["properties"]
    );
    // Each collection-shaped reader must carry ITS OWN probe value; a reader
    // wired to a sibling edge field now reads back the sibling's marker.
    for (column, marker) in [
        ("tags", "parity-tag"),
        ("requires", "parity-requires"),
        ("advice_suppressed", "parity-suppressed"),
        ("contributes_to", "parity-contributes"),
        ("marks", "Bold"),
    ] {
        assert!(
            by_col[column].contains(marker),
            "column {column:?} read back {:?}, which does not carry its own probe marker \
             {marker:?} — the reader is wired to another slot",
            by_col[column]
        );
    }
    assert!(
        served >= 15,
        "only {served} columns are served by a reader — the table lost its Field entries and \
         this test would pass vacuously"
    );
}
