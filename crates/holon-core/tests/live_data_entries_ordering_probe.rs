//! Risk probe (error-remedy Inc 1, design §3.3 + risk register row 1): does
//! `MutableBTreeMap` ordering serve as the stable sort key for the
//! keyed-map-to-ordered-vec bridge a named `RowSource` needs?
//!
//! Probed against the `focus_roots` mirror, because it is a real
//! `LiveData<FocusRoot>` in production whose `id_fn` builds a COMPOSITE key
//! (`"{region}\u{1F}{root_id}"`, `turso_block_query_source.rs:143-149`). If the
//! composite key orders stably, a single-field key trivially does.

use std::sync::Arc;

use futures_signals::signal_vec::SignalVec;
use futures_signals::signal_vec::VecDiff;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::live_data::LiveData;
use holon_core::storage::block_query::FocusRoot;

/// The production `focus_roots` key, verbatim.
fn focus_root_key(row: &StorageEntity) -> anyhow::Result<String> {
    let region = row
        .get("region")
        .and_then(|v| v.as_string())
        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?;
    let root_id = row
        .get("root_id")
        .and_then(|v| v.as_string())
        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?;
    Ok(format!("{region}\u{1F}{root_id}"))
}

fn parse_focus_root(row: &StorageEntity) -> anyhow::Result<FocusRoot> {
    Ok(FocusRoot::new(
        row.get("region")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?,
        row.get("root_id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?,
    ))
}

fn row(region: &str, root_id: &str) -> StorageEntity {
    let mut e = StorageEntity::new();
    e.insert("region".into(), Value::String(region.to_string()));
    e.insert("root_id".into(), Value::String(root_id.to_string()));
    e
}

fn live(initial: Vec<StorageEntity>) -> Arc<LiveData<FocusRoot>> {
    LiveData::new(initial, focus_root_key, parse_focus_root)
}

/// Drain every diff the SignalVec has ready, and fold them into a Vec the way a
/// widget would.
fn drain<S>(vec_signal: &mut std::pin::Pin<&mut S>, into: &mut Vec<(String, Arc<FocusRoot>)>)
where
    S: SignalVec<Item = (String, Arc<FocusRoot>)>,
{
    use std::task::Poll;
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    while let Poll::Ready(Some(diff)) = vec_signal.as_mut().poll_vec_change(&mut cx) {
        apply(diff, into);
    }
}

fn apply(diff: VecDiff<(String, Arc<FocusRoot>)>, into: &mut Vec<(String, Arc<FocusRoot>)>) {
    match diff {
        VecDiff::Replace { values } => *into = values,
        VecDiff::InsertAt { index, value } => into.insert(index, value),
        VecDiff::UpdateAt { index, value } => into[index] = value,
        VecDiff::RemoveAt { index } => {
            into.remove(index);
        }
        VecDiff::Push { value } => into.push(value),
        VecDiff::Pop {} => {
            into.pop();
        }
        VecDiff::Clear {} => into.clear(),
        VecDiff::Move {
            old_index,
            new_index,
        } => {
            let v = into.remove(old_index);
            into.insert(new_index, v);
        }
    }
}

#[test]
fn entries_signal_vec_orders_by_the_composite_key_and_inserts_land_in_place() {
    // Deliberately out of key order, and with a region whose rows must group.
    let data = live(vec![
        row("main", "b-9"),
        row("aside", "b-1"),
        row("main", "b-10"),
    ]);

    let sv = data.entries_signal_vec();
    futures::pin_mut!(sv);
    let mut seen: Vec<(String, Arc<FocusRoot>)> = Vec::new();
    drain(&mut sv, &mut seen);

    let keys: Vec<&str> = seen.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        vec!["aside\u{1F}b-1", "main\u{1F}b-10", "main\u{1F}b-9"],
        "initial order must be BTreeMap key order"
    );

    // An insert must land at its sorted index, not at the end.
    data.insert(
        "main\u{1F}b-0".to_string(),
        Arc::new(FocusRoot::new("main", "b-0")),
    );
    drain(&mut sv, &mut seen);
    let keys: Vec<&str> = seen.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "aside\u{1F}b-1",
            "main\u{1F}b-0",
            "main\u{1F}b-10",
            "main\u{1F}b-9"
        ],
        "an insert must land at its sorted index"
    );
}

/// The order is TOTAL and DETERMINISTIC, but it is LEXICOGRAPHIC — so a key
/// built from a number does not order numerically. Pinned because it is the
/// trap a named source falls into, not a defect in the bridge: `b-10` before
/// `b-9` above is the same fact.
#[test]
fn the_sort_key_is_lexicographic_not_numeric() {
    let data = live(vec![row("r", "2"), row("r", "10"), row("r", "1")]);
    let sv = data.entries_signal_vec();
    futures::pin_mut!(sv);
    let mut seen: Vec<(String, Arc<FocusRoot>)> = Vec::new();
    drain(&mut sv, &mut seen);

    let roots: Vec<&str> = seen.iter().map(|(_, v)| v.root_id.as_str()).collect();
    assert_eq!(
        roots,
        vec!["1", "10", "2"],
        "lexicographic: a named source needing numeric order must zero-pad its key"
    );
}

/// Rebuilding the same content in a different insertion order yields the same
/// vec — the property a stable sort key has to have.
#[test]
fn order_is_independent_of_insertion_order() {
    let forward = live(vec![row("a", "1"), row("b", "2"), row("c", "3")]);
    let backward = live(vec![row("c", "3"), row("b", "2"), row("a", "1")]);

    let a = forward.entries_signal_vec();
    let b = backward.entries_signal_vec();
    futures::pin_mut!(a);
    futures::pin_mut!(b);
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    drain(&mut a, &mut sa);
    drain(&mut b, &mut sb);

    let ka: Vec<&str> = sa.iter().map(|(k, _)| k.as_str()).collect();
    let kb: Vec<&str> = sb.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(ka, kb);
}
