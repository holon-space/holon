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

fn law(name: &str, commits: Vec<Vec<Change>>) -> Result<(), TestCaseError> {
    let catalog = catalog();
    let plan = check(&plan(name), &catalog).expect("the corpus is well typed");
    let mut worker = worker();
    let mut dd = Dataflow::<DynRow>::build(&mut worker, &catalog, &[plan.clone()]);
    let mut inputs: Vec<Multiset<DynRow>> = vec![Multiset::new(); 2];
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
            .expect("the corpus stays below MAX_DEPTH");
        let expected = batch::run(&plan, &inputs).expect("the corpus stays below MAX_DEPTH");
        prop_assert_eq!(&*dd.output(0), &expected, "plan {} at time {}", name, time);
    }
    Ok(())
}

macro_rules! law_tests {
    ($($name:ident),* $(,)?) => {
        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]
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
);

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
    assert_eq!(dd.output(0).len() as u64, MAX_DEPTH + 1);

    let last = u32::try_from(MAX_DEPTH).unwrap();
    dd.update(EDGES, edge(last, last + 1), 1);
    assert_eq!(dd.commit(&mut worker), Err(EngineError::DepthBound));
}
