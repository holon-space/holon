//! Canonical block-property keys for shared-subtree (mount) rows.
//!
//! These live in `holon-api` — the lowest crate every layer depends on — so the
//! share backend (`holon-loro`), the org write-back layer (`holon-orgmode` /
//! `holon-filesystem`), and the SQL projection all agree on ONE spelling
//! instead of re-declaring the strings per crate. `holon-loro::shared_tree`
//! re-exports these for its existing call sites.

/// Block property key that marks a row as a share-participating node.
/// Value [`SHARE_ROLE_MOUNT`] identifies the local mount block.
pub const SHARE_ROLE_PROPERTY: &str = "share-role";

/// Value of [`SHARE_ROLE_PROPERTY`] for a shared-tree mount row.
pub const SHARE_ROLE_MOUNT: &str = "mount";

/// Block property key storing the shared tree's UUID. Present on the mount row
/// and stamped onto every projected descendant of the shared subtree so
/// write-back / routing can identify blocks belonging to a share.
pub const SHARED_TREE_ID_PROPERTY: &str = "shared-tree-id";
