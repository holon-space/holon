//! The lowering law: after every commit, the dataflow output of a plan equals
//! the batch backend over the inputs at that commit.
#![cfg(not(target_family = "wasm"))]

use std::rc::Rc;

use holon_views::batch;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::error::EngineError;
use holon_views::error::MAX_DEPTH;
use holon_views::lower::Dataflow;
use holon_views::plan::Agg;
use holon_views::plan::Catalog;
use holon_views::plan::Checked;
use holon_views::plan::Col;
use holon_views::plan::ColType;
use holon_views::plan::Expr;
use holon_views::plan::Plan;
use holon_views::plan::RelationId;
use holon_views::plan::Schema;
use holon_views::plan::check;
use holon_views::row::Datum;
use holon_views::row::DynRow;
use holon_views::row::Id;
use holon_views::row::Row;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use timely::WorkerConfig;
use timely::communication::Allocator;
use timely::communication::allocator::Thread;
use timely::worker::Worker;

/// `nodes(id, flag, label)`
const NODES: RelationId = RelationId(0);
/// `edges(src, dst)`
const EDGES: RelationId = RelationId(1);
const LABELS: [&str; 3] = ["a", "b", "c"];

fn catalog() -> Catalog {
    Catalog {
        relations: vec![
            Schema(vec![ColType::Id, ColType::Bool, ColType::Text]),
            Schema(vec![ColType::Id, ColType::Id]),
        ],
    }
}

fn worker() -> Worker {
    Worker::new(
        WorkerConfig::default(),
        Allocator::Thread(Thread::default()),
        None,
    )
}

fn c(i: u16) -> Expr {
    Expr::Col(Col(i))
}

fn node(id: u32, flag: bool, label: &str) -> DynRow {
    DynRow::build(
        &(),
        [
            Datum::Id(Id(id)),
            Datum::Bool(flag),
            Datum::Text(label.into()),
        ],
    )
}

fn edge(src: u32, dst: u32) -> DynRow {
    DynRow::build(&(), [Datum::Id(Id(src)), Datum::Id(Id(dst))])
}

/// `(src, dst)` for every path of one or more edges.
fn reach(edges: &Rc<Plan>, step_edges: &Rc<Plan>) -> Rc<Plan> {
    edges.iterate(
        &Plan::recur()
            .join(step_edges, vec![(Col(1), Col(0))])
            .project(vec![c(0), c(3)]),
    )
}

fn plan(name: &str) -> Rc<Plan> {
    let nodes = Plan::scan(NODES);
    let edges = Plan::scan(EDGES);
    match name {
        "scan" => nodes,
        "filter" => nodes.filter(c(1)),
        "filter_not_eq" => nodes.filter(Expr::Not(Box::new(Expr::Eq(
            Box::new(c(2)),
            Box::new(Expr::Lit(Datum::Text("a".into()))),
        )))),
        "project_to_duplicates" => nodes.project(vec![c(1), Expr::Lit(Datum::Int(7))]),
        "join" => nodes.join(&edges, vec![(Col(0), Col(0))]),
        "join_on_two_keys" => edges.join(&edges, vec![(Col(0), Col(1)), (Col(1), Col(0))]),
        "reduce_by_key" => edges.reduce(vec![Col(0)], vec![Agg::Count]),
        "reduce_to_one_group" => nodes.reduce(vec![], vec![Agg::Count, Agg::Count]),
        "iterate" => reach(&edges, &edges),
        "join_inside_iterate" => {
            let into_flagged = edges
                .join(&nodes.filter(c(1)), vec![(Col(1), Col(0))])
                .project(vec![c(0), c(1)]);
            reach(&edges, &into_flagged)
        }
        "join_of_iterate" => reach(&edges, &edges).join(&nodes, vec![(Col(1), Col(0))]),
        "reduce_of_iterate" => reach(&edges, &edges).reduce(vec![Col(0)], vec![Agg::Count]),
        "join_recur_with_itself" => edges.iterate(
            &Plan::recur()
                .join(&Plan::recur(), vec![(Col(1), Col(0))])
                .project(vec![c(0), c(3)]),
        ),
        other => unreachable!("no plan {other}"),
    }
}

/// A change to the inputs. A delete removes one present row, picked by
/// position; on an empty relation it changes nothing.
#[derive(Debug, Clone)]
enum Change {
    Node(u32, bool, usize),
    Edge(u32, u32),
    Delete(RelationId, usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        (0..6u32, any::<bool>(), 0..LABELS.len()).prop_map(|(id, f, l)| Change::Node(id, f, l)),
        (0..6u32, 0..6u32).prop_map(|(src, dst)| Change::Edge(src, dst)),
        (prop_oneof![Just(NODES), Just(EDGES)], any::<usize>())
            .prop_map(|(r, pick)| Change::Delete(r, pick)),
    ]
}

