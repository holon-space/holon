//! One complete fetch of a remote list, and the local rows it is reconciled
//! against.

use std::collections::BTreeMap;

use anyhow::Context as _;
use anyhow::Result;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;

use crate::spec::CompiledListSync;
use crate::spec::RowKey;

/// One row of a remote list, as the connection's `response` mapping produced
/// it. The columns are the peer's, renamed by that mapping into the declared
/// type's; nothing here knows either vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRow {
    pub id: String,
    pub columns: BTreeMap<String, Value>,
}

/// One row of the local mirror table.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalRow {
    pub id: String,
    pub columns: BTreeMap<String, Value>,
}

impl LocalRow {
    pub fn column(&self, name: &str) -> Option<&Value> {
        self.columns.get(name)
    }

    /// The RFC 3339 text of a nullable timestamp column, or `None` when the
    /// column is absent or NULL.
    pub fn timestamp(&self, column: &str) -> Option<&str> {
        match self.columns.get(column) {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s),
            _ => None,
        }
    }
}

/// Every row one COMPLETE fetch carried, keyed by the connection's declared
/// key, together with the version envelope that fetch published.
///
/// Only a fetch that succeeded and mapped IN FULL can produce one. That is what
/// licenses the reconciler to read absence as deletion: a truncated, failed or
/// malformed fetch raises in the mapping and never reaches this constructor.
#[derive(Debug, Clone)]
pub struct ListSnapshot {
    rows: BTreeMap<RowKey, RemoteRow>,
    envelope: BTreeMap<String, Value>,
    version: i64,
    fetched_at: String,
}

impl ListSnapshot {
    /// Build one whole-list snapshot from the rows the pull mapping produced.
    pub fn from_rows(
        compiled: &CompiledListSync,
        rows: &[TypedRowSet],
        fetched_at: impl Into<String>,
    ) -> Result<Self> {
        let spec = compiled.spec();
        let envelope = columns_of(one_row(rows, &spec.list_row_type)?);
        let version = match envelope.get(&spec.version_column) {
            Some(Value::Integer(n)) => *n,
            other => anyhow::bail!(
                "the `{}` row's `{}` must be a whole number, got {other:?}; that column is the \
                 cursor a commit is based on and a re-read is compared against",
                spec.list_row_type,
                spec.version_column
            ),
        };

        let mut keyed: BTreeMap<RowKey, RemoteRow> = BTreeMap::new();
        for row in rows_of(rows, &spec.entity) {
            let columns = columns_of(row);
            let id = match columns.get("id") {
                Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
                other => anyhow::bail!(
                    "a `{}` row carries no usable `id` (got {other:?}); the response mapping \
                     derives it, because a row with no id has no local row to be written to",
                    spec.entity
                ),
            };
            check_row_reference(compiled, &id)?;
            let key = compiled.key_of(&json_of(&columns)?)?;
            let remote = RemoteRow { id, columns };
            if let Some(existing) = keyed.insert(key.clone(), remote) {
                anyhow::bail!(
                    "two `{}` rows share the key {key}; under a complete snapshot the pair would \
                     collapse into one row and the survivor's absence would read as a deletion of \
                     the other (first id '{}')",
                    spec.entity,
                    existing.id
                );
            }
        }

        Ok(Self {
            rows: keyed,
            envelope,
            version,
            fetched_at: fetched_at.into(),
        })
    }

    pub fn rows(&self) -> impl Iterator<Item = (&RowKey, &RemoteRow)> {
        self.rows.iter()
    }

    /// The version envelope this fetch published, verbatim. It travels into the
    /// next commit unread: a peer versioning two things separately needs no
    /// Rust that knows there are two.
    pub fn envelope(&self) -> &BTreeMap<String, Value> {
        &self.envelope
    }

    /// The one monotonic number this crate compares.
    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// When this fetch completed, RFC 3339. Becomes the watermark of every row
    /// it carried.
    pub fn fetched_at(&self) -> &str {
        &self.fetched_at
    }
}

/// The single row of `type_name`, or a loud failure. A snapshot needs exactly
/// one version envelope, and both zero and two are a mapping that did not do
/// its job.
fn one_row<'a>(rows: &'a [TypedRowSet], type_name: &'a str) -> Result<&'a StorageEntity> {
    let found: Vec<_> = rows_of(rows, type_name).collect();
    match found.as_slice() {
        [row] => Ok(row),
        other => anyhow::bail!(
            "the mapped stream carries {} `{type_name}` rows, and a snapshot has exactly one",
            other.len()
        ),
    }
}

fn rows_of<'a>(
    rows: &'a [TypedRowSet],
    type_name: &'a str,
) -> impl Iterator<Item = &'a StorageEntity> + 'a {
    let type_name = type_name.to_string();
    rows.iter()
        .filter(move |s| s.type_name == type_name)
        .flat_map(|s| s.rows.iter())
}

fn columns_of(row: &StorageEntity) -> BTreeMap<String, Value> {
    row.iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// Whether `id` addresses a row of the type this connection mirrors.
///
/// A local id is a typed REFERENCE, so its scheme names the entity — not the
/// list the row came from, and not the peer. The dispatcher refuses a reference
/// whose scheme disagrees with the entity it is written to, so a row keyed any
/// other way is one no follow-up operation can reach: the round would decide
/// correctly and every write it decided would fail. Refusing here names the
/// mapping that derived it.
fn check_row_reference(compiled: &CompiledListSync, id: &str) -> Result<()> {
    let uri = holon_api::entity_uri::EntityUri::parse(id).with_context(|| {
        format!(
            "a `{}` row is keyed '{id}', which is not a reference at all; the response mapping \
             derives the id, and it must derive one under the scheme '{}'",
            compiled.spec().entity,
            compiled.entity_scheme()
        )
    })?;
    anyhow::ensure!(
        uri.scheme() == compiled.entity_scheme(),
        "a `{}` row is keyed '{id}', whose scheme '{}' is not the type it mirrors; the id must \
         be a reference under '{}', because a write to this row is dispatched by that scheme and \
         a foreign one is refused as somebody else's reference",
        compiled.spec().entity,
        uri.scheme(),
        compiled.entity_scheme()
    );
    Ok(())
}

/// A column map as the JSON document the key expression reads.
///
/// `Value` serializes untagged, so this is the peer's own JSON shape back
/// again — which is what lets a sidecar author write `.id` or `[.name, .cat]`
/// against the columns they declared rather than against a Rust encoding.
pub(crate) fn json_of(columns: &BTreeMap<String, Value>) -> Result<serde_json::Value> {
    serde_json::to_value(columns).context("rendering a row as the JSON its key is derived from")
}

/// Read the rows of the local mirror table. Read-only by design: the writes go
/// back through the dispatcher as follow-up operations.
#[async_trait::async_trait]
pub trait LocalRowReader: Send + Sync {
    async fn load(&self) -> Result<Vec<LocalRow>>;
}

/// One local row's key, named in the failure so a table written past the
/// reconciler is a diagnosable condition rather than a wrong outcome.
pub(crate) fn key_of_local(compiled: &CompiledListSync, row: &LocalRow) -> Result<RowKey> {
    compiled
        .key_of(&json_of(&row.columns)?)
        .with_context(|| format!("the local row '{}'", row.id))
}
