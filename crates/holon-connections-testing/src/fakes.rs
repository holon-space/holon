//! Two structurally different fakes over the [`FixtureListPeer`] contract.
//!
//! Both reach the same reconciler by declaring their own block; the difference
//! is exactly what a sidecar declares. Nothing here names a product — a
//! reviewer reading only this file cannot tell which list API was first
//! mirrored with it.

use std::sync::Arc;

use holon_api::Value;
use holon_api::entity::TypeDefinition;
use holon_connections::CompiledListSync;
use holon_connections::ListSyncSpec;
use holon_connections::LocalRow;
use holon_connections::RemoteRow;

use crate::axes::CacheMode;
use crate::axes::ChangeDetection;
use crate::axes::CommitGranularity;
use crate::axes::FixtureProfile;
use crate::axes::KeyShape;
use crate::peer::FixtureListPeer;

const FRESH_TOMBSTONE: &str = "2026-09-14T12:00:00+00:00";
const SEEN_WATERMARK: &str = "2026-09-13T00:00:00+00:00";

/// One fake: its compiled connection, its profile, and the vocabulary the
/// fixtures need to build a row for it. The row builders are the one part that
/// differs between the two key shapes; the round itself never looks at them.
pub struct Fake {
    pub label: &'static str,
    pub compiled: Arc<CompiledListSync>,
    pub profile: FixtureProfile,
    pub merge_column: &'static str,
    pub latch_column: &'static str,
    pub tombstone_column: &'static str,
    pub watermark_column: &'static str,
    /// The peer's clean row for logical identity `n` with the given merge and
    /// latch values.
    pub remote_row: fn(u8, i64, bool) -> RemoteRow,
    /// The local mirror row for `n`, with tombstone and seen-watermark flags.
    pub local_row: fn(u8, i64, bool, bool, bool) -> LocalRow,
}

impl Fake {
    /// A fixture peer over this fake, seeded with the rows `rows` at `version`.
    pub fn peer(&self, rows: &[u8], version: i64) -> FixtureListPeer {
        self.peer_with_cache(self.profile.cache_mode, rows, version)
    }

    /// The same peer with ONE axis overridden — how a table varies one axis at
    /// a time over an otherwise identical connection.
    pub fn peer_with_cache(
        &self,
        cache_mode: CacheMode,
        rows: &[u8],
        version: i64,
    ) -> FixtureListPeer {
        let rows: Vec<RemoteRow> = rows
            .iter()
            .map(|n| (self.remote_row)(*n, 1, false))
            .collect();
        let profile = FixtureProfile {
            cache_mode,
            ..self.profile
        };
        FixtureListPeer::seeded(self.compiled.clone(), profile, rows, version)
    }
}

fn compile(connection: &str, type_yaml: &str, spec_yaml: &str) -> Arc<CompiledListSync> {
    let declared: TypeDefinition = serde_yaml::from_str(type_yaml).expect("the declared type");
    let spec: ListSyncSpec = serde_yaml::from_str(spec_yaml).expect("the list_sync block");
    CompiledListSync::compile(connection, spec, &declared).expect("the connection compiles")
}

// ---------------------------------------------------------------------------
// Fake 1 — an id-keyed cursor peer.
// ---------------------------------------------------------------------------

const KEYED_TYPE: &str = r#"
name: keyed_row
default_lifetime: persistent
primary_key: id
fields:
  - { name: id, sql_type: TEXT, primary_key: true }
  - { name: content, sql_type: TEXT }
  - { name: rank, sql_type: REAL, nullable: true }
  - { name: done, sql_type: INTEGER }
  - { name: removed_at, sql_type: TEXT, nullable: true }
  - { name: synced_at, sql_type: TEXT, nullable: true }
soft_delete:
  tombstone_field: removed_at
  retention_days: 7
"#;

const KEYED_LIST_SYNC: &str = r#"
entity: keyed_row
table: keyed_row_raw
pull_tool: pull_list
commit_tool: commit_batch
list_row_type: keyed_cursor
version_column: version
key: .id
watermark_column: synced_at
batch_row_type: keyed_batch
command_row_type: keyed_command
merge_columns: [rank]
latch_columns: [done]
"#;

/// The peer issues the row id, so identity is `.id`; it versions itself with a
/// cursor, applies a commit as one batch, and serves no cached body.
pub fn id_keyed_cursor() -> Fake {
    Fake {
        label: "id-keyed cursor",
        compiled: compile("keyed", KEYED_TYPE, KEYED_LIST_SYNC),
        profile: FixtureProfile {
            key_shape: KeyShape::PeerIssuedId,
            change_detection: ChangeDetection::VersionCursor,
            commit_granularity: CommitGranularity::Batched,
            cache_mode: CacheMode::Fresh,
        },
        merge_column: "rank",
        latch_column: "done",
        tombstone_column: "removed_at",
        watermark_column: "synced_at",
        remote_row: |n, rank, done| RemoteRow {
            id: format!("keyed-row:{n}"),
            columns: id_keyed_columns(n, rank, done),
        },
        local_row: |n, rank, done, tomb, seen| LocalRow {
            id: format!("keyed-row:{n}"),
            columns: id_keyed_local_columns(n, rank, done, tomb, seen),
        },
    }
}

