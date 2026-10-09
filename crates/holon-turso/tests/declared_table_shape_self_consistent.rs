//! Contract: a table freshly created from its declaration reads back with the
//! declared shape, so the boot-time shape check finds nothing to adapt or
//! refuse on a database this binary created.

use std::path::Path;

use holon_turso::sql_utils::sql_statements;
use holon_turso::table_shape::TableReconcile;
use holon_turso::table_shape::reconcile_table;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

async fn fresh(path: &Path) -> (TursoBackend, DbHandle) {
    let db = TursoBackend::open_database(path).expect("open");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

fn declared_tables() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sql/schema");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("schema dir")
        .map(|e| e.expect("entry").path())
        .collect();
    files.sort();
    let mut tables = Vec::new();
    for file in files {
        let sql = std::fs::read_to_string(&file).expect("read schema file");
        for statement in sql_statements(&sql) {
            let code: String = statement
                .lines()
                .filter(|l| !l.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join(" ");
            if code
                .trim_start()
                .to_ascii_uppercase()
                .starts_with("CREATE TABLE")
            {
                tables.push((file.display().to_string(), statement.to_string()));
            }
        }
    }
    tables
}

#[tokio::test(flavor = "multi_thread")]
async fn every_declared_table_matches_its_own_fresh_shape() {
    let tables = declared_tables();
    assert!(
        tables.len() > 10,
        "found only {} declarations",
        tables.len()
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let (_b, handle) = fresh(&dir.path().join("fresh.db")).await;
    let mut drifted = Vec::new();
    for (file, create) in &tables {
        let outcome = reconcile_table(&handle, create)
            .await
            .unwrap_or_else(|e| panic!("{file}: {e}\n{create}"));
        if outcome != TableReconcile::Unchanged {
            drifted.push(format!("{file}: {outcome:?}"));
        }
    }
    assert!(drifted.is_empty(), "{drifted:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_generated_type_table_matches_its_own_fresh_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_b, handle) = fresh(&dir.path().join("fresh.db")).await;
    for create in [
        "CREATE TABLE IF NOT EXISTS \"a_raw\" (\n  \"id\" TEXT PRIMARY KEY NOT NULL,\n  \"q\" \
         REAL NOT NULL,\n  \"n\" TEXT DEFAULT 'x'\n)",
        "CREATE TABLE IF NOT EXISTS \"b_raw\" (\n  \"k1\" TEXT NOT NULL,\n  \"k2\" INTEGER NOT \
         NULL,\n  \"v\" TEXT,\n  PRIMARY KEY (\"k1\", \"k2\")\n)",
        "CREATE TABLE IF NOT EXISTS \"c_raw\" (\n  \"id\" TEXT PRIMARY KEY,\n  \"flag\" BOOLEAN \
         NOT NULL DEFAULT 0,\n  \"at\" DATETIME\n)",
    ] {
        assert_eq!(
            reconcile_table(&handle, create).await.expect(create),
            TableReconcile::Unchanged,
            "{create}"
        );
    }
}
