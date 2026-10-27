use std::collections::HashMap;

use super::*;

/// Minimal in-memory tree for the precondition tests: `parent_of` map.
struct MapTree {
    parent: HashMap<EntityUri, EntityUri>,
}

impl MapTree {
    fn new(edges: &[(&str, &str)]) -> Self {
        let parent = edges
            .iter()
            .map(|(child, par)| (EntityUri::block(child), EntityUri::block(par)))
            .collect();
        Self { parent }
    }
}

impl BlockTreeView for MapTree {
    fn block_exists(&self, id: &EntityUri) -> bool {
        self.parent.contains_key(id) || self.parent.values().any(|p| p == id)
    }
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        self.parent.get(id).cloned()
    }
    fn children_of(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.parent
            .iter()
            .filter(|(_, p)| *p == parent)
            .map(|(c, _)| c.clone())
            .collect()
    }
}

fn uri(s: &str) -> EntityUri {
    EntityUri::block(s)
}

#[test]
fn valid_move_passes() {
    // a -> root, b -> root; move b after a (siblings under root)
    let tree = MapTree::new(&[("a", "root"), ("b", "root")]);
    let m = BlockMutation::Move {
        id: uri("b"),
        new_parent: uri("root"),
        after: Some(uri("a")),
    };
    assert_eq!(m.validate(&tree), Ok(()));
}

#[test]
fn move_into_own_descendant_is_cycle() {
    // child -> a -> root; moving a under child would create a cycle.
    let tree = MapTree::new(&[("a", "root"), ("child", "a")]);
    let m = BlockMutation::Move {
        id: uri("a"),
        new_parent: uri("child"),
        after: None,
    };
    assert_eq!(
        m.validate(&tree),
        Err(InvalidMove::WouldCreateCycle {
            id: uri("a"),
            new_parent: uri("child"),
        })
    );
}

#[test]
fn move_under_self_is_cycle() {
    let tree = MapTree::new(&[("a", "root")]);
    let m = BlockMutation::Move {
        id: uri("a"),
        new_parent: uri("a"),
        after: None,
    };
    assert!(matches!(
        m.validate(&tree),
        Err(InvalidMove::WouldCreateCycle { .. })
    ));
}

#[test]
fn after_self_is_self_reference() {
    let tree = MapTree::new(&[("a", "root"), ("b", "root")]);
    let m = BlockMutation::MoveAfter {
        id: uri("b"),
        after: Some(uri("b")),
    };
    assert_eq!(m.validate(&tree), Err(InvalidMove::SelfReference(uri("b"))));
}

#[test]
fn after_non_sibling_is_rejected() {
    // x is under a different parent than b's parent (root).
    let tree = MapTree::new(&[("a", "root"), ("b", "root"), ("x", "a")]);
    let m = BlockMutation::MoveAfter {
        id: uri("b"),
        after: Some(uri("x")),
    };
    assert_eq!(
        m.validate(&tree),
        Err(InvalidMove::AfterNotSibling {
            anchor: uri("x"),
            parent: uri("root"),
        })
    );
}

#[test]
fn move_missing_subject_or_parent() {
    let tree = MapTree::new(&[("a", "root")]);
    let missing_subject = BlockMutation::Move {
        id: uri("ghost"),
        new_parent: uri("root"),
        after: None,
    };
    assert_eq!(
        missing_subject.validate(&tree),
        Err(InvalidMove::Missing(uri("ghost")))
    );
    let missing_parent = BlockMutation::Move {
        id: uri("a"),
        new_parent: uri("ghost"),
        after: None,
    };
    assert_eq!(
        missing_parent.validate(&tree),
        Err(InvalidMove::Missing(uri("ghost")))
    );
}

#[test]
fn create_requires_existing_parent() {
    let tree = MapTree::new(&[("a", "root")]);
    let m = BlockMutation::Create {
        id: uri("new"),
        parent: uri("ghost"),
        after: None,
    };
    assert_eq!(m.validate(&tree), Err(InvalidMove::Missing(uri("ghost"))));
}

#[test]
fn create_after_non_sibling_rejected() {
    let tree = MapTree::new(&[("a", "root"), ("x", "a")]);
    let m = BlockMutation::Create {
        id: uri("new"),
        parent: uri("root"),
        after: Some(uri("x")),
    };
    assert_eq!(
        m.validate(&tree),
        Err(InvalidMove::AfterNotSibling {
            anchor: uri("x"),
            parent: uri("root"),
        })
    );
}

#[test]
fn cyclic_move_maps_to_api_error() {
    let err: ApiError = InvalidMove::WouldCreateCycle {
        id: uri("a"),
        new_parent: uri("b"),
    }
    .into();
    assert!(matches!(err, ApiError::CyclicMove { .. }));
}
