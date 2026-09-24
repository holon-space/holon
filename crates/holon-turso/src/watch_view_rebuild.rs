//! Rebuilding every watch view in one actor turn, and the correction each
//! rebuilt view's live subscribers are sent.
//!
//! `CREATE MATERIALIZED VIEW` populates without emitting CDC, so a subscriber
//! of a rebuilt view learns how it differs from the old one only through
//! [`resync_changes`].

use std::collections::BTreeMap;

use holon_api::Change;
use holon_api::Value;
use holon_core::storage::StorageEntity;

use crate::turso::RowChange;
use crate::turso::extract_change_origin_from_data;
use crate::turso::take_watch_key;

/// Whether a live subscriber listens to a view, with the SQL the view was
/// ensured from when that was recorded.
pub enum Listened {
    Unlistened,
    Listened { sql: Option<String> },
}

/// Asked of each view, before its drop and again after it.
pub type ListenedTo = std::sync::Arc<dyn Fn(&str) -> Listened + Send + Sync>;

/// What `DbHandle::rebuild_watch_view` did with one view.
#[derive(Debug)]
pub enum ViewRebuild {
    Dropped,
    Recreated { sql: String },
}

/// The changes that take a subscriber holding `before` of `relation` to
/// `after`. Rows are identified the way live CDC identifies them: by watch
/// key and `id`, or `_rowid` for a row without one.
pub(crate) fn resync_changes(
    relation: &str,
    before: Vec<StorageEntity>,
    after: Vec<StorageEntity>,
) -> Result<Vec<RowChange>, String> {
    let mut before = keyed(relation, "before the rebuild", before)?;
    let after = keyed(relation, "after the rebuild", after)?;
    let mut changes = Vec::new();
    for (key, (watch_key, data)) in after {
        let change = match before.remove(&key) {
            None => Change::Created {
                origin: extract_change_origin_from_data(&data),
                data,
            },
            Some((_, held)) if held == data => continue,
            Some(_) => Change::Updated {
                id: key.1,
                origin: extract_change_origin_from_data(&data),
                data,
            },
        };
        changes.push(RowChange {
            relation_name: relation.to_string(),
            change,
            watch_key,
        });
    }
    for ((_, id), (watch_key, held)) in before {
        changes.push(RowChange {
            relation_name: relation.to_string(),
            change: Change::Deleted {
                id,
                origin: extract_change_origin_from_data(&held),
            },
            watch_key,
        });
    }
    Ok(changes)
}

type RowKey = (Option<String>, String);

fn keyed(
    relation: &str,
    when: &str,
    rows: Vec<StorageEntity>,
) -> Result<BTreeMap<RowKey, (Option<String>, StorageEntity)>, String> {
    let mut keyed = BTreeMap::new();
    for mut row in rows {
        let rowid = match row.get("_rowid") {
            Some(Value::Integer(n)) => n.to_string(),
            Some(Value::String(s)) => s.clone(),
            other => {
                return Err(format!(
                    "a row of {relation} {when} has no integer _rowid ({other:?})"
                ));
            }
        };
        // Live CDC carries `_rowid` as a string; a row compared or sent in
        // another shape would look changed to every consumer.
        row.insert("_rowid".into(), Value::String(rowid.clone()));
        let watch_key = take_watch_key(&mut row);
        let id = match row.get("id") {
            Some(Value::String(id)) => id.clone(),
            _ => rowid,
        };
        let key = (watch_key.clone(), id);
        if keyed.contains_key(&key) {
            return Err(format!(
                "{relation} {when} holds two rows keyed {key:?}, so its subscribers, which key \
                 rows the same way, cannot be sent an exact correction"
            ));
        }
        keyed.insert(key, (watch_key, row));
    }
    Ok(keyed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, rowid: i64, extra: &str, watch: Option<&str>) -> StorageEntity {
        let mut row = StorageEntity::new();
        row.insert("id".into(), Value::String(id.into()));
        row.insert("_rowid".into(), Value::Integer(rowid));
        row.insert("extra".into(), Value::String(extra.into()));
        if let Some(watch) = watch {
            row.insert(
                crate::turso::WATCH_KEY_COLUMN.into(),
                Value::String(watch.into()),
            );
        }
        row
    }

    fn summary(changes: &[RowChange]) -> Vec<(String, String, Option<String>)> {
        changes
            .iter()
            .map(|change| {
                let (kind, id) = match &change.change {
                    Change::Created { data, .. } => ("created", format!("{:?}", data.get("id"))),
                    Change::Updated { id, .. } => ("updated", id.clone()),
                    Change::Deleted { id, .. } => ("deleted", id.clone()),
                    Change::FieldsChanged { .. } => unreachable!("never emitted"),
                };
                (kind.to_string(), id, change.watch_key.clone())
            })
            .collect()
    }

    #[test]
    fn a_resync_creates_updates_and_deletes_per_watch() {
        let before = vec![
            row("same", 1, "x", Some("w1")),
            row("changed", 2, "x", Some("w1")),
            row("lost", 3, "x", Some("w1")),
            row("same", 4, "x", Some("w2")),
        ];
        let after = vec![
            row("same", 1, "x", Some("w1")),
            row("changed", 2, "y", Some("w1")),
            row("same", 4, "x", Some("w2")),
            row("gained", 5, "x", Some("w2")),
        ];
        let changes = resync_changes("v", before, after).expect("resync");
        assert_eq!(
            summary(&changes),
            vec![
                ("updated".into(), "changed".into(), Some("w1".into())),
                (
                    "created".into(),
                    format!("{:?}", Some(&Value::String("gained".into()))),
                    Some("w2".into())
                ),
                ("deleted".into(), "lost".into(), Some("w1".into())),
            ]
        );
        for change in &changes {
            if let Change::Created { data, .. } | Change::Updated { data, .. } = &change.change {
                assert!(
                    !data.contains_key(crate::turso::WATCH_KEY_COLUMN),
                    "the routing column must not reach a consumer: {data:?}"
                );
                assert!(
                    matches!(data.get("_rowid"), Some(Value::String(_))),
                    "_rowid must travel as text, as live CDC sends it: {data:?}"
                );
            }
        }
    }

    #[test]
    fn a_resync_refuses_rows_it_cannot_tell_apart() {
        let err = resync_changes(
            "v",
            vec![row("dup", 1, "x", None), row("dup", 2, "y", None)],
            vec![],
        )
        .expect_err("two rows keyed the same");
        assert!(err.contains("two rows keyed"), "{err}");
    }
}
