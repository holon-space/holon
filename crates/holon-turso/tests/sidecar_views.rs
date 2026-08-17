//! Sidecar-declared derived views go through `reconcile_named_view` — the same
//! code path `finish_integration` uses for the YAML `views:` section. This
//! test creates the shipped `session_status` view shape over a cache-style
//! table and verifies IVM keeps it correct through writes.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// The exact SELECTs shipped in assets/integrations/claude-history.yaml
/// (`views:` entries `session_last_message` + `session_status`).
/// Keep in sync with the YAML. Two chained views because Turso IVM rejects a
/// CASE over aggregate expressions at CREATE; the CASE lives in the second,
/// non-aggregating view over the first.
const SESSION_LAST_MESSAGE_SQL: &str = "SELECT 'cc-session:' || session_id AS session_id, MAX(timestamp) AS last_ts, \
     substr(MAX(timestamp || '|' || role), 26) AS last_role FROM cc_message GROUP BY session_id";

const SESSION_STATUS_SQL: &str = "SELECT session_id, last_ts, last_role, iif(last_role = 'user', 'working', iif(last_role = \
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

// ---------------------------------------------------------------------------
// Google Calendar `gcal_upcoming` chained views
// ---------------------------------------------------------------------------

/// The exact chained SELECTs shipped in assets/integrations/gcal.yaml (`views:`
/// entries `upcoming_flagged` + `upcoming`). Keep in sync with the YAML. Two
/// chained views because Turso IVM drops every row when strftime('now') sits in
/// a WHERE — but strftime IN THE SELECT projection works. The column is
/// `end_time` (not `end`) because END is a SQL keyword and the cache-table DDL
/// builder does not quote identifiers.
const GCAL_UPCOMING_FLAGGED_SQL: &str = "SELECT id, calendar_id, summary, start, end_time, all_day, location, status, updated, \
     iif(start >= strftime('%Y-%m-%dT%H:%M:%S', 'now') \
         AND start < strftime('%Y-%m-%dT%H:%M:%S', 'now', '+7 days'), 1, 0) AS is_upcoming \
     FROM gcal_event";

const GCAL_UPCOMING_SQL: &str = "SELECT id, calendar_id, summary, start, end_time, all_day, location, status, updated \
     FROM gcal_upcoming_flagged WHERE is_upcoming = 1";

async fn setup_gcal() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE gcal_event (id TEXT PRIMARY KEY, calendar_id TEXT, summary TEXT, \
             start TEXT, end_time TEXT, all_day INTEGER, location TEXT, status TEXT, updated TEXT)",
        )
        .await
        .expect("create gcal_event cache table");
    handle
}

/// Insert an event whose `start` is computed relative to `now` via strftime, so
/// the test anchors deterministically inside/outside the +7d window.
async fn insert_event(handle: &DbHandle, id: &str, summary: &str, start_modifier: &str) {
    let sql = format!(
        "INSERT INTO gcal_event (id, summary, start, end_time, all_day, status) \
         VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%S','now','{start_modifier}'), '', 0, 'confirmed')"
    );
    handle
        .execute(
            &sql,
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(summary.into()),
            ],
        )
        .await
        .expect("insert event");
}

async fn read_upcoming(handle: &DbHandle) -> Vec<String> {
    let rows = handle
        .query(
            "SELECT summary FROM gcal_upcoming ORDER BY start",
            HashMap::new(),
        )
        .await
        .expect("query gcal_upcoming");
    rows.iter()
        .map(|r| match r.get("summary") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("summary: unexpected value {other:?}"),
        })
        .collect()
}

/// gcal_upcoming filters to events starting within now..+7d, and IVM keeps it
/// correct as new events are written.
#[tokio::test]
async fn gcal_upcoming_view_creates_and_tracks_writes() {
    let handle = setup_gcal().await;

    // Two inside the window, two outside.
    insert_event(&handle, "e_past", "yesterday", "-1 day").await;
    insert_event(&handle, "e_soon", "in 3 days", "+3 days").await;
    insert_event(&handle, "e_far", "in 30 days", "+30 days").await;

    let created = reconcile_named_view(&handle, "gcal_upcoming_flagged", GCAL_UPCOMING_FLAGGED_SQL)
        .await
        .expect("flagged view DDL must succeed — shipped views: SQL");
    assert!(created, "first reconcile must create the flagged view");
    let created = reconcile_named_view(&handle, "gcal_upcoming", GCAL_UPCOMING_SQL)
        .await
        .expect("upcoming view DDL must succeed — shipped views: SQL");
    assert!(created, "first reconcile must create the upcoming view");

    assert_eq!(
        read_upcoming(&handle).await,
        vec!["in 3 days".to_string()],
        "only the event starting within now..+7d is upcoming"
    );

    // A newly-synced event inside the window shows up (IVM tracks the write).
    insert_event(&handle, "e_tomorrow", "tomorrow", "+1 day").await;
    assert_eq!(
        read_upcoming(&handle).await,
        vec!["tomorrow".to_string(), "in 3 days".to_string()],
        "IVM must add the new in-window event, ordered by start"
    );

    // Reconcile again with identical SQL: no-op.
    let recreated = reconcile_named_view(&handle, "gcal_upcoming", GCAL_UPCOMING_SQL)
        .await
        .expect("reconcile");
    assert!(!recreated, "unchanged SQL must not recreate the view");
}
