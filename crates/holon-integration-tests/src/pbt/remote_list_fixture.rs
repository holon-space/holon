//! The composed keystone's remote-list fixture: ONE generic connection whose
//! entity, sidecar block and row codec are the single source of truth for both
//! the SUT component and the oracle.
//!
//! A reviewer reading only this file cannot tell which product the generic
//! reconciler was first used with. The vocabulary is the domain's own: peer,
//! list, row, key, watermark, batch. The connection is a content-keyed one
//! (identity is `[.label, .bucket]`, the peer issues no id) with a cache-bust
//! requirement — the harder of the two shapes the reconciler declares for.

use holon_api::entity::FieldSchema;
use holon_api::entity::HomeProfileId;
use holon_api::entity::SoftDelete;
use holon_api::entity::TypeDefinition;
use holon_connections::CacheBuster;
use holon_connections::ListSyncSpec;

pub const ENTITY: &str = "content_row";
pub const TABLE: &str = "content_row_raw";
pub const LIST_ROW_TYPE: &str = "content_cursor";
pub const VERSION_COLUMN: &str = "version";
pub const WATERMARK_COLUMN: &str = "synced_at";
pub const TOMBSTONE_COLUMN: &str = "removed_at";
pub const MERGE_COLUMN: &str = "rank";
pub const LATCH_COLUMN: &str = "done";

/// The peer-authoritative columns, in the invariant's projection order: the id
/// first, then the columns the peer serves (the bookkeeping watermark and
/// tombstone are absent by construction).
pub const PROJECTION: &[&str] = &["id", "label", "bucket", "rank", "done"];

/// The identity pair the key expression reads — the content a row is identified
/// by.
pub const IDENTITY_COLUMNS: &[&str] = &["label", "bucket"];

pub fn type_definition() -> TypeDefinition {
    let mut fields = vec![
        FieldSchema::new("id", "TEXT").primary_key(),
        FieldSchema::new("label", "TEXT"),
        FieldSchema::new("bucket", "TEXT"),
        FieldSchema::new("rank", "REAL").nullable(),
        FieldSchema::new("done", "INTEGER"),
        FieldSchema::new(TOMBSTONE_COLUMN, "TEXT").nullable(),
        FieldSchema::new(WATERMARK_COLUMN, "TEXT").nullable(),
    ];
    // The engine stamps `_provenance` into the overflow bag on every create, so
    // a type without the pair cannot be declared at all (same requirement as a
    // production yaml declaration).
    fields.extend(FieldSchema::overflow_pair());
    let mut def = TypeDefinition::new(ENTITY, fields);
    def.home = Some(HomeProfileId::parse("holon-native").expect("a well-formed profile id"));
    def.soft_delete = Some(SoftDelete {
        tombstone_field: TOMBSTONE_COLUMN.to_string(),
        retention_days: 7,
    });
    def
}

pub fn list_sync_spec() -> ListSyncSpec {
    ListSyncSpec {
        entity: ENTITY.to_string(),
        table: TABLE.to_string(),
        pull_tool: "pull_list".to_string(),
        commit_tool: "commit_batch".to_string(),
        list_row_type: LIST_ROW_TYPE.to_string(),
        version_column: VERSION_COLUMN.to_string(),
        key: "[.label, .bucket]".to_string(),
        watermark_column: WATERMARK_COLUMN.to_string(),
        batch_row_type: "content_batch".to_string(),
        command_row_type: "content_command".to_string(),
        merge_columns: vec![MERGE_COLUMN.to_string()],
        cache_buster: CacheBuster::EpochMillis,
        latch_columns: vec![LATCH_COLUMN.to_string()],
    }
}

/// The id a content-keyed row is stored under, derived from the identity pair
/// the key expression reads — the same derivation a sidecar's `response`
/// mapping performs.
pub fn row_id(label: &str, bucket: &str) -> String {
    format!("content-row:{label}:{bucket}")
}
