//! Holon's views over a generated block forest: after every commit the
//! dataflow equals the batch backend, and the batch backend equals a direct
//! spec of each view. Plus the payload codec and arrangement sharing.
#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::REMOVED_MARKER_KEY;
use holon_api::RemovedTag;
use holon_api::Value;
use holon_api::block::SnapshotBlock;
use holon_views::batch;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::error::EngineError;
use holon_views::intern::Interner;
use holon_views::lower::Dataflow;
use holon_views::payload::Payload;
use holon_views::plan::Col;
use holon_views::plan::Expr;
use holon_views::plan::Plan;
use holon_views::plan::check_all;
use holon_views::row::Datum;
use holon_views::row::DynRow;
use holon_views::row::Row;
use holon_views::views::BLOCKS;
use holon_views::views::ID;
use holon_views::views::IS_PAGE;
use holon_views::views::PARENT;
use holon_views::views::block_row;
use holon_views::views::catalog;
use holon_views::views::views;
use proptest::collection::hash_map;
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use timely::WorkerConfig;
use timely::communication::Allocator;
use timely::communication::allocator::Thread;
use timely::worker::Worker;

fn worker() -> Worker {
    Worker::new(
        WorkerConfig::default(),
        Allocator::Thread(Thread::default()),
        None,
    )
}

fn uri(n: u32) -> EntityUri {
    EntityUri::block(&format!("b{n}"))
}

fn snapshot(
    id: EntityUri,
    parent: EntityUri,
    page: bool,
    sort: &str,
    props: &HashMap<String, Value>,
) -> SnapshotBlock {
    let mut block = Block {
        id,
        parent_id: parent,
        properties: props.clone(),
        created_at: 0,
        updated_at: 0,
        ..Block::default()
    };
    block.set_page(page);
    SnapshotBlock {
        block,
        sort_key: sort.to_string(),
    }
}

#[derive(Debug, Clone)]
struct Node {
    /// `None` under the root sentinel.
    parent: Option<u32>,
    page: bool,
    sort: String,
    props: HashMap<String, Value>,
}

#[derive(Debug, Default)]
struct Forest {
    nodes: BTreeMap<u32, Node>,
    next: u32,
}

/// One edit of the forest. A `usize` picks a block by position among the
/// candidates; with no candidate the edit changes nothing.
#[derive(Debug, Clone)]
enum Edit {
    Create {
        parent: Option<usize>,
        page: bool,
        sort: u8,
    },
    /// To the root, or to a block outside the moved subtree.
    Move {
        node: usize,
        parent: Option<usize>,
    },
    TogglePage(usize),
    SetProps(usize, HashMap<String, Value>),
    DeleteLeaf(usize),
    DeleteSubtree(usize),
    /// The source feeds its whole state again: every row retracted and
    /// inserted in the same commit.
    Replace,
}

fn pick(candidates: impl IntoIterator<Item = u32>, i: usize) -> Option<u32> {
    let all: Vec<u32> = candidates.into_iter().collect();
    (!all.is_empty()).then(|| all[i % all.len()])
}

impl Forest {
    fn subtree(&self, root: u32) -> BTreeSet<u32> {
        let mut out = BTreeSet::from([root]);
        loop {
            let below: Vec<u32> = self
                .nodes
                .iter()
                .filter(|(n, node)| {
                    !out.contains(*n) && node.parent.is_some_and(|p| out.contains(&p))
                })
                .map(|(n, _)| *n)
                .collect();
            if below.is_empty() {
                return out;
            }
            out.extend(below);
        }
    }

