use std::collections::BTreeMap;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use futures::stream::StreamExt as _;
use futures_signals::signal_map::MapDiff;
use proptest::prelude::*;

use super::*;

/// Synthetic item: `group` is the derived key `key_fn` returns; `tag`
/// lets a same-group `Update` still change the value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    group: u8,
    tag: u8,
}

fn arc(group: u8, tag: u8) -> Arc<Item> {
    Arc::new(Item { group, tag })
}

/// Deterministic async key fn: the group is right there in the value.
async fn key_of(v: Arc<Item>) -> Result<u8> {
    Ok(v.group)
}

/// Drain a stream to quiescence with a no-op waker. Our synthetic `key_fn`
/// is immediately ready, so the combinator never parks on a real wake — a
/// `Pending` means "no more output from the inputs fed so far". A real
/// awaiting `key_fn` would instead need a runtime; that is exercised by
/// the live-feed integration test below.
fn drain<X>(s: &mut (impl Stream<Item = X> + Unpin)) -> Vec<X> {
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut out = Vec::new();
    while let Poll::Ready(Some(x)) = s.poll_next_unpin(&mut cx) {
        out.push(x);
    }
    out
}

// ---- reference model -------------------------------------------------

/// Naively group a flat element->value map by the value's group.
fn naive_grouping(
    model: &BTreeMap<String, Arc<Item>>,
) -> BTreeMap<u8, BTreeMap<String, Arc<Item>>> {
    let mut out: BTreeMap<u8, BTreeMap<String, Arc<Item>>> = BTreeMap::new();
    for (k, v) in model {
        out.entry(v.group).or_default().insert(k.clone(), v.clone());
    }
    out
}

/// Fold emitted `GroupedDiff`s into the same shape (empty groups pruned).
fn fold_emitted(
    folded: &mut BTreeMap<u8, BTreeMap<String, Arc<Item>>>,
    diffs: Vec<Result<GroupedDiff<u8, Item>>>,
) {
    for d in diffs {
        match d.expect("no key_fn error expected in convergence property") {
            GroupedDiff::Upsert { group, key, value } => {
                folded.entry(group).or_default().insert(key, value);
            }
            GroupedDiff::Remove { group, key } => {
                if let Some(g) = folded.get_mut(&group) {
                    g.remove(&key);
                    if g.is_empty() {
                        folded.remove(&group);
                    }
                }
            }
        }
    }
}

/// Apply one `MapDiff` to the flat reference model.
fn apply_to_model(model: &mut BTreeMap<String, Arc<Item>>, diff: &MapDiff<String, Arc<Item>>) {
    match diff {
        MapDiff::Replace { entries } => {
            model.clear();
            for (k, v) in entries {
                model.insert(k.clone(), v.clone());
            }
        }
        MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
            model.insert(key.clone(), value.clone());
        }
        MapDiff::Remove { key } => {
            model.remove(key);
        }
        MapDiff::Clear {} => model.clear(),
    }
}

// ---- generator -------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    /// Insert-or-update key `k` with (group, tag). Covers insert,
    /// update-same-group (group unchanged), update-change-group.
    Set {
        k: u8,
        group: u8,
        tag: u8,
    },
    Remove {
        k: u8,
    },
    Replace {
        entries: Vec<(u8, u8, u8)>,
    },
    Clear,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0u8..5, 0u8..3, 0u8..3).prop_map(|(k, group, tag)| Op::Set { k, group, tag }),
        4 => (0u8..5).prop_map(|k| Op::Remove { k }),
        1 => prop::collection::vec((0u8..5, 0u8..3, 0u8..3), 0..4)
            .prop_map(|entries| Op::Replace { entries }),
        1 => Just(Op::Clear),
    ]
}

fn kname(k: u8) -> String {
    format!("k{k}")
}