fn commits() -> impl Strategy<Value = Vec<Vec<Change>>> {
    vec(vec(change(), 0..6), 1..40)
}

/// Where a generated sub-plan sits.
#[derive(Clone, Copy)]
enum Ctx<'a> {
    Top {
        iterate_left: bool,
    },
    /// The step of an `Iterate` over this schema, before its `Recur`.
    Step(&'a Schema),
    /// Inside a step, where no `Recur` may appear: below a `Reduce` (`check`
    /// refuses it) or after the step's one `Recur`.
    Invariant,
}

/// Builds a well-typed plan from a tape of choices. Choice 0 is the simplest
/// one and the tape reads as 0s past its end, so a shrunk tape is a smaller
/// plan.
struct Gen {
    tape: Vec<u32>,
    pos: usize,
}

impl Gen {
    fn pick(&mut self, n: usize) -> usize {
        let choice = self.tape.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        choice as usize % n
    }

    fn plan(&mut self, depth: usize, ctx: &mut Ctx) -> (Rc<Plan>, Schema) {
        match self.pick(if depth == 0 { 1 } else { 7 }) {
            0 => {
                let relation = match (self.pick(3), *ctx) {
                    (0 | 1, Ctx::Step(schema)) => {
                        let schema = schema.clone();
                        *ctx = Ctx::Invariant;
                        return (Plan::recur(), schema);
                    }
                    (1, _) => NODES,
                    _ => EDGES,
                };
                let schema = catalog().schema(relation).unwrap().clone();
                (Plan::scan(relation), schema)
            }
            1 => {
                let (input, schema) = self.plan(depth - 1, ctx);
                let pred = self.pred(&schema, 2);
                (input.filter(pred), schema)
            }
            2 => {
                let (input, schema) = self.plan(depth - 1, ctx);
                let exprs: Vec<Expr> = (0..self.pick(4)).map(|_| self.expr(&schema)).collect();
                let out = exprs.iter().map(|e| expr_type(e, &schema)).collect();
                (input.project(exprs), Schema(out))
            }
            3 | 4 => {
                let (left, ls) = self.plan(depth - 1, ctx);
                let (right, rs) = self.plan(depth - 1, ctx);
                let pairs: Vec<(Col, Col)> = cols(&ls)
                    .flat_map(|l| cols(&rs).map(move |r| (l, r)))
                    .filter(|(l, r)| ls.0[l.index()] == rs.0[r.index()])
                    .collect();
                if pairs.is_empty() {
                    return (left, ls);
                }
                let keys = (0..=self.pick(2))
                    .map(|_| pairs[self.pick(pairs.len())])
                    .collect();
                let mut schema = ls;
                schema.0.extend(rs.0);
                (left.join(&right, keys), schema)
            }
            5 => {
                let (input, schema) = match ctx {
                    Ctx::Top { .. } => self.plan(depth - 1, ctx),
                    Ctx::Step(_) | Ctx::Invariant => self.plan(depth - 1, &mut Ctx::Invariant),
                };
                let key: Vec<Col> = match schema.arity() {
                    0 => vec![],
                    arity => (0..self.pick(3))
                        .map(|_| Col(self.pick(arity) as u16))
                        .collect(),
                };
                let aggs = vec![Agg::Count; 1 + self.pick(2)];
                let mut out: Vec<ColType> = key.iter().map(|c| schema.0[c.index()]).collect();
                out.extend(aggs.iter().map(|Agg::Count| ColType::Int));
                (input.reduce(key, aggs), Schema(out))
            }
            _ => match ctx {
                Ctx::Top { iterate_left: true } if depth >= 2 => {
                    *ctx = Ctx::Top {
                        iterate_left: false,
                    };
                    let (seed, schema) = self.plan(depth - 1, ctx);
                    let (body, bs) = self.plan(depth - 2, &mut Ctx::Step(&schema));
                    let back = schema
                        .0
                        .iter()
                        .map(|t| match self.col_of(&bs, *t) {
                            Some(c) => Expr::Col(c),
                            None => self.lit(*t),
                        })
                        .collect();
                    (seed.iterate(&body.project(back)), schema)
                }
                _ => self.plan(depth - 1, ctx),
            },
        }
    }

