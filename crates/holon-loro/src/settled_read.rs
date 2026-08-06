//! The settled-read policy for STRUCTURAL reads of the Loro block tree — the
//! one place that decides which live tree nodes a reader may answer with.
//!
//! `LoroDocument::with_write` takes no lock and one block create is TWO
//! doc-state steps (`tree.create()`, then the `STABLE_ID` meta insert), so a
//! reader on another task observes live nodes that are *half-born*: alive, but
//! with no stable id and therefore no `EntityUri`. Feeding one to
//! `block_uri_from_meta` panics by contract and kills the task running the
//! user's edit. Every structural reader must withhold instead, and a node
//! under a half-born ancestor must be withheld with it — its own URI is only
//! reachable through one that does not exist yet, so admitting it would
//! present it as a root.
//!
//! Readers route through [`classify`] (one node) or [`scan_live_tree`] (a whole
//! tree, closure included) rather than re-deriving the policy.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityUri;

use crate::loro_backend::LoroMapExt;
use crate::loro_backend::STABLE_ID;

/// Read the stable ID from a node's metadata.
pub(crate) fn read_stable_id(meta: &loro::LoroMap) -> Option<String> {
    meta.get_typed(STABLE_ID, |val| val.as_string().map(|s| s.to_string()))
}

/// Get the parent TreeID of a node (`None` = tree root, deleted, or unknown).
pub(crate) fn get_node_parent(tree: &loro::LoroTree, node: loro::TreeID) -> Option<loro::TreeID> {
    match tree.parent(node)? {
        loro::TreeParentId::Node(pid) => Some(pid),
        _ => None,
    }
}

/// What one live tree node is worth to a structural reader.
///
/// Exhaustive on purpose: the three cases carry different recovery semantics
/// (settled / retry-later / torn walk) and every reader must state its handling
/// of each, so a new one cannot inherit the panic by omission.
pub(crate) enum LiveNode {
    /// `STABLE_ID` has landed — the node has an `EntityUri`.
    Settled(String),
    /// Alive with readable meta but no `STABLE_ID`: an in-flight create, not a
    /// corrupt node. Withhold; callers re-read until it appears.
    HalfBorn,
    /// `tree.get_meta` refused the node — a concurrent commit removed it
    /// between enumeration and this read (a torn walk).
    MetaUnreadable,
}

/// Apply the settled-read policy to one live node.
pub(crate) fn classify(tree: &loro::LoroTree, node: loro::TreeID) -> LiveNode {
    let Ok(meta) = tree.get_meta(node) else {
        return LiveNode::MetaUnreadable;
    };
    match read_stable_id(&meta) {
        Some(sid) => LiveNode::Settled(sid),
        None => LiveNode::HalfBorn,
    }
}

/// Disclose a node withheld because its own `STABLE_ID` had not landed yet.
/// `site` names the reader so the windows stay distinguishable in logs.
pub(crate) fn warn_half_born(site: &str, node: loro::TreeID, parent: &str) {
    tracing::warn!(
        site,
        ?node,
        parent,
        "live node has no STABLE_ID yet (in-flight create, meta not landed); withholding it from \
         this answer — callers re-read until it appears"
    );
}

/// Disclose a node withheld because an ANCESTOR is half-born. Distinct from
/// [`warn_half_born`]: this node is itself settled, and the diagnosis points at
/// a different node than the one named here.
pub(crate) fn warn_unreachable_under_half_born(site: &str, node: loro::TreeID) {
    tracing::warn!(
        site,
        ?node,
        "withholding a node whose ancestor's STABLE_ID has not landed — it is unreachable from \
         this answer"
    );
}

/// One settled scan of a whole tree: which live nodes a structural reader may
/// answer with, and the stable IDs it may key them by.
pub(crate) struct SettledScan {
    /// Live, settled, reachable nodes in enumeration order — the answerable
    /// set.
    pub(crate) admitted: Vec<loro::TreeID>,
    /// URI of every SETTLED live node, admitted or withheld. A withheld node
    /// keeps its entry: only its reachability is in doubt, not its identity.
    pub(crate) uris: HashMap<loro::TreeID, EntityUri>,
}