/// Translate an abstract op into a *valid* `MapDiff` given the live set:
/// existing key -> `Update`, new key -> `Insert`, `Remove` only if present.
/// Returns `None` for a no-op (Remove of an absent key).
fn to_diff(
    live: &mut std::collections::BTreeSet<String>,
    op: &Op,
) -> Option<MapDiff<String, Arc<Item>>> {
    match op {
        Op::Set { k, group, tag } => {
            let key = kname(*k);
            let value = arc(*group, *tag);
            if live.insert(key.clone()) {
                Some(MapDiff::Insert { key, value })
            } else {
                Some(MapDiff::Update { key, value })
            }
        }
        Op::Remove { k } => {
            let key = kname(*k);
            if live.remove(&key) {
                Some(MapDiff::Remove { key })
            } else {
                None
            }
        }
        Op::Replace { entries } => {
            live.clear();
            let mut seen = std::collections::BTreeMap::new();
            for (k, group, tag) in entries {
                seen.insert(kname(*k), arc(*group, *tag));
            }
            for k in seen.keys() {
                live.insert(k.clone());
            }
            Some(MapDiff::Replace {
                entries: seen.into_iter().collect(),
            })
        }
        Op::Clear => {
            live.clear();
            Some(MapDiff::Clear {})
        }
    }
}

