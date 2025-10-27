//! Sidecar-declared derived views go through `reconcile_named_view` — the same
//! code path `finish_integration` uses for the YAML `views:` section. This
//! test creates the shipped `session_status` view shape over a cache-style
//! table and verifies IVM keeps it correct through writes.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// The exact SELECTs shipped in docs/integrations/claude-history.yaml
/// (`views:` entries `session_last_message` + `session_status`).
/// Keep in sync with the YAML. Two chained views because Turso IVM rejects a
/// CASE over aggregate expressions at CREATE; the CASE lives in the second,
/// non-aggregating view over the first.
const SESSION_LAST_MESSAGE_SQL: &str =
    "SELECT 'cc-session:' || session_id AS session_id, MAX(timestamp) AS last_ts, \
     substr(MAX(timestamp || '|' || role), 26) AS last_role FROM cc_message GROUP BY session_id";

const SESSION_STATUS_SQL: &str =
    "SELECT session_id, last_ts, last_role, iif(last_role = 'user', 'working', iif(last_role = \
     'assistant' AND last_ts > strftime('%Y-%m-%dT%H:%M:%S', 'now', '-10 minutes'), \
     'waiting-on-user', 'idle')) AS status FROM cc_session_last_message";

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // Leak the backend so the actor stays alive for the test duration.
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE cc_message (uuid TEXT PRIMARY KEY, session_id TEXT, role TEXT, \
             timestamp TEXT, content TEXT)",
        )
        .await
        .expect("create cache table");
    handle
}

async fn insert(handle: &DbHandle, uuid: &str, sid: &str, role: &str, ts: &str) {
    handle
        .execute(
            "INSERT INTO cc_message VALUES (?, ?, ?, ?, 'x')",
            vec![
                turso::Value::Text(uuid.into()),
                turso::Value::Text(sid.into()),
                turso::Value::Text(role.into()),
                turso::Value::Text(ts.into()),
            ],
        )
        .await
        .expect("insert message");
}

async fn read_status(handle: &DbHandle) -> Vec<(String, String, String)> {
    let rows = handle
        .query(
            "SELECT session_id, last_role, status FROM cc_session_status ORDER BY session_id",
            HashMap::new(),
        )
        .await
        .expect("query view");
    let mut out: Vec<(String, String, String)> = rows
        .iter()
        .map(|r| {
            let s = |k: &str| match r.get(k) {
                Some(holon_api::Value::String(s)) => s.clone(),
                other => panic!("column {k}: unexpected value {other:?}"),
            };
            (s("session_id"), s("last_role"), s("status"))
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn session_status_view_creates_and_tracks_writes() {
    let handle = setup().await;

    // Old session: assistant spoke last, long ago -> idle
    insert(&handle, "m1", "s1", "user", "2020-01-01T10:00:00.000Z").await;
    insert(&handle, "m2", "s1", "assistant", "2020-01-01T10:01:00.000Z").await;
    // Active session: user spoke last -> working
    insert(&handle, "m3", "s2", "user", "2999-01-01T09:00:00.000Z").await;

    let created =
        reconcile_named_view(&handle, "cc_session_last_message", SESSION_LAST_MESSAGE_SQL)
            .await
            .expect("rollup view DDL must succeed — this is the shipped views: SQL");
    assert!(created, "first reconcile must create the rollup view");
    let created = reconcile_named_view(&handle, "cc_session_status", SESSION_STATUS_SQL)
        .await
        .expect("status view DDL must succeed — this is the shipped views: SQL");
    assert!(created, "first reconcile must create the status view");

    assert_eq!(
        read_status(&handle).await,
        vec![
            (
                "cc-session:s1".to_string(),
                "assistant".to_string(),
                "idle".to_string()
            ),
            (
                "cc-session:s2".to_string(),
                "user".to_string(),
                "working".to_string()
            ),
        ]
    );

    // Assistant answers recently (far-future ts > now-10min) -> waiting-on-user
    insert(&handle, "m4", "s2", "assistant", "2999-01-01T09:05:00.000Z").await;
    assert_eq!(
        read_status(&handle).await[1],
        (
            "cc-session:s2".to_string(),
            "assistant".to_string(),
            "waiting-on-user".to_string()
        )
    );

    // New session appears -> new row
    insert(&handle, "m5", "s3", "user", "2999-02-01T00:00:00.000Z").await;
    assert_eq!(read_status(&handle).await.len(), 3);

    // Reconcile again with identical SQL: no-op
    let recreated = reconcile_named_view(&handle, "cc_session_status", SESSION_STATUS_SQL)
        .await
        .expect("reconcile");
    assert!(!recreated, "unchanged SQL must not recreate the view");
}