    /// A `Bool` expression over `schema`.
    fn pred(&mut self, schema: &Schema, depth: usize) -> Expr {
        match self.pick(if depth == 0 { 3 } else { 4 }) {
            0 => self.lit(ColType::Bool),
            1 => match self.col_of(schema, ColType::Bool) {
                Some(c) => Expr::Col(c),
                None => self.lit(ColType::Bool),
            },
            2 => {
                let a = self.expr(schema);
                let t = expr_type(&a, schema);
                let b = match (self.pick(2), self.col_of(schema, t)) {
                    (1, Some(c)) => Expr::Col(c),
                    _ => self.lit(t),
                };
                Expr::Eq(Box::new(a), Box::new(b))
            }
            _ => Expr::Not(Box::new(self.pred(schema, depth - 1))),
        }
    }

    /// A column of `schema`, a literal, or a predicate.
    fn expr(&mut self, schema: &Schema) -> Expr {
        match self.pick(3) {
            0 if schema.arity() > 0 => Expr::Col(Col(self.pick(schema.arity()) as u16)),
            1 => {
                let t = [ColType::Bool, ColType::Int, ColType::Id, ColType::Text][self.pick(4)];
                self.lit(t)
            }
            _ => self.pred(schema, 1),
        }
    }

    fn col_of(&mut self, schema: &Schema, t: ColType) -> Option<Col> {
        let of_type: Vec<Col> = cols(schema).filter(|c| schema.0[c.index()] == t).collect();
        match of_type.len() {
            0 => None,
            n => Some(of_type[self.pick(n)]),
        }
    }

    fn lit(&mut self, t: ColType) -> Expr {
        Expr::Lit(match t {
            ColType::Bool => Datum::Bool(self.pick(2) == 1),
            ColType::Int => Datum::Int(self.pick(4) as i64),
            ColType::Id => Datum::Id(Id(self.pick(6) as u32)),
            ColType::Text => Datum::Text(LABELS[self.pick(LABELS.len())].into()),
            ColType::Payload => unreachable!("the law catalog has no payload column"),
        })
    }
}

fn cols(schema: &Schema) -> impl Iterator<Item = Col> + '_ {
    (0..schema.arity()).map(|i| Col(i as u16))
}

fn expr_type(expr: &Expr, schema: &Schema) -> ColType {
    match expr {
        Expr::Col(c) => schema.0[c.index()],
        Expr::Lit(d) => d.col_type(),
        Expr::Eq(..) | Expr::Not(_) => ColType::Bool,
    }
}

/// Depth ≤ 4, every operator, at most one `Iterate`, whose step reads `Recur`
/// at most once: a step that joins `Recur` with itself costs `|X|³` per round.
fn random_plan() -> impl Strategy<Value = Rc<Plan>> {
    vec(any::<u32>(), 0..64).prop_map(|tape| {
        let (plan, _) = Gen { tape, pos: 0 }.plan(4, &mut Ctx::Top { iterate_left: true });
        plan
    })
}

fn law(name: &str, commits: Vec<Vec<Change>>) -> Result<(), TestCaseError> {
    law_of(&plan(name), commits)
}

fn law_of(plan: &Rc<Plan>, commits: Vec<Vec<Change>>) -> Result<(), TestCaseError> {
    let catalog = catalog();
    let plan = check(plan, &catalog).expect("every tested plan is well typed");
    let mut worker = worker();
    let mut dd = Dataflow::<DynRow>::build(&mut worker, &catalog, &[plan.clone()]);
    let mut inputs: Vec<Multiset<DynRow>> = vec![Multiset::new(); 2];
    let mut output = Multiset::new();
    for commit in commits {
        for change in commit {
            let (relation, row, diff) = match change {
                Change::Node(id, flag, label) => (NODES, node(id, flag, LABELS[label]), 1),
                Change::Edge(src, dst) => (EDGES, edge(src, dst), 1),
                Change::Delete(relation, pick) => {
                    let present = &inputs[usize::from(relation.0)];
                    if present.is_empty() {
                        continue;
                    }
                    let row = present.keys().nth(pick % present.len()).unwrap().clone();
                    (relation, row, -1)
                }
            };
            add(&mut inputs[usize::from(relation.0)], row.clone(), diff);
            dd.update(relation, row, diff);
        }
        let time = dd
            .commit(&mut worker)
            .expect("ids 0..6 stay below MAX_DEPTH");
        let expected = batch::run(&plan, &inputs).expect("ids 0..6 stay below MAX_DEPTH");
        for (row, diff) in dd.take_changes(0).unwrap() {
            add(&mut output, row, diff);
        }
        prop_assert_eq!(&output, &expected, "at time {}", time);
    }
    Ok(())
}

fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/plan_law.proptest-regressions"
        )))),
        ..ProptestConfig::with_cases(256)
    }
}

macro_rules! law_tests {
    ($($name:ident),* $(,)?) => {
        proptest! {
            #![proptest_config(config())]
            $(
                #[test]
                fn $name(commits in commits()) {
                    law(stringify!($name), commits)?;
                }
            )*
        }
    };
}

law_tests!(
    scan,
    filter,
    filter_not_eq,
    project_to_duplicates,
    join,
    join_on_two_keys,
    reduce_by_key,
    reduce_to_one_group,
    iterate,
    join_inside_iterate,
    join_of_iterate,
    reduce_of_iterate,
    join_recur_with_itself,
);

proptest! {
    #![proptest_config(config())]
    #[test]
    fn random_plans(plan in random_plan(), commits in commits()) {
        law_of(&plan, commits)?;
    }
}

#[test]
fn a_group_that_empties_disappears() {
    let fill_then_empty = || {
        vec![
            vec![
                Change::Edge(1, 2),
                Change::Edge(1, 3),
                Change::Node(4, true, 0),
            ],
            vec![Change::Delete(EDGES, 0)],
            vec![Change::Delete(EDGES, 0), Change::Delete(NODES, 0)],
        ]
    };
    law("reduce_by_key", fill_then_empty()).unwrap();
    law("reduce_to_one_group", fill_then_empty()).unwrap();
}

/// The flagged nodes and every node below them.
fn below_flagged() -> Rc<Checked> {
    let plan = Plan::scan(NODES).filter(c(1)).project(vec![c(0)]).iterate(
        &Plan::recur()
            .join(&Plan::scan(EDGES), vec![(Col(0), Col(0))])
            .project(vec![c(2)]),
    );
    check(&plan, &catalog()).unwrap()
}

/// Node 0, flagged, heads a chain of `depth` edges.
fn chain(depth: u64) -> Vec<(RelationId, DynRow)> {
    let depth = u32::try_from(depth).unwrap();
    std::iter::once((NODES, node(0, true, "a")))
        .chain((0..depth).map(|i| (EDGES, edge(i, i + 1))))
        .collect()
}

fn batch_inputs(rows: &[(RelationId, DynRow)]) -> Vec<Multiset<DynRow>> {
    let mut inputs = vec![Multiset::new(); 2];
    for (relation, row) in rows {
        add(&mut inputs[usize::from(relation.0)], row.clone(), 1);
    }
    inputs
}

#[test]
fn batch_resolves_max_depth_and_refuses_one_more() {
    let plan = below_flagged();
    let deepest = batch::run(&plan, &batch_inputs(&chain(MAX_DEPTH))).unwrap();
    assert_eq!(deepest.len() as u64, MAX_DEPTH + 1);
    assert_eq!(
        batch::run(&plan, &batch_inputs(&chain(MAX_DEPTH + 1))),
        Err(EngineError::DepthBound)
    );
}

#[test]
fn dataflow_resolves_max_depth_and_refuses_one_more() {
    let mut worker = worker();
    let mut dd = Dataflow::<DynRow>::build(&mut worker, &catalog(), &[below_flagged()]);
    for (relation, row) in chain(MAX_DEPTH) {
        dd.update(relation, row, 1);
    }
    dd.commit(&mut worker).unwrap();
    assert_eq!(dd.take_changes(0).unwrap().len() as u64, MAX_DEPTH + 1);

    let last = u32::try_from(MAX_DEPTH).unwrap();
    dd.update(EDGES, edge(last, last + 1), 1);
    assert_eq!(dd.commit(&mut worker), Err(EngineError::DepthBound));
}

#[test]
fn a_depth_fault_is_sticky() {
    let mut worker = worker();
    let mut dd = Dataflow::<DynRow>::build(&mut worker, &catalog(), &[below_flagged()]);
    for (relation, row) in chain(MAX_DEPTH + 1) {
        dd.update(relation, row, 1);
    }
    assert_eq!(dd.commit(&mut worker), Err(EngineError::DepthBound));
    assert_eq!(dd.take_changes(0).err(), Some(EngineError::DepthBound));

    let last = u32::try_from(MAX_DEPTH).unwrap();
    dd.update(EDGES, edge(last, last + 1), -1);
    assert_eq!(dd.commit(&mut worker), Err(EngineError::DepthBound));
    assert_eq!(dd.commit(&mut worker), Err(EngineError::DepthBound));
    assert_eq!(dd.take_changes(0).err(), Some(EngineError::DepthBound));
}