    fn apply(&mut self, edit: &Edit) {
        let blocks = || self.nodes.keys().copied().collect::<Vec<_>>();
        match edit {
            Edit::Create { parent, page, sort } => {
                let parent = parent.and_then(|i| pick(blocks(), i));
                self.nodes.insert(
                    self.next,
                    Node {
                        parent,
                        page: *page,
                        sort: format!("{sort:02x}"),
                        props: HashMap::new(),
                    },
                );
                self.next += 1;
            }
            Edit::Move { node, parent } => {
                let Some(node) = pick(blocks(), *node) else {
                    return;
                };
                let below = self.subtree(node);
                let parent = parent
                    .and_then(|i| pick(blocks().into_iter().filter(|b| !below.contains(b)), i));
                self.nodes.get_mut(&node).unwrap().parent = parent;
            }
            Edit::TogglePage(i) => {
                if let Some(n) = pick(blocks(), *i) {
                    let node = self.nodes.get_mut(&n).unwrap();
                    node.page = !node.page;
                }
            }
            Edit::SetProps(i, props) => {
                if let Some(n) = pick(blocks(), *i) {
                    self.nodes.get_mut(&n).unwrap().props = props.clone();
                }
            }
            Edit::DeleteLeaf(i) => {
                let parents: BTreeSet<u32> = self.nodes.values().filter_map(|n| n.parent).collect();
                if let Some(n) = pick(blocks().into_iter().filter(|b| !parents.contains(b)), *i) {
                    self.nodes.remove(&n);
                }
            }
            Edit::DeleteSubtree(i) => {
                if let Some(n) = pick(blocks(), *i) {
                    for b in self.subtree(n) {
                        self.nodes.remove(&b);
                    }
                }
            }
            Edit::Replace => {}
        }
    }

    fn snapshot(&self, n: u32) -> SnapshotBlock {
        let node = &self.nodes[&n];
        let parent = node.parent.map_or_else(EntityUri::no_parent, uri);
        snapshot(uri(n), parent, node.page, &node.sort, &node.props)
    }
}

fn row(cols: impl IntoIterator<Item = Datum>) -> DynRow {
    DynRow::build(&(), cols)
}

fn id(interner: &mut Interner, uri: &EntityUri) -> Datum {
    Datum::Id(interner.intern(uri))
}

/// `(parent, sort, id)` of every block.
fn spec_children(forest: &Forest, interner: &mut Interner) -> Multiset<DynRow> {
    let mut out = Multiset::new();
    for n in forest.nodes.keys() {
        let block = forest.snapshot(*n);
        let cols = [
            id(interner, &block.block.parent_id),
            Datum::Text(block.sort_key.as_str().into()),
            id(interner, &uri(*n)),
        ];
        add(&mut out, row(cols), 1);
    }
    out
}

/// `(node, page)`: walk up from each block to the first page.
fn spec_owning_page(forest: &Forest, interner: &mut Interner) -> Multiset<DynRow> {
    let mut out = Multiset::new();
    for n in forest.nodes.keys() {
        let mut at = Some(*n);
        while let Some(a) = at {
            if forest.nodes[&a].page {
                add(
                    &mut out,
                    row([id(interner, &uri(*n)), id(interner, &uri(a))]),
                    1,
                );
                break;
            }
            at = forest.nodes[&a].parent;
        }
    }
    out
}

/// Every payload of the `row` view decodes to the block of its id.
fn check_row_view(
    rows: &Multiset<DynRow>,
    forest: &Forest,
    interner: &Interner,
) -> Result<(), TestCaseError> {
    prop_assert_eq!(rows.len(), forest.nodes.len());
    for (r, n) in rows {
        prop_assert_eq!(*n, 1);
        let (Datum::Id(id), Datum::Payload(payload)) = (r.get(&(), Col(0)), r.get(&(), Col(1)))
        else {
            panic!("the row view yields (Id, Payload), got {r:?}");
        };
        let block = payload.decode().unwrap();
        prop_assert_eq!(&block.block.id, interner.uri(id));
        let n: u32 = block.block.id.id().trim_start_matches('b').parse().unwrap();
        prop_assert_eq!(block, forest.snapshot(n));
    }
    Ok(())
}

