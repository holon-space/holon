//! The `(name, cat)` reconciler's rules against the phone API's wire shape
//! (`docs/Plans/Kitchen.md` §4,
//! `docs/Plans/ThatShoppingList-API-2026-09-01.md`).
//!
//! Every snapshot here is built by parsing a whole response body, not by
//! assembling the parsed type: the checked flag comes from `pickedItems`
//! membership and the categories from the list's own `options.cats`, so a test
//! that skipped the parser would be testing a shape the peer never sends.

use anyhow::Result;
use chrono::Duration;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping::ItemKey;
use holon_kitchen::shopping::LocalIntent;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::PushIntent;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_rows::RowMapper;

const FETCHED_AT: &str = "2026-09-01T10:00:00Z";

/// The vocabulary the fixture lists publish. `Kleidung_clothes_1976D2` carries
/// the icon/colour decoration the capture recorded.
const CATS: &[&str] = &["R", "B", "Ca", "Ir", "Kleidung_clothes_1976D2"];

/// Build a whole list response: active items, checked-off items, and a version.
fn body(
    items: &[(&str, &str, Option<f64>)],
    picked: &[(&str, &str)],
    version: i64,
) -> serde_json::Map<String, serde_json::Value> {
    let items: Vec<serde_json::Value> = items
        .iter()
        .map(|(name, cat, count)| match count {
            Some(c) => serde_json::json!({"name": name, "cat": cat, "count": c}),
            None => serde_json::json!({"name": name, "cat": cat}),
        })
        .collect();
    let picked: serde_json::Map<String, serde_json::Value> = picked
        .iter()
        .map(|(name, cat)| {
            (
                (*name).to_string(),
                serde_json::json!({"cat": cat, "date": "2026-09-01T08:00:00Z"}),
            )
        })
        .collect();
    serde_json::json!({
        "items": items,
        "pickedItems": picked,
        "version": version,
        "options": {"prices": false, "cats": CATS},
    })
    .as_object()
    .cloned()
    .expect("the fixture body is an object")
}

/// The shipped sidecar. Reading the filter from the asset is what makes these
/// snapshots the ones production builds: the mapping edited there is the
/// mapping exercised here.
const SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

/// A snapshot built the way production builds one: the response through the
/// shipped sidecar's `response` mapping, then the rows.
fn snapshot_from_body(
    body: &serde_json::Map<String, serde_json::Value>,
    fetched_at: &str,
) -> Result<CompleteSnapshot> {
    let doc: serde_yaml::Value = serde_yaml::from_str(SIDECAR).expect("the sidecar parses");
    let filter = doc["holon"]["tools"]["pull_list"]["response"]
        .as_str()
        .expect("holon.tools.pull_list.response is a jaq filter");
    let mapper = RowMapper::compile("shopping/pull_list.response", filter)?;
    let rows = mapper.map_to_row_sets(&serde_json::Value::Object(body.clone()))?;
    CompleteSnapshot::from_rows(&rows, fetched_at)
}

fn snapshot(items: &[(&str, &str, Option<f64>)]) -> CompleteSnapshot {
    snapshot_from_body(&body(items, &[], 7), FETCHED_AT).expect("well-formed response")
}

fn snapshot_with_picked(
    items: &[(&str, &str, Option<f64>)],
    picked: &[(&str, &str)],
) -> CompleteSnapshot {
    snapshot_from_body(&body(items, picked, 7), FETCHED_AT).expect("well-formed response")
}

