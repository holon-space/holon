//! The incremental backend: a plan lowered to differential dataflow. After
//! every commit its output equals [`crate::batch::run`] over the inputs at
//! that commit.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use differential_dataflow::VecCollection;
use differential_dataflow::input::Input;
use differential_dataflow::input::InputSession;
use differential_dataflow::lattice::Lattice;
use differential_dataflow::operators::Iterate;
use differential_dataflow::operators::arrange::Arranged;
use differential_dataflow::operators::arrange::TraceAgent;
use differential_dataflow::trace::implementations::ValSpine;
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
/// Rows keyed by a key row.
type Arr<'s, T, R> = Arranged<'s, TraceAgent<ValSpine<R, R, T, isize>>>;
type Fault = Rc<RefCell<Option<EngineError>>>;

/// What one scope built, by [`Checked`] node, so a node two plans share is
/// built once.
struct Cache<'s, T: Timestamp + Lattice, R: Row> {
    colls: HashMap<usize, Coll<'s, T, R>>,
    /// By node and key columns.
    arrangements: HashMap<(usize, Vec<Col>), Arr<'s, T, R>>,
    /// Arrangements built in every scope of the dataflow.
    built: Rc<Cell<usize>>,
}

impl<'s, T: Timestamp + Lattice, R: Row> Cache<'s, T, R> {
    fn new(built: Rc<Cell<usize>>) -> Self {
        Cache {
            colls: HashMap::new(),
            arrangements: HashMap::new(),
            built,
        }
    }
}

/// The scope a plan is lowered into resolves the leaves of that scope.
trait Env<'s, T: Timestamp + Lattice, R: Row> {
    /// `None` for an operator, which [`lower`] builds from its inputs.
    fn leaf(&mut self, plan: &Rc<Checked>) -> Option<Coll<'s, T, R>>;
    fn cache(&mut self) -> &mut Cache<'s, T, R>;
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
    if let Some(c) = env.cache().colls.get(&key_of(plan)) {
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
                let l = arrange(left, keys.iter().map(|(l, _)| *l).collect(), env);
                let r = arrange(right, keys.iter().map(|(_, r)| *r).collect(), env);
                let (ll, la) = (R::layout(left.schema()), left.schema().arity());
                let (rl, ra) = (R::layout(right.schema()), right.schema().arity());
                let out = R::layout(plan.schema());
                l.join_core(r, move |_, a, b| {
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
    env.cache().colls.insert(key_of(plan), out.clone());
    out
}

/// The rows of `plan` keyed by its `cols`.
fn arrange<'s, T, R, E>(plan: &Rc<Checked>, cols: Vec<Col>, env: &mut E) -> Arr<'s, T, R>
where
    T: Timestamp + Lattice,
    R: Row,
    E: Env<'s, T, R>,
{
    let id = (key_of(plan), cols.clone());
    if let Some(a) = env.cache().arrangements.get(&id) {
        return a.clone();
    }
    let key_layout = R::layout(&plan.schema().pick(&cols).expect("checked key columns"));
    let arranged = keyed(lower(plan, env), plan.schema(), cols, key_layout).arrange_by_key();
    let cache = env.cache();
    cache.built.set(cache.built.get() + 1);
    cache.arrangements.insert(id, arranged.clone());
    arranged
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
    cache: Cache<'s, u64, R>,
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

    fn cache(&mut self) -> &mut Cache<'s, u64, R> {
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
        let built = self.cache.built.clone();
        seed.iterate(move |sub, x| {
            let mut cache = Cache::new(built);
            cache.colls = entered
                .into_iter()
                .map(|(k, c)| (k, c.enter(sub)))
                .collect();
            let mut inner = Inner { rec: x, cache };
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
    cache: Cache<'s, Product<u64, u64>, R>,
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

    fn cache(&mut self) -> &mut Cache<'s, Product<u64, u64>, R> {
        &mut self.cache
    }
}

/// One dataflow over the relations of a catalog, with one output per plan.
pub struct Dataflow<R: Row> {
    /// By [`RelationId`].
    inputs: Vec<InputSession<u64, R, isize>>,
    /// Per plan, the output changes not yet taken.
    changes: Vec<Rc<RefCell<Multiset<R>>>>,
    probe: Handle<u64>,
    fault: Fault,
    /// The time of the next commit.
    time: u64,
    /// Whether an update waits for the next commit.
    pending: bool,
    arrangements: usize,
}

impl<R: Row> Dataflow<R> {
    pub fn build(worker: &mut Worker, catalog: &Catalog, plans: &[Rc<Checked>]) -> Self {
        let probe = Handle::new();
        let fault = Fault::default();
        let built = Rc::new(Cell::new(0));
        let changes: Vec<Rc<RefCell<Multiset<R>>>> = plans.iter().map(|_| Rc::default()).collect();
        let inputs = worker.dataflow::<u64, _, _>(|scope| {
            let (inputs, scans) = catalog
                .relations
                .iter()
                .map(|_| scope.new_collection::<R, isize>())
                .unzip();
            let mut env = Top {
                scans,
                fault: fault.clone(),
                cache: Cache::new(built.clone()),
            };
            for (plan, out) in plans.iter().zip(&changes) {
                let out = out.clone();
                lower(plan, &mut env)
                    .inspect(move |(row, _, diff)| add(&mut out.borrow_mut(), row.clone(), *diff))
                    .probe_with(&probe);
            }
            inputs
        });
        Dataflow {
            inputs,
            changes,
            probe,
            fault,
            time: 0,
            pending: false,
            arrangements: built.get(),
        }
    }

    /// The arrangements the dataflow keeps, one per shared (node, key).
    pub fn arrangements(&self) -> usize {
        self.arrangements
    }

    pub fn update(&mut self, relation: RelationId, row: R, diff: isize) {
        self.inputs[usize::from(relation.0)].update(row, diff);
        self.pending = true;
    }

    /// Makes `time` the time of the next commit.
    pub fn advance_to(&mut self, time: u64) {
        assert!(
            !self.pending,
            "updates wait for the commit at {}",
            self.time
        );
        assert!(
            time >= self.time,
            "time {time} is before the next commit at {}",
            self.time
        );
        for input in &mut self.inputs {
            input.advance_to(time);
        }
        self.time = time;
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
        self.pending = false;
        Ok(t)
    }

    /// The change of `plans[i]`'s output since the last take, through the
    /// last commit; after the first error, that error.
    pub fn take_changes(&mut self, i: usize) -> Result<Multiset<R>, EngineError> {
        self.faulted()?;
        Ok(std::mem::take(&mut *self.changes[i].borrow_mut()))
    }

    fn faulted(&self) -> Result<(), EngineError> {
        match self.fault.borrow().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
