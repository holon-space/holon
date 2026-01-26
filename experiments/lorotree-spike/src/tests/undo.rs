//! Hypothesis: Loro's UndoManager works across structure + content operations
//! within a single LoroTree LoroDoc, providing unified undo/redo.

use loro::{Container, LoroDoc, LoroText, LoroTree, TreeID, UndoManager, ValueOrContainer};
use std::thread;
use std::time::Duration;

fn setup() -> (LoroDoc, LoroTree) {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);
    (doc, tree)
}

fn set_text(tree: &LoroTree, node: TreeID, content: &str) {
    let meta = tree.get_meta(node).unwrap();
    let _text: LoroText = meta
        .insert_container("content_raw", LoroText::new())
        .unwrap();
    _text.insert(0, content).unwrap();
}

fn read_text(tree: &LoroTree, node: TreeID) -> String {
    let meta = tree.get_meta(node).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
        _ => String::new(),
    }
}

fn get_text(tree: &LoroTree, node: TreeID) -> LoroText {
    let meta = tree.get_meta(node).unwrap();
    match meta.get("content_raw") {
        Some(ValueOrContainer::Container(Container::Text(t))) => t,
        _ => panic!("no content_raw text container on node"),
    }
}

#[test]
fn undo_content_edit() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    let node = tree.create(None).unwrap();
    set_text(&tree, node, "Hello");
    doc.commit();

    thread::sleep(Duration::from_millis(1100));

    let text = get_text(&tree, node);
    text.insert(5, " World").unwrap();
    doc.commit();

    assert_eq!(read_text(&tree, node), "Hello World");

    assert!(undo_mgr.undo().unwrap());
    assert_eq!(read_text(&tree, node), "Hello");

    assert!(undo_mgr.redo().unwrap());
    assert_eq!(read_text(&tree, node), "Hello World");
}

#[test]
fn undo_node_creation() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    let root = tree.create(None).unwrap();
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    let child = tree.create(root).unwrap();
    set_text(&tree, child, "New child");
    doc.commit();

    assert_eq!(tree.children_num(root).unwrap_or(0), 1);

    assert!(undo_mgr.undo().unwrap());
    let children_after = tree.children_num(root).unwrap_or(0);
    println!("Children after undo of creation: {children_after}");
}

#[test]
fn undo_move_operation() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    let parent_a = tree.create(None).unwrap();
    let parent_b = tree.create(None).unwrap();
    let block = tree.create(parent_a).unwrap();
    set_text(&tree, block, "Movable");
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    tree.mov(block, parent_b).unwrap();
    doc.commit();

    assert_eq!(tree.children_num(parent_a).unwrap_or(0), 0);
    assert_eq!(tree.children_num(parent_b).unwrap_or(0), 1);

    assert!(undo_mgr.undo().unwrap());

    println!(
        "After undo move - parent_a children: {}",
        tree.children_num(parent_a).unwrap_or(0)
    );
    println!(
        "After undo move - parent_b children: {}",
        tree.children_num(parent_b).unwrap_or(0)
    );
    println!("Block content still intact: {}", read_text(&tree, block));
}

#[test]
fn undo_interleaved_structure_and_content() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    // Step 1: Create tree structure
    let root = tree.create(None).unwrap();
    set_text(&tree, root, "Root");
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    // Step 2: Add a child
    let _child = tree.create(root).unwrap();
    set_text(&tree, _child, "Child");
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    // Step 3: Edit root's content
    let text = get_text(&tree, root);
    text.insert(4, " (edited)").unwrap();
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    assert_eq!(read_text(&tree, root), "Root (edited)");

    // Undo step 3: content edit
    assert!(undo_mgr.undo().unwrap());
    assert_eq!(read_text(&tree, root), "Root");

    // Undo step 2: child creation
    assert!(undo_mgr.undo().unwrap());
    println!(
        "After undoing child creation, root children: {}",
        tree.children_num(root).unwrap_or(0)
    );

    // Redo step 2
    assert!(undo_mgr.redo().unwrap());
    println!(
        "After redo, root children: {}",
        tree.children_num(root).unwrap_or(0)
    );
}

#[test]
fn undo_delete() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    let root = tree.create(None).unwrap();
    let child = tree.create(root).unwrap();
    set_text(&tree, child, "Will be deleted");
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    tree.delete(child).unwrap();
    doc.commit();

    assert_eq!(tree.children_num(root).unwrap_or(0), 0);

    assert!(undo_mgr.undo().unwrap());
    println!(
        "After undo delete, root children: {}",
        tree.children_num(root).unwrap_or(0)
    );
    println!("Restored child content: {}", read_text(&tree, child));
}

#[test]
fn merge_interval_groups_rapid_edits() {
    let (doc, tree) = setup();
    let mut undo_mgr = UndoManager::new(&doc);

    let node = tree.create(None).unwrap();
    set_text(&tree, node, "A");
    doc.commit();
    thread::sleep(Duration::from_millis(1100));

    // Rapid edits within merge interval (1s default)
    let text = get_text(&tree, node);
    text.insert(1, "B").unwrap();
    doc.commit();
    text.insert(2, "C").unwrap();
    doc.commit();
    text.insert(3, "D").unwrap();
    doc.commit();

    assert_eq!(read_text(&tree, node), "ABCD");

    assert!(undo_mgr.undo().unwrap());
    let after_undo = read_text(&tree, node);
    println!("After one undo of merged edits: '{after_undo}'");
}
