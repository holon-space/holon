//! One fixture list peer, generic over what a sidecar declares and over the
//! four behavioural axes of [`FixtureProfile`].
//!
//! The peer implements [`holon_connections::RemoteListPeer`] — the SAME port
//! the production connector calls — and the round it serves is therefore the
//! REAL [`holon_connections::sync_once`] over the REAL reconciler, not a
//! reimplementation. A transport is the one thing the production crate cannot
//! own; this crate's job is to stand in for a transport so the round can be
//! driven in-process.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_connections::CommandVerb;
use holon_connections::CommitAck;
use holon_connections::CommitBatch;
use holon_connections::CompiledListSync;
use holon_connections::ListSnapshot;
use holon_connections::LocalIntent;
use holon_connections::LocalRow;
use holon_connections::LocalRowReader;
use holon_connections::RemoteListPeer;
use holon_connections::RemoteRow;
use holon_connections::RowKey;
use holon_core::file_format::TypedRowSet;

use crate::axes::CacheMode;
use crate::axes::ChangeDetection;
use crate::axes::CommitGranularity;
use crate::axes::FixtureProfile;

/// The fetch time a snapshot publishes. Fixed so a round is reproducible from
/// its inputs — the watermark a row carries is this constant, never the wall
/// clock.
pub const FETCHED_AT: &str = "2026-09-15T00:00:00+00:00";

/// One change the caller makes to the peer's list, BEFORE a sync round reads
/// it. The round then reconciles the mutation against the mirror.
#[derive(Debug, Clone, PartialEq)]
pub enum ListMutation {
    /// The peer gains a row (or, under a natural key, replaces the row the key
    /// folds to).
    Add(RemoteRow),
    /// The peer drops the row this key identifies.
    Remove { key: RowKey },
}

/// The peer's list and its version bookkeeping.
#[derive(Debug, Default)]
pub struct FixtureList {
    /// The peer's rows, keyed by the connection's reconciliation key.
    rows: BTreeMap<RowKey, RemoteRow>,
    version: i64,
    commits: usize,
    /// Commands this peer has applied across every commit it was sent.
    applied: usize,
    /// The body a caching transport holds, present only while
    /// [`CacheMode::CachedNeedsBust`] holds. A commit does NOT invalidate it:
    /// only a pull carrying the connection's declared freshness argument reads
    /// past it.
    cache: Option<ListSnapshot>,
}

/// A list peer that applies the commands it is sent and versions itself, so
/// "the peer moved between polls" and "the peer serves a stale body" are
/// scenarios a test can stage.
pub struct FixtureListPeer {
    compiled: Arc<CompiledListSync>,
    profile: FixtureProfile,
    state: Mutex<FixtureList>,
}

impl FixtureListPeer {
    pub fn new(compiled: Arc<CompiledListSync>, profile: FixtureProfile) -> Self {
        Self {
            compiled,
            profile,
            state: Mutex::new(FixtureList::default()),
        }
    }

    /// An empty list whose first pull carries `version`.
    pub fn seeded(
        compiled: Arc<CompiledListSync>,
        profile: FixtureProfile,
        rows: Vec<RemoteRow>,
        version: i64,
    ) -> Self {
        let peer = Self::new(compiled, profile);
        {
            let mut state = peer.state.lock().expect("the fixture list");
            for row in rows {
                let key = peer.key_of_row(&row).expect("a seed row's key");
                state.rows.insert(key, row);
            }
            state.version = version;
        }
        peer
    }

    pub fn profile(&self) -> FixtureProfile {
        self.profile
    }

    /// The peer's current list, as `(key, row)` — the oracle a mirror is judged
    /// against. Bookkeeping columns are absent by construction.
    pub fn rows(&self) -> Vec<(RowKey, RemoteRow)> {
        let state = self.state.lock().expect("the fixture list");
        state
            .rows
            .iter()
            .map(|(k, r)| (k.clone(), r.clone()))
            .collect()
    }

    pub fn version(&self) -> i64 {
        self.state.lock().expect("the fixture list").version
    }

    pub fn commits(&self) -> usize {
        self.state.lock().expect("the fixture list").commits
    }

    /// How many commands the peer has been asked to apply, across every commit
    /// — the round's own push count, read from the remote end of the round
    /// rather than from the round's self-report.
    pub fn commands_applied(&self) -> usize {
        self.state.lock().expect("the fixture list").applied
    }

    /// Apply one remote change and re-version the list. This is the remote half
    /// of "the peer moved"; the round's pull observes it.
    pub fn mutate(&self, mutation: &ListMutation) {
        let mut state = self.state.lock().expect("the fixture list");
        match mutation {
            ListMutation::Add(row) => {
                let key = self
                    .key_of_row(row)
                    .expect("a mutated-in row derives its key");
                state.rows.insert(key, row.clone());
            }
            ListMutation::Remove { key } => {
                state.rows.remove(key);
            }
        }
        state.version += 1;
    }

    /// The key the connection's declared expression derives from a row's
    /// columns. Opaque by design: a server id and a content pair both come back
    /// as one [`RowKey`].
    fn key_of_row(&self, row: &RemoteRow) -> Result<RowKey> {
        let doc = serde_json::to_value(&row.columns)?;
        self.compiled.key_of(&doc)
    }

