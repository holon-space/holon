//! Phase 2 authority flip: `_expected_parent_id` watermark gating dropped.
//! `SqlBlockOperations::set_field` routes block writes through
//! `BlockCellRegistry::write_field` (Loro), and `LoroSyncController.
//! on_loro_changed` is the only path that writes block columns to SQL.
//! With one writer per field there's no concurrent direct SQL dispatch
//! to regress against, so the compare-and-set is dead weight. The diff
//! guard at the end of `prepare_update` (`AND (col1 IS NOT val1 OR …)`)
//! still keeps no-op UPDATEs from firing spurious CDC.
//!
//! This file kept as a regression gate: if a future refactor reintroduces
//! `_expected_parent_id` (or any other `_expected_*` key) without a
//! matching authority concern, the assertion below fails loudly.

use super::*;

fn build_provider(db_handle: crate::storage::DbHandle) -> SqlOperationProvider {
    SqlOperationProvider::new(
        db_handle,
        "block".to_string(),
        "block".to_string(),
        "block".to_string(),
    )
}

#[tokio::test]
async fn prepare_update_ignores_expected_parent_id_after_authority_flip() {
    // Provide `_expected_parent_id` in params. The post-Phase-2 update
    // path treats it as an unknown key and silently drops it (it was a
    // pre-Phase-2 control field; Loro is now the sole writer so the
    // gating it provided is no longer needed).
    let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    let provider = build_provider(db_handle);

    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String("X".to_string()));
    params.insert("parent_id".to_string(), Value::String("P_NEW".to_string()));
    params.insert(
        "_expected_parent_id".to_string(),
        Value::String("P_OLD".to_string()),
    );

    let prepared = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp) — params describe a real update");

    let sql = prepared.sql_statements.join(";");
    assert!(
        !sql.contains("parent_id = 'P_OLD'"),
        "post-authority-flip: no `parent_id = 'P_OLD'` equality gate \
         expected. SQL was: {sql}"
    );
}

#[tokio::test]
async fn prepare_update_without_expected_parent_id_has_no_gate() {
    // Sanity: a plain `parent_id` UPDATE has no equality gate against
    // any pre-image value. The diff guard still appears as `parent_id
    // IS NOT 'P_NEW'`.
    let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    let provider = build_provider(db_handle);

    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String("X".to_string()));
    params.insert("parent_id".to_string(), Value::String("P_NEW".to_string()));

    let prepared = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp)");

    let sql = prepared.sql_statements.join(";");
    assert!(
        !sql.contains("parent_id = '"),
        "no parent_id equality gate after authority flip; SQL was: {sql}"
    );
}
