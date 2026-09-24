//! Contract: `MatviewManager::rebuild_watch_views` leaves every live
//! subscriber holding exactly what its rebuilt view holds, and a view it
//! cannot rebuild neither stops the others nor goes unreported.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use holon_api::Change;
use holon_api::Value;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::DbHandle;
use holon_turso::turso::RowChangeStream;
use holon_turso::turso::TursoBackend;

async fn live_database() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    for ddl in [
        "CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')",
        "CREATE TABLE t2 (id TEXT PRIMARY KEY, extra TEXT)",
        "CREATE TABLE t3 (id TEXT PRIMARY KEY, extra TEXT)",
    ] {
        handle.execute_ddl(ddl).await.expect("create table");
    }
    handle
}

async fn run(handle: &DbHandle, sql: &str) {
    handle
        .execute(sql, Vec::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn manager(handle: &DbHandle) -> MatviewManager {
    MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())))
}

type Rows = BTreeMap<String, String>;

fn project(rows: Vec<holon_core::storage::StorageEntity>) -> Rows {
    rows.into_iter().map(|row| row_entry(&row)).collect()
}

/// A row keyed the way live CDC keys it: by a text `id`, else by `_rowid`.
fn row_entry(row: &holon_core::storage::StorageEntity) -> (String, String) {
    let key = match (row.get("id"), row.get("_rowid")) {
        (Some(Value::String(id)), _) => id.clone(),
        (_, Some(Value::String(rowid))) => rowid.clone(),
        (_, Some(Value::Integer(rowid))) => rowid.to_string(),
        other => panic!("row {row:?} has neither a text id nor a _rowid: {other:?}"),
    };
    let extra = match row.get("extra") {
        Some(Value::String(extra)) => extra.clone(),
        other => panic!("row {row:?}: extra is {other:?}, not text"),
    };
    (key, extra)
}

/// What a subscriber holds: the view's snapshot with every change it has
/// been sent applied, in order.
struct Subscriber {
    rows: Rows,
    stream: RowChangeStream,
}

impl Subscriber {
    async fn open(manager: &MatviewManager, sql: &str) -> (String, Self) {
        let (view, stream) = manager
            .ensure_and_subscribe(sql, None)
            .await
            .expect("subscribe");
        let rows = project(manager.query_view(&view).await.expect("snapshot"));
        (view, Self { rows, stream })
    }

    /// Apply every batch that arrives until the stream is quiet.
    async fn drain(&mut self) {
        while let Ok(batch) =
            tokio::time::timeout(Duration::from_millis(500), self.stream.next()).await
        {
            let batch = batch.expect("the stream ended");
            for item in batch.inner.items {
                match item.change {
                    Change::Created { data, .. } | Change::Updated { data, .. } => {
                        let (id, extra) = row_entry(&data);
                        self.rows.insert(id, extra);
                    }
                    Change::Deleted { id, .. } => {
                        self.rows.remove(&id);
                    }
                    Change::FieldsChanged { .. } => panic!("a matview emits no FieldsChanged"),
                }
            }
        }
    }
}

#[tokio::test]
async fn a_rebuild_leaves_each_subscriber_holding_exactly_its_rebuilt_view() {
    let handle = live_database().await;
    for n in 0..20 {
        run(
            &handle,
            &format!("INSERT INTO t2 (id, extra) VALUES ('row{n}', 'x')"),
        )
        .await;
    }
    let manager = manager(&handle);
    // Every IVM-corruption shape known in the fork is fixed, so `random()`
    // stands in for a stale view: rebuilding it changes which rows it holds
    // and what they say.
    let sql = "SELECT id, CAST(random() AS TEXT) AS extra FROM t2 WHERE random() % 2 = 0";
    let (view, mut subscriber) = Subscriber::open(&manager, sql).await;

    self::manager(&handle)
        .rebuild_watch_views()
        .await
        .expect("rebuild watch views");
    subscriber.drain().await;

    let rebuilt = project(manager.query_view(&view).await.expect("read rebuilt view"));
    assert_eq!(
        subscriber.rows, rebuilt,
        "after the rebuild the subscriber of {view} must hold exactly the rebuilt view's rows"
    );
}

#[tokio::test]
async fn a_rebuild_recreates_every_listened_view_it_can_and_names_each_one_it_cannot() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let (healthy, mut healthy_stream) = manager
        .ensure_and_subscribe("SELECT id, content FROM items", None)
        .await
        .expect("subscribe healthy");
    let (_, _broken) = Subscriber::open(&manager, "SELECT id, extra FROM t2").await;
    let (_, _other_broken) = Subscriber::open(&manager, "SELECT id, extra FROM t3").await;
    let broken = MatviewManager::compute_view_name("SELECT id, extra FROM t2");
    let other_broken = MatviewManager::compute_view_name("SELECT id, extra FROM t3");
    for table in ["t2", "t3"] {
        handle
            .execute_ddl(&format!("DROP TABLE {table}"))
            .await
            .expect("drop table");
    }

    let err = self::manager(&handle)
        .rebuild_watch_views()
        .await
        .expect_err("two listened views cannot be recreated");
    let message = format!("{err:#}");
    for view in [&broken, &other_broken] {
        assert!(
            message.contains(view.as_str()),
            "the error must name every view that failed, {view} among them: {message}"
        );
    }

    run(
        &handle,
        "INSERT INTO items (id, content) VALUES ('after', 'x')",
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), healthy_stream.next())
        .await
        .unwrap_or_else(|_| {
            panic!("{healthy} was not recreated past the failures: its stream went silent")
        })
        .expect("the healthy stream ended");
}

#[tokio::test]
async fn a_subscriber_whose_view_cannot_be_recreated_is_told_so() {
    let handle = live_database().await;
    let manager = manager(&handle);
    let sql = "SELECT id, extra FROM t2";
    let (view, mut unkeyed) = manager
        .ensure_and_subscribe(sql, None)
        .await
        .expect("subscribe unkeyed");
    let (_, mut keyed) = manager
        .ensure_and_subscribe(sql, Some("one-watch"))
        .await
        .expect("subscribe keyed");
    handle.execute_ddl("DROP TABLE t2").await.expect("drop t2");

    self::manager(&handle)
        .rebuild_watch_views()
        .await
        .expect_err("a listened view over a dropped table cannot be recreated");

    for (who, stream) in [("unkeyed", &mut unkeyed), ("keyed", &mut keyed)] {
        let batch = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("the {who} subscriber of {view} was never told"))
            .expect("the stream ended");
        let note = batch
            .metadata
            .degraded
            .unwrap_or_else(|| panic!("the {who} subscriber got a batch with no disclosure"));
        assert!(
            note.contains(&view),
            "the {who} subscriber's disclosure must name {view}: {note}"
        );
    }
}