fn local(name: &str, cat: &str, count: Option<f64>) -> LocalShoppingItem {
    let category = ShoppingCategory::unresolved(cat);
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

// ---------------------------------------------------------------------------
// The list's own vocabulary
// ---------------------------------------------------------------------------

#[test]
fn the_vocabulary_comes_from_the_list_not_from_this_build() {
    let snapshot = snapshot(&[("Milk", "R", None)]);
    let vocabulary = snapshot.vocabulary();
    assert_eq!(vocabulary.len(), CATS.len());
    // The decoration is stripped off the code, so an item's plain `Kleidung`
    // resolves against the decorated entry.
    assert!(vocabulary.codes().any(|c| c == "Kleidung"));

    let resolved = vocabulary.resolve("Kleidung");
    assert!(resolved.is_recognized());
    assert_eq!(resolved.entry().and_then(|e| e.icon()), Some("clothes"));
    assert_eq!(resolved.entry().and_then(|e| e.color()), Some("1976D2"));
}

#[test]
fn a_code_the_list_never_published_is_carried_not_rejected() {
    // A list gaining an aisle must not take the whole fetch down: the code is
    // kept verbatim and marked unrecognized, never mapped to a neighbour.
    let snapshot = snapshot(&[("Salmon", "Fish", Some(1.0))]);
    let item = snapshot.items().next().expect("the item survived");
    assert_eq!(item.category.as_wire(), "Fish");
    assert!(!item.category.is_recognized());
}

#[test]
fn a_duplicate_category_code_is_a_construction_error() {
    let mut response = body(&[("Milk", "R", None)], &[], 7);
    response.insert(
        "options".into(),
        serde_json::json!({"prices": false, "cats": ["R", "R_shed_ABCDEF"]}),
    );
    let err = snapshot_from_body(&response, FETCHED_AT)
        .expect_err("an ambiguous vocabulary would mislabel items");
    assert!(format!("{err:#}").contains("twice"), "{err:#}");
}

// ---------------------------------------------------------------------------
// checked = pickedItems membership
// ---------------------------------------------------------------------------

#[test]
fn a_checked_off_item_arrives_from_picked_items() {
    let snapshot = snapshot_with_picked(&[("Milk", "R", None)], &[("Bread", "B")]);
    let bread = snapshot
        .items()
        .find(|i| i.name == "Bread")
        .expect("the checked item is part of the list");
    assert!(bread.checked);
    assert!(
        !snapshot
            .items()
            .find(|i| i.name == "Milk")
            .expect("Milk")
            .checked
    );
}

#[test]
fn a_peer_side_check_reaches_a_row_we_already_hold() {
    let held = local("Bread", "B", None);
    let outcome = ShoppingReconciler::default()
        .reconcile(
            &[held.clone()],
            &snapshot_with_picked(&[], &[("Bread", "B")]),
        )
        .expect("reconcile");

    assert!(
        outcome.local.contains(&LocalIntent::Check {
            id: held.id.clone()
        }),
        "the peer's check did not reach the row: {:?}",
        outcome.local
    );
    assert!(outcome.push.is_empty());
}

#[test]
fn a_peer_side_uncheck_never_clears_a_local_check() {
    // §4's asymmetric rule: a wrong check skips one item, a wrong uncheck
    // re-buys it every trip. With no timestamp to arbitrate, check-off wins.
    let mut held = local("Bread", "B", None);
    held.checked = true;

    let outcome = ShoppingReconciler::default()
        .reconcile(&[held.clone()], &snapshot(&[("Bread", "B", None)]))
        .expect("reconcile");

    assert_eq!(
        outcome.local,
        vec![LocalIntent::TouchLastSeenRemote {
            id: held.id,
            at: FETCHED_AT.into(),
        }],
        "the pull cleared a local check"
    );
}

#[test]
fn a_local_check_is_never_pushed() {
    // DISCLOSED LIMITATION: the peer moves an item between `items` and
    // `pickedItems` with a command shape the capture never isolated, so sending
    // a guessed encoding risks deleting the item instead of ticking it.
    // `checked` therefore travels inbound only.
    let mut held = local("Bread", "B", None);
    held.checked = true;

    let outcome = ShoppingReconciler::default()
        .reconcile(&[held], &snapshot(&[("Bread", "B", None)]))
        .expect("reconcile");

    assert!(
        outcome.push.is_empty(),
        "a check was pushed on an unpinned command encoding: {:?}",
        outcome.push
    );
}

#[test]
fn an_item_in_both_collections_counts_as_checked() {
    let snapshot = snapshot_with_picked(&[("Bread", "B", None)], &[("Bread", "B")]);
    assert_eq!(snapshot.len(), 1, "the two halves are one item");
    assert!(
        snapshot.items().next().expect("Bread").checked,
        "a contradictory pair resolved against the asymmetric rule"
    );
}

// ---------------------------------------------------------------------------
// Both-sides mutation
// ---------------------------------------------------------------------------

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
fn both_sides_adding_different_items_converges_in_one_round() {
    let mut mine = local("Oat milk", "R", Some(1.0));
    mine.last_seen_remote = None;
    let held = local("Bread", "B", None);

    let outcome = ShoppingReconciler::default()
        .reconcile(
            &[mine.clone(), held.clone()],
            &snapshot(&[("Bread", "B", None), ("Butter", "Ca", Some(2.0))]),
        )
        .expect("reconcile");

    // Theirs comes in, mine goes out, and neither deletes the other.
    let inserted: Vec<&str> = outcome
        .local
        .iter()
        .filter_map(|i| match i {
            LocalIntent::Insert(row) => Some(row.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inserted, vec!["Butter"]);
    assert_eq!(outcome.push, vec![PushIntent::Add(mine)]);
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
    // R4, ACCEPTED as a disclosed limitation (docs/Plans/Kitchen.md §7): the
    // peer emits a rename as `del <old>` + `add <new>` in one commit, so
    // local-only state on the old key cannot survive it. There is no fix
    // without a server-issued item id. This is the EXPECTED outcome, asserted.
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
    assert!(
        outcome.push.is_empty(),
        "a peer-side rename must not be echoed back at the peer: {:?}",
        outcome.push
    );
}

// ---------------------------------------------------------------------------
// Bad input
// ---------------------------------------------------------------------------

#[test]
fn a_record_without_a_name_fails_the_whole_snapshot() {
    let mut response = body(&[], &[], 7);
    response.insert("items".into(), serde_json::json!([{"cat": "B"}]));

    let err =
        snapshot_from_body(&response, FETCHED_AT).expect_err("a nameless item has no identity");
    // Skipping it would silently turn a real item into a deletion.
    assert!(
        format!("{err:#}").contains("`name` must be a non-empty string"),
        "{err:#}"
    );
}

#[test]
fn a_response_without_a_version_fails_the_whole_snapshot() {
    let mut response = body(&[("Milk", "R", None)], &[], 7);
    response.remove("version");

    let err = snapshot_from_body(&response, FETCHED_AT)
        .expect_err("a snapshot with no version cannot base a commit");
    assert!(format!("{err:#}").contains("no `version`"), "{err:#}");
}

#[test]
fn two_local_rows_under_one_key_are_refused() {
    let dup = local("Milk", "R", Some(1.0));
    let err = ShoppingReconciler::default()
        .reconcile(&[dup.clone(), dup], &snapshot(&[]))
        .expect_err("the (name, cat) key is the row identity");
    assert!(format!("{err:#}").contains("share the key"), "{err:#}");
}
