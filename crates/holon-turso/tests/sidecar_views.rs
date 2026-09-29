//! Sidecar-declared derived views go through `reconcile_named_view` — the same
//! code path `finish_integration` uses for the YAML `views:` section. These
//! tests create the shipped view shapes over cache-style tables and verify IVM
//! keeps them correct through writes and through clock ticks: the views read
//! the current time from the `clock` relation, never from `'now'`.

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
     substr(MAX(timestamp || '|' || role), 26) AS last_role, 'minute' AS clock_grain FROM \
     cc_message GROUP BY session_id";

const SESSION_STATUS_SQL: &str = "SELECT m.session_id, m.last_ts, m.last_role, iif(m.last_role = 'user', 'working', \
     iif(m.last_role = 'assistant' AND m.last_ts > strftime('%Y-%m-%dT%H:%M:%S', c.updated_at, \
     '-10 minutes'), 'waiting-on-user', 'idle')) AS status FROM cc_session_last_message m JOIN \
     clock c ON c.grain = m.clock_grain";

/// The `clock` relation with one row for `grain`, whose tick instant is `utc`.
async fn with_clock(handle: &DbHandle, grain: &str, utc: &str) {
    holon_turso::schema_module::SchemaModule::ensure_schema(
        &holon_turso::schema_modules::CoreSchemaModule,
        handle,
    )
    .await
    .expect("core schema with the clock relation");
    handle
        .execute(
            "INSERT INTO clock (grain, today, epoch_day, updated_at) VALUES (?, 'label', 0, ?)",
            vec![
                turso::Value::Text(grain.into()),
                turso::Value::Text(utc.into()),
            ],
        )
        .await
        .expect("seed clock row");
}

/// What the clock scheduler writes when `grain` ticks at the UTC instant `utc`.
async fn tick(handle: &DbHandle, grain: &str, utc: &str) {
    handle
        .execute(
            "UPDATE clock SET epoch_day = epoch_day + 1, updated_at = ? WHERE grain = ?",
            vec![
                turso::Value::Text(utc.into()),
                turso::Value::Text(grain.into()),
            ],
        )
        .await
        .expect("tick the clock");
}

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
    with_clock(&handle, "minute", "2026-09-29T12:00:00+00:00").await;
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

async fn status_of(handle: &DbHandle, session_id: &str) -> String {
    read_status(handle)
        .await
        .into_iter()
        .find(|(id, _, _)| id == session_id)
        .unwrap_or_else(|| panic!("no status row for {session_id}"))
        .2
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

    // Assistant answered at 11:55; at the 12:00 tick that is recent.
    insert(&handle, "m6", "s4", "user", "2026-09-29T11:50:00.000Z").await;
    insert(&handle, "m7", "s4", "assistant", "2026-09-29T11:55:00.000Z").await;
    assert_eq!(status_of(&handle, "cc-session:s4").await, "waiting-on-user");

    // No write touches s4. Ten minutes later the clock alone makes it idle.
    tick(&handle, "minute", "2026-09-29T12:10:00+00:00").await;
    assert_eq!(
        status_of(&handle, "cc-session:s4").await,
        "idle",
        "a session with no new message must go idle once the minute clock passes its \
         10-minute window"
    );

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
/// entries `upcoming_flagged` + `upcoming`). Keep in sync with the YAML. The
/// window is a flag in the SELECT list and the chained view filters the plain
/// column. The column is `end_time` (not `end`) because END is a SQL keyword
/// and the cache-table DDL builder does not quote identifiers.
const GCAL_EVENT_CLOCK_SQL: &str = "SELECT id, calendar_id, summary, start, end_time, all_day, location, status, \
     updated, 'hour' AS clock_grain FROM gcal_event";

const GCAL_UPCOMING_FLAGGED_SQL: &str = "SELECT e.id, e.calendar_id, e.summary, e.start, e.end_time, e.all_day, e.location, \
     e.status, e.updated, iif(e.start >= strftime('%Y-%m-%dT%H:%M:%S', c.updated_at) AND e.start \
     < strftime('%Y-%m-%dT%H:%M:%S', c.updated_at, '+7 days'), 1, 0) AS is_upcoming FROM \
     gcal_event_clock e JOIN clock c ON c.grain = e.clock_grain";

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
    with_clock(&handle, "hour", HOUR_TICK).await;
    handle
}

