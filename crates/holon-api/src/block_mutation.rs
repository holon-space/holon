//! Typed block-tree mutation vocabulary (ADR 0005).
//!
//! Sibling order is "an ordered list of children per parent" — never a
//! per-block key. The only ways to change the tree are
//! [`BlockMutation::Create`], [`BlockMutation::Move`], and
//! [`BlockMutation::MoveAfter`]. Each carries an `after: Option<EntityUri>`
//! positioning intent (`None` = first child).
//!
//! [`BlockMutation::validate`] is the single domain-level precondition check —
//! including cycle detection — that adapters run **before** dispatch, so the
//! illegal-state rules live in one place instead of being re-derived per
//! adapter. Adapters MAY re-check (defense in depth) but MUST NOT be the sole
//! guard.

use crate::ApiError;
use crate::entity_uri::EntityUri;

/// Read-only view of the block tree, supplied by an adapter so the domain can
/// run structural preconditions (ancestry / sibling membership) without knowing
/// how the adapter stores the tree.
pub trait BlockTreeView {
    /// Whether a block with this id currently exists (and is not deleted).
    fn block_exists(&self, id: &EntityUri) -> bool;
    /// The current parent of `id`, or `None` for a root / unknown block.
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri>;
    /// The current ordered children of `parent` (membership is all this check
    /// needs; order is not consulted here).
    fn children_of(&self, parent: &EntityUri) -> Vec<EntityUri>;
}

/// Precondition violation for a [`BlockMutation`] (the ADR 0005 table).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidMove {
    /// `after == Some(id)` — a block cannot be placed after itself.
    #[error("self-reference: cannot place block {0} after itself")]
    SelfReference(EntityUri),
    /// The `after` anchor is not currently a child of the target parent.
    #[error("after-anchor {anchor} is not a child of target parent {parent}")]
    AfterNotSibling {
        anchor: EntityUri,
        parent: EntityUri,
    },
    /// The moved block is an ancestor of (or equal to) the new parent.
    #[error("would create a cycle: {id} is an ancestor of {new_parent}")]
    WouldCreateCycle {
        id: EntityUri,
        new_parent: EntityUri,
    },
    /// A referenced block (the subject, or the target parent) does not exist.
    #[error("missing block: {0}")]
    Missing(EntityUri),
}

impl From<InvalidMove> for ApiError {
    fn from(e: InvalidMove) -> Self {
        match e {
            InvalidMove::WouldCreateCycle { id, new_parent } => ApiError::CyclicMove {
                id: id.to_string(),
                target_parent: new_parent.to_string(),
            },
            InvalidMove::Missing(id) => ApiError::BlockNotFound { id: id.to_string() },
            other => ApiError::InvalidOperation {
                message: other.to_string(),
            },
        }
    }
}

/// A typed mutation of the block tree (ADR 0005). `after = None` means "first
/// child of the target parent".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockMutation {
    /// Create `id` under `parent`, positioned after `after`.
    Create {
        id: EntityUri,
        parent: EntityUri,
        after: Option<EntityUri>,
    },
    /// Reparent `id` under `new_parent`, positioned after `after`.
    Move {
        id: EntityUri,
        new_parent: EntityUri,
        after: Option<EntityUri>,
    },
    /// Reorder `id` within its current parent, positioned after `after`.
    MoveAfter {
        id: EntityUri,
        after: Option<EntityUri>,
    },
}

impl BlockMutation {
    /// Check the ADR 0005 precondition table against `tree`. Returns the first
    /// violation found, or `Ok(())` if the mutation is legal.
    pub fn validate(&self, tree: &impl BlockTreeView) -> Result<(), InvalidMove> {
        match self {
            BlockMutation::Create { id, parent, after } => {
                if !tree.block_exists(parent) {
                    return Err(InvalidMove::Missing(parent.clone()));
                }
                check_after(after.as_ref(), id, parent, tree)
            }
            BlockMutation::Move {
                id,
                new_parent,
                after,
            } => {
                if !tree.block_exists(id) {
                    return Err(InvalidMove::Missing(id.clone()));
                }
                // A sentinel/no_parent target is virtual (always valid) — moving a
                // block to the top level is legitimate (e.g. outdent of a depth-1
                // child), mirroring the `Create` arm's virtual-parent allowance.
                let new_parent_virtual = new_parent.is_no_parent() || new_parent.is_sentinel();
                if !new_parent_virtual && !tree.block_exists(new_parent) {
                    return Err(InvalidMove::Missing(new_parent.clone()));
                }
                if is_ancestor_or_self(id, new_parent, tree) {
                    return Err(InvalidMove::WouldCreateCycle {
                        id: id.clone(),
                        new_parent: new_parent.clone(),
                    });
                }
                check_after(after.as_ref(), id, new_parent, tree)
            }
            BlockMutation::MoveAfter { id, after } => {
                if !tree.block_exists(id) {
                    return Err(InvalidMove::Missing(id.clone()));
                }
                // The target parent is the block's current parent; the anchor
                // must be a current sibling.
                let parent = tree.parent_of(id).unwrap_or_else(EntityUri::no_parent);
                check_after(after.as_ref(), id, &parent, tree)
            }
        }
    }
}

/// `after` precondition: not a self-reference, and (when `Some`) a current
/// child of `parent`.
fn check_after(
    after: Option<&EntityUri>,
    subject: &EntityUri,
    parent: &EntityUri,
    tree: &impl BlockTreeView,
) -> Result<(), InvalidMove> {
    let Some(anchor) = after else {
        return Ok(());
    };
    if anchor == subject {
        return Err(InvalidMove::SelfReference(subject.clone()));
    }
    if !tree.children_of(parent).iter().any(|c| c == anchor) {
        return Err(InvalidMove::AfterNotSibling {
            anchor: anchor.clone(),
            parent: parent.clone(),
        });
    }
    Ok(())
}

/// Walk up from `start` via `parent_of`; return true if `ancestor` is reached
/// (or equals `start`). A visited set guards against an already-corrupt tree.
fn is_ancestor_or_self(ancestor: &EntityUri, start: &EntityUri, tree: &impl BlockTreeView) -> bool {
    use std::collections::HashSet;
    let mut current = Some(start.clone());
    let mut seen: HashSet<EntityUri> = HashSet::new();
    while let Some(node) = current {
        if &node == ancestor {
            return true;
        }
        if !seen.insert(node.clone()) {
            break; // pre-existing cycle in the tree; stop walking
        }
        current = tree.parent_of(&node);
    }
    false
}

#[cfg(test)]
mod tests {
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
}
