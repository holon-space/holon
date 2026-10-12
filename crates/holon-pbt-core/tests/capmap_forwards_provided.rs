//! `#[capmap_adapter]` must FORWARD a provided (default-bodied) `&self` cap
//! method to the registered provider, exactly as it forwards a required one.
//!
//! Inheriting the trait default instead makes every invariant that reads such
//! a method off the `CapMap` observe the DEFAULT rather than the live
//! reference/SUT — a silent oracle fake that cannot fail, so it hides prod
//! bugs instead of catching them.

use std::sync::Arc;

use holon_pbt_core::composition::CapMap;

#[holon_macros::capmap_adapter]
pub trait RefToyTree {
    fn block_count(&self) -> usize;

    /// Substrates without a collapse concept answer `false`.
    fn is_collapsed(&self, _: u64) -> bool {
        false
    }
}

#[holon_macros::capmap_adapter]
pub trait SutToyTree {
    async fn live_ids(&self) -> Vec<u64>;

    /// Substrates without a dirty-editor concept answer `false`.
    async fn editor_dirty(&self) -> bool {
        false
    }
}

struct Toy;

#[async_trait::async_trait(?Send)]
impl SutToyTree for Toy {
    async fn live_ids(&self) -> Vec<u64> {
        vec![1, 2]
    }
    async fn editor_dirty(&self) -> bool {
        true
    }
}

impl RefToyTree for Toy {
    fn block_count(&self) -> usize {
        2
    }
    fn is_collapsed(&self, _: u64) -> bool {
        true
    }
}

fn toy_caps() -> CapMap {
    let mut caps = CapMap::new();
    let toy = Arc::new(Toy);
    caps.insert(toy.clone() as Arc<dyn RefToyTree>);
    caps.insert(toy as Arc<dyn SutToyTree>);
    caps
}

#[test]
fn capmap_forwards_a_provided_sync_method_to_the_provider() {
    let caps = toy_caps();
    assert_eq!(caps.block_count(), 2, "required method must forward");
    assert!(
        caps.is_collapsed(7),
        "CapMap answered the trait DEFAULT (false) for a provided method; the \
         registered provider says true — an invariant reading this cap through \
         the map observes a fake"
    );
}

#[test]
fn capmap_forwards_a_provided_async_method_to_the_provider() {
    let caps = toy_caps();
    assert_eq!(
        futures::executor::block_on(caps.live_ids()),
        vec![1, 2],
        "required async method must forward"
    );
    assert!(
        futures::executor::block_on(caps.editor_dirty()),
        "CapMap answered the trait DEFAULT (false) for a provided async method; \
         the registered provider says true"
    );
}
