//! The `(name, cat)` reconciler's rules, including the ones that exist only so
//! the write leg can be added without reshaping anything
//! (`docs/Plans/Kitchen.md` §4).

use chrono::Duration;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping::ItemKey;
use holon_kitchen::shopping::LocalIntent;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::PushIntent;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;

const FETCHED_AT: &str = "2026-09-01T10:00:00Z";

fn snapshot(items: &[(&str, &str, Option<f64>)]) -> CompleteSnapshot {
    let records: Vec<_> = items
        .iter()
        .map(|(name, cat, count)| {
            let mut record = serde_json::Map::new();
            record.insert("name".into(), serde_json::Value::String((*name).into()));
            record.insert("cat".into(), serde_json::Value::String((*cat).into()));
            if let Some(c) = count {
                record.insert(
                    "count".into(),
                    serde_json::Value::Number(serde_json::Number::from_f64(*c).unwrap()),
                );
            }
            record
        })
        .collect();
    CompleteSnapshot::from_records(&records, FETCHED_AT).expect("well-formed records")
}

fn local(name: &str, cat: &str, count: Option<f64>) -> LocalShoppingItem {
    let category = ShoppingCategory::parse(cat);
    LocalShoppingItem {
        id: ItemKey::new(name, &category).row_id(),
        name: name.to_string(),
        category,
        count,
        checked: false,
        product_id: None,
        deleted_at: None,
        last_seen_remote: Some("2026-08-31T10:00:00Z".into()),
    }
}

#[test]
fn a_key_the_peer_dropped_is_deleted() {
    let held = local("Bread", "B", None);
    let outcome = ShoppingReconciler::default()
        .reconcile(&[held.clone()], &snapshot(&[]))
        .expect("reconcile");

    assert_eq!(outcome.local, vec![LocalIntent::Delete { id: held.id }]);
    assert!(outcome.push.is_empty());
}

#[test]
fn a_local_row_the_peer_never_carried_is_pushed_not_deleted() {
    let mut added = local("Oat milk", "R", Some(1.0));
    added.last_seen_remote = None;

    let outcome = ShoppingReconciler::default()
        .reconcile(&[added.clone()], &snapshot(&[]))
        .expect("reconcile");

    // Absence in a snapshot says nothing about a row the peer has never sent.
    assert!(
        outcome.local.is_empty(),
        "a local addition was deleted by the peer's silence: {:?}",
        outcome.local
    );
    assert_eq!(outcome.push, vec![PushIntent::Add(added)]);
}

#[test]
fn a_live_tombstone_survives_a_pull_that_still_lists_the_item() {
    let mut deleted = local("Bread", "B", None);
    deleted.deleted_at = Some("2026-09-01T09:00:00Z".into());

    let outcome = ShoppingReconciler::default()
        .reconcile(&[deleted.clone()], &snapshot(&[("Bread", "B", None)]))
        .expect("reconcile");

    assert!(
        outcome.local.is_empty(),
        "the pull resurrected a locally deleted item: {:?}",
        outcome.local
    );
    assert_eq!(
        outcome.push,
        vec![PushIntent::Remove {
            key: deleted.key(),
            deleted_at: "2026-09-01T09:00:00Z".into(),
        }]
    );
}

#[test]
fn an_expired_tombstone_stops_suppressing_the_item() {
    let mut deleted = local("Bread", "B", None);
    deleted.deleted_at = Some("2026-08-01T09:00:00Z".into());

    let outcome = ShoppingReconciler::with_tombstone_window(Duration::days(7))
        .reconcile(&[deleted.clone()], &snapshot(&[("Bread", "B", Some(2.0))]))
        .expect("reconcile");

    match outcome.local.as_slice() {
        [LocalIntent::Insert(row)] => {
            assert_eq!(row.id, deleted.id);
            assert_eq!(row.count, Some(2.0));
            assert_eq!(row.deleted_at, None);
        }
        other => panic!("expected the item to come back, got {other:?}"),
    }
    assert!(outcome.push.is_empty());
}

#[test]
fn a_tombstone_the_peer_has_caught_up_with_is_reaped() {
    let mut deleted = local("Bread", "B", None);
    deleted.deleted_at = Some("2026-09-01T09:00:00Z".into());

    let outcome = ShoppingReconciler::default()
        .reconcile(&[deleted.clone()], &snapshot(&[]))
        .expect("reconcile");

    assert_eq!(
        outcome.local,
        vec![LocalIntent::ReapTombstone { id: deleted.id }]
    );
}

#[test]
fn the_fetched_count_wins_and_the_watermark_moves() {
    let held = local("Milk", "R", Some(1.0));

    let outcome = ShoppingReconciler::default()
        .reconcile(&[held.clone()], &snapshot(&[("Milk", "R", Some(4.0))]))
        .expect("reconcile");

    assert_eq!(
        outcome.local,
        vec![
            LocalIntent::SetCount {
                id: held.id.clone(),
                count: Some(4.0),
            },
            LocalIntent::TouchLastSeenRemote {
                id: held.id,
                at: FETCHED_AT.into(),
            },
        ]
    );
}

#[test]
fn an_unchanged_item_only_moves_its_watermark() {
    let held = local("Milk", "R", Some(1.0));

    let outcome = ShoppingReconciler::default()
        .reconcile(&[held.clone()], &snapshot(&[("Milk", "R", Some(1.0))]))
        .expect("reconcile");

    assert_eq!(
        outcome.local,
        vec![LocalIntent::TouchLastSeenRemote {
            id: held.id,
            at: FETCHED_AT.into(),
        }]
    );
}

#[test]
fn a_peer_side_rename_arrives_as_a_delete_and_an_add() {
    // The peer emits a rename as `del <old>` + `add <new>` in one commit, so
    // local-only state on the old key cannot survive it. Disclosed, not a bug.
    let mut held = local("Milk", "R", Some(1.0));
    held.checked = true;
    held.product_id = Some("product:milk".into());

    let outcome = ShoppingReconciler::default()
        .reconcile(&[held.clone()], &snapshot(&[("Oat milk", "R", Some(1.0))]))
        .expect("reconcile");

    let mut deletes = 0;
    let mut inserts = 0;
    for intent in &outcome.local {
        match intent {
            LocalIntent::Delete { id } => {
                assert_eq!(id, &held.id);
                deletes += 1;
            }
            LocalIntent::Insert(row) => {
                assert_eq!(row.name, "Oat milk");
                assert!(!row.checked, "local-only state cannot follow a rename");
                assert_eq!(row.product_id, None);
                inserts += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((deletes, inserts), (1, 1));
}

#[test]
fn a_record_without_a_name_fails_the_whole_snapshot() {
    let mut record = serde_json::Map::new();
    record.insert("cat".into(), serde_json::Value::String("B".into()));

    let err = CompleteSnapshot::from_records(&[record], FETCHED_AT)
        .expect_err("a nameless item has no identity");
    // Skipping it would silently turn a real item into a deletion.
    assert!(format!("{err:#}").contains("`name` is missing"), "{err:#}");
}

#[test]
fn two_local_rows_under_one_key_are_refused() {
    let dup = local("Milk", "R", Some(1.0));
    let err = ShoppingReconciler::default()
        .reconcile(&[dup.clone(), dup], &snapshot(&[]))
        .expect_err("the (name, cat) key is the row identity");
    assert!(format!("{err:#}").contains("share the key"), "{err:#}");
}