const HOUR_TICK: &str = "2026-09-29T12:00:00+00:00";

/// Insert an event whose `start` is `start_modifier` away from [`HOUR_TICK`].
async fn insert_event(handle: &DbHandle, id: &str, summary: &str, start_modifier: &str) {
    let sql = format!(
        "INSERT INTO gcal_event (id, summary, start, end_time, all_day, status) \
         VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%S','{HOUR_TICK}','{start_modifier}'), '', 0, \
         'confirmed')"
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

/// gcal_upcoming filters to events starting within now..+7d of the hour clock,
/// and IVM keeps it correct as new events are written and as the clock ticks.
#[tokio::test]
async fn gcal_upcoming_view_creates_and_tracks_writes() {
    let handle = setup_gcal().await;

    // Two inside the window, two outside.
    insert_event(&handle, "e_past", "yesterday", "-1 day").await;
    insert_event(&handle, "e_soon", "in 3 days", "+3 days").await;
    insert_event(&handle, "e_far", "in 30 days", "+30 days").await;

    reconcile_named_view(&handle, "gcal_event_clock", GCAL_EVENT_CLOCK_SQL)
        .await
        .expect("clock-grain view DDL must succeed — shipped views: SQL");
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

    // No write touches the events. Two hours later "tomorrow" has not started,
    // but an event starting in one hour has; and a day later "tomorrow" is past.
    insert_event(&handle, "e_hour", "in 1 hour", "+1 hour").await;
    assert_eq!(read_upcoming(&handle).await.len(), 3);
    tick(&handle, "hour", "2026-09-29T14:00:00+00:00").await;
    assert_eq!(
        read_upcoming(&handle).await,
        vec!["tomorrow".to_string(), "in 3 days".to_string()],
        "an event whose start the hour clock passed must leave the upcoming window"
    );
    tick(&handle, "hour", "2026-10-01T12:00:00+00:00").await;
    assert_eq!(
        read_upcoming(&handle).await,
        vec!["in 3 days".to_string()],
        "the window moves with the clock, with no write to the event"
    );

    // Reconcile again with identical SQL: no-op.
    let recreated = reconcile_named_view(&handle, "gcal_upcoming", GCAL_UPCOMING_SQL)
        .await
        .expect("reconcile");
    assert!(!recreated, "unchanged SQL must not recreate the view");
}

/// The production order: an integration creates its views at connect, and the
/// clock row of their grain appears after that, when the session starts
/// ticking the grain. The views must fill in when the row arrives.
#[tokio::test]
async fn views_created_before_their_clock_row_fill_in_when_the_row_arrives() {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    holon_turso::schema_module::SchemaModule::ensure_schema(
        &holon_turso::schema_modules::CoreSchemaModule,
        &handle,
    )
    .await
    .expect("core schema with the clock relation");
    handle
        .execute_ddl(
            "CREATE TABLE cc_message (uuid TEXT PRIMARY KEY, session_id TEXT, role TEXT, \
             timestamp TEXT, content TEXT)",
        )
        .await
        .expect("create cache table");
    insert(&handle, "m1", "s1", "user", "2026-09-29T11:50:00.000Z").await;
    insert(&handle, "m2", "s1", "assistant", "2026-09-29T11:55:00.000Z").await;

    reconcile_named_view(&handle, "cc_session_last_message", SESSION_LAST_MESSAGE_SQL)
        .await
        .expect("rollup view");
    reconcile_named_view(&handle, "cc_session_status", SESSION_STATUS_SQL)
        .await
        .expect("status view");
    assert!(
        read_status(&handle).await.is_empty(),
        "with no minute row there is no `now` to classify against"
    );

    handle
        .execute(
            "INSERT INTO clock (grain, today, epoch_day, updated_at) VALUES ('minute', 'label', 0, \
             '2026-09-29T12:00:00+00:00')",
            vec![],
        )
        .await
        .expect("seed the minute row after the views exist");
    assert_eq!(status_of(&handle, "cc-session:s1").await, "waiting-on-user");

    tick(&handle, "minute", "2026-09-29T12:10:00+00:00").await;
    assert_eq!(status_of(&handle, "cc-session:s1").await, "idle");
}
