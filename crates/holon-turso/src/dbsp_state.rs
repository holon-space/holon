//! Which DBSP state tables a database holds that no live view of its own
//! circuit version owns. A view owns the tables
//! `{DBSP_TABLE_PREFIX}{version}_{key}` whose key is the view name, or the
//! view name followed by `__n<node>` or `__b<branch>` and a suffix. A view's
//! version is the one in its marker table
//! `{DBSP_METADATA_TABLE_PREFIX}{version}_{view}`.

use std::collections::HashMap;

use holon_api::Value;
use turso_core::schema::DBSP_METADATA_TABLE_PREFIX;
use turso_core::schema::DBSP_TABLE_PREFIX;

use crate::turso::DbHandle;

/// `{version}_{view}` split out of a table name that starts with `prefix`.
fn version_and_view<'a>(prefix: &str, table: &'a str) -> (u32, &'a str) {
    let (version, view) = table
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once('_'))
        .unwrap_or_else(|| {
            panic!("{table} is not a table name of the form {prefix}<version>_<view>")
        });
    let version = version.parse().unwrap_or_else(|e| {
        panic!("{table}: the circuit version {version:?} is not a number: {e}")
    });
    (version, view)
}

fn owns(view: &str, key: &str) -> bool {
    key.strip_prefix(view).is_some_and(|rest| {
        rest.is_empty()
            || ["__n", "__b"].iter().any(|tag| {
                rest.strip_prefix(tag)
                    .is_some_and(|id| id.starts_with(|c: char| c.is_ascii_digit()))
            })
    })
}

/// Whether `table`, a DBSP state table name, belongs to `view`.
pub(crate) fn is_state_of(table: &str, view: &str) -> bool {
    owns(view, version_and_view(DBSP_TABLE_PREFIX, table).1)
}

/// State tables whose view is not in `live_views`, or whose circuit version
/// is not the version of the live view that owns them. `live_views` holds
/// each live materialized view with its version; the engine keeps views of
/// different versions side by side.
pub fn left_behind(state_tables: &[String], live_views: &[(String, u32)]) -> Vec<String> {
    state_tables
        .iter()
        .filter(|table| {
            let (version, key) = version_and_view(DBSP_TABLE_PREFIX, table);
            !live_views
                .iter()
                .any(|(view, current)| owns(view, key) && *current == version)
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
    let tables_named = |prefix: &str| {
        format!("SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE '{prefix}%'")
    };
    let live_views: Vec<(String, u32)> = names(handle, &tables_named(DBSP_METADATA_TABLE_PREFIX))
        .await
        .iter()
        .map(|marker| version_and_view(DBSP_METADATA_TABLE_PREFIX, marker))
        .filter(|(_, view)| views.iter().any(|v| v == view))
        .map(|(version, view)| (view.to_string(), version))
        .collect();
    left_behind(
        &names(handle, &tables_named(DBSP_TABLE_PREFIX)).await,
        &live_views,
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const VIEWS: [&str; 4] = ["block", "block_with_path", "blocks", "a"];
    const SUFFIXES: [&str; 5] = ["", "__n3", "__n12__a4", "__n7__r", "__b1"];

    /// Whether the view is live, its current version, and its state tables as
    /// (suffix index, version).
    type View = (bool, u32, Vec<(usize, u32)>);

    /// One [`View`] per view of [`VIEWS`].
    fn database() -> impl Strategy<Value = Vec<View>> {
        proptest::collection::vec(
            (
                any::<bool>(),
                0u32..4,
                proptest::collection::vec((0..SUFFIXES.len(), 0u32..4), 0..5),
            ),
            VIEWS.len(),
        )
    }

    fn left_behind_is_exactly_dropped_or_other_version(db: &[View]) -> Result<(), TestCaseError> {
        let mut tables = Vec::new();
        let mut expected = Vec::new();
        let mut live = Vec::new();
        for (view, (is_live, current, owned)) in VIEWS.iter().zip(db) {
            if *is_live {
                live.push((view.to_string(), *current));
            }
            for (suffix, version) in owned {
                let table = format!("{DBSP_TABLE_PREFIX}{version}_{view}{}", SUFFIXES[*suffix]);
                if !tables.contains(&table) {
                    if !*is_live || version != current {
                        expected.push(table.clone());
                    }
                    tables.push(table);
                }
            }
        }
        prop_assert_eq!(left_behind(&tables, &live), expected);
        Ok(())
    }

    proptest! {
        #[test]
        fn exactly_the_tables_of_dropped_views_or_of_another_version_are_left_behind(
            db in database()
        ) {
            // A non-recursive view at version 2 beside a recursive one at version 3.
            left_behind_is_exactly_dropped_or_other_version(&[
                (true, 2, vec![(1, 2)]),
                (true, 3, vec![(3, 3), (3, 2)]),
                (false, 0, vec![]),
                (false, 0, vec![]),
            ])?;
            left_behind_is_exactly_dropped_or_other_version(&db)?;
        }
    }
}
