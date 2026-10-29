//! The generic remote-list reconciler, over BOTH identity shapes a sidecar can
//! declare: a peer that issues row ids (`key: .id`) and one where identity is
//! content (`key: "[.name, .cat]"`).
//!
//! Running one property body against two connections is the point. What the
//! reconciler may know about a list is exactly what its sidecar declares, so a
//! property that holds for one key shape and not the other would mean identity
//! had leaked into the Rust.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::Value;
use holon_api::entity::TypeDefinition;
use holon_connections::CompiledListSync;
use holon_connections::ListSnapshot;
use holon_connections::ListSyncSpec;
use holon_connections::LocalIntent;
use holon_connections::LocalRow;
use holon_connections::PushIntent;
use holon_connections::ReconcileOutcome;
use holon_connections::RemoteListReconciler;
use holon_core::file_format::TypedRowSet;
use proptest::prelude::*;

const FETCHED_AT: &str = "2026-09-14T12:00:00+00:00";
const FRESH_TOMBSTONE: &str = "2026-09-13T12:00:00+00:00";
const EXPIRED_TOMBSTONE: &str = "2026-08-01T12:00:00+00:00";

// ---------------------------------------------------------------------------
// The two connections, declared exactly as a sidecar declares them.
// ---------------------------------------------------------------------------

const SHOPPING_TYPE: &str = r#"
name: shopping_item
default_lifetime: persistent
primary_key: id
fields:
  - { name: id, sql_type: TEXT, primary_key: true }
  - { name: name, sql_type: TEXT }
  - { name: cat, sql_type: TEXT }
  - { name: count, sql_type: TEXT, nullable: true }
  - { name: checked, sql_type: INTEGER }
  - { name: deleted_at, sql_type: TEXT, nullable: true }
  - { name: last_seen_remote, sql_type: TEXT, nullable: true }
  - { name: remote_base, sql_type: TEXT, nullable: true }
soft_delete:
  tombstone_field: deleted_at
  retention_days: 7
"#;

const SHOPPING_LIST_SYNC: &str = r#"
entity: shopping_item
pull_tool: pull_list
commit_tool: commit
list_row_type: shopping_list
version_column: version
key: "[.name, .cat]"
display_name: '.name + " in " + .cat'
watermark_column: last_seen_remote
base_column: remote_base
batch_row_type: shopping_commit
command_row_type: shopping_command
merge_columns: [count]
latch_columns: [checked]
"#;

/// A second system, reached through the same reconciler by declaring its own
/// block. Todoist-shaped: the peer issues the row id, so identity is `.id` and
/// no content pair is involved.
const TASK_TYPE: &str = r#"
name: todo_task
default_lifetime: persistent
primary_key: id
fields:
  - { name: id, sql_type: TEXT, primary_key: true }
  - { name: content, sql_type: TEXT }
  - { name: day_order, sql_type: REAL, nullable: true }
  - { name: checked, sql_type: INTEGER }
  - { name: removed_at, sql_type: TEXT, nullable: true }
  - { name: synced_at, sql_type: TEXT, nullable: true }
  - { name: merge_base, sql_type: TEXT, nullable: true }
soft_delete:
  tombstone_field: removed_at
  retention_days: 3
"#;

const TASK_LIST_SYNC: &str = r#"
entity: todo_task
pull_tool: list_tasks
commit_tool: sync_commands
list_row_type: todo_cursor
version_column: sync_token
key: .id
display_name: .content
watermark_column: synced_at
base_column: merge_base
batch_row_type: todo_batch
command_row_type: todo_command
merge_columns: [day_order]
latch_columns: [checked]
"#;

/// One connection under test, plus the vocabulary the fixtures need to build a
/// row for it. Nothing below this struct knows which peer it is driving.
struct Conn {
    label: &'static str,
    compiled: Arc<CompiledListSync>,
    /// The local primary key of the identity `n`. Derived from content for the
    /// content-keyed peer, minted by the peer for the id-keyed one — which is
    /// the whole difference between them.
    row_id: fn(u8) -> String,
    /// The identity-bearing columns of `n`, as both legs spell them.
    identity: fn(u8) -> Vec<(&'static str, Value)>,
    /// How a disclosure names `n` to a person.
    display_name: fn(u8) -> String,
    merge_column: &'static str,
    /// Merge value `v` as the mirror column reads it back.
    local_merge: fn(i64) -> Value,
    /// Merge value `v` as the peer's JSON carries it.
    remote_merge: fn(i64) -> Value,
    tombstone_column: &'static str,
    watermark_column: &'static str,
}

/// Shopping amounts are free text, compared as the peer's own spelling.
const AMOUNTS: [&str; 4] = ["1", "2kg", "0,5 l", "4 Zehen"];

fn amount(v: i64) -> Value {
    Value::String(AMOUNTS[v as usize].into())
}

fn compile(connection: &str, type_yaml: &str, spec_yaml: &str) -> Arc<CompiledListSync> {
    let declared: TypeDefinition = serde_yaml::from_str(type_yaml).expect("the declared type");
    let spec: ListSyncSpec = serde_yaml::from_str(spec_yaml).expect("the list_sync block");
    CompiledListSync::compile(connection, spec, &declared).expect("the connection compiles")
}

fn shopping() -> Conn {
    Conn {
        label: "shopping (content key)",
        compiled: compile("shopping", SHOPPING_TYPE, SHOPPING_LIST_SYNC),
        row_id: |n| format!("shopping-item:staples:item{n}"),
        identity: |n| {
            vec![
                ("name", Value::String(format!("item{n}"))),
                ("cat", Value::String("staples".into())),
            ]
        },
        display_name: |n| format!("item{n} in staples"),
        merge_column: "count",
        local_merge: amount,
        remote_merge: amount,
        tombstone_column: "deleted_at",
        watermark_column: "last_seen_remote",
    }
}

fn tasks() -> Conn {
    Conn {
        label: "tasks (server id key)",
        compiled: compile("tasks", TASK_TYPE, TASK_LIST_SYNC),
        row_id: |n| format!("todo-task:{n}"),
        identity: |n| vec![("content", Value::String(format!("task {n}")))],
        display_name: |n| format!("task {n}"),
        merge_column: "day_order",
        // A REAL column reads the peer's whole number back as a float; the
        // merge comparison has to see through that difference.
        local_merge: |v| Value::Float(v as f64),
        remote_merge: Value::Integer,
        tombstone_column: "removed_at",
        watermark_column: "synced_at",
    }
}

fn connections() -> Vec<Conn> {
    vec![shopping(), tasks()]
}

// ---------------------------------------------------------------------------
// Scenario: where each identity lives, and what each side says about it.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tombstone {
    None,
    Fresh,
    Expired,
}

