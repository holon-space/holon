//! The incremental backend: a plan lowered to differential dataflow. After
//! every commit its output equals [`crate::batch::run`] over the inputs at
//! that commit.

use std::cell::Ref;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use differential_dataflow::VecCollection;
use differential_dataflow::input::Input;
use differential_dataflow::input::InputSession;
use differential_dataflow::lattice::Lattice;
use differential_dataflow::operators::Iterate;
use timely::dataflow::operators::probe::Handle;
use timely::order::Product;
use timely::progress::Timestamp;
use timely::worker::Worker;

use crate::batch::Multiset;
use crate::batch::add;
use crate::batch::col;
use crate::batch::concat;
use crate::error::EngineError;
use crate::error::MAX_DEPTH;
use crate::eval::eval;
use crate::eval::holds;
use crate::plan::Agg;
use crate::plan::Catalog;
use crate::plan::Checked;
use crate::plan::Col;
use crate::plan::Op;
use crate::plan::RelationId;
use crate::plan::Schema;
use crate::row::Datum;
use crate::row::Row;

type Coll<'s, T, R> = VecCollection<'s, T, R, isize>;
type Fault = Rc<RefCell<Option<EngineError>>>;

/// The scope a plan is lowered into resolves the leaves of that scope.
trait Env<'s, T: Timestamp + Lattice, R: Row> {
    /// `None` for an operator, which [`lower`] builds from its inputs.
    fn leaf(&mut self, plan: &Rc<Checked>) -> Option<Coll<'s, T, R>>;
    fn cache(&mut self) -> &mut HashMap<usize, Coll<'s, T, R>>;
}

fn key_of(plan: &Rc<Checked>) -> usize {
    Rc::as_ptr(plan) as usize
}

fn lower<'s, T, R, E>(plan: &Rc<Checked>, env: &mut E) -> Coll<'s, T, R>
where
    T: Timestamp + Lattice,
    R: Row,
    E: Env<'s, T, R>,
{
    if let Some(c) = env.cache().get(&key_of(plan)) {
        return c.clone();
    }
    let out = match env.leaf(plan) {
        Some(c) => c,
        None => match plan.op() {
            Op::Filter { input, pred } => {
                let (pred, layout) = (pred.clone(), R::layout(input.schema()));
                lower(input, env).filter(move |row| holds(&pred, row, &layout))
            }
            Op::Project { input, exprs } => {
                let (exprs, layout) = (exprs.clone(), R::layout(input.schema()));
                let out = R::layout(plan.schema());
                lower(input, env)
                    .map(move |row| R::build(&out, exprs.iter().map(|e| eval(e, &row, &layout))))
            }
            Op::Join { left, right, keys } => {
                let key_schema = Schema(
                    keys.iter()
                        .map(|(l, _)| left.schema().0[l.index()])
                        .collect(),
                );
                let key_layout = R::layout(&key_schema);
                let left_keys = keys.iter().map(|(l, _)| *l).collect();
                let right_keys = keys.iter().map(|(_, r)| *r).collect();
                let l = keyed(
                    lower(left, env),
                    left.schema(),
                    left_keys,
                    key_layout.clone(),
                );
                let r = keyed(lower(right, env), right.schema(), right_keys, key_layout);
                let (ll, la) = (R::layout(left.schema()), left.schema().arity());
                let (rl, ra) = (R::layout(right.schema()), right.schema().arity());
                let out = R::layout(plan.schema());
                l.arrange_by_key()
                    .join_core(r.arrange_by_key(), move |_, a, b| {
                        Some(R::build(&out, concat(a, &ll, la, b, &rl, ra)))
                    })
            }
            Op::Reduce { input, key, aggs } => {
                let key_layout = R::layout(&Schema(plan.schema().0[..key.len()].to_vec()));
                let (key_arity, aggs, out) = (key.len(), aggs.clone(), R::layout(plan.schema()));
                keyed(
                    lower(input, env),
                    input.schema(),
                    key.clone(),
                    key_layout.clone(),
                )
                .map(|(key, _)| (key, ()))
                .reduce(|_, group, counts| {
                    counts.push((group.iter().map(|(_, n)| *n as i64).sum::<i64>(), 1))
                })
                .map(move |(key, count)| {
                    let key = (0..key_arity).map(|i| key.get(&key_layout, col(i)));
                    R::build(
                        &out,
                        key.chain(aggs.iter().map(|Agg::Count| Datum::Int(count))),
                    )
                })
            }
            Op::Scan(_) | Op::Iterate { .. } | Op::Recur => {
                unreachable!("{:?} is a leaf of its scope", plan.op())
            }
        },
    };
    env.cache().insert(key_of(plan), out.clone());
    out
}

