//! Hypothesis: LoroTree supports create, move, delete with native cycle detection
//! and fractional indexing for sibling ordering.

use loro::{LoroDoc, LoroTree, LoroValue, TreeID, ValueOrContainer};

fn setup() -> (LoroDoc, LoroTree) {
    let doc = LoroDoc::new();
    let tree = doc.get_tree("blocks");
    tree.enable_fractional_index(0);
    (doc, tree)
}

fn set_content(tree: &LoroTree, node: TreeID, content: &str) {
    let meta = tree.get_meta(node).unwrap();
    meta.insert("content", content).unwrap();
}

fn get_content(tree: &LoroTree, node: TreeID) -> String {
    let meta = tree.get_meta(node).unwrap();
    match meta.get("content") {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        other => panic!("expected string content on {node:?}, got {other:?}"),
    }
}

fn child_count(tree: &LoroTree, parent: TreeID) -> usize {
    tree.children_num(parent).unwrap_or(0)
}

#[test]
fn create_root_nodes() {
    let (_doc, tree) = setup();

    let a = tree.create(None).unwrap();
    let b = tree.create(None).unwrap();
    set_content(&tree, a, "Node A");
    set_content(&tree, b, "Node B");

    assert_eq!(get_content(&tree, a), "Node A");
    assert_eq!(get_content(&tree, b), "Node B");
    assert_eq!(tree.roots().len(), 2);
}

#[test]
fn create_children() {
    let (_doc, tree) = setup();

    let parent = tree.create(None).unwrap();
    let child1 = tree.create(parent).unwrap();
    let child2 = tree.create(parent).unwrap();
    set_content(&tree, parent, "Parent");
    set_content(&tree, child1, "Child 1");
    set_content(&tree, child2, "Child 2");

    let children = tree.children(parent).unwrap();
    assert_eq!(children.len(), 2);
    assert!(children.contains(&child1));
    assert!(children.contains(&child2));
}

#[test]
fn move_node_between_parents() {
    let (_doc, tree) = setup();

    let parent_a = tree.create(None).unwrap();
    let parent_b = tree.create(None).unwrap();
    let child = tree.create(parent_a).unwrap();
    set_content(&tree, child, "Movable");

    assert_eq!(child_count(&tree, parent_a), 1);
    assert_eq!(child_count(&tree, parent_b), 0);

    tree.mov(child, parent_b).unwrap();

    assert_eq!(child_count(&tree, parent_a), 0);
    assert_eq!(child_count(&tree, parent_b), 1);
    assert_eq!(get_content(&tree, child), "Movable");
}

#[test]
fn delete_hides_descendants() {
    let (_doc, tree) = setup();

    let root = tree.create(None).unwrap();
    let child = tree.create(root).unwrap();
    let grandchild = tree.create(child).unwrap();
    set_content(&tree, grandchild, "Deep");

    tree.delete(child).unwrap();

    assert_eq!(child_count(&tree, root), 0);
    // metadata of deleted nodes should still be readable
    assert_eq!(get_content(&tree, grandchild), "Deep");
}

#[test]
fn cycle_detection_rejects_move() {
    let (_doc, tree) = setup();

    let a = tree.create(None).unwrap();
    let b = tree.create(a).unwrap();

    let result = tree.mov(a, b);
    assert!(
        result.is_err(),
        "Moving a parent under its own descendant should fail"
    );
}

#[test]
fn fractional_index_ordering() {
    let (_doc, tree) = setup();

    let parent = tree.create(None).unwrap();
    let first = tree.create_at(parent, 0).unwrap();
    let third = tree.create_at(parent, 1).unwrap();
    let second = tree.create_at(parent, 1).unwrap();

    set_content(&tree, first, "1st");
    set_content(&tree, second, "2nd");
    set_content(&tree, third, "3rd");

    assert_eq!(child_count(&tree, parent), 3);

    let fi_first = tree.fractional_index(first).unwrap();
    let fi_second = tree.fractional_index(second).unwrap();
    let fi_third = tree.fractional_index(third).unwrap();

    assert!(
        fi_first < fi_second,
        "first < second: {fi_first} < {fi_second}"
    );
    assert!(
        fi_second < fi_third,
        "second < third: {fi_second} < {fi_third}"
    );
}

#[test]
fn mov_before_and_after() {
    let (_doc, tree) = setup();

    let parent = tree.create(None).unwrap();
    let a = tree.create(parent).unwrap();
    let b = tree.create(parent).unwrap();
    let c = tree.create(parent).unwrap();

    tree.mov_before(c, a).unwrap();

    let fi_c = tree.fractional_index(c).unwrap();
    let fi_a = tree.fractional_index(a).unwrap();
    let fi_b = tree.fractional_index(b).unwrap();

    assert!(fi_c < fi_a, "C should be before A");
    assert!(fi_a < fi_b, "A should be before B");
}

#[test]
fn large_tree_10k_nodes() {
    let (doc, tree) = setup();

    let root = tree.create(None).unwrap();
    let mut parents = vec![root];

    // Create 100 headings under root, each with 100 children = 10k nodes
    // Flat-ish structure avoids deep recursion
    for i in 0..100 {
        let heading = tree.create(root).unwrap();
        set_content(&tree, heading, &format!("heading-{i}"));
        for j in 0..100 {
            let child = tree.create(heading).unwrap();
            set_content(&tree, child, &format!("node-{}-{}", i, j));
        }
        parents.push(heading);
    }

    assert_eq!(child_count(&tree, root), 100);
    assert_eq!(child_count(&tree, parents[1]), 100);

    doc.commit();
    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    println!(
        "10k node tree snapshot: {} bytes ({:.1} KB)",
        snapshot.len(),
        snapshot.len() as f64 / 1024.0
    );
}

#[test]
fn tree_id_format() {
    let (_doc, tree) = setup();
    let node = tree.create(None).unwrap();
    println!("TreeID Debug: {:?}", node);
    println!("TreeID Display: {}", node);
    println!("peer={}, counter={}", node.peer, node.counter);
    let encoded = format!("{}:{}", node.peer, node.counter);
    println!("Our encoding: block:{encoded}");

    // Round-trip
    let parts: Vec<&str> = encoded.split(':').collect();
    let peer: u64 = parts[0].parse().unwrap();
    let counter: i32 = parts[1].parse().unwrap();
    let restored = loro::TreeID::new(peer, counter);
    assert_eq!(node, restored);
}