fn law(commits: Vec<Vec<Edit>>) -> Result<(), TestCaseError> {
    let catalog = catalog();
    let plans = check_all(&views().plans(), &catalog).expect("the views are well typed");
    let mut worker = worker();
    let mut dd = Dataflow::<DynRow>::build(&mut worker, &catalog, &plans);
    let mut interner = Interner::default();
    let mut forest = Forest::default();
    let mut fed: BTreeMap<u32, DynRow> = BTreeMap::new();
    let mut outputs = vec![Multiset::new(); plans.len()];
    for commit in commits {
        let replace = commit.iter().any(|e| matches!(e, Edit::Replace));
        for edit in &commit {
            forest.apply(edit);
        }
        let rows: BTreeMap<u32, DynRow> = forest
            .nodes
            .keys()
            .map(|n| {
                let row = block_row(&mut interner, &forest.snapshot(*n))
                    .expect("generated floats are finite");
                (*n, row)
            })
            .collect();
        for (n, row) in &fed {
            if replace || rows.get(n) != Some(row) {
                dd.update(BLOCKS, row.clone(), -1);
            }
        }
        for (n, row) in &rows {
            if replace || fed.get(n) != Some(row) {
                dd.update(BLOCKS, row.clone(), 1);
            }
        }
        fed = rows;
        let time = dd
            .commit(&mut worker)
            .expect("a generated forest is acyclic and shallow");
        let inputs = vec![
            fed.values()
                .map(|r| (r.clone(), 1))
                .collect::<Multiset<DynRow>>(),
        ];
        let batch: Vec<Multiset<DynRow>> = plans
            .iter()
            .map(|p| batch::run(p, &inputs).expect("a generated forest is shallow"))
            .collect();
        for (i, expected) in batch.iter().enumerate() {
            for (row, diff) in dd.take_changes(i).unwrap() {
                add(&mut outputs[i], row, diff);
            }
            prop_assert_eq!(&outputs[i], expected, "view {} at time {}", i, time);
        }
        prop_assert_eq!(
            &batch[0],
            &spec_children(&forest, &mut interner),
            "children at time {}",
            time
        );
        prop_assert_eq!(
            &batch[1],
            &spec_owning_page(&forest, &mut interner),
            "owning_page at time {}",
            time
        );
        check_row_view(&batch[2], &forest, &interner)?;
    }
    Ok(())
}

fn float() -> impl Strategy<Value = f64> {
    use proptest::num::f64::*;
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        POSITIVE | NEGATIVE | NORMAL | SUBNORMAL | ZERO
    ]
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Removed(RemovedTag)),
        "[a-c]{0,2}".prop_map(Value::String),
        any::<i64>().prop_map(Value::Integer),
        float().prop_map(Value::Float),
        any::<bool>().prop_map(Value::Boolean),
        "[a-c]{0,2}".prop_map(Value::DateTime),
        "[a-c]{0,2}".prop_map(Value::Json),
        Just(Value::Null),
        // The map that untagged `Value` serde reads back as `Removed`.
        Just(Value::Object(HashMap::from([(
            REMOVED_MARKER_KEY.into(),
            Value::Boolean(true)
        )]))),
    ];
    leaf.prop_recursive(2, 12, 3, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..3).prop_map(Value::Array),
            hash_map("[a-c]", inner, 0..3).prop_map(Value::Object),
        ]
    })
}

fn props() -> impl Strategy<Value = HashMap<String, Value>> {
    hash_map("[a-e]", value(), 0..5)
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        3 => (option::of(any::<usize>()), any::<bool>(), 0..4u8)
            .prop_map(|(parent, page, sort)| Edit::Create { parent, page, sort }),
        2 => (any::<usize>(), option::of(any::<usize>()))
            .prop_map(|(node, parent)| Edit::Move { node, parent }),
        1 => any::<usize>().prop_map(Edit::TogglePage),
        1 => (any::<usize>(), props()).prop_map(|(i, p)| Edit::SetProps(i, p)),
        1 => any::<usize>().prop_map(Edit::DeleteLeaf),
        1 => any::<usize>().prop_map(Edit::DeleteSubtree),
        1 => Just(Edit::Replace),
    ]
}

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/views_law.proptest-regressions"
        )))),
        ..ProptestConfig::with_cases(cases)
    }
}