/// Each row keyed by its `cols`, the key built with `key_layout`.
fn keyed<'s, T, R>(
    c: Coll<'s, T, R>,
    schema: &Schema,
    cols: Vec<Col>,
    key_layout: R::Layout,
) -> VecCollection<'s, T, (R, R), isize>
where
    T: Timestamp + Lattice,
    R: Row,
{
    let layout = R::layout(schema);
    c.map(move |row| {
        let key = R::build(&key_layout, cols.iter().map(|c| row.get(&layout, *c)));
        (key, row)
    })
}

/// The maximal sub-plans of a step that do not read its `Recur`.
fn invariant_parts(plan: &Rc<Checked>, out: &mut Vec<Rc<Checked>>) {
    if !plan.reads_recur() {
        out.push(plan.clone());
        return;
    }
    match plan.op() {
        Op::Recur => {}
        Op::Filter { input, .. } | Op::Project { input, .. } | Op::Reduce { input, .. } => {
            invariant_parts(input, out)
        }
        Op::Join { left, right, .. } => {
            invariant_parts(left, out);
            invariant_parts(right, out);
        }
        Op::Scan(_) | Op::Iterate { .. } => unreachable!("{:?} reads no Recur", plan.op()),
    }
}

struct Top<'s, R: Row> {
    /// By [`RelationId`].
    scans: Vec<Coll<'s, u64, R>>,
    fault: Fault,
    cache: HashMap<usize, Coll<'s, u64, R>>,
}

impl<'s, R: Row> Env<'s, u64, R> for Top<'s, R> {
    fn leaf(&mut self, plan: &Rc<Checked>) -> Option<Coll<'s, u64, R>> {
        match plan.op() {
            Op::Scan(relation) => Some(self.scans[usize::from(relation.0)].clone()),
            Op::Iterate { seed, step } => Some(self.iterate(seed, step)),
            Op::Recur => unreachable!("a checked Recur sits in a step"),
            Op::Filter { .. } | Op::Project { .. } | Op::Join { .. } | Op::Reduce { .. } => None,
        }
    }

    fn cache(&mut self) -> &mut HashMap<usize, Coll<'s, u64, R>> {
        &mut self.cache
    }
}

impl<'s, R: Row> Top<'s, R> {
    /// Round `k` of the loop holds `X(k+1)`, so a round past [`MAX_DEPTH`]
    /// that still changes a row is a chain deeper than the bound.
    fn iterate(&mut self, seed: &Rc<Checked>, step: &Rc<Checked>) -> Coll<'s, u64, R> {
        let seed = lower(seed, self);
        let mut parts = Vec::new();
        invariant_parts(step, &mut parts);
        let entered: Vec<(usize, Coll<'s, u64, R>)> =
            parts.iter().map(|p| (key_of(p), lower(p, self))).collect();
        let (step, fault, seed_again) = (step.clone(), self.fault.clone(), seed.clone());
        seed.iterate(move |sub, x| {
            let mut inner = Inner {
                rec: x,
                cache: entered
                    .into_iter()
                    .map(|(k, c)| (k, c.enter(sub)))
                    .collect(),
            };
            lower(&step, &mut inner)
                .concat(seed_again.enter(sub))
                .distinct()
                .inspect(move |(_, time, _)| {
                    if time.inner >= MAX_DEPTH {
                        fault.borrow_mut().get_or_insert(EngineError::DepthBound);
                    }
                })
        })
    }
}

