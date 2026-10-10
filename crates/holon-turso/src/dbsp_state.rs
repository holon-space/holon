//! Which DBSP state tables a database holds that no live view of the current
//! circuit version owns. Turso's `DROP VIEW` removes only the state table
//! named `{DBSP_TABLE_PREFIX}{DBSP_CIRCUIT_VERSION}_{view}`; anything else is
//! left behind.

use std::collections::HashMap;

use holon_api::Value;
use turso_core::schema::DBSP_TABLE_PREFIX;

use crate::turso::DbHandle;

/// `{version}_{view}` split out of a state table name.
fn version_and_view(table: &str) -> (&str, &str) {
    table
        .strip_prefix(DBSP_TABLE_PREFIX)
        .and_then(|rest| rest.split_once('_'))
        .unwrap_or_else(|| panic!("{table} is not a DBSP state table name"))
}

/// State tables whose view is not in `live_views`, or whose circuit version
/// is not the newest one a live view carries. The current version is read off
/// the tables because turso keeps the constant private; a stale table is
/// always older than the current one.
pub fn left_behind(state_tables: &[String], live_views: &[String]) -> Vec<String> {
    let current = state_tables
        .iter()
        .map(|table| version_and_view(table))
        .filter(|(_, view)| live_views.iter().any(|v| v == view))
        .map(|(version, _)| version)
        .max_by_key(|version| version.parse::<u32>().expect("a numeric circuit version"));
    state_tables
        .iter()
        .filter(|table| {
            let (version, view) = version_and_view(table);
            !live_views.iter().any(|v| v == view) || Some(version) != current
        })
        .cloned()
        .collect()
}

async fn names(handle: &DbHandle, sql: &str) -> Vec<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|row| match row.get("name") {
            Some(Value::String(name)) => name.clone(),
            other => panic!("{sql}: name is {other:?}"),
        })
        .collect()
}

/// [`left_behind`] over what `handle`'s database holds right now.
pub async fn left_behind_in(handle: &DbHandle) -> Vec<String> {
    let views = names(handle, "SELECT name FROM sqlite_schema WHERE type = 'view'").await;
    let tables = names(
        handle,
        &format!(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE '{DBSP_TABLE_PREFIX}%'"
        ),
    )
    .await;
    left_behind(&tables, &views)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_state_table_of_a_live_view_at_the_current_version_is_not_left_behind() {
        let tables = s(&[
            "__turso_internal_dbsp_state_v1_a",
            "__turso_internal_dbsp_state_v1_b",
        ]);
        assert!(left_behind(&tables, &s(&["a", "b"])).is_empty());
    }

    #[test]
    fn a_state_table_of_a_dropped_view_is_left_behind() {
        let tables = s(&[
            "__turso_internal_dbsp_state_v1_a",
            "__turso_internal_dbsp_state_v1_gone",
        ]);
        assert_eq!(
            left_behind(&tables, &s(&["a"])),
            s(&["__turso_internal_dbsp_state_v1_gone"])
        );
    }

    #[test]
    fn a_state_table_of_a_live_view_at_another_version_is_left_behind() {
        let tables = s(&[
            "__turso_internal_dbsp_state_v1_a",
            "__turso_internal_dbsp_state_v0_a",
        ]);
        assert_eq!(
            left_behind(&tables, &s(&["a"])),
            s(&["__turso_internal_dbsp_state_v0_a"])
        );
    }
}