#[derive(Debug, Clone, Copy)]
enum Placement {
    /// Only the peer has it.
    RemoteOnly,
    /// Only we have it. `ever_seen` is the watermark: without one the peer has
    /// never carried the row, so its absence says nothing about it.
    LocalOnly { ever_seen: bool, tomb: Tombstone },
    Both {
        tomb: Tombstone,
        local_count: i64,
        remote_count: i64,
        local_checked: bool,
        remote_checked: bool,
    },
}

#[derive(Debug, Clone)]
struct Scenario(Vec<Placement>);

fn placement() -> impl Strategy<Value = Placement> {
    let tomb = prop_oneof![
        6 => Just(Tombstone::None),
        2 => Just(Tombstone::Fresh),
        1 => Just(Tombstone::Expired),
    ];
    prop_oneof![
        2 => Just(Placement::RemoteOnly),
        2 => (any::<bool>(), tomb.clone())
            .prop_map(|(ever_seen, tomb)| Placement::LocalOnly { ever_seen, tomb }),
        5 => (tomb, 0i64..4, 0i64..4, any::<bool>(), any::<bool>()).prop_map(
            |(tomb, local_count, remote_count, local_checked, remote_checked)| Placement::Both {
                tomb,
                local_count,
                remote_count,
                local_checked,
                remote_checked,
            }
        ),
    ]
}

fn scenario() -> impl Strategy<Value = Scenario> {
    prop::collection::vec(placement(), 0..8).prop_map(Scenario)
}

fn tombstone_value(tomb: Tombstone) -> Value {
    match tomb {
        Tombstone::None => Value::Null,
        Tombstone::Fresh => Value::String(FRESH_TOMBSTONE.into()),
        Tombstone::Expired => Value::String(EXPIRED_TOMBSTONE.into()),
    }
}

/// The local mirror row for identity `n`.
fn local_row(
    conn: &Conn,
    n: u8,
    count: i64,
    checked: bool,
    tomb: Tombstone,
    seen: bool,
) -> LocalRow {
    let mut columns: BTreeMap<String, Value> = BTreeMap::new();
    columns.insert("id".into(), Value::String((conn.row_id)(n)));
    for (column, value) in (conn.identity)(n) {
        columns.insert(column.into(), value);
    }
    columns.insert(conn.merge_column.into(), (conn.local_merge)(count));
    columns.insert("checked".into(), Value::Integer(i64::from(checked)));
    columns.insert(conn.tombstone_column.into(), tombstone_value(tomb));
    columns.insert(
        conn.watermark_column.into(),
        if seen {
            Value::String("2026-09-13T00:00:00+00:00".into())
        } else {
            Value::Null
        },
    );
    LocalRow {
        id: (conn.row_id)(n),
        columns,
    }
}

/// The peer's row for identity `n`, as the connection's `response` mapping
/// produced it: the peer's columns under the declared type's names.
fn remote_row(conn: &Conn, n: u8, count: i64, checked: bool) -> holon_api::entity::StorageEntity {
    let mut row: holon_api::entity::StorageEntity = Default::default();
    row.insert("id".into(), Value::String((conn.row_id)(n)));
    for (column, value) in (conn.identity)(n) {
        row.insert(column.into(), value);
    }
    row.insert(conn.merge_column.into(), (conn.remote_merge)(count));
    row.insert("checked".into(), Value::Integer(i64::from(checked)));
    row
}

fn snapshot_of(
    conn: &Conn,
    remote: Vec<holon_api::entity::StorageEntity>,
    version: i64,
) -> ListSnapshot {
    let spec = conn.compiled.spec();
    let mut cursor: holon_api::entity::StorageEntity = Default::default();
    cursor.insert("id".into(), Value::String("cursor".into()));
    cursor.insert(spec.version_column.as_str().into(), Value::Integer(version));
    let sets = vec![
        TypedRowSet {
            type_name: spec.list_row_type.clone(),
            owner_column: "id".into(),
            owner_value: "cursor".into(),
            rows: vec![cursor],
        },
        TypedRowSet {
            type_name: spec.entity.clone(),
            owner_column: "id".into(),
            owner_value: "list".into(),
            rows: remote,
        },
    ];
    ListSnapshot::from_rows(&conn.compiled, &sets, FETCHED_AT).expect("the snapshot maps in full")
}

/// The scenario as the two sides the reconciler reads.
fn sides(conn: &Conn, scenario: &Scenario) -> (Vec<LocalRow>, ListSnapshot) {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for (index, placement) in scenario.0.iter().enumerate() {
        let n = index as u8;
        match *placement {
            Placement::RemoteOnly => remote.push(remote_row(conn, n, 1, false)),
            Placement::LocalOnly { ever_seen, tomb } => {
                local.push(local_row(conn, n, 1, false, tomb, ever_seen));
            }
            Placement::Both {
                tomb,
                local_count,
                remote_count,
                local_checked,
                remote_checked,
            } => {
                local.push(local_row(conn, n, local_count, local_checked, tomb, true));
                remote.push(remote_row(conn, n, remote_count, remote_checked));
            }
        }
    }
    (local, snapshot_of(conn, remote, 7))
}