/// The step of an `Iterate`: `Recur` is the loop variable, and every part
/// that does not read it was lowered outside the loop and entered.
struct Inner<'s, R: Row> {
    rec: Coll<'s, Product<u64, u64>, R>,
    cache: HashMap<usize, Coll<'s, Product<u64, u64>, R>>,
}

impl<'s, R: Row> Env<'s, Product<u64, u64>, R> for Inner<'s, R> {
    fn leaf(&mut self, plan: &Rc<Checked>) -> Option<Coll<'s, Product<u64, u64>, R>> {
        match plan.op() {
            Op::Recur => Some(self.rec.clone()),
            Op::Scan(_) | Op::Iterate { .. } => {
                unreachable!("{:?} reads no Recur, so it was entered", plan.op())
            }
            Op::Filter { .. } | Op::Project { .. } | Op::Join { .. } | Op::Reduce { .. } => None,
        }
    }

    fn cache(&mut self) -> &mut HashMap<usize, Coll<'s, Product<u64, u64>, R>> {
        &mut self.cache
    }
}

/// One dataflow over the relations of a catalog, with one output per plan.
pub struct Dataflow<R: Row> {
    /// By [`RelationId`].
    inputs: Vec<InputSession<u64, R, isize>>,
    outputs: Vec<Rc<RefCell<Multiset<R>>>>,
    probe: Handle<u64>,
    fault: Fault,
    /// The time of the next commit.
    time: u64,
}

impl<R: Row> Dataflow<R> {
    pub fn build(worker: &mut Worker, catalog: &Catalog, plans: &[Rc<Checked>]) -> Self {
        let probe = Handle::new();
        let fault = Fault::default();
        let outputs: Vec<Rc<RefCell<Multiset<R>>>> = plans.iter().map(|_| Rc::default()).collect();
        let inputs = worker.dataflow::<u64, _, _>(|scope| {
            let (inputs, scans) = catalog
                .relations
                .iter()
                .map(|_| scope.new_collection::<R, isize>())
                .unzip();
            let mut env = Top {
                scans,
                fault: fault.clone(),
                cache: HashMap::new(),
            };
            for (plan, out) in plans.iter().zip(&outputs) {
                let out = out.clone();
                lower(plan, &mut env)
                    .inspect(move |(row, _, diff)| add(&mut out.borrow_mut(), row.clone(), *diff))
                    .probe_with(&probe);
            }
            inputs
        });
        Dataflow {
            inputs,
            outputs,
            probe,
            fault,
            time: 0,
        }
    }

    pub fn update(&mut self, relation: RelationId, row: R, diff: isize) {
        self.inputs[usize::from(relation.0)].update(row, diff);
    }

    /// Applies the updates since the last commit, as one version; returns
    /// its time. After the first error every commit returns that error.
    pub fn commit(&mut self, worker: &mut Worker) -> Result<u64, EngineError> {
        let t = self.time;
        for input in &mut self.inputs {
            input.advance_to(t + 1);
            input.flush();
        }
        worker.step_while(|| self.probe.less_than(&(t + 1)));
        self.faulted()?;
        self.time = t + 1;
        Ok(t)
    }

    /// The output of `plans[i]`, accumulated through the last commit; after
    /// the first error, that error.
    pub fn output(&self, i: usize) -> Result<Ref<'_, Multiset<R>>, EngineError> {
        self.faulted()?;
        Ok(self.outputs[i].borrow())
    }

    fn faulted(&self) -> Result<(), EngineError> {
        match self.fault.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
