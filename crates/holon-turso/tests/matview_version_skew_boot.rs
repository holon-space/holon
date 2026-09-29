//! Contract: a database whose PERSISTED matview definitions this binary cannot
//! load is deleted and rebuilt at open (Martin: no migration; the org files
//! rebuild the derived database).
//!
//! A stored matview whose SELECT no longer type-checks against the current
//! base-table schema (a column the newer schema dropped) lands in the engine's
//! `incompatible_views`, and the engine then refuses every write to the tables
//! that feed it. Boot writes to those tables before any schema module could
//! replace the view, so the database cannot be used as it is.
//!
//! This is the shape of Martin's production database (three real dependents of
//! a `block` view stored with a dropped `depth` column; bugfunnel entry
//! `2026-08-28-matview-version-skew-false-cycle-boot-fail`). The DDL here is
//! the extracted shape, not a checked-in database blob.

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
    let last_commit = (0..frames).rfind(|&i| commit_of(i));
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
async fn a_database_with_a_view_this_binary_cannot_load_is_rebuilt_at_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("skew.db");

    write_old_shape_db(&path).await;
    rename_base_column_at_rest(&path);
    let on_disk = std::fs::read(&path).expect("read skewed file");
    assert!(
        on_disk.windows(b"b.depth".len()).any(|w| w == b"b.depth"),
        "the stale `block` definition must be in the file before the open"
    );

    let db = TursoBackend::open_database(&path).unwrap_or_else(|e| {
        panic!("opening a database with an unusable view must rebuild it, not fail: {e}")
    });
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    let rows = handle
        .query(
            "SELECT name FROM sqlite_master WHERE name IN ('block_raw', 'block', \
             'block_with_path', 'block_requirement_edges', 'watch_view_896c82d172bdae55')",
            std::collections::HashMap::new(),
        )
        .await
        .expect("read schema");
    let names: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("name"))
        .map(|v| format!("{v:?}"))
        .collect();
    assert!(
        names.is_empty(),
        "the database must be rebuilt from scratch, but it still holds {names:?}"
    );
    handle.shutdown().await.expect("shutdown");
    drop(backend);
}

/// The rebuild above must be caused by the stale `block` definition alone:
/// with `block` still satisfiable, the identical fan-out of dependents opens
/// and keeps every object.
#[tokio::test(flavor = "multi_thread")]
async fn same_fanout_without_skew_opens_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("noskew.db");

    let db = TursoBackend::open_database(&path).expect("open for seeding");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
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
    drop(handle);
    drop(backend);

    let reopened: Arc<_> = TursoBackend::open_database(&path).expect("reopen unskewed database");
    let (backend, handle) = TursoBackend::new(reopened, broadcast::channel(64).0).expect("backend");
    let rows = handle
        .query(
            "SELECT name FROM sqlite_master WHERE name IN ('block_raw', 'block', \
             'block_with_path', 'block_requirement_edges', 'watch_view_896c82d172bdae55')",
            std::collections::HashMap::new(),
        )
        .await
        .expect("read schema");
    assert_eq!(
        rows.len(),
        5,
        "a usable database must keep every object, but it holds only {rows:?}"
    );
    handle.shutdown().await.expect("shutdown");
    drop(backend);
}
