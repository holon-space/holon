//! The generic remote-list reconciler: `(local rows, one complete snapshot)`
//! into intents.
//!
//! Holon is a peer here, never the master. Where the peer issues no row id,
//! identity is whatever content the sidecar's `key` expression names, and
//! absence inside a COMPLETE fetch is the only deletion signal there is. Both
//! consequences are the same for every list; what differs between two peers is
//! declared, not written.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context as _;
use anyhow::Result;
use chrono::DateTime;
use chrono::FixedOffset;
use holon_api::Value;

use crate::snapshot::ListSnapshot;
use crate::snapshot::LocalRow;
use crate::snapshot::RemoteRow;
use crate::snapshot::key_of_local;
use crate::spec::CompiledListSync;
use crate::spec::RowKey;

/// A change to apply to the local mirror rows.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalIntent {
    Insert {
        id: String,
        columns: BTreeMap<String, Value>,
    },
    /// The peer's value for a column of a row we already hold.
    SetColumn {
        id: String,
        column: String,
        value: Value,
    },
    /// Move the watermark forward for a row this snapshot still carried.
    TouchWatermark { id: String, at: String },
    /// The key is gone from a complete snapshot: an authoritative deletion.
    Delete { id: String },
    /// The peer no longer carries a key we had tombstoned, so the tombstone has
    /// done its job and the row can go.
    ReapTombstone { id: String },
}

/// A change the peer needs, delivered as one command of a commit batch.
#[derive(Debug, Clone, PartialEq)]
pub enum PushIntent {
    Add {
        key: RowKey,
        row: LocalRow,
    },
    /// The local row travels with the key: a peer's `request` mapping is what
    /// decides how much of it the removal command needs, and a key alone leaves
    /// it nothing to name the record with.
    Remove {
        key: RowKey,
        row: LocalRow,
    },
}

/// What one reconciliation decided.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconcileOutcome {
    pub local: Vec<LocalIntent>,
    pub push: Vec<PushIntent>,
}

/// Turns `(local rows, one complete remote snapshot)` into intents, under the
/// identity and the merge policy its connection declared.
#[derive(Debug)]
pub struct RemoteListReconciler {
    compiled: std::sync::Arc<CompiledListSync>,
}

impl RemoteListReconciler {
    pub fn new(compiled: std::sync::Arc<CompiledListSync>) -> Self {
        Self { compiled }
    }

    pub fn compiled(&self) -> &CompiledListSync {
        &self.compiled
    }