/// Scan every live node of `tree` under the settled-read policy: classify each,
/// close the withholding over the descendants of half-born nodes, and disclose
/// every withheld node with the warning that names its actual cause.
///
/// Errs on a stored parent cycle (see [`withheld_closure`]).
pub(crate) fn scan_live_tree(tree: &loro::LoroTree, site: &str) -> anyhow::Result<SettledScan> {
    let mut uris: HashMap<loro::TreeID, EntityUri> = HashMap::new();
    let mut half_born: HashSet<loro::TreeID> = HashSet::new();
    let mut live: Vec<loro::TreeID> = Vec::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        match classify(tree, node.id) {
            LiveNode::Settled(sid) => {
                live.push(node.id);
                uris.insert(node.id, EntityUri::block(&sid));
            }
            LiveNode::HalfBorn => {
                live.push(node.id);
                half_born.insert(node.id);
            }
            // The node is already gone; it seeds no withholding because its
            // descendants (if any) are gone with it.
            LiveNode::MetaUnreadable => {}
        }
    }

    let withheld = withheld_closure(&live, &half_born, |n| get_node_parent(tree, n))?;

    let mut admitted = Vec::with_capacity(live.len());
    for node in live {
        if withheld.contains(&node) {
            if half_born.contains(&node) {
                warn_half_born(site, node, &format!("{:?}", tree.parent(node)));
            } else {
                warn_unreachable_under_half_born(site, node);
            }
            continue;
        }
        admitted.push(node);
    }
    Ok(SettledScan { admitted, uris })
}

/// Close the withholding over descendants: a node is withheld once ANY ancestor
/// is half-born. Each walk memoizes its whole path into the result, so the
/// scan stays linear in the tree.
///
/// A walk that revisits a node it is already standing on can never terminate: a
/// stored parent cycle is corruption a structural read must not paper over, so
/// it is an `Err` naming the cycle rather than a truncated answer.
fn withheld_closure(
    live: &[loro::TreeID],
    half_born: &HashSet<loro::TreeID>,
    parent_of: impl Fn(loro::TreeID) -> Option<loro::TreeID>,
) -> anyhow::Result<HashSet<loro::TreeID>> {
    let mut withheld = half_born.clone();
    for node in live {
        let mut path = Vec::new();
        let mut on_path: HashSet<loro::TreeID> = HashSet::new();
        let mut cur = Some(*node);
        let unreachable = loop {
            let Some(n) = cur else { break false };
            if withheld.contains(&n) {
                break true;
            }
            if !on_path.insert(n) {
                return Err(anyhow::anyhow!(
                    "loro block tree has a stored parent cycle through node {n:?} (walked up from \
                     {node:?}); refusing to answer a structural read over a corrupt tree"
                ));
            }
            path.push(n);
            cur = parent_of(n);
        };
        if unreachable {
            withheld.extend(path);
        }
    }
    Ok(withheld)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(counter: i32) -> loro::TreeID {
        loro::TreeID::new(1, counter)
    }

    /// The closure walk terminates on a stored parent cycle and names it,
    /// instead of spinning forever with unbounded path growth. Loro's own
    /// `mov` refuses to create a cycle, so the corrupt state is unreachable
    /// through the tree API — the walk is exercised directly over the parent
    /// relation it consumes.
    #[test]
    fn a_stored_parent_cycle_is_refused_not_walked_forever() {
        let (a, b, c) = (tid(1), tid(2), tid(3));
        let parents: HashMap<loro::TreeID, loro::TreeID> =
            [(a, b), (b, c), (c, a)].into_iter().collect();

        let err = withheld_closure(&[a, b, c], &HashSet::new(), |n| parents.get(&n).copied())
            .expect_err("a parent cycle must be refused, not walked");

        let msg = err.to_string();
        assert!(
            msg.contains("stored parent cycle"),
            "the error must name the corruption: {msg}"
        );
    }

    /// The acyclic path is unaffected by the cycle guard: a node under a
    /// half-born ancestor is withheld, its settled siblings are not.
    #[test]
    fn withholding_closes_over_descendants_only() {
        let (root, half, deep, sibling) = (tid(1), tid(2), tid(3), tid(4));
        let parents: HashMap<loro::TreeID, loro::TreeID> =
            [(half, root), (deep, half), (sibling, root)]
                .into_iter()
                .collect();

        let withheld =
            withheld_closure(&[root, half, deep, sibling], &HashSet::from([half]), |n| {
                parents.get(&n).copied()
            })
            .unwrap();

        assert_eq!(withheld, HashSet::from([half, deep]));
    }
}