/// Run the incremental-convergence property for one op sequence against
/// one engine. After EVERY input event the folded emitted output must
/// equal the naive grouping of the reference model. This single equality
/// is strictly stronger than P1/P2/P3 combined:
///   P1 (convergence): equality at the final step.
///   P2 (retraction on group change): a missing old-group `Remove` leaves
///       the element in `folded[old]` -> mismatch at that step.
///   P3 (no leak on Remove): a removed element still in `folded` ->
///       mismatch at that step.
fn run_convergence(ops: Vec<Op>) -> std::result::Result<(), TestCaseError> {
    let mut live = std::collections::BTreeSet::new();
    let diffs: Vec<MapDiff<String, Arc<Item>>> =
        ops.iter().filter_map(|op| to_diff(&mut live, op)).collect();

    // One stream per engine, fed via a channel so we can drive it one
    // diff at a time and drain the outputs of each step in isolation.
    let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
    let mut stream = Box::pin(group_diffs(rx, key_of));

    let mut model: BTreeMap<String, Arc<Item>> = BTreeMap::new();
    let mut folded: BTreeMap<u8, BTreeMap<String, Arc<Item>>> = BTreeMap::new();

    for diff in diffs {
        apply_to_model(&mut model, &diff);
        tx.unbounded_send(diff).expect("channel open");
        let step = drain(&mut stream);
        fold_emitted(&mut folded, step);

        let expected = naive_grouping(&model);
        let folded_pruned: BTreeMap<u8, BTreeMap<String, Arc<Item>>> = folded
            .iter()
            .filter(|(_, g)| !g.is_empty())
            .map(|(k, g)| (*k, g.clone()))
            .collect();
        prop_assert_eq!(
            &folded_pruned,
            &expected,
            "MatView(fold of emitted GroupedDiffs) != View(naive grouping of live map). \
                 A group retains a stale element — the old group was not retracted on a \
                 group-key change, or a removed element was not retracted."
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Incremental convergence holds after every event (P1/P2/P3).
    #[test]
    fn prop_convergence(ops in prop::collection::vec(op_strategy(), 0..40)) {
        run_convergence(ops)?;
    }
}

// ---- hardcoded regressions (shrunk strawman failures) ----------------

/// Group-key change must retract the old group (P2). A block moves from
/// document 0 to document 1: the old group must not keep it.
#[test]
fn regression_group_change_retracts_old_group() {
    let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
    let mut stream = Box::pin(group_diffs(rx, key_of));

    tx.unbounded_send(MapDiff::Insert {
        key: "b".into(),
        value: arc(0, 0),
    })
    .unwrap();
    tx.unbounded_send(MapDiff::Update {
        key: "b".into(),
        value: arc(1, 0),
    })
    .unwrap();

    let out: Vec<_> = drain(&mut stream).into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(
        out,
        vec![
            GroupedDiff::Upsert {
                group: 0,
                key: "b".into(),
                value: arc(0, 0)
            },
            GroupedDiff::Remove {
                group: 0,
                key: "b".into()
            },
            GroupedDiff::Upsert {
                group: 1,
                key: "b".into(),
                value: arc(1, 0)
            },
        ],
        "a group-key change must emit Remove(old) before Upsert(new)"
    );
}

/// Remove routes via the accumulator to the last-known group (P3), even
/// though the removed value is gone.
#[test]
fn regression_remove_retracts_via_accumulator() {
    let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
    let mut stream = Box::pin(group_diffs(rx, key_of));

    tx.unbounded_send(MapDiff::Insert {
        key: "b".into(),
        value: arc(2, 0),
    })
    .unwrap();
    tx.unbounded_send(MapDiff::Remove { key: "b".into() })
        .unwrap();

    let out: Vec<_> = drain(&mut stream).into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(
        out,
        vec![
            GroupedDiff::Upsert {
                group: 2,
                key: "b".into(),
                value: arc(2, 0)
            },
            GroupedDiff::Remove {
                group: 2,
                key: "b".into()
            },
        ],
        "Remove must retract from the last-known group via the accumulator"
    );
}

/// `key_fn` error surfaces as an `Err` item and ends the stream — never a
/// silently skipped element.
#[tokio::test]
async fn key_fn_error_surfaces_and_ends_stream() {
    let (tx, rx) = futures::channel::mpsc::unbounded::<MapDiff<String, Arc<Item>>>();
    let key_fn = |v: Arc<Item>| async move {
        if v.group == 9 {
            anyhow::bail!("synthetic authoritative-read failure");
        }
        Ok(v.group)
    };
    let mut stream = Box::pin(group_diffs(rx, key_fn));

    tx.unbounded_send(MapDiff::Insert {
        key: "a".into(),
        value: arc(9, 0),
    })
    .unwrap();
    drop(tx);

    let first = stream.next().await.expect("one item");
    assert!(
        first.is_err(),
        "key_fn error must surface as an Err item, not be skipped"
    );
    assert!(
        stream.next().await.is_none(),
        "stream must end after a key_fn error, not keep folding on a stale accumulator"
    );
}

/// The `group_by` method actually wires the live feed's signal map: an
/// initial snapshot seeds via `Replace`, and a subsequent CDC-driven
/// update fans out Remove(old)+Upsert(new). Exercises the real async path
/// end-to-end through a tokio runtime (not the no-op-waker drain).
#[tokio::test]
async fn group_by_wires_live_feed_snapshot_and_updates() {
    use crate::Change;
    use crate::StorageEntity;
    use crate::Value;

    fn row(id: &str, group: &str) -> StorageEntity {
        let mut r = StorageEntity::new();
        r.insert("id".into(), Value::String(id.into()));
        r.insert("group".into(), Value::String(group.into()));
        r
    }

    let live: Arc<LiveData<String>> = LiveData::new(
        vec![row("a", "g1")],
        |r| Ok(r.get("id").unwrap().as_string().unwrap().to_string()),
        |r| Ok(r.get("group").unwrap().as_string().unwrap().to_string()),
    );

    let mut stream = Box::pin(
        live.group_by(|v: Arc<String>| async move { Ok::<String, anyhow::Error>((*v).clone()) }),
    );

    // Initial snapshot Replace -> Upsert for the seeded row.
    let first = stream.next().await.expect("snapshot item").unwrap();
    assert_eq!(
        first,
        GroupedDiff::Upsert {
            group: "g1".into(),
            key: "a".into(),
            value: Arc::new("g1".into()),
        }
    );

    // A CDC update that changes the derived group must retract g1.
    live.apply_changes(vec![Change::Updated {
        id: "a".into(),
        data: row("a", "g2"),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);

    let retract = stream.next().await.expect("retract").unwrap();
    assert_eq!(
        retract,
        GroupedDiff::Remove {
            group: "g1".into(),
            key: "a".into()
        }
    );
    let readd = stream.next().await.expect("re-add").unwrap();
    assert_eq!(
        readd,
        GroupedDiff::Upsert {
            group: "g2".into(),
            key: "a".into(),
            value: Arc::new("g2".into()),
        }
    );
}
