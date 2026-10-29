//! A stopped commit listener sees no later commit; a listener beside it keeps
//! seeing every one.

use std::sync::Arc;
use std::sync::Mutex;

use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_listener_sees_no_later_commit_and_the_other_one_does() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = TursoBackend::open_database(dir.path().join("holon.db")).expect("open");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    handle
        .execute_ddl("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
        .await
        .expect("create t");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW t_view AS SELECT id, n FROM t")
        .await
        .expect("create t_view");

    let seen: Arc<Mutex<Vec<(&str, u64)>>> = Arc::default();
    let listen = |who: &'static str| {
        let seen = seen.clone();
        handle.listen_from_snapshot(
            "SELECT id FROM t_view",
            move |event| {
                if event.relation() == "t_view" {
                    seen.lock().unwrap().push((who, event.envelope().commit_id));
                }
            },
            |_| {},
        )
    };
    let kept = listen("kept").await.expect("listen kept");
    let stopped = listen("stopped").await.expect("listen stopped");
    assert_ne!(kept, stopped, "two listeners must get two ids");

    handle
        .execute_ddl("INSERT INTO t (n) VALUES (1)")
        .await
        .expect("first insert");
    handle.stop_listening(stopped).await.expect("stop");
    handle
        .execute_ddl("INSERT INTO t (n) VALUES (2)")
        .await
        .expect("second insert");

    let seen = seen.lock().unwrap().clone();
    let by = |who| seen.iter().filter(|(w, _)| *w == who).count();
    assert_eq!(
        by("kept"),
        2,
        "the kept listener sees both commits: {seen:?}"
    );
    assert_eq!(
        by("stopped"),
        1,
        "the stopped listener sees only the commit before the stop: {seen:?}"
    );
}
