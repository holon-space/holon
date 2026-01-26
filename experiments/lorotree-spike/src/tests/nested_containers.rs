//! Hypothesis: LoroTree node metadata (get_meta()) supports nested LoroText containers
//! for CRDT collaborative text editing within tree nodes.

use loro::{Container, LoroDoc, LoroText, LoroTree, LoroValue, TreeID, ValueOrContainer};

fn setup() -> (LoroDoc, LoroTree) {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);
    (doc, tree)
}

fn get_text_container(tree: &LoroTree, node: TreeID, key: &str) -> LoroText {
    let meta = tree.get_meta(node).unwrap();
    match meta.get(key) {
        Some(ValueOrContainer::Container(Container::Text(t))) => t,
        other => panic!("expected LoroText at key '{key}', got {other:?}"),
    }
}

fn create_block(tree: &LoroTree, parent: Option<TreeID>, content: &str) -> TreeID {
    let node = tree.create(parent).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert("content_type", "text").unwrap();
    let text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();
    text.insert(0, content).unwrap();
    node
}

fn create_source_block(
    tree: &LoroTree,
    parent: Option<TreeID>,
    language: &str,
    code: &str,
) -> TreeID {
    let node = tree.create(parent).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert("content_type", "source").unwrap();
    meta.insert("source_language", language).unwrap();
    let text: LoroText = meta
        .insert_container("source_code", LoroText::new())
        .unwrap();
    text.insert(0, code).unwrap();
    node
}

fn read_content(tree: &LoroTree, node: TreeID) -> String {
    let meta = tree.get_meta(node).unwrap();
    let content_type = match meta.get("content_type") {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        _ => "text".to_string(),
    };
    let key = if content_type == "source" {
        "source_code"
    } else {
        "content_raw"
    };
    match meta.get(key) {
        Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
        _ => String::new(),
    }
}

fn get_str(tree: &LoroTree, node: TreeID, key: &str) -> String {
    let meta = tree.get_meta(node).unwrap();
    match meta.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        other => panic!("expected string at '{key}', got {other:?}"),
    }
}

#[test]
fn nested_lorotext_in_tree_node() {
    let (_doc, tree) = setup();
    let node = create_block(&tree, None, "Hello, world!");
    assert_eq!(read_content(&tree, node), "Hello, world!");
}

#[test]
fn edit_nested_lorotext() {
    let (_doc, tree) = setup();
    let node = create_block(&tree, None, "Hello");

    let text = get_text_container(&tree, node, "content_raw");
    text.insert(5, ", world!").unwrap();

    assert_eq!(read_content(&tree, node), "Hello, world!");
}

#[test]
fn source_block_with_code() {
    let (_doc, tree) = setup();
    let node = create_source_block(
        &tree,
        None,
        "holon_prql",
        "from children | select {id, content}",
    );
    assert_eq!(
        read_content(&tree, node),
        "from children | select {id, content}"
    );
    assert_eq!(get_str(&tree, node, "source_language"), "holon_prql");
}

#[test]
fn document_node_with_name() {
    let (_doc, tree) = setup();
    let doc_node = tree.create(None).unwrap();
    let meta = tree.get_meta(doc_node).unwrap();
    meta.insert("name", "projects").unwrap();
    let _text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();

    assert_eq!(get_str(&tree, doc_node, "name"), "projects");
}

#[test]
fn multiple_blocks_with_nested_text() {
    let (_doc, tree) = setup();

    let doc = tree.create(None).unwrap();
    let meta = tree.get_meta(doc).unwrap();
    meta.insert("name", "index").unwrap();

    let h1 = create_block(&tree, Some(doc), "* Heading 1");
    let h2 = create_block(&tree, Some(doc), "* Heading 2");
    let _body = create_block(&tree, Some(h1), "Some body text under heading 1");
    let _src = create_source_block(&tree, Some(h2), "python", "print('hello')");

    assert_eq!(tree.children_num(doc).unwrap_or(0), 2);
    assert_eq!(tree.children_num(h1).unwrap_or(0), 1);
    assert_eq!(tree.children_num(h2).unwrap_or(0), 1);
}

#[test]
fn properties_as_json_string() {
    let (_doc, tree) = setup();
    let node = create_block(&tree, None, "A task");

    let meta = tree.get_meta(node).unwrap();
    let props = serde_json::json!({
        "task_state": "TODO",
        "priority": 2,
        "tags": ["work", "urgent"],
    });
    meta.insert("properties", props.to_string()).unwrap();

    let stored = get_str(&tree, node, "properties");
    let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(parsed["task_state"], "TODO");
    assert_eq!(parsed["priority"], 2);
}

#[test]
fn content_survives_move() {
    let (_doc, tree) = setup();

    let parent_a = tree.create(None).unwrap();
    let parent_b = tree.create(None).unwrap();
    let block = create_block(&tree, Some(parent_a), "Traveling content");

    tree.mov(block, parent_b).unwrap();
    assert_eq!(read_content(&tree, block), "Traveling content");
}

#[test]
fn get_or_create_container_for_reuse() {
    let (_doc, tree) = setup();
    let node = tree.create(None).unwrap();
    let meta = tree.get_meta(node).unwrap();

    let text1: LoroText = meta
        .get_or_create_container("content_raw", LoroText::new())
        .unwrap();
    text1.insert(0, "Hello").unwrap();

    let text2: LoroText = meta
        .get_or_create_container("content_raw", LoroText::new())
        .unwrap();
    assert_eq!(text2.to_string(), "Hello");

    text2.insert(5, " World").unwrap();
    assert_eq!(text1.to_string(), "Hello World");
}
