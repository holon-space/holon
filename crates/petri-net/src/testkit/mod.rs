//! Shared generators and laws, for this crate's tests and for stores in other
//! crates (behind the `testkit` feature).

mod laws;
mod nets;
mod store_laws;

pub use laws::{Recording, check_atomicity, check_footprint};
pub use nets::{ArcS, BindSpec, Call, Guard, NetCase, NetSpec, OutS, SETTLE_BOUND, TSpec, arb_net_case, arb_net_spec};
pub use store_laws::{StoreOp, arb_store_case, check_store_laws, check_store_laws_on};

use proptest::prelude::*;

use crate::{Discipline, Kinds, RowView, Store};

/// The kinds every testkit net and store case uses. Tokens are `u32` so a
/// generator can make each written token unique.
#[derive(Clone, Copy, Debug)]
pub struct TestKinds;

impl Kinds for TestKinds {
    type Place = u8;
    type Key = u8;
    type Token = u32;
    type Binding = u32;
}

pub const KEYS: u8 = 3;

pub fn arb_discipline() -> impl Strategy<Value = Discipline> {
    prop_oneof![
        Just(Discipline::Single),
        Just(Discipline::Register),
        Just(Discipline::Fifo),
        (1u32..=3).prop_map(|size| Discipline::Counted { size }),
    ]
}

/// Places `0..n` with drawn disciplines.
pub fn arb_places(max: usize) -> impl Strategy<Value = Vec<(u8, Discipline)>> {
    prop::collection::vec(arb_discipline(), 1..=max)
        .prop_map(|ds| ds.into_iter().enumerate().map(|(i, d)| (i as u8, d)).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Content {
    Tokens(Vec<u32>),
    Count(u32),
}

pub(crate) fn content<S: Store<TestKinds>>(store: &S, (p, k): (u8, u8)) -> Content {
    match store.row(&p, &k) {
        RowView::Tokens(t) => Content::Tokens(t.into_iter().copied().collect()),
        RowView::Count(n) => Content::Count(n),
    }
}
