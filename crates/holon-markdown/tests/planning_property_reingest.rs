//! Re-ingest of an edited priority, planning line or `key:: value` property,
//! through each adapter's half of the controller's update step
//! (`FileSyncController`'s per-block loop): the `content_differs` gate, then
//! `build_block_params` with the previous parse. The file is authoritative, so
//! the op must carry the file's new value — or the removal sentinel when the
//! file dropped it.
//!
//! Entry `2026-09-26-markdown-ingest-never-updates-planning-or-property-changes`.

use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::ObsidianMarkdownAdapter;

fn parse_one(adapter: &dyn FileFormatAdapter, source: &str) -> (Block, EntityUri) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Groceries.md");
    std::fs::write(&path, source).unwrap();
    let parsed = adapter
        .parse(&path, source, &EntityUri::no_parent(), &root)
        .unwrap();
    let block = parsed
        .blocks
        .iter()
        .find(|b| b.content.contains("milk"))
        .unwrap_or_else(|| panic!("no milk block in {:?}", parsed.blocks))
        .clone();
    (block, parsed.document.id.clone())
}

/// The update op the controller emits for the milk block when the file goes
/// from `before` to `after`; panics when the gate says nothing changed.
fn reingest(adapter: &dyn FileFormatAdapter, before: &str, after: &str) -> StorageEntity {
    let (old, doc) = parse_one(adapter, before);
    let (new, doc_after) = parse_one(adapter, after);
    assert_eq!(old.id, new.id, "the edit changed the block's identity");
    assert_eq!(doc, doc_after, "the edit changed the document's identity");
    assert!(
        adapter.content_differs(&old, &new),
        "no update op for {before:?} -> {after:?}: the store keeps the old value"
    );
    adapter.build_block_params(&new, &doc, &doc, Some(&old))
}

fn assert_param(op: &StorageEntity, key: &str, expected: Value) {
    assert_eq!(op.get(key), Some(&expected), "wrong `{key}` in {op:?}");
}

const SCHEDULED: &str = "- TODO buy milk\n  SCHEDULED: <2026-01-01 Thu>\n";

#[test]
fn logseq_removed_scheduled_line_clears_scheduled() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        SCHEDULED,
        "- TODO buy milk\n",
    );
    assert_param(&op, "scheduled", Value::REMOVED);
}

#[test]
fn logseq_changed_scheduled_date_updates_scheduled() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        SCHEDULED,
        "- TODO buy milk\n  SCHEDULED: <2026-02-02 Mon>\n",
    );
    assert_param(&op, "scheduled", Value::String("<2026-02-02 Mon>".into()));
}

#[test]
fn logseq_removed_deadline_line_clears_deadline() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO buy milk\n  DEADLINE: <2026-01-01 Thu>\n",
        "- TODO buy milk\n",
    );
    assert_param(&op, "deadline", Value::REMOVED);
}

#[test]
fn logseq_removed_priority_clears_priority() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO [#A] buy milk\n",
        "- TODO buy milk\n",
    );
    assert_param(&op, "priority", Value::REMOVED);
}

#[test]
fn logseq_changed_priority_updates_priority() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO [#A] buy milk\n",
        "- TODO [#B] buy milk\n",
    );
    assert_param(&op, "priority", Value::Integer(2));
}

#[test]
fn obsidian_removed_priority_clears_priority() {
    let op = reingest(
        &ObsidianMarkdownAdapter::new(),
        "- [ ] [#A] buy milk\n",
        "- [ ] buy milk\n",
    );
    assert_param(&op, "priority", Value::REMOVED);
}

#[test]
fn logseq_removed_property_clears_it() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- buy milk\n  aisle:: 3\n",
        "- buy milk\n",
    );
    assert_param(&op, "aisle", Value::REMOVED);
}

#[test]
fn logseq_changed_property_updates_it() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- buy milk\n  aisle:: 3\n",
        "- buy milk\n  aisle:: 4\n",
    );
    assert_param(&op, "aisle", Value::String("4".into()));
}

#[test]
fn logseq_added_property_is_stored() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- buy milk\n",
        "- buy milk\n  aisle:: 3\n",
    );
    assert_param(&op, "aisle", Value::String("3".into()));
}

#[test]
fn a_create_carries_the_property_and_no_absent_field() {
    let adapter = LogseqMarkdownAdapter::new();
    let (block, doc) = parse_one(&adapter, "- buy milk\n  aisle:: 3\n");
    let create = adapter.build_block_params(&block, &doc, &doc, None);
    assert_param(&create, "aisle", Value::String("3".into()));
    for absent in ["priority", "scheduled", "deadline"] {
        assert!(!create.contains_key(absent), "{absent} in {create:?}");
    }
}

#[test]
fn an_unchanged_block_emits_no_op() {
    let adapter = LogseqMarkdownAdapter::new();
    let source = "- TODO [#A] buy milk\n  SCHEDULED: <2026-01-01 Thu>\n  aisle:: 3\n";
    let (old, _) = parse_one(&adapter, source);
    let (new, _) = parse_one(&adapter, source);
    assert!(!adapter.content_differs(&old, &new));
}

/// `contributes-to::` and `tags::` spell typed edges: the parser lifts them
/// into the edge, and the params carry them there, not as properties.
#[test]
fn edge_spelled_properties_reach_their_typed_edges() {
    let adapter = LogseqMarkdownAdapter::new();
    let (block, doc) = parse_one(
        &adapter,
        "- buy milk\n  contributes-to:: groceries\n  Tags:: work\n",
    );
    assert_eq!(
        block.contributes_to,
        vec![EntityUri::block("groceries")],
        "{block:?}"
    );
    assert!(block.tags.contains("work"), "{block:?}");
    let create = adapter.build_block_params(&block, &doc, &doc, None);
    assert_param(
        &create,
        "contributes_to",
        Value::Array(vec![Value::String("block:groceries".into())]),
    );
    assert_param(
        &create,
        "tags",
        Value::Array(vec![Value::String("work".into())]),
    );
    for spelled in ["contributes-to", "Tags"] {
        assert!(!create.contains_key(spelled), "{spelled} in {create:?}");
    }
}

#[test]
fn logseq_removed_edge_property_clears_the_edge() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- buy milk\n  contributes-to:: groceries\n",
        "- buy milk\n",
    );
    assert_param(&op, "contributes_to", Value::Array(Vec::new()));
}

/// An edge value that names no block refuses the file rather than vanishing.
#[test]
fn an_unparseable_edge_value_refuses_the_file() {
    let adapter = LogseqMarkdownAdapter::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let path = root.join("Groceries.md");
    let source = "- buy milk\n  contributes-to:: {{aisle}}\n";
    std::fs::write(&path, source).unwrap();
    let Err(err) = adapter.parse(&path, source, &EntityUri::no_parent(), &root) else {
        panic!("a slot outside a template names no block, yet the file parsed");
    };
    assert!(
        format!("{err:#}").contains("contributes-to"),
        "the error must name the key: {err:#}"
    );
}