    /// Build one complete snapshot of the peer's list.
    fn build_snapshot(&self, state: &FixtureList) -> Result<ListSnapshot> {
        let spec = self.compiled.spec();
        let mut cursor = StorageEntity::default();
        cursor.insert("id".into(), Value::String("cursor".into()));
        cursor.insert(
            spec.version_column.clone().into(),
            Value::Integer(state.version),
        );

        let entity_rows: Vec<StorageEntity> = state
            .rows
            .values()
            .map(|row| {
                let mut entity = StorageEntity::default();
                entity.insert("id".into(), Value::String(row.id.clone()));
                for (column, value) in &row.columns {
                    entity.insert(column.as_str().into(), value.clone());
                }
                entity
            })
            .collect();

        let sets = vec![
            TypedRowSet {
                type_name: spec.list_row_type.clone(),
                owner_column: "id".into(),
                owner_value: "cursor".into(),
                rows: vec![cursor],
            },
            TypedRowSet {
                type_name: spec.entity.clone(),
                owner_column: "id".into(),
                owner_value: "list".into(),
                rows: entity_rows,
            },
        ];
        ListSnapshot::from_rows(&self.compiled, &sets, FETCHED_AT)
    }

    /// A command's columns, minus this connection's own watermark and tombstone
    /// bookkeeping. The peer stores only the columns it actually serves; the
    /// watermark the local leg stamps on every insert is not the peer's list.
    fn remote_row_of(&self, command: &holon_connections::CommitCommand) -> RemoteRow {
        let spec = self.compiled.spec();
        let tombstone = self.compiled.tombstone_column().to_string();
        let mut columns = command.columns.clone();
        columns.remove(&spec.watermark_column);
        columns.remove(&tombstone);
        let id = columns
            .get("id")
            .and_then(Value::as_string)
            .unwrap_or_else(|| {
                panic!(
                    "fixture peer: a commit command carries no string `id` column; the \
                     connection's row codec must name one, and a peer that stored an empty id \
                     would hold a row nothing can address"
                )
            })
            .to_string();
        RemoteRow { id, columns }
    }
}

#[async_trait::async_trait]
impl RemoteListPeer for FixtureListPeer {
    async fn pull(&self) -> Result<ListSnapshot> {
        let mut state = self.state.lock().expect("the fixture list");
        if self.profile.cache_mode == CacheMode::Fresh {
            return self.build_snapshot(&state);
        }
        // A caching transport serves the body it holds. The round's only lever
        // on it is the freshness argument the connection DECLARES in its
        // sidecar: a pull carrying it is answered from origin, a pull without
        // it is answered from the cache however old that is. This is what
        // `sync::pull_at_least` guards — a body older than a write the round
        // knows landed would read that write as missing.
        if !self.compiled.spec().cache_buster.is_declared() {
            if let Some(cached) = &state.cache {
                return Ok(cached.clone());
            }
        }
        let snapshot = self.build_snapshot(&state)?;
        state.cache = Some(snapshot.clone());
        Ok(snapshot)
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck> {
        let mut state = self.state.lock().expect("the fixture list");
        if self.profile.change_detection == ChangeDetection::VersionCursor {
            anyhow::ensure!(
                batch.version == state.version,
                "fixture peer (cursor): a commit based on version {} reached a list at version \
                 {}; a stale commit over a list this round has not read would lose whoever else \
                 is editing",
                batch.version,
                state.version
            );
        }

        let mut applied = 0i64;
        for command in &batch.commands {
            match command.verb {
                CommandVerb::Add => {
                    state
                        .rows
                        .insert(command.key.clone(), self.remote_row_of(command));
                }
                CommandVerb::Remove => {
                    state.rows.remove(&command.key);
                }
            }
            applied += 1;
        }
        state.version += match self.profile.commit_granularity {
            CommitGranularity::Batched => i64::from(applied > 0),
            CommitGranularity::PerRow => applied,
        };
        state.commits += 1;
        state.applied += applied as usize;
        // The cache is deliberately NOT dropped here. A commit is an outbound
        // write; the body a transport holds does not change because of it, so
        // the verifying re-pull in a round that expects to see its own write
        // must carry the connection's declared bust to see it.
        Ok(CommitAck {
            version: state.version,
        })
    }
}

/// An in-memory mirror: the local rows a round decides against. The
/// table-driven test and the reconciler PBT use it where no Turso table exists;
/// the keystone reads the real table instead.
#[derive(Clone, Debug, Default)]
pub struct FixtureRows(pub Vec<LocalRow>);

#[async_trait::async_trait]
impl LocalRowReader for FixtureRows {
    async fn load(&self) -> Result<Vec<LocalRow>> {
        Ok(self.0.clone())
    }
}

/// Apply a round's local intents to an in-memory mirror, so the next round can
/// be reconciled against the same snapshot. This is the mirror write the
/// dispatcher performs in production, spelled once here for tests that have no
/// dispatcher.
pub fn apply_local_intents(
    compiled: &CompiledListSync,
    local: &[LocalRow],
    intents: &[LocalIntent],
) -> Result<Vec<LocalRow>> {
    let mut by_id: BTreeMap<String, LocalRow> = local
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect();
    for intent in intents {
        match intent {
            LocalIntent::Insert { id, columns } => {
                let mut columns = columns.clone();
                columns.insert("id".into(), Value::String(id.clone()));
                by_id.insert(
                    id.clone(),
                    LocalRow {
                        id: id.clone(),
                        columns,
                    },
                );
            }
            LocalIntent::SetColumn { id, column, value } => {
                let row = by_id.get_mut(id).expect("a set targets a row that exists");
                row.columns.insert(column.clone(), value.clone());
            }
            LocalIntent::TouchWatermark { id, at } => {
                let row = by_id
                    .get_mut(id)
                    .expect("a touch targets a row that exists");
                row.columns.insert(
                    compiled.spec().watermark_column.clone(),
                    Value::String(at.clone()),
                );
            }
            LocalIntent::Delete { id } | LocalIntent::ReapTombstone { id } => {
                by_id.remove(id);
            }
        }
    }
    Ok(by_id.into_values().collect())
}
