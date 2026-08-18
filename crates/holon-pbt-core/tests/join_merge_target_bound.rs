//! `join_merge_target`'s visible-outline descent must terminate.
//!
//! @pbt kind ref
//! @pbt covers join-backspace — the descent's cycle bound
//!
//! The walk reads child order and collapse state from separate places, so a
//! child graph that is not a tree is representable. Unbounded, the walk spins
//! forever; bounded, it fails loud naming the start block and the tail of the
//! walk. Mirrors `holon_core::traits::OUTLINE_DESCENT_LIMIT`, whose SUT-side
//! twin is pinned by
//! `holon-core::block_operations_tests::join_block_descent_into_a_child_cycle_fails_loud`.

use std::collections::BTreeSet;
use std::collections::HashMap;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::join_merge_target;

/// Children come from an explicit map, so the fixture can spell a cycle the
/// `parent_id`-derived view could not.
struct CyclicRef {
    children: HashMap<EntityUri, Vec<EntityUri>>,
    previous_sibling: Option<EntityUri>,
}

impl RefBlockTree for CyclicRef {
    fn block_content(&self, _: &EntityUri) -> Option<&str> {
        Some("")
    }
    fn is_text_block(&self, _: &EntityUri) -> bool {
        true
    }
    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        vec![]
    }
    fn focus_root_ids(&self, _: CapRegion) -> BTreeSet<EntityUri> {
        BTreeSet::new()
    }
    fn previous_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
        self.previous_sibling.clone()
    }
    fn next_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn parent_of(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn grandparent(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.children.get(parent).cloned().unwrap_or_default()
    }
    fn is_descendant_of_any(&self, _: &EntityUri, _: &BTreeSet<EntityUri>) -> bool {
        false
    }
    fn main_panel_renders(&self, _: &EntityUri) -> bool {
        true
    }
    fn is_layout_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_focusable(&self, _: &EntityUri) -> bool {
        true
    }
    fn is_no_content_update(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_page_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        BTreeSet::new()
    }
}

fn two_cycle() -> CyclicRef {
    let (x, y) = (EntityUri::block("X"), EntityUri::block("Y"));
    CyclicRef {
        children: HashMap::from([(x.clone(), vec![y.clone()]), (y.clone(), vec![x.clone()])]),
        previous_sibling: Some(x),
    }
}

#[test]
#[should_panic(expected = "descent exceeded 4096 steps from block:B")]
fn descent_into_a_child_cycle_fails_loud() {
    join_merge_target(&EntityUri::block("B"), &two_cycle());
}

#[test]
#[should_panic(expected = "last visited: [EntityUri(\"")]
fn the_failure_names_the_visited_ids() {
    join_merge_target(&EntityUri::block("B"), &two_cycle());
}

/// The bound must not truncate a legitimately deep outline: a chain one step
/// under the limit still resolves to its deepest block.
#[test]
fn a_deep_but_acyclic_outline_still_resolves() {
    let depth = holon_pbt_core::capabilities::OUTLINE_DESCENT_LIMIT - 1;
    let ids: Vec<EntityUri> = (0..depth)
        .map(|i| EntityUri::block(&format!("d{i}")))
        .collect();
    let children = ids
        .windows(2)
        .map(|w| (w[0].clone(), vec![w[1].clone()]))
        .collect();

    let state = CyclicRef {
        children,
        previous_sibling: Some(ids[0].clone()),
    };

    assert_eq!(
        join_merge_target(&EntityUri::block("B"), &state).as_ref(),
        ids.last()
    );
}
