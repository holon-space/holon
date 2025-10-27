//! Reproduce the outbound parent_id stomping race: a stale Loro outbound
//! `update` op whose `before` snapshot showed parent_id=P0 must NOT clobber
//! a fresh local SQL write that has already advanced the row to a different
//! parent. Same shape as Bug #1 for content (`_expected_content` guard);
//! this is its analog for `parent_id`.
//!
//! On `main` (before the fix) `prepare_update` does not consume
//! `_expected_parent_id` and the generated WHERE clause has no
//! `parent_id = '<old>'` gate — the stale UPDATE goes through. After the
//! fix, the WHERE clause includes the gate and the stale UPDATE affects
//! zero rows.

use super::*;

fn build_provider(db_handle: crate::storage::DbHandle) -> SqlOperationProvider {
    SqlOperationProvider::new(
        db_handle,
        "block".to_string(),
        "block".to_string(),
        "block".to_string(),
    )
}

fn params_with_expected_parent(
    id: &str,
    new_parent: &str,
    expected_parent: &str,
) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(id.to_string()));
    params.insert(
        "parent_id".to_string(),
        Value::String(new_parent.to_string()),
    );
    params.insert(
        "_expected_parent_id".to_string(),
        Value::String(expected_parent.to_string()),
    );
    params
}

#[tokio::test]
async fn prepare_update_emits_expected_parent_id_gate() {
    let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    let provider = build_provider(db_handle);

    let params = params_with_expected_parent("X", "P_NEW", "P_OLD");

    let prepared = provider
        .prepare_update(&params)
        .await
        .expect("prepare_update")
        .expect("Some(PreparedOp) — params describe a real update");

    let sql = prepared.sql_statements.join(";");
    assert!(
        sql.contains("parent_id = 'P_OLD'"),
        "expected WHERE clause to gate on parent_id = 'P_OLD' (the Loro \
         snapshot's pre-image), so a stale outbound UPDATE no-ops when SQL \
         has already advanced. SQL was: {sql}"
    );
}

#[tokio::test]
async fn prepare_update_without_expected_parent_id_has_no_gate() {
    // Sanity: when `_expected_parent_id` is absent, no `parent_id = '...'`
    // appears in the WHERE clause beyond the diff guard's `IS NOT` check.
    // This guards against accidentally always-on gating.
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
    // The diff guard adds `parent_id IS NOT 'P_NEW'`. That's fine.
    // The point is there is no equality gate against a pre-image value.
    assert!(
        !sql.contains("parent_id = '"),
        "no parent_id equality gate when _expected_parent_id is absent; \
         SQL was: {sql}"
    );
}
