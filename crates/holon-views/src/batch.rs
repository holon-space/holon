//! The reference backend: every plan recomputed from its whole inputs.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::rc::Rc;

use crate::error::EngineError;
use crate::error::MAX_DEPTH;
use crate::eval::eval;
use crate::eval::holds;
use crate::plan::Agg;
use crate::plan::Checked;
use crate::plan::Op;
use crate::row::Datum;
use crate::row::Row;

/// Row -> multiplicity; no entry has multiplicity 0.
pub type Multiset<R> = BTreeMap<R, isize>;

pub fn add<R: Ord>(set: &mut Multiset<R>, row: R, diff: isize) {
    match set.entry(row) {
        Entry::Occupied(mut e) => {
            *e.get_mut() += diff;
            if *e.get() == 0 {
                e.remove();
            }
        }
        Entry::Vacant(e) => {
            if diff != 0 {
                e.insert(diff);
            }
        }
    }
}

/// `inputs[r]` is the content of relation `r`; every multiplicity is positive.
pub fn run<R: Row>(plan: &Checked, inputs: &[Multiset<R>]) -> Result<Multiset<R>, EngineError> {
    eval_plan(plan, inputs, None)
}

fn eval_plan<R: Row>(
    plan: &Checked,
    inputs: &[Multiset<R>],
    recur: Option<&Multiset<R>>,
) -> Result<Multiset<R>, EngineError> {
    let out_layout = R::layout(plan.schema());
    let mut out = Multiset::new();
    match plan.op() {
        Op::Scan(relation) => return Ok(inputs[usize::from(relation.0)].clone()),
        Op::Recur => return Ok(recur.expect("a checked Recur sits in a step").clone()),
        Op::Filter { input, pred } => {
            let layout = R::layout(input.schema());
            for (row, n) in eval_plan(input, inputs, recur)? {
                if holds(pred, &row, &layout) {
                    add(&mut out, row, n);
                }
            }
        }
        Op::Project { input, exprs } => {
            let layout = R::layout(input.schema());
            for (row, n) in eval_plan(input, inputs, recur)? {
                let cols = exprs.iter().map(|e| eval(e, &row, &layout));
                add(&mut out, R::build(&out_layout, cols), n);
            }
        }
        Op::Join { left, right, keys } => {
            let (ll, rl) = (R::layout(left.schema()), R::layout(right.schema()));
            let mut rights: BTreeMap<Vec<Datum>, Vec<(R, isize)>> = BTreeMap::new();
            for (b, n) in eval_plan(right, inputs, recur)? {
                let key = keys.iter().map(|(_, r)| b.get(&rl, *r)).collect();
                rights.entry(key).or_default().push((b, n));
            }
            for (a, m) in eval_plan(left, inputs, recur)? {
                let key: Vec<Datum> = keys.iter().map(|(l, _)| a.get(&ll, *l)).collect();
                for (b, n) in rights.get(&key).into_iter().flatten() {
                    let cols = concat(
                        &a,
                        &ll,
                        left.schema().arity(),
                        b,
                        &rl,
                        right.schema().arity(),
                    );
                    add(&mut out, R::build(&out_layout, cols), m * n);
                }
            }
        }
        Op::Reduce { input, key, aggs } => {
            let layout = R::layout(input.schema());
            let mut groups: BTreeMap<Vec<Datum>, isize> = BTreeMap::new();
            for (row, n) in eval_plan(input, inputs, recur)? {
                *groups
                    .entry(key.iter().map(|c| row.get(&layout, *c)).collect())
                    .or_default() += n;
            }
            for (key, count) in groups {
                assert!(count > 0, "the group {key:?} has count {count}");
                let aggs = aggs.iter().map(|Agg::Count| Datum::Int(count as i64));
                add(
                    &mut out,
                    R::build(&out_layout, key.into_iter().chain(aggs)),
                    1,
                );
            }
        }
        Op::Iterate { seed, step } => return iterate(seed, step, inputs),
    }
    Ok(out)
}

/// The sequence `X0 = seed`, `X(k+1) = distinct(seed ∪ step(Xk))` up to its
/// fixed point: the sequence differential dataflow's `iterate` computes.
fn iterate<R: Row>(
    seed: &Rc<Checked>,
    step: &Rc<Checked>,
    inputs: &[Multiset<R>],
) -> Result<Multiset<R>, EngineError> {
    let seed = eval_plan(seed, inputs, None)?;
    let mut x = seed.clone();
    for round in 0.. {
        let mut next = eval_plan(step, inputs, Some(&x))?;
        for (row, n) in &seed {
            add(&mut next, row.clone(), *n);
        }
        let next: Multiset<R> = next
            .into_iter()
            .map(|(row, n)| {
                assert!(n > 0, "the step derived {row:?} with multiplicity {n}");
                (row, 1)
            })
            .collect();
        if next == x {
            return Ok(x);
        }
        if round >= MAX_DEPTH {
            return Err(EngineError::DepthBound);
        }
        x = next;
    }
    unreachable!("the rounds are unbounded")
}

pub(crate) fn concat<R: Row>(
    a: &R,
    al: &R::Layout,
    a_arity: usize,
    b: &R,
    bl: &R::Layout,
    b_arity: usize,
) -> impl Iterator<Item = Datum> {
    let left: Vec<Datum> = (0..a_arity).map(|i| a.get(al, col(i))).collect();
    let right: Vec<Datum> = (0..b_arity).map(|i| b.get(bl, col(i))).collect();
    left.into_iter().chain(right)
}

pub(crate) fn col(i: usize) -> crate::plan::Col {
    crate::plan::Col(u16::try_from(i).expect("a row has fewer than 65536 columns"))
}
