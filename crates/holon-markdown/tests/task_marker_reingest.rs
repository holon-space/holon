//! Re-ingest of an edited task marker, through each adapter's half of the
//! controller's update step (`FileSyncController`'s per-block loop): the
//! `content_differs` gate, then `build_block_params` with the previous parse.
//! The file is authoritative, so the op must carry the file's new marker — or
//! the removal sentinel when the file dropped it.
//!
//! Entry `2026-09-26-markdown-ingest-never-updates-a-changed-task-marker`.

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
/// from `before` to `after`; `None` when the gate says nothing changed.
fn reingest(adapter: &dyn FileFormatAdapter, before: &str, after: &str) -> Option<StorageEntity> {
    let (old, doc) = parse_one(adapter, before);
    let (new, doc_after) = parse_one(adapter, after);
    assert_eq!(old.id, new.id, "the edit changed the block's identity");
    assert_eq!(doc, doc_after, "the edit changed the document's identity");
    adapter
        .content_differs(&old, &new)
        .then(|| adapter.build_block_params(&new, &doc, &doc, Some(&old)))
}

fn assert_task_state_removed(op: Option<StorageEntity>, shape: &str) {
    let op = op.unwrap_or_else(|| {
        panic!("{shape}: no update op — the store keeps the task state the file dropped")
    });
    assert_eq!(
        op.get("task_state"),
        Some(&Value::REMOVED),
        "{shape}: the op does not clear task_state: {op:?}"
    );
    assert_eq!(
        op.get("task_state_category"),
        Some(&Value::REMOVED),
        "{shape}: the op does not clear task_state_category: {op:?}"
    );
}

fn assert_task_state(op: Option<StorageEntity>, expected: &str, shape: &str) {
    let op = op.unwrap_or_else(|| panic!("{shape}: no update op"));
    assert_eq!(
        op.get("task_state"),
        Some(&Value::String(expected.into())),
        "{shape}: wrong task_state in {op:?}"
    );
}

#[test]
fn logseq_removed_todo_clears_the_task_state() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO buy milk\n",
        "- buy milk\n",
    );
    assert_task_state_removed(op, "logseq TODO removed");
}

#[test]
fn logseq_done_to_plain_clears_the_task_state() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- DONE buy milk\n",
        "- buy milk\n",
    );
    assert_task_state_removed(op, "logseq DONE to plain");
}

#[test]
fn logseq_todo_to_done_updates_the_task_state() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO buy milk\n",
        "- DONE buy milk\n",
    );
    assert_task_state(op, "DONE", "logseq TODO to DONE");
}

#[test]
fn obsidian_removed_checkbox_clears_the_task_state() {
    let op = reingest(
        &ObsidianMarkdownAdapter::new(),
        "- [x] buy milk\n",
        "- buy milk\n",
    );
    assert_task_state_removed(op, "obsidian checkbox removed");
}

#[test]
fn obsidian_unchecked_checkbox_reopens_the_task() {
    let op = reingest(
        &ObsidianMarkdownAdapter::new(),
        "- [x] buy milk\n",
        "- [ ] buy milk\n",
    );
    assert_task_state(op, "TODO", "obsidian checked to unchecked");
}

#[test]
fn an_unchanged_marker_emits_no_op() {
    assert_eq!(
        reingest(
            &LogseqMarkdownAdapter::new(),
            "- TODO buy milk\n",
            "- TODO buy milk\n"
        ),
        None
    );
    assert_eq!(
        reingest(
            &ObsidianMarkdownAdapter::new(),
            "- [x] buy milk\n",
            "- [x] buy milk\n"
        ),
        None
    );
}

#[test]
fn a_content_only_edit_keeps_the_marker() {
    let op = reingest(
        &LogseqMarkdownAdapter::new(),
        "- TODO buy milk\n",
        "- TODO buy oat milk\n",
    );
    assert_task_state(op, "TODO", "logseq content-only edit");
    let op = reingest(
        &ObsidianMarkdownAdapter::new(),
        "- [x] buy milk\n",
        "- [x] buy oat milk\n",
    );
    assert_task_state(op, "DONE", "obsidian content-only edit");
}

#[test]
fn a_plain_block_never_carries_a_task_state_key() {
    let adapter = LogseqMarkdownAdapter::new();
    let op = reingest(&adapter, "- buy milk\n", "- buy oat milk\n").expect("content changed");
    assert!(!op.contains_key("task_state"), "{op:?}");
    assert!(!op.contains_key("task_state_category"), "{op:?}");

    let (created, doc) = parse_one(&adapter, "- buy milk\n");
    let create = adapter.build_block_params(&created, &doc, &doc, None);
    assert!(!create.contains_key("task_state"), "{create:?}");
    assert!(!create.contains_key("task_state_category"), "{create:?}");
}