/// The same value with every map rebuilt, so its iteration order changes,
/// and the sign of every zero flipped.
fn equal_twin(value: &Value) -> Value {
    match value {
        Value::Float(f) if *f == 0.0 => Value::Float(-f),
        Value::Array(items) => Value::Array(items.iter().map(equal_twin).collect()),
        Value::Object(map) => Value::Object(twin_map(map)),
        other => other.clone(),
    }
}

fn twin_map(map: &HashMap<String, Value>) -> HashMap<String, Value> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.reverse();
    entries
        .into_iter()
        .map(|(k, v)| (k.clone(), equal_twin(v)))
        .collect()
}

fn block_with(props: &HashMap<String, Value>) -> SnapshotBlock {
    snapshot(uri(1), uri(0), false, "80", props)
}

proptest! {
    #![proptest_config(config(512))]
    #[test]
    fn block_forest(commits in vec(vec(edit(), 1..6), 1..30)) {
        law(commits)?;
    }
}

proptest! {
    #![proptest_config(config(256))]
    #[test]
    fn a_payload_decodes_to_its_block(props in props()) {
        let block = block_with(&props);
        prop_assert_eq!(Payload::encode(&block).unwrap().decode().unwrap(), block);
    }

    #[test]
    fn equal_blocks_give_equal_bytes(props in props()) {
        let (block, twin) = (block_with(&props), block_with(&twin_map(&props)));
        prop_assert_eq!(&block, &twin);
        prop_assert_eq!(Payload::encode(&block).unwrap(), Payload::encode(&twin).unwrap());
    }
}

#[test]
fn bytes_that_encode_no_block_are_no_payload() {
    let refused = serde_json::from_str::<Payload>("[110, 111]").unwrap_err();
    assert!(
        refused.to_string().contains("does not decode to a block"),
        "{refused}"
    );
}

#[test]
fn a_non_finite_float_is_refused() {
    for f in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let nested = Value::Object(HashMap::from([(
            "x".into(),
            Value::Array(vec![Value::Float(f)]),
        )]));
        let block = block_with(&HashMap::from([("k".into(), nested)]));
        assert_eq!(
            Payload::encode(&block),
            Err(EngineError::NonFiniteFloat {
                id: uri(1),
                key: "k".into()
            })
        );
    }
}

#[test]
fn the_views_arrange_only_the_two_inputs_of_the_owning_page_step() {
    let catalog = catalog();
    let plans = check_all(&views().plans(), &catalog).unwrap();
    let dd = Dataflow::<DynRow>::build(&mut worker(), &catalog, &plans);
    // The recursion by node, the non-pages by parent.
    assert_eq!(dd.arrangements(), 2);
}

#[test]
fn plans_that_share_a_sub_plan_share_its_arrangements() {
    let blocks = Plan::scan(BLOCKS);
    let with_parent = blocks.join(&blocks, vec![(PARENT, ID)]);
    let under_pages = blocks.join(&blocks.filter(Expr::Col(IS_PAGE)), vec![(PARENT, ID)]);
    let plans = [
        with_parent.clone(),
        with_parent.project(vec![Expr::Col(ID)]),
        under_pages,
    ];
    let catalog = catalog();
    let plans = check_all(&plans, &catalog).unwrap();
    let dd = Dataflow::<DynRow>::build(&mut worker(), &catalog, &plans);
    // `blocks` by parent and by id, the pages by id.
    assert_eq!(dd.arrangements(), 3);
}
