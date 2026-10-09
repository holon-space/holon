//! Dry-run of the boot-time table shape check against a COPY of a real
//! database: prints how every stored table differs from its declaration and
//! what boot would do about it, without creating or changing anything.
//!
//! Run with:
//!   HOLON_SHAPE_DRYRUN_DB=/path/to/copy/holon.db \
//!     cargo test -p holon-app --test real_db_shape_dryrun -- --ignored
//! --nocapture
//!
//! Opening a database may checkpoint its WAL, so point it at a copy of the
//! `.db` + `.db-wal` pair, never at the original.

use std::collections::HashMap;
use std::path::Path;

use holon_turso::sql_utils::sql_statements;
use holon_turso::table_classes::class_of;
use holon_turso::table_shape::DeclaredTable;
use holon_turso::table_shape::ShapeDiff;
use holon_turso::table_shape::TableShape;
use holon_turso::turso::TursoBackend;
use holon_turso::turso_adapter::TursoAdapter;
use tokio::sync::broadcast;

fn hand_written_declarations() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../holon-turso/sql/schema");
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
                tables.push(statement.to_string());
            }
        }
    }
    tables
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs HOLON_SHAPE_DRYRUN_DB pointing at a copy of a real database"]
async fn print_the_shape_diff_of_every_declared_table() {
    let path = std::env::var("HOLON_SHAPE_DRYRUN_DB").expect("HOLON_SHAPE_DRYRUN_DB");
    let db = TursoBackend::open_database(Path::new(&path)).expect("open the copy");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    let registry = holon_profiles::create_default_registry().expect("default registry");
    holon_kitchen::register_kitchen_types(&registry).expect("kitchen types");
    let mut declarations = hand_written_declarations();
    for type_def in registry.all() {
        let raw = TursoAdapter::raw_type_def(&type_def);
        if type_def.name == "block" || type_def.id_references.is_some() || raw.fields.is_empty() {
            continue;
        }
        declarations.push(raw.to_create_table_sql());
    }

    println!("| table | stored | verdict | rows | difference |");
    println!("|---|---|---|---|---|");
    for create in declarations {
        let declared = DeclaredTable::parse(&create).expect("parse declaration");
        let stored = TableShape::stored(&handle, declared.table())
            .await
            .expect("stored shape");
        if stored.columns.is_empty() {
            println!("| {} | absent | created on boot | 0 | |", declared.table());
            continue;
        }
        let diff = ShapeDiff::between(&stored, &declared.shape());
        let rows = handle
            .query(
                &format!("SELECT COUNT(*) AS n FROM \"{}\"", declared.table()),
                HashMap::new(),
            )
            .await
            .expect("row count")
            .first()
            .and_then(|r| r.get("n").cloned());
        let verdict = if diff.is_empty() {
            "unchanged".to_string()
        } else if diff.is_lossless() {
            "columns added".to_string()
        } else {
            match class_of(declared.table(), &HashMap::new()) {
                Some(holon_turso::table_classes::Class::Rebuilt) => "REBUILT".to_string(),
                class => format!("REFUSED ({class:?})"),
            }
        };
        println!(
            "| {} | present | {verdict} | {rows:?} | {} |",
            declared.table(),
            if diff.is_empty() {
                String::new()
            } else {
                diff.to_string()
            }
        );
    }
}