fn id_keyed_columns(n: u8, rank: i64, done: bool) -> std::collections::BTreeMap<String, Value> {
    let mut columns = std::collections::BTreeMap::new();
    columns.insert("id".into(), Value::String(format!("keyed-row:{n}")));
    columns.insert("content".into(), Value::String(format!("label {n}")));
    columns.insert("rank".into(), Value::Integer(rank));
    columns.insert("done".into(), Value::Integer(i64::from(done)));
    columns
}

fn id_keyed_local_columns(
    n: u8,
    rank: i64,
    done: bool,
    tomb: bool,
    seen: bool,
) -> std::collections::BTreeMap<String, Value> {
    let mut columns = id_keyed_columns(n, rank, done);
    columns.insert("rank".into(), Value::Float(rank as f64));
    columns.insert(
        "removed_at".into(),
        if tomb {
            Value::String(FRESH_TOMBSTONE.into())
        } else {
            Value::Null
        },
    );
    columns.insert(
        "synced_at".into(),
        if seen {
            Value::String(SEEN_WATERMARK.into())
        } else {
            Value::Null
        },
    );
    columns
}

// ---------------------------------------------------------------------------
// Fake 2 — a content-keyed snapshot peer with a cache-bust requirement.
// ---------------------------------------------------------------------------

const CONTENT_TYPE: &str = r#"
name: content_row
default_lifetime: persistent
primary_key: id
fields:
  - { name: id, sql_type: TEXT, primary_key: true }
  - { name: label, sql_type: TEXT }
  - { name: bucket, sql_type: TEXT }
  - { name: rank, sql_type: REAL, nullable: true }
  - { name: done, sql_type: INTEGER }
  - { name: removed_at, sql_type: TEXT, nullable: true }
  - { name: synced_at, sql_type: TEXT, nullable: true }
soft_delete:
  tombstone_field: removed_at
  retention_days: 7
"#;

const CONTENT_LIST_SYNC: &str = r#"
entity: content_row
table: content_row_raw
pull_tool: pull_list
commit_tool: commit_batch
list_row_type: content_cursor
version_column: version
key: "[.label, .bucket]"
watermark_column: synced_at
cache_buster: epoch_millis
batch_row_type: content_batch
command_row_type: content_command
merge_columns: [rank]
latch_columns: [done]
"#;

/// The peer issues no row id, so identity is the content pair `[.label,
/// .bucket]`; it serves a full snapshot each round (last-write-wins), applies a
/// commit one command per version bump, and serves a cached body that needs a
/// bust knob.
pub fn content_keyed_snapshot_cache_bust() -> Fake {
    Fake {
        label: "content-keyed snapshot (cache-bust)",
        compiled: compile("content", CONTENT_TYPE, CONTENT_LIST_SYNC),
        profile: FixtureProfile {
            key_shape: KeyShape::NaturalKey,
            change_detection: ChangeDetection::FullSnapshot,
            commit_granularity: CommitGranularity::PerRow,
            cache_mode: CacheMode::CachedNeedsBust,
        },
        merge_column: "rank",
        latch_column: "done",
        tombstone_column: "removed_at",
        watermark_column: "synced_at",
        remote_row: |n, rank, done| RemoteRow {
            id: content_row_id(n),
            columns: content_columns(n, rank, done),
        },
        local_row: |n, rank, done, tomb, seen| LocalRow {
            id: content_row_id(n),
            columns: content_local_columns(n, rank, done, tomb, seen),
        },
    }
}

/// The stored id of a content-keyed row, derived from the content pair the key
/// reads — the same derivation a sidecar's `response` mapping performs.
fn content_row_id(n: u8) -> String {
    format!("content-row:label{n}:bucket")
}

fn content_columns(n: u8, rank: i64, done: bool) -> std::collections::BTreeMap<String, Value> {
    let mut columns = std::collections::BTreeMap::new();
    columns.insert("id".into(), Value::String(content_row_id(n)));
    columns.insert("label".into(), Value::String(format!("label{n}")));
    columns.insert("bucket".into(), Value::String("bucket".into()));
    columns.insert("rank".into(), Value::Integer(rank));
    columns.insert("done".into(), Value::Integer(i64::from(done)));
    columns
}

fn content_local_columns(
    n: u8,
    rank: i64,
    done: bool,
    tomb: bool,
    seen: bool,
) -> std::collections::BTreeMap<String, Value> {
    let mut columns = content_columns(n, rank, done);
    columns.insert("rank".into(), Value::Float(rank as f64));
    columns.insert(
        "removed_at".into(),
        if tomb {
            Value::String(FRESH_TOMBSTONE.into())
        } else {
            Value::Null
        },
    );
    columns.insert(
        "synced_at".into(),
        if seen {
            Value::String(SEEN_WATERMARK.into())
        } else {
            Value::Null
        },
    );
    columns
}

/// Both fakes, in a stable order — the table a scenario runs against.
pub fn fakes() -> Vec<Fake> {
    vec![id_keyed_cursor(), content_keyed_snapshot_cache_bust()]
}