    pub fn reconcile(
        &self,
        local: &[LocalRow],
        snapshot: &ListSnapshot,
    ) -> Result<ReconcileOutcome> {
        let spec = self.compiled.spec();
        let tombstone_column = self.compiled.tombstone_column();
        let fetched_at = parse_timestamp(snapshot.fetched_at(), "the snapshot's fetch time")?;

        let mut by_key: BTreeMap<RowKey, &LocalRow> = BTreeMap::new();
        for row in local {
            let key = key_of_local(&self.compiled, row)?;
            if let Some(existing) = by_key.insert(key.clone(), row) {
                anyhow::bail!(
                    "the local rows '{}' and '{}' share the key {key}; that key is the row \
                     identity, so a duplicate means the table was written past the reconciler",
                    existing.id,
                    row.id
                );
            }
        }

        let mut outcome = ReconcileOutcome::default();
        let mut seen: BTreeSet<RowKey> = BTreeSet::new();
        for (key, remote) in snapshot.rows() {
            seen.insert(key.clone());
            match by_key.get(key) {
                None => outcome.local.push(LocalIntent::Insert {
                    id: remote.id.clone(),
                    columns: mirrored_columns(&self.compiled, remote, snapshot.fetched_at()),
                }),
                Some(row) => match row.timestamp(tombstone_column) {
                    // A live tombstone means WE deleted it and the peer has not
                    // been told yet; the pull must not undo our own delete.
                    Some(deleted_at)
                        if !self.expired(
                            parse_timestamp(deleted_at, "a local tombstone")?,
                            fetched_at,
                        ) =>
                    {
                        outcome.push.push(PushIntent::Remove {
                            key: key.clone(),
                            row: (*row).clone(),
                        });
                    }
                    // An expired tombstone has outlived its purpose; the peer
                    // still lists the row, so it comes back on the local id.
                    Some(_) => outcome.local.push(LocalIntent::Insert {
                        id: row.id.clone(),
                        columns: mirrored_columns(&self.compiled, remote, snapshot.fetched_at()),
                    }),
                    None => {
                        for column in &spec.merge_columns {
                            let incoming =
                                remote.columns.get(column).cloned().unwrap_or(Value::Null);
                            if !same_value(row.column(column), &incoming) {
                                outcome.local.push(LocalIntent::SetColumn {
                                    id: row.id.clone(),
                                    column: column.clone(),
                                    value: incoming,
                                });
                            }
                        }
                        for column in &spec.latch_columns {
                            if truthy(remote.columns.get(column)) && !truthy(row.column(column)) {
                                outcome.local.push(LocalIntent::SetColumn {
                                    id: row.id.clone(),
                                    column: column.clone(),
                                    value: Value::Integer(1),
                                });
                            }
                        }
                        outcome.local.push(LocalIntent::TouchWatermark {
                            id: row.id.clone(),
                            at: snapshot.fetched_at().to_string(),
                        });
                    }
                },
            }
        }

        for (key, row) in &by_key {
            if seen.contains(key) {
                continue;
            }
            let tombstoned = row.timestamp(tombstone_column).is_some();
            let ever_seen = row.timestamp(&spec.watermark_column).is_some();
            match (tombstoned, ever_seen) {
                (true, _) => outcome
                    .local
                    .push(LocalIntent::ReapTombstone { id: row.id.clone() }),
                // The peer has carried this row before and no longer does.
                // Inside a COMPLETE snapshot that is the deletion signal.
                (false, true) => outcome
                    .local
                    .push(LocalIntent::Delete { id: row.id.clone() }),
                // Never sent by the peer, so its absence says nothing about it.
                (false, false) => outcome.push.push(PushIntent::Add {
                    key: key.clone(),
                    row: (*row).clone(),
                }),
            }
        }

        Ok(outcome)
    }

    fn expired(&self, deleted_at: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> bool {
        now - deleted_at > self.compiled.tombstone_window()
    }
}

/// The remote row as it is written locally: the peer's columns the declared
/// type actually has, plus the watermark this fetch licenses and an empty
/// tombstone.
fn mirrored_columns(
    compiled: &CompiledListSync,
    remote: &RemoteRow,
    fetched_at: &str,
) -> BTreeMap<String, Value> {
    let mut columns: BTreeMap<String, Value> = remote
        .columns
        .iter()
        .filter(|(name, _)| compiled.declares_column(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    columns.insert(
        compiled.spec().watermark_column.clone(),
        Value::String(fetched_at.to_string()),
    );
    columns.insert(compiled.tombstone_column().to_string(), Value::Null);
    columns
}

/// Whether a local column and the peer's value for it say the same thing.
///
/// The peer's JSON writes a count as an integer and the declared `REAL` column
/// reads back as a float, so both spellings of one number compare equal.
/// Without that, every round would rewrite the column and only its watermark
/// would look changed.
fn same_value(local: Option<&Value>, incoming: &Value) -> bool {
    match (local, incoming) {
        (Some(Value::Integer(a)), Value::Float(b)) => (*a as f64) == *b,
        (Some(Value::Float(a)), Value::Integer(b)) => *a == (*b as f64),
        (None, Value::Null) => true,
        (Some(a), b) => a == b,
        (None, _) => false,
    }
}

/// Whether a latch column is set. A latch is stored as `INTEGER` and arrives
/// from a mapping as either a boolean or a number, so both spellings of "on"
/// count and nothing else does.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Boolean(b)) => *b,
        Some(Value::Integer(n)) => *n != 0,
        Some(Value::Float(f)) => *f != 0.0,
        _ => false,
    }
}

fn parse_timestamp(raw: &str, what: &str) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(raw)
        .with_context(|| format!("{what} is not an RFC 3339 timestamp: '{raw}'"))
}
