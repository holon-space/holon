//! Smoke test for the C2 automation-journal matview (INC 2 DONE gate — MCP
//! `list_tables` visibility): `AutomationsJournalSchemaModule` creates the
//! `automations_journal` matview over `block_history`, it is visible in
//! `sqlite_master` (what `list_tables` reads), and history rows project into it
//! grouped by `(origin, transition_id, day)` with per-group counts (ADR 0024
//! P8).

use std::collections::HashMap;

use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_turso::schema_modules::AutomationsJournalSchemaModule;
use holon_turso::schema_modules::HistorySchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

#[tokio::test(flavor = "multi_thread")]
async fn automations_journal_matview_is_listed_and_projects_grouped_counts() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("automations_journal.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    HistorySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_history schema");
    AutomationsJournalSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("automations_journal matview");

    // MCP `list_tables` reads sqlite_master — the matview must be visible at boot.
    let listed = handle
        .query(
            "SELECT name FROM sqlite_master WHERE name = 'automations_journal'",
            HashMap::new(),
        )
        .await
        .expect("sqlite_master query");
    assert_eq!(
        listed.len(),
        1,
        "automations_journal must be visible in list_tables (sqlite_master): {listed:?}"
    );

    // Two rule effects in one (origin, transition, day) group + one in another.
    for (seq, transition, day) in [
        (1, "delegate-work", "2026-07-16"),
        (2, "delegate-work", "2026-07-16"),
        (3, "delegate-work", "2026-07-17"),
    ] {
        handle
            .execute(
                "INSERT INTO block_history (seq, entity_name, block_id, op_name, origin, \
                 transition_id, at_millis, day, op_group) VALUES (?, 'block', ?, 'set_field', \
                 'rule', ?, ?, ?, ?)",
                vec![
                    turso::Value::Integer(seq),
                    turso::Value::Text(format!("blk-{seq}")),
                    turso::Value::Text(transition.into()),
                    turso::Value::Integer(seq * 1000),
                    turso::Value::Text(day.into()),
                    turso::Value::Integer(seq),
                ],
            )
            .await
            .expect("history insert");
    }

    let rows = handle
        .query(
            "SELECT origin, transition_id, day, effect_count FROM automations_journal ORDER BY day",
            HashMap::new(),
        )
        .await
        .expect("journal query");
    let counts: Vec<(String, i64)> = rows
        .iter()
        .map(|r| {
            let day = r.get("day").unwrap().as_string().unwrap().to_string();
            let count = match r.get("effect_count") {
                Some(holon_api::Value::Integer(i)) => *i,
                other => panic!("effect_count not an integer: {other:?}"),
            };
            (day, count)
        })
        .collect();
    assert_eq!(
        counts,
        vec![("2026-07-16".to_string(), 2), ("2026-07-17".to_string(), 1)],
        "effects group by (origin, transition_id, day) with per-group counts",
    );
}