fn ids_of<'a>(intents: impl Iterator<Item = &'a LocalIntent>) -> BTreeSet<String> {
    intents
        .map(|intent| match intent {
            LocalIntent::Insert { id, .. }
            | LocalIntent::SetColumn { id, .. }
            | LocalIntent::TouchWatermark { id, .. }
            | LocalIntent::Delete { id }
            | LocalIntent::ReapTombstone { id } => id.clone(),
        })
        .collect()
}

fn deleted_ids(outcome: &ReconcileOutcome) -> BTreeSet<String> {
    ids_of(
        outcome
            .local
            .iter()
            .filter(|intent| matches!(intent, LocalIntent::Delete { .. })),
    )
}

/// Apply the outcome's local intents to the mirror, so the next round can be
/// reconciled against the same snapshot. This is the mirror write the
/// dispatcher performs in production, spelled once here.
fn apply(conn: &Conn, local: &[LocalRow], outcome: &ReconcileOutcome) -> Vec<LocalRow> {
    let mut by_id: BTreeMap<String, LocalRow> = local
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect();
    for intent in &outcome.local {
        match intent {
            LocalIntent::Insert { id, columns } => {
                let mut columns = columns.clone();
                columns.insert("id".into(), Value::String(id.clone()));
                by_id.insert(
                    id.clone(),
                    LocalRow {
                        id: id.clone(),
                        columns,
                    },
                );
            }
            LocalIntent::SetColumn { id, column, value } => {
                let row = by_id.get_mut(id).expect("a set targets a row that exists");
                row.columns.insert(column.clone(), value.clone());
            }
            LocalIntent::TouchWatermark { id, at } => {
                let row = by_id
                    .get_mut(id)
                    .expect("a touch targets a row that exists");
                row.columns
                    .insert(conn.watermark_column.into(), Value::String(at.clone()));
            }
            LocalIntent::Delete { id } | LocalIntent::ReapTombstone { id } => {
                by_id.remove(id);
            }
        }
    }
    by_id.into_values().collect()
}

