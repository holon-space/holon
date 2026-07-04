//! Contract: the watch/DDL path always completes *boundedly* — it must never
//! hang the DB actor.
//!
//! Clicking a page drives `render_entity` → `MatviewManager::watch` →
//! `ensure_view` → `CREATE MATERIALIZED VIEW watch_view_<hash> AS <sql>`. When
//! that view selects FROM another matview, Turso IVM (matview-on-matview is
//! unsupported) can hang the `CREATE` forever. Because the Turso actor runs
//! commands sequentially, an unbounded DDL await parks the whole actor and
//! every later query hangs — the app freezes with no error.
//!
//! The guard in `handle_ddl` bounds each DDL await (`HOLON_DDL_TIMEOUT_MS`
//! override for tests) and returns a loud `Err` on timeout. This test builds
//! the chained-matview hang scenario (mirroring
//! `crates/holon/examples/turso_ivm_chained_matview_stale_rows.rs`) and asserts
//! the watch path returns *within* the bound — either `Ok` (this Turso build
//! creates the chained view fast) or `Err` (the guard fired). Either way the
//! contract "watch DDL always completes boundedly" holds. A hang would surface
//! as the outer test-side timeout elapsing.

use std::sync::Arc;
use std::time::Duration;

use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::TursoBackend;

#[tokio::test]
async fn watch_ddl_on_chained_matview_never_hangs() {
    // Short guard so a real hang is caught quickly; the outer assertion below
    // uses ~2× as a generous test-side ceiling.
    const GUARD_MS: u64 = 800;
    unsafe {
        std::env::set_var("HOLON_DDL_TIMEOUT_MS", GUARD_MS.to_string());
    }

    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // Leak the backend so the actor stays alive for the test duration.
    std::mem::forget(_backend);

    // Base schema + MV-A, mirroring the repro example.
    handle
        .execute_ddl(
            "CREATE TABLE items (\
                id TEXT PRIMARY KEY, \
                parent_id TEXT NOT NULL, \
                content TEXT DEFAULT '')",
        )
        .await
        .expect("create items");
    handle
        .execute_ddl(
            "CREATE TABLE navigation_history (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                region TEXT NOT NULL, \
                block_id TEXT)",
        )
        .await
        .expect("create navigation_history");
    handle
        .execute_ddl(
            "CREATE TABLE navigation_cursor (\
                region TEXT PRIMARY KEY, \
                history_id INTEGER REFERENCES navigation_history(id))",
        )
        .await
        .expect("create navigation_cursor");

    // MV-A: current_focus (matview over base tables).
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW current_focus AS \
             SELECT nc.region, nh.block_id, nh.id AS hid \
             FROM navigation_cursor nc \
             JOIN navigation_history nh ON nc.history_id = nh.id",
        )
        .await
        .expect("create MV-A current_focus");

    // MV-B chained on MV-A — this is the shape that can hang Turso IVM. Drive it
    // through the real watch path (MatviewManager::watch → ensure_view →
    // execute_ddl_with_deps), exactly as render_entity does in prod.
    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let chained_sql = "SELECT cf.region, cf.block_id, i.id AS root_id \
         FROM current_focus AS cf \
         JOIN items AS i ON i.parent_id = cf.block_id";

    // Bounded-completion contract: watch() must resolve (Ok or Err) well within
    // the outer ceiling. A hang is a test failure via elapsed timeout.
    let ceiling = Duration::from_millis(GUARD_MS * 3);
    let outcome = tokio::time::timeout(ceiling, mgr.watch(chained_sql)).await;

    assert!(
        outcome.is_ok(),
        "watch DDL on a chained matview HUNG past {:?} — the actor freeze guard \
         did not fire. This is the app-freeze bug.",
        ceiling
    );

    match outcome.unwrap() {
        Ok(_) => {
            // This Turso build created the chained view fast — contract holds.
        }
        Err(e) => {
            // Guard fired (or Turso rejected it). Contract holds: bounded, loud.
            let msg = format!("{e:?}");
            assert!(
                !msg.is_empty(),
                "watch returned an empty error; expected an enriched message"
            );
        }
    }
}
