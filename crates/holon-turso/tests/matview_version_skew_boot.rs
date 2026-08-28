//! Contract: opening a database whose PERSISTED matview definitions were
//! written by an EARLIER binary version must still boot.
//!
//! A stored matview whose SELECT no longer type-checks against the current
//! base-table schema (a column the newer schema dropped) is a *degraded* view,
//! not a corrupt database: the engine already has the `incompatible_views`
//! channel for exactly this, and Holon's `reconcile_named_view` DROP+CREATEs
//! any view whose stored SQL differs from the module's canonical SQL — but it
//! only ever runs if `Database::open_file_with_flags` returns.
//!
//! It does not. `populate_materialized_views` classifies the *dependents* of
//! the failed view as a circular dependency and returns a fatal error, because
//! it tests dependency with a raw substring match (`view.sql.contains(other)`)
//! — every view selecting `FROM block` "references" the pending name `block`.
//! Boot dies with "possible circular dependency" over an acyclic graph, and no
//! reconciliation can run. See bugfunnel entry
//! `2026-08-28-matview-version-skew-false-cycle-boot-fail`.
//!
//! This is the shape of Martin's production database (three real dependents of
//! a `block` view stored with a dropped `depth` column). The DDL here is the
//! extracted shape, not a checked-in database blob.

use std::sync::Arc;

use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

/// Writes a database in the OLD schema shape: `block_raw` still has `depth`,
/// and the stored `block` matview selects it. Then drops the column, which is
/// what the newer binary's schema module does — leaving the persisted view
/// definition behind, exactly as the production database carries it.
async fn write_old_shape_db(path: &std::path::Path) {
    let db = TursoBackend::open_database(path).expect("open for seeding");
    // Not leaked: the file is edited on disk afterwards, so every connection to
    // it must be gone or the surviving one writes its own schema back out.
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, depth INTEGER)")
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW block AS SELECT b.id, b.parent_id, b.depth FROM block_raw b",
        )
        .await
        .expect("create block");
    // The three dependents that appear in the production failure. None of them
    // references another dependent — the graph is a fan-out, never a cycle.
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block_with_path AS SELECT id, parent_id FROM block")
        .await
        .expect("create block_with_path");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block_requirement_edges AS SELECT id FROM block")
        .await
        .expect("create block_requirement_edges");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW watch_view_896c82d172bdae55 AS SELECT * FROM block")
        .await
        .expect("create watch_view");

    handle.shutdown().await.expect("shutdown seeding actor");
    drop(handle);
    drop(backend);
}

/// Applies the version skew at rest, which is the only way to reach the state
/// the production database is in: `ALTER TABLE` refuses to touch a table with
/// dependent matviews, so no DDL sequence can produce a stored `block` whose
/// SELECT no longer type-checks. The bytes get there anyway — Martin's WAL
/// shows the schema page carrying a `block` definition that its own
/// `block_raw` cannot satisfy.
///
/// Renaming the column rather than deleting it keeps every record byte-length
/// identical, so the b-tree stays valid without re-encoding it.
fn rename_base_column_at_rest(path: &std::path::Path) {
    let mut bytes = fold_wal_into_db(path);
    let needle = b"depth INTEGER";
    let mut hits = 0usize;
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            bytes[i..i + needle.len()].copy_from_slice(b"xepth INTEGER");
            hits += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    assert_eq!(hits, 1, "expected exactly one base-table DDL to patch");
    std::fs::write(path, bytes).expect("write patched db file");
}

/// Checkpoints by hand. The engine exposes no checkpoint call, and patching WAL
/// frames in place invalidates their checksums so recovery silently discards
/// them — which passes this test for the wrong reason. Applying the frames up
/// to the last commit and removing the WAL leaves one authoritative file to
/// patch.
fn fold_wal_into_db(path: &std::path::Path) -> Vec<u8> {
    let mut db = std::fs::read(path).expect("read db file");
    let wal_path = path.with_extension("db-wal");
    let Ok(wal) = std::fs::read(&wal_path) else {
        return db;
    };
    let page_size = u32::from_be_bytes(wal[8..12].try_into().unwrap()) as usize;
    let frame = 24 + page_size;
    let frames = (wal.len() - 32) / frame;
    let commit_of = |i: usize| {
        let off = 32 + i * frame;
        u32::from_be_bytes(wal[off + 4..off + 8].try_into().unwrap()) != 0
    };
    let last_commit = (0..frames).filter(|&i| commit_of(i)).next_back();
    let Some(last_commit) = last_commit else {
        return db;
    };
    for i in 0..=last_commit {
        let off = 32 + i * frame;
        let pgno = u32::from_be_bytes(wal[off..off + 4].try_into().unwrap()) as usize;
        db.resize(db.len().max(pgno * page_size), 0);
        db[(pgno - 1) * page_size..pgno * page_size]
            .copy_from_slice(&wal[off + 24..off + 24 + page_size]);
    }
    std::fs::remove_file(&wal_path).expect("remove folded WAL");
    db
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_survives_matview_definition_written_by_older_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("skew.db");

    write_old_shape_db(&path).await;
    rename_base_column_at_rest(&path);

    // The whole contract: reopening must return. A view whose stored SELECT no
    // longer type-checks is degraded, and so are its dependents — none of that
    // is a circular dependency, and none of it may take the database down.
    let reopened = TursoBackend::open_database(&path);
    let err = match reopened {
        Ok(db) => {
            assert_skew_survived(&db).await;
            return;
        }
        Err(e) => e.to_string(),
    };
    assert!(
        !err.contains("circular dependency"),
        "boot reported a circular dependency over an acyclic fan-out (block -> three independent \
         dependents); the stale `block` definition must degrade to an incompatible view instead: \
         {err}"
    );
    panic!("reopening a version-skewed database failed: {err}");
}