// ---------------------------------------------------------------------------
// Properties. Each runs over EVERY connection.
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    /// A second reconcile against the same snapshot decides nothing new. If it
    /// did, a list nobody is editing would keep producing writes — and a round
    /// that never quiesces cannot tell a real remote change from its own echo.
    #[test]
    fn a_round_converges_against_an_unchanged_snapshot(scenario in scenario()) {
        for conn in connections() {
            let (local, snapshot) = sides(&conn, &scenario);
            let reconciler = RemoteListReconciler::new(conn.compiled.clone());
            let first = reconciler.reconcile(&local, &snapshot).expect("first round");
            let settled = apply(&conn, &local, &first);
            let second = reconciler.reconcile(&settled, &snapshot).expect("second round");

            let unsettled: Vec<_> = second
                .local
                .iter()
                .filter(|intent| !matches!(intent, LocalIntent::TouchWatermark { .. }))
                .collect();
            prop_assert!(
                unsettled.is_empty(),
                "{}: a settled mirror still decides {unsettled:?}",
                conn.label
            );
            // Pushes DO survive: a tombstone the peer has not accepted yet is
            // still outstanding after a local write, and the round commits it.
            // What must not survive is a push the first round already resolved
            // by writing locally.
            prop_assert!(
                second.push.len() <= first.push.len(),
                "{}: the second round pushes MORE than the first",
                conn.label
            );
        }
    }

    /// A row the peer has never carried is never deleted by its absence. This
    /// is the one asymmetry that makes absence-as-deletion safe: without a
    /// watermark, "not in the snapshot" carries no information at all.
    #[test]
    fn an_unseen_local_row_is_pushed_never_deleted(scenario in scenario()) {
        for conn in connections() {
            let (local, snapshot) = sides(&conn, &scenario);
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&local, &snapshot)
                .expect("the round");
            let deleted = deleted_ids(&outcome);
            let pushed: BTreeSet<String> = outcome
                .push
                .iter()
                .map(|push| match push {
                    PushIntent::Add { row, .. }
                    | PushIntent::Update { row, .. }
                    | PushIntent::Remove { row, .. } => row.id.clone(),
                })
                .collect();

            for (index, placement) in scenario.0.iter().enumerate() {
                let Placement::LocalOnly { ever_seen: false, tomb: Tombstone::None } = *placement
                else {
                    continue;
                };
                let id = (conn.row_id)(index as u8);
                prop_assert!(!deleted.contains(&id), "{}: deleted the unseen row {id}", conn.label);
                prop_assert!(pushed.contains(&id), "{}: never pushed the unseen row {id}", conn.label);
            }
        }
    }

    /// No round unsets a latch column except on a live row Holon holds set,
    /// whose base says the peer served it set and which the peer now serves
    /// unset — across a whole list of rows, tombstones and bases at once. A
    /// row an `Insert` writes is one Holon holds no live copy of, so the insert
    /// path cannot carry an unset past that rule either.
    #[test]
    fn a_latch_column_is_never_unset_by_a_round(
        scenario in scenario(),
        bases in prop::collection::vec(prop::option::of(any::<bool>()), 8),
    ) {
        for conn in connections() {
            let (mut local, snapshot) = sides(&conn, &scenario);
            for row in &mut local {
                let index = (0..scenario.0.len())
                    .find(|n| (conn.row_id)(*n as u8) == row.id)
                    .expect("every local row comes from a placement");
                if let Some(checked) = bases[index] {
                    row.columns.insert(
                        conn.compiled.spec().base_column.clone(),
                        Value::String(
                            serde_json::json!({ "checked": i64::from(checked) }).to_string(),
                        ),
                    );
                }
            }
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&local, &snapshot)
                .expect("the round");
            for intent in &outcome.local {
                match intent {
                    LocalIntent::SetColumn { id, column, value } if column == "checked" => {
                        if *value == Value::Integer(1) {
                            continue;
                        }
                        let index = (0..scenario.0.len())
                            .find(|n| (conn.row_id)(*n as u8) == *id)
                            .expect("a set targets a placed row");
                        let lowered_by_peer = matches!(
                            scenario.0[index],
                            Placement::Both {
                                tomb: Tombstone::None,
                                local_checked: true,
                                remote_checked: false,
                                ..
                            }
                        ) && bases[index] == Some(true);
                        prop_assert!(
                            *value == Value::Integer(0) && lowered_by_peer,
                            "{}: a round set the latch of {id} to {value:?} under {:?} with base \
                             {:?}",
                            conn.label,
                            scenario.0[index],
                            bases[index]
                        );
                    }
                    LocalIntent::Insert { id, .. } => {
                        let replaced = local.iter().find(|row| row.id == *id);
                        prop_assert!(
                            replaced.is_none_or(|row| {
                                row.column(conn.tombstone_column)
                                    == Some(&Value::String(EXPIRED_TOMBSTONE.into()))
                            }),
                            "{}: an insert replaced the live row {id}",
                            conn.label
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// A peer's set latch always reaches Holon; an unset one only where the
    /// base says the peer served it set. A wrong check skips one item; a wrong
    /// un-check re-buys it every trip, so a Holon set the peer has not served
    /// stays. `base` is `Some(None)` for a base recorded before the latch was
    /// part of it: it holds no `checked` key, so it never lowers.
    #[test]
    fn a_latch_column_lowers_only_on_the_peers_own_unset(
        base in prop::option::of(prop::option::of(any::<bool>())),
        mine in any::<bool>(),
        theirs in any::<bool>(),
    ) {
        for conn in connections() {
            let mut row = local_row(&conn, 0, 1, mine, Tombstone::None, true);
            if let Some(checked) = base {
                let served = match (conn.remote_merge)(1) {
                    Value::String(text) => serde_json::Value::String(text),
                    Value::Integer(number) => serde_json::Value::from(number),
                    other => panic!("no JSON spelling for the merge value {other:?}"),
                };
                let mut recorded = serde_json::json!({ conn.merge_column: served });
                if let Some(checked) = checked {
                    recorded["checked"] = serde_json::Value::from(i64::from(checked));
                }
                row.columns.insert(
                    conn.compiled.spec().base_column.clone(),
                    Value::String(recorded.to_string()),
                );
            }
            let snapshot = snapshot_of(&conn, vec![remote_row(&conn, 0, 1, theirs)], 1);
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&[row], &snapshot)
                .expect("the round");
            let latched: Vec<&Value> = outcome
                .local
                .iter()
                .filter_map(|intent| match intent {
                    LocalIntent::SetColumn { column, value, .. } if column == "checked" => {
                        Some(value)
                    }
                    _ => None,
                })
                .collect();
            let want = match (theirs, mine, base) {
                (true, false, _) => vec![Value::Integer(1)],
                (false, true, Some(Some(true))) => vec![Value::Integer(0)],
                _ => vec![],
            };
            prop_assert_eq!(
                latched,
                want.iter().collect::<Vec<_>>(),
                "{}: base {:?}, Holon {}, peer {}",
                conn.label, base, mine, theirs
            );
            prop_assert!(outcome.push.is_empty(), "{}: a latch change was pushed", conn.label);
        }
    }

    /// A live tombstone is pushed as a removal and is not overwritten locally:
    /// the pull must not undo a deletion the peer has not been told about.
    #[test]
    fn a_live_tombstone_is_pushed_not_resurrected(scenario in scenario()) {
        for conn in connections() {
            let (local, snapshot) = sides(&conn, &scenario);
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&local, &snapshot)
                .expect("the round");
            let removed: BTreeSet<String> = outcome
                .push
                .iter()
                .filter_map(|push| match push {
                    PushIntent::Remove { row, .. } => Some(row.id.clone()),
                    PushIntent::Add { .. } | PushIntent::Update { .. } => None,
                })
                .collect();
            let written = ids_of(outcome.local.iter());

            for (index, placement) in scenario.0.iter().enumerate() {
                let Placement::Both { tomb: Tombstone::Fresh, .. } = *placement else {
                    continue;
                };
                let id = (conn.row_id)(index as u8);
                prop_assert!(removed.contains(&id), "{}: {id} was not pushed as removed", conn.label);
                prop_assert!(!written.contains(&id), "{}: {id} was written over its tombstone", conn.label);
            }
        }
    }

    /// The peer's value and the column's read-back spelling of it compare
    /// equal: a free-text amount as the same text, a number as the same number.
    /// Without that, every round would rewrite the merge column and a quiet
    /// list would look permanently busy.
    #[test]
    fn an_unchanged_merge_column_is_not_rewritten(counts in prop::collection::vec(0i64..4, 1..6)) {
        for conn in connections() {
            let scenario = Scenario(
                counts
                    .iter()
                    .map(|count| Placement::Both {
                        tomb: Tombstone::None,
                        local_count: *count,
                        remote_count: *count,
                        local_checked: true,
                        remote_checked: true,
                    })
                    .collect(),
            );
            let (mut local, snapshot) = sides(&conn, &scenario);
            for (row, count) in local.iter_mut().zip(&counts) {
                let served = match (conn.remote_merge)(*count) {
                    Value::String(text) => serde_json::Value::String(text),
                    Value::Integer(number) => serde_json::Value::from(number),
                    other => panic!("no JSON spelling for the merge value {other:?}"),
                };
                let base = serde_json::json!({ conn.merge_column: served, "checked": 1 });
                row.columns.insert(
                    conn.compiled.spec().base_column.clone(),
                    Value::String(base.to_string()),
                );
            }
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&local, &snapshot)
                .expect("the round");
            let sets: Vec<_> = outcome
                .local
                .iter()
                .filter(|intent| matches!(intent, LocalIntent::SetColumn { .. }))
                .collect();
            prop_assert!(sets.is_empty(), "{}: rewrote an unchanged row: {sets:?}", conn.label);
            prop_assert!(outcome.push.is_empty(), "{}: pushed an agreed row", conn.label);
        }
    }

    /// A merge column settles three-way against the value the peer last
    /// served: Holon's change alone is pushed, the peer's alone is taken, and
    /// anything else takes the peer's value and discloses Holon's — an unknown
    /// base included.
    #[test]
    fn a_merge_column_settles_three_way(
        base in prop::option::of(0i64..3),
        mine in 0i64..3,
        theirs in 0i64..3,
    ) {
        for conn in connections() {
            let mut row = local_row(&conn, 0, mine, false, Tombstone::None, true);
            if let Some(base) = base {
                let served = match (conn.remote_merge)(base) {
                    Value::String(text) => serde_json::Value::String(text),
                    Value::Integer(number) => serde_json::Value::from(number),
                    other => panic!("no JSON spelling for the merge value {other:?}"),
                };
                row.columns.insert(
                    conn.compiled.spec().base_column.clone(),
                    Value::String(serde_json::json!({ conn.merge_column: served }).to_string()),
                );
            }
            let snapshot = snapshot_of(&conn, vec![remote_row(&conn, 0, theirs, false)], 1);
            let outcome = RemoteListReconciler::new(conn.compiled.clone())
                .reconcile(&[row], &snapshot)
                .expect("the round");
            let taken = outcome.local.iter().any(|intent| {
                matches!(intent, LocalIntent::SetColumn { column, .. } if column == conn.merge_column)
            });
            let pushed = outcome
                .push
                .iter()
                .any(|push| matches!(push, PushIntent::Update { .. }));
            let (want_taken, want_pushed, want_disclosed) = if mine == theirs {
                (false, false, false)
            } else if base == Some(theirs) {
                (false, true, false)
            } else if base == Some(mine) {
                (true, false, false)
            } else {
                (true, false, true)
            };
            prop_assert_eq!(
                (taken, pushed, !outcome.overwritten.is_empty()),
                (want_taken, want_pushed, want_disclosed),
                "{}: base {:?}, Holon {}, peer {} → (taken, pushed, disclosed); disclosed {:?}",
                conn.label, base, mine, theirs, outcome.overwritten
            );
            for disclosure in &outcome.overwritten {
                let named = format!("`{}` of {}: ", conn.merge_column, (conn.display_name)(0));
                prop_assert!(
                    disclosure.starts_with(&named)
                        && !disclosure.contains('[')
                        && !disclosure.contains("Some("),
                    "{}: the disclosure {disclosure:?} does not open with {named:?} or prints a \
                     key or a debug value",
                    conn.label
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Identity is load-bearing: a key that folds two rows is a failure, never a
// silent merge. Under a complete snapshot the survivor's absence would read as
// a deletion of the other.
// ---------------------------------------------------------------------------

#[test]
fn two_remote_rows_sharing_a_key_are_refused() {
    for conn in connections() {
        let spec = conn.compiled.spec();
        let mut cursor: holon_api::entity::StorageEntity = Default::default();
        cursor.insert("id".into(), Value::String("cursor".into()));
        cursor.insert(spec.version_column.as_str().into(), Value::Integer(1));
        // Two DISTINCT rows that the connection's key expression collapses:
        // the same content pair under the content key, the same id under the
        // server key.
        let mut twin = remote_row(&conn, 3, 2, false);
        twin.insert("id".into(), Value::String((conn.row_id)(3)));
        let sets = vec![
            TypedRowSet {
                type_name: spec.list_row_type.clone(),
                owner_column: "id".into(),
                owner_value: "cursor".into(),
                rows: vec![cursor],
            },
            TypedRowSet {
                type_name: spec.entity.clone(),
                owner_column: "id".into(),
                owner_value: "list".into(),
                rows: vec![remote_row(&conn, 3, 1, false), twin],
            },
        ];
        let refused = ListSnapshot::from_rows(&conn.compiled, &sets, FETCHED_AT);
        let message = refused
            .expect_err(&format!("{}: a folded key was accepted", conn.label))
            .to_string();
        assert!(
            message.contains("share the key"),
            "{}: the refusal does not name the collision: {message}",
            conn.label
        );
    }
}

#[test]
fn two_local_rows_sharing_a_key_are_refused() {
    for conn in connections() {
        // Two mirror rows under different primary keys that the connection's
        // key expression collapses. The key columns stay identical — that is
        // what makes them one identity — and only the primary key differs.
        let mut twin = local_row(&conn, 5, 1, false, Tombstone::None, true);
        twin.id = format!("{}-duplicate", twin.id);
        let local = vec![local_row(&conn, 5, 1, false, Tombstone::None, true), twin];
        let snapshot = snapshot_of(&conn, vec![], 1);
        let message = RemoteListReconciler::new(conn.compiled.clone())
            .reconcile(&local, &snapshot)
            .expect_err(&format!("{}: a folded local key was accepted", conn.label))
            .to_string();
        assert!(
            message.contains("share the key"),
            "{}: the refusal does not name the collision: {message}",
            conn.label
        );
    }
}

// ---------------------------------------------------------------------------
// One whole round against an OFFLINE fixture peer, for both connections. No
// transport: the peer is a trait, so a list that lives in a `Mutex` is as valid
// a peer as an HTTP endpoint — which is the property that keeps the round, the
// reconciler and the intents in every wasm graph.
// ---------------------------------------------------------------------------

/// A list that applies the commands it is sent and versions itself, so "the
/// peer moved between polls" is a scenario a test can stage.
struct FixturePeer {
    compiled: Arc<CompiledListSync>,
    state: std::sync::Mutex<FixtureList>,
}

struct FixtureList {
    /// Identity `n` → (merge value, latch).
    items: BTreeMap<u8, (i64, bool)>,
    version: i64,
    commits: usize,
}

impl FixturePeer {
    fn new(conn: &Conn, items: &[u8]) -> Self {
        Self {
            compiled: conn.compiled.clone(),
            state: std::sync::Mutex::new(FixtureList {
                items: items.iter().map(|n| (*n, (1, false))).collect(),
                version: 4,
                commits: 0,
            }),
        }
    }
}

/// The identity a command names, recovered from the key the connection derived.
/// The fixture peer reads its own vocabulary back out of the command row, the
/// way a real `request` mapping does.
fn identity_of(command: &holon_connections::CommitCommand) -> u8 {
    let text = match command.columns.get("id") {
        Some(Value::String(s)) => s.clone(),
        other => panic!("a command row carries no id (got {other:?})"),
    };
    text.rsplit(':')
        .next()
        .and_then(|tail| tail.trim_start_matches("item").parse().ok())
        .unwrap_or_else(|| panic!("the command row's id '{text}' names no fixture identity"))
}

#[async_trait::async_trait]
impl holon_connections::RemoteListPeer for FixturePeer {
    async fn pull(&self) -> anyhow::Result<ListSnapshot> {
        let state = self.state.lock().expect("the fixture list");
        let conn = if self.compiled.spec().entity == "shopping_item" {
            shopping()
        } else {
            tasks()
        };
        let rows: Vec<_> = state
            .items
            .iter()
            .map(|(n, (count, checked))| remote_row(&conn, *n, *count, *checked))
            .collect();
        Ok(snapshot_of(&conn, rows, state.version))
    }

    async fn commit(
        &self,
        batch: &holon_connections::CommitBatch,
    ) -> anyhow::Result<holon_connections::CommitAck> {
        let mut state = self.state.lock().expect("the fixture list");
        for command in &batch.commands {
            let n = identity_of(command);
            match command.verb {
                holon_connections::CommandVerb::Add => {
                    state.items.insert(n, (1, false));
                }
                holon_connections::CommandVerb::Remove => {
                    state.items.remove(&n);
                }
                holon_connections::CommandVerb::Update => {
                    unreachable!("this PBT never edits a merge column locally")
                }
            }
        }
        state.version += 1;
        state.commits += 1;
        Ok(holon_connections::CommitAck {
            version: state.version,
        })
    }
}

struct FixtureRows(Vec<LocalRow>);

#[async_trait::async_trait]
impl holon_connections::LocalRowReader for FixtureRows {
    async fn load(&self) -> anyhow::Result<Vec<LocalRow>> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn a_round_pushes_a_never_seen_row_and_then_converges() {
    for conn in connections() {
        let peer = FixturePeer::new(&conn, &[1, 2]);
        let reconciler = RemoteListReconciler::new(conn.compiled.clone());
        // Identity 9 exists only locally and the peer has never carried it.
        let rows = FixtureRows(vec![local_row(&conn, 9, 1, false, Tombstone::None, false)]);

        let first = holon_connections::sync_once(&peer, &rows, &reconciler, "device", 1_000)
            .await
            .expect("one round");
        assert_eq!(
            first.committed, 1,
            "{}: the addition was not committed",
            conn.label
        );
        assert!(
            !first.retried,
            "{}: an uncontended round retried",
            conn.label
        );
        assert!(
            peer.state
                .lock()
                .expect("the fixture list")
                .items
                .contains_key(&9),
            "{}: the peer never received the addition",
            conn.label
        );

        let again = holon_connections::sync_once(&peer, &rows, &reconciler, "device", 2_000)
            .await
            .expect("second round");
        assert_eq!(
            again.committed, 0,
            "{}: the round did not converge",
            conn.label
        );
    }
}

#[tokio::test]
async fn a_round_pushes_a_live_tombstone_as_a_removal() {
    for conn in connections() {
        let peer = FixturePeer::new(&conn, &[3]);
        let reconciler = RemoteListReconciler::new(conn.compiled.clone());
        // The reconciler measures the tombstone against the snapshot's fetch
        // time, which the fixture stamps from the constant above — so a fixed
        // date here stays inside the window whatever day the suite runs.
        let rows = FixtureRows(vec![local_row(&conn, 3, 1, false, Tombstone::Fresh, true)]);

        let outcome = holon_connections::sync_once(&peer, &rows, &reconciler, "device", 1_000)
            .await
            .expect("one round");

        assert_eq!(
            outcome.committed, 1,
            "{}: the deletion was not committed",
            conn.label
        );
        assert!(
            !peer
                .state
                .lock()
                .expect("the fixture list")
                .items
                .contains_key(&3),
            "{}: the peer still lists a locally deleted row",
            conn.label
        );
    }
}

/// A freshness shape nobody implements is a load failure naming the offender,
/// not a connection that silently sends nothing. The declaration is the whole
/// contract, so a typo in it must not become a peer serving stale bodies.
#[test]
fn an_unknown_cache_buster_is_refused_by_name() {
    let yaml = format!("{SHOPPING_LIST_SYNC}cache_buster: every_other_tuesday\n");
    let message = serde_yaml::from_str::<ListSyncSpec>(&yaml)
        .expect_err("an unknown freshness shape was accepted")
        .to_string();
    assert!(
        message.contains("every_other_tuesday"),
        "the refusal does not name the offending value: {message}"
    );
    assert!(
        message.contains("epoch_millis"),
        "the refusal does not say what IS accepted: {message}"
    );
}

/// Every pull rewrites the merge base, so a base kept in the row's primary key,
/// the list's version cursor or a column the key reads would move the row's
/// identity or the cursor under it. The sidecar is refused where it loads.
#[test]
fn a_base_column_carrying_identity_or_the_version_is_refused() {
    let declared: TypeDefinition = serde_yaml::from_str(SHOPPING_TYPE).expect("the declared type");
    for (what, base, spec_yaml) in [
        (
            "the primary key",
            "id",
            SHOPPING_LIST_SYNC.replace("base_column: remote_base", "base_column: id"),
        ),
        (
            "the version column",
            "remote_base",
            SHOPPING_LIST_SYNC.replace("version_column: version", "version_column: remote_base"),
        ),
        (
            "a column the key reads",
            "name",
            SHOPPING_LIST_SYNC.replace("base_column: remote_base", "base_column: name"),
        ),
    ] {
        let spec: ListSyncSpec = serde_yaml::from_str(&spec_yaml).expect("the list_sync block");
        let message = format!(
            "{:#}",
            CompiledListSync::compile("shopping", spec, &declared)
                .expect_err(&format!("a base kept in {what} was accepted"))
        );
        assert!(
            message.contains(&format!("merge base in '{base}'")),
            "the refusal of a base kept in {what} does not name it: {message}"
        );
    }
}

/// The common case costs nothing to declare: a sidecar that says nothing about
/// freshness gets a connection that asks for none.
#[test]
fn a_connection_declares_no_cache_buster_by_default() {
    let spec: ListSyncSpec = serde_yaml::from_str(SHOPPING_LIST_SYNC).expect("the block parses");
    assert_eq!(spec.cache_buster, holon_connections::CacheBuster::None);
    assert!(!spec.cache_buster.is_declared());
}

// ---------------------------------------------------------------------------
// A mirror row's id is a REFERENCE to the entity it mirrors
// ---------------------------------------------------------------------------

/// The scheme of a mirror row's id names the DECLARED TYPE — not the list it
/// came from, not the peer. A dispatcher that checks a reference's scheme
/// against its entity refuses anything else as a foreign reference, so a row
/// keyed any other way is a row no follow-up operation can write to.
#[test]
fn every_built_row_id_is_a_reference_to_the_declared_type() {
    for conn in connections() {
        let expected = holon_api::EntityName::new(conn.compiled.spec().entity.clone());
        let snapshot = snapshot_of(
            &conn,
            (0..3).map(|n| remote_row(&conn, n, 1, false)).collect(),
            1,
        );
        for (_, row) in snapshot.rows() {
            let uri = holon_api::entity_uri::EntityUri::parse(&row.id).unwrap_or_else(|e| {
                panic!(
                    "{}: the row id '{}' is not a reference: {e}",
                    conn.label, row.id
                )
            });
            assert_eq!(
                uri.scheme(),
                expected.as_str(),
                "{}: the row id '{}' names something other than the type it mirrors",
                conn.label,
                row.id
            );
        }
    }
}

/// An id carrying someone else's scheme is refused where it enters, not
/// written and discovered later. The peer's mapping is what derives it, so a
/// mapping that derives the wrong one is a load-bearing failure.
#[test]
fn a_row_id_under_a_foreign_scheme_is_refused() {
    let conn = shopping();
    let spec = conn.compiled.spec();
    let mut cursor: holon_api::entity::StorageEntity = Default::default();
    cursor.insert("id".into(), Value::String("cursor".into()));
    cursor.insert(spec.version_column.as_str().into(), Value::Integer(1));
    let mut row = remote_row(&conn, 1, 1, false);
    // `shopping:` names the LIST, which the row already carries in its own
    // column; it does not name the entity.
    row.insert("id".into(), Value::String("shopping:staples:item1".into()));
    let sets = vec![
        TypedRowSet {
            type_name: spec.list_row_type.clone(),
            owner_column: "id".into(),
            owner_value: "cursor".into(),
            rows: vec![cursor],
        },
        TypedRowSet {
            type_name: spec.entity.clone(),
            owner_column: "id".into(),
            owner_value: "list".into(),
            rows: vec![row],
        },
    ];
    let message = ListSnapshot::from_rows(&conn.compiled, &sets, FETCHED_AT)
        .expect_err("a foreign-scheme id was accepted")
        .to_string();
    assert!(
        message.contains("shopping-item"),
        "the refusal does not name the scheme the row must carry: {message}"
    );
}

// ---------------------------------------------------------------------------
// A peer that takes a push of its tick, and matches an addition by name alone.
// ---------------------------------------------------------------------------

const PICKED_LIST_SYNC: &str = r#"
entity: shopping_item
pull_tool: pull_list
commit_tool: commit
list_row_type: shopping_list
version_column: version
key: "[.name, .cat]"
display_name: '.name + " in " + .cat'
watermark_column: last_seen_remote
base_column: remote_base
batch_row_type: shopping_commit
command_row_type: shopping_command
merge_columns: [count, checked]
peer_match: .name
"#;

fn picked_list() -> Conn {
    Conn {
        label: "shopping (tick merged, matched by name)",
        compiled: compile("picked", SHOPPING_TYPE, PICKED_LIST_SYNC),
        ..shopping()
    }
}

/// A flag as one side spells it: the mirror's `INTEGER`, or a mapping's
/// boolean.
fn flag(on: bool, as_boolean: bool) -> Value {
    if as_boolean {
        Value::Boolean(on)
    } else {
        Value::Integer(i64::from(on))
    }
}

fn item(name: u8, cat: u8, seen: bool) -> LocalRow {
    let mut columns: BTreeMap<String, Value> = BTreeMap::new();
    let id = format!("shopping-item:cat{cat}:item{name}");
    columns.insert("id".into(), Value::String(id.clone()));
    columns.insert("name".into(), Value::String(format!("item{name}")));
    columns.insert("cat".into(), Value::String(format!("cat{cat}")));
    columns.insert("count".into(), amount(0));
    columns.insert("checked".into(), Value::Integer(0));
    columns.insert("deleted_at".into(), Value::Null);
    columns.insert(
        "last_seen_remote".into(),
        if seen {
            Value::String("2026-09-13T00:00:00+00:00".into())
        } else {
            Value::Null
        },
    );
    LocalRow { id, columns }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A flag merged as a column settles three-way whichever spelling each side
    /// and the base carry it in: `1` and `true` are one value, so an unchanged
    /// flag is neither rewritten nor pushed.
    #[test]
    fn a_merged_flag_settles_three_way_across_its_spellings(
        base in prop::option::of((any::<bool>(), any::<bool>())),
        mine in any::<bool>(),
        theirs in (any::<bool>(), any::<bool>()),
    ) {
        let conn = picked_list();
        let mut row = local_row(&conn, 0, 0, mine, Tombstone::None, true);
        if let Some((on, as_boolean)) = base {
            let served = if as_boolean { serde_json::json!(on) } else { serde_json::json!(i64::from(on)) };
            row.columns.insert(
                "remote_base".into(),
                Value::String(serde_json::json!({ "count": AMOUNTS[0], "checked": served }).to_string()),
            );
        }
        let mut remote = remote_row(&conn, 0, 0, false);
        remote.insert("checked".into(), flag(theirs.0, theirs.1));
        let snapshot = snapshot_of(&conn, vec![remote], 1);
        let outcome = RemoteListReconciler::new(conn.compiled.clone())
            .reconcile(&[row], &snapshot)
            .expect("the round");
        let taken = outcome.local.iter().any(|intent| {
            matches!(intent, LocalIntent::SetColumn { column, .. } if column == "checked")
        });
        let pushed = outcome.push.iter().any(|push| matches!(push, PushIntent::Update { .. }));
        let base = base.map(|(on, _)| on);
        let theirs = theirs.0;
        let want = if mine == theirs {
            (false, false, false)
        } else if base == Some(theirs) {
            (false, true, false)
        } else if base == Some(mine) {
            (true, false, false)
        } else {
            (true, false, true)
        };
        prop_assert_eq!(
            (taken, pushed, !outcome.overwritten.is_empty()),
            want,
            "base {:?}, Holon {}, peer {} → (taken, pushed, disclosed); disclosed {:?}",
            base, mine, theirs, outcome.overwritten
        );
    }

    /// The peer matches an addition by name alone, so of Holon's additions under
    /// one name at most one is sent, and none under a name the peer holds; every
    /// other one is withheld and named.
    #[test]
    fn an_addition_the_peer_would_match_to_another_item_is_withheld(
        held in prop::collection::btree_set((0u8..3, 0u8..3), 0..4),
        added in prop::collection::btree_set((0u8..3, 0u8..3), 0..6),
    ) {
        let conn = picked_list();
        let added: BTreeSet<(u8, u8)> = added.difference(&held).copied().collect();
        let remote = held
            .iter()
            .map(|&(name, cat)| {
                let mut row: holon_api::entity::StorageEntity = Default::default();
                for (column, value) in item(name, cat, true).columns {
                    if ["id", "name", "cat", "count", "checked"].contains(&column.as_str()) {
                        row.insert(column.into(), value);
                    }
                }
                row
            })
            .collect();
        let local: Vec<LocalRow> = added.iter().map(|&(name, cat)| item(name, cat, false)).collect();
        let snapshot = snapshot_of(&conn, remote, 1);
        let outcome = RemoteListReconciler::new(conn.compiled.clone())
            .reconcile(&local, &snapshot)
            .expect("the round");

        let mut sent: BTreeMap<String, usize> = BTreeMap::new();
        for push in &outcome.push {
            let PushIntent::Add { row, .. } = push else {
                prop_assert!(false, "an addition-only round pushed {push:?}");
                unreachable!()
            };
            let Some(Value::String(name)) = row.columns.get("name") else {
                panic!("an addition without a name: {row:?}");
            };
            *sent.entry(name.clone()).or_default() += 1;
        }
        let held_names: BTreeSet<String> = held.iter().map(|(n, _)| format!("item{n}")).collect();
        let added_names: BTreeSet<String> = added.iter().map(|(n, _)| format!("item{n}")).collect();
        let want: BTreeMap<String, usize> =
            added_names.difference(&held_names).map(|name| (name.clone(), 1)).collect();
        prop_assert_eq!(&sent, &want, "held {:?}, added {:?}", held, added);
        prop_assert_eq!(
            outcome.withheld.len() + outcome.push.len(),
            added.len(),
            "every addition is sent or withheld: {:?}",
            outcome.withheld
        );
        for disclosure in &outcome.withheld {
            prop_assert!(
                added.iter().any(|(n, c)| disclosure.starts_with(&format!("item{n} in cat{c} is not sent")))
                    && !disclosure.contains('['),
                "the disclosure {disclosure:?} does not open with the withheld item's name"
            );
        }
    }
}
