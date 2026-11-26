use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};

use super::{KEYS, TestKinds, arb_places};
use crate::{Applied, Discipline, Out, RowView, Store};

type Row = (u8, u8);

/// One step against a store; a step whose precondition does not hold in the
/// current marking is skipped.
#[derive(Clone, Debug)]
pub enum StoreOp {
    Write(Row),
    Drop(Row),
    Move { from: Row, to: Row },
    /// Move the head of `from` into `to`, then the head of `to` back.
    Bounce { from: Row, to: Row },
    Swap(Row, Row),
    UnitIn(Row),
    UnitOut(Row),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Content {
    Tokens(Vec<u32>),
    Count(u32),
}

pub fn arb_store_case() -> impl Strategy<Value = (Vec<(u8, Discipline)>, Vec<StoreOp>)> {
    arb_places(4).prop_flat_map(|places| {
        let n = places.len() as u8;
        let row = (0..n, 0..KEYS);
        let op = prop_oneof![
            3 => row.clone().prop_map(StoreOp::Write),
            1 => row.clone().prop_map(StoreOp::Drop),
            2 => (row.clone(), row.clone()).prop_map(|(from, to)| StoreOp::Move { from, to }),
            2 => (row.clone(), row.clone()).prop_map(|(from, to)| StoreOp::Bounce { from, to }),
            1 => (row.clone(), row.clone()).prop_map(|(a, b)| StoreOp::Swap(a, b)),
            2 => row.clone().prop_map(StoreOp::UnitIn),
            1 => row.prop_map(StoreOp::UnitOut),
        ];
        (Just(places), prop::collection::vec(op, 0..40))
    })
}

/// Runs the store laws (stamp-sound, move-neutral) over drawn cases on stores
/// that `factory` makes for the drawn places.
pub fn check_store_laws<S: Store<TestKinds>>(factory: impl Fn(&[(u8, Discipline)]) -> S) {
    let mut runner = TestRunner::new(Config { failure_persistence: None, ..Config::default() });
    if let Err(e) = runner.run(&arb_store_case(), |(places, ops)| check_store_laws_on(&factory, &places, &ops)) {
        panic!("store laws: {e}");
    }
}

/// The store laws on one case; also the entry for fixed cases.
pub fn check_store_laws_on<S: Store<TestKinds>>(
    factory: &impl Fn(&[(u8, Discipline)]) -> S,
    places: &[(u8, Discipline)],
    ops: &[StoreOp],
) -> Result<(), TestCaseError> {
    let mut store = factory(places);
    let mut case = Case { places, next_token: 0, seen: Vec::new() };
    case.observe(&store)?;
    for op in ops {
        match op {
            StoreOp::Bounce { from, to } => {
                let before = (content(&store, *from), store.row_stamp(&from.0, &from.1));
                let Some(there) = case.plan_move(&store, *from, *to) else { continue };
                store.apply(there);
                case.observe(&store)?;
                let Some(back) = case.plan_move(&store, *to, *from) else { continue };
                store.apply(back);
                case.observe(&store)?;
                if content(&store, *from) == before.0 {
                    prop_assert_eq!(
                        store.row_stamp(&from.0, &from.1),
                        before.1,
                        "move-neutral: row {:?} has its content back after a bounce through {:?}",
                        from,
                        to
                    );
                }
            }
            op => {
                if let Some(firing) = case.plan(&store, op) {
                    store.apply(firing);
                    case.observe(&store)?;
                }
            }
        }
    }
    Ok(())
}

fn content<S: Store<TestKinds>>(store: &S, (p, k): Row) -> Content {
    match store.row(&p, &k) {
        RowView::Tokens(t) => Content::Tokens(t.into_iter().copied().collect()),
        RowView::Count(n) => Content::Count(n),
    }
}

struct Case<'a, R> {
    places: &'a [(u8, Discipline)],
    next_token: u32,
    seen: Vec<(Row, Content, R)>,
}

impl<R: Clone + Eq + std::fmt::Debug> Case<'_, R> {
    fn discipline(&self, p: u8) -> Discipline {
        self.places[p as usize].1
    }

    fn observe<S: Store<TestKinds, RowStamp = R>>(&mut self, store: &S) -> Result<(), TestCaseError> {
        for &(p, _) in self.places {
            for k in 0..KEYS {
                let (c, r) = (content(store, (p, k)), store.row_stamp(&p, &k));
                for (row, c0, r0) in &self.seen {
                    if *row == (p, k) && *r0 == r {
                        prop_assert_eq!(c0, &c, "stamp-sound: row {:?} has stamp {:?} for two contents", row, r);
                    }
                }
                self.seen.push(((p, k), c, r));
            }
        }
        Ok(())
    }

    fn plan_move<S: Store<TestKinds>>(&mut self, store: &S, from: Row, to: Row) -> Option<Applied<TestKinds>> {
        self.plan(store, &StoreOp::Move { from, to })
    }

    fn plan<S: Store<TestKinds>>(&mut self, store: &S, op: &StoreOp) -> Option<Applied<TestKinds>> {
        let len = |(p, k): Row| store.row(&p, &k).len();
        let tokens = |(p, _): Row| !self.discipline(p).is_counted();
        let accepts = |row: Row, after_consumes: usize| self.discipline(row.0) != Discipline::Single || after_consumes == 0;
        let firing = |consumes: Vec<Row>, outputs: Vec<(Row, Out<TestKinds>)>| Applied {
            consumes,
            outputs: outputs.into_iter().map(|((p, k), o)| (p, k, o)).collect(),
        };
        match *op {
            StoreOp::Write(row) if tokens(row) && accepts(row, len(row)) => {
                self.next_token += 1;
                Some(firing(vec![], vec![(row, Out::New(self.next_token))]))
            }
            StoreOp::Drop(row) if len(row) > 0 => Some(firing(vec![row], vec![])),
            StoreOp::Move { from, to } if tokens(from) && tokens(to) && len(from) > 0 => {
                let after = len(to) - usize::from(from == to);
                accepts(to, after).then(|| firing(vec![from], vec![(to, Out::Moved(0))]))
            }
            StoreOp::Swap(a, b) if a != b && tokens(a) && tokens(b) && len(a) > 0 && len(b) > 0 => {
                Some(firing(vec![a, b], vec![(a, Out::Moved(1)), (b, Out::Moved(0))]))
            }
            StoreOp::UnitIn(row) => match self.discipline(row.0) {
                Discipline::Counted { size } if (len(row) as u32) < size => Some(firing(vec![], vec![(row, Out::Unit)])),
                _ => None,
            },
            StoreOp::UnitOut(row) if !tokens(row) && len(row) > 0 => Some(firing(vec![row], vec![])),
            _ => None,
        }
    }
}