/// A successful open only proves the contract if the skew is still in the file.
/// Without this, a hand-checkpoint that dropped the schema would read as a
/// pass.
async fn assert_skew_survived(db: &Arc<turso_core::Database>) {
    let (backend, handle) =
        TursoBackend::new(db.clone(), broadcast::channel(64).0).expect("backend");
    let rows = handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE name IN ('block_raw', 'block')",
            std::collections::HashMap::new(),
        )
        .await
        .expect("read schema");
    let sql: String = rows
        .iter()
        .filter_map(|r| r.get("sql"))
        .map(|v| format!("{v:?}"))
        .collect();
    assert!(
        sql.contains("xepth INTEGER"),
        "the renamed base column is gone — the skew did not survive: {sql}"
    );
    assert!(
        sql.contains("b.depth"),
        "the stale `block` definition is gone — nothing was skewed: {sql}"
    );
    handle.shutdown().await.expect("shutdown");
    drop(backend);
}

/// Keeps the diagnosis honest: the failure above must be caused by the stale
/// `block` definition alone. With `block` still satisfiable, the identical
/// fan-out of dependents opens fine — so nothing about these three views is
/// inherently circular.
#[tokio::test(flavor = "multi_thread")]
async fn same_fanout_without_skew_opens_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("noskew.db");

    let db = TursoBackend::open_database(&path).expect("open for seeding");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT)")
        .await
        .expect("create block_raw");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block AS SELECT b.id, b.parent_id FROM block_raw b")
        .await
        .expect("create block");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block_with_path AS SELECT id, parent_id FROM block")
        .await
        .expect("create block_with_path");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block_requirement_edges AS SELECT id FROM block")
        .await
        .expect("create block_requirement_edges");
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW watch_view_896c82d172bdae55 AS SELECT * FROM block")
        .await
        .expect("create watch_view");
    handle.shutdown().await.expect("shutdown seeding actor");

    let reopened: Arc<_> = TursoBackend::open_database(&path).expect("reopen unskewed database");
    drop(reopened);
}

/// The second half of the contract: booting is only worth anything if the
/// database then repairs itself. `reconcile_named_view` is what every schema
/// module runs on startup, so the stale `block` must survive a DROP+CREATE with
/// the CURRENT canonical SELECT even though the engine has parked it in
/// `incompatible_views`, and its dependents must come back with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_the_engine_marked_incompatible_is_repaired_by_reconcile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repair.db");

    write_old_shape_db(&path).await;
    rename_base_column_at_rest(&path);

    let db = TursoBackend::open_database(&path).expect("reopen version-skewed database");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    // `depth` is gone from the base table (renamed to `xepth`), so this is the
    // shape a current binary's schema module would declare.
    let repaired = holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "block",
        "SELECT b.id, b.parent_id FROM block_raw b",
    )
    .await
    .expect("reconcile the stale view");
    assert!(repaired, "the stale definition should have been replaced");

    for (name, select) in [
        ("block_with_path", "SELECT id, parent_id FROM block"),
        ("block_requirement_edges", "SELECT id FROM block"),
        ("watch_view_896c82d172bdae55", "SELECT * FROM block"),
    ] {
        holon_turso::matview_manager::reconcile_named_view(&handle, name, select)
            .await
            .unwrap_or_else(|e| panic!("reconcile dependent '{name}': {e:#}"));
    }

    let rows = handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type='view'",
            std::collections::HashMap::new(),
        )
        .await
        .expect("read schema back");
    let schema: String = rows
        .iter()
        .filter_map(|r| r.get("sql"))
        .map(|v| format!("{v:?}"))
        .collect();
    assert!(
        !schema.contains("b.depth"),
        "the stale definition is still on disk: {schema}"
    );

    handle.shutdown().await.expect("shutdown");
    drop(backend);

    // The repair must hold across a restart, with no degraded views left.
    let reopened = TursoBackend::open_database(&path).expect("reopen after repair");
    drop(reopened);
}
