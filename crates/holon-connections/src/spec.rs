//! What a sidecar declares so one generic reconciler can mirror its list.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use chrono::Duration;
use holon_rows::RowMapper;
use serde::Deserialize;
use serde::Serialize;

/// The `holon.list_sync` block of a sidecar.
///
/// Every field below is a name the PEER chose or a policy the list has, and
/// none of them is compiled into Rust: a second system reaches the same
/// reconciler by declaring its own. Unknown keys are refused by name, so a typo
/// is a load failure rather than a silently un-synced column.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListSyncSpec {
    /// The declared type whose rows this connection mirrors. Also the row type
    /// the pull mapping emits one item under.
    pub entity: String,
    /// The SQL table those rows are read back from.
    pub table: String,
    /// The manual's tool that fetches one complete snapshot.
    pub pull_tool: String,
    /// The manual's tool that commits a batch of commands.
    pub commit_tool: String,
    /// Row type carrying the list's version envelope. Exactly one such row
    /// reaches a snapshot, and its columns travel verbatim into the next
    /// commit — so an envelope with two numbers needs no Rust that knows it.
    pub list_row_type: String,
    /// The column of that row holding the monotonic version cursor. The one
    /// number this crate compares; the rest of the envelope is opaque.
    pub version_column: String,
    /// A `jaq` expression over one item row yielding that row's reconciliation
    /// key — `.id` where the peer issues one, `[.name, .cat]` where identity is
    /// content. Two rows that yield one key are a load-bearing failure, never a
    /// silent fold: under a complete snapshot, a collapsed pair reads as a
    /// deletion.
    pub key: String,
    /// Column holding the last complete fetch that carried a row, RFC 3339.
    /// NULL marks a row the peer has never sent, whose absence therefore says
    /// nothing about it.
    pub watermark_column: String,
    /// Row types the write leg emits for the `request` mapping to select on.
    pub batch_row_type: String,
    pub command_row_type: String,
    /// Columns taken from the peer whenever they differ from the local value.
    #[serde(default)]
    pub merge_columns: Vec<String>,
    /// How this connection defeats a cached read, if it needs to at all.
    ///
    /// Declared rather than assumed: a cache-buster is one peer's concern, and
    /// a generic round that always sent one would put that peer's vocabulary in
    /// every other connection's request.
    #[serde(default)]
    pub cache_buster: CacheBuster,
    /// Columns that only ever travel peer→local in the truthy direction.
    ///
    /// A shopping list's `checked` is the worked example: with no timestamp to
    /// arbitrate with, a wrong check skips one item while a wrong un-check
    /// re-buys it every trip, so the losing direction is made unrepresentable
    /// rather than remembered at each call site.
    #[serde(default)]
    pub latch_columns: Vec<String>,
}

/// The freshness argument a connection's pull carries, under the name
/// `nocache`.
///
/// It is load-bearing where it is declared, not decoration: the write leg
/// re-reads to confirm its own commit landed, and a cached body there reports
/// that write as missing and gets it sent twice. It is also the peer's own
/// idea — one wants epoch milliseconds, another a UUID, most want nothing — so
/// the shape is declared and an unknown one is refused by name at load. A shape
/// no sidecar asks for yet is not written here; the refusal names what IS
/// accepted, which is how the next one arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBuster {
    /// The peer serves a fresh body per request; the pull carries no
    /// freshness argument at all.
    #[default]
    None,
    /// A fresh epoch-millisecond value.
    EpochMillis,
}

impl CacheBuster {
    /// Whether a pull for this connection carries the argument.
    pub fn is_declared(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// A [`ListSyncSpec`] with its key filter compiled and the declared type's
/// deletion policy resolved.
///
/// Compiling once per connection rather than per response is the difference
/// between a millisecond and a millisecond per poll; the cost measurement that
/// ruled this acceptable is ADR 0034's jaq kill criterion.
pub struct CompiledListSync {
    spec: ListSyncSpec,
    key: RowMapper,
    tombstone_column: String,
    entity_scheme: holon_api::EntityName,
    tombstone_window: Duration,
    declared_columns: BTreeSet<String>,
}

impl std::fmt::Debug for CompiledListSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledListSync")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

impl CompiledListSync {
    /// Compile `spec` against the declared type it mirrors.
    ///
    /// The tombstone column and its retention come from the type's own
    /// `soft_delete`, never from a constant here: the window is a property of
    /// the declared type, and two spellings of it would let a yaml edit
    /// silently disagree with the reconciler.
    pub fn compile(
        connection: &str,
        spec: ListSyncSpec,
        declared: &holon_api::entity::TypeDefinition,
    ) -> Result<Arc<Self>> {
        anyhow::ensure!(
            declared.name == spec.entity,
            "connection '{connection}' declares `list_sync.entity` as '{}', but it was compiled \
             against the type '{}'",
            spec.entity,
            declared.name
        );
        let soft_delete = declared.soft_delete.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "connection '{connection}' mirrors the type '{}', which declares no soft \
                 deletion; a local delete would then leave no tombstone for a round to push, and \
                 the next pull would resurrect the row",
                spec.entity
            )
        })?;
        let key = RowMapper::compile(format!("{connection}: holon.list_sync.key"), &spec.key)
            .with_context(|| {
                format!("connection '{connection}': the `list_sync.key` expression")
            })?;

        let declared_columns: BTreeSet<String> =
            declared.fields.iter().map(|f| f.name.clone()).collect();
        let mut policies: BTreeSet<&str> = BTreeSet::new();
        for (policy, columns) in [
            ("merge_columns", &spec.merge_columns),
            ("latch_columns", &spec.latch_columns),
        ] {
            for column in columns {
                anyhow::ensure!(
                    declared_columns.contains(column),
                    "connection '{connection}' lists '{column}' under `list_sync.{policy}`, but                      the type '{}' declares no such column; a policy on a column that does not                      exist would never match and the column would silently not sync",
                    spec.entity
                );
                anyhow::ensure!(
                    *column != soft_delete.tombstone_field && *column != spec.watermark_column,
                    "connection '{connection}' lists '{column}' under `list_sync.{policy}`, but                      that column carries this connection's own tombstone or watermark bookkeeping                      and cannot also take the peer's value"
                );
                anyhow::ensure!(
                    policies.insert(column),
                    "connection '{connection}' lists '{column}' under both `list_sync.merge_columns`                      and `list_sync.latch_columns`; one column cannot be both peer-authoritative                      and peer-truthy-only"
                );
            }
        }

        Ok(Arc::new(Self {
            tombstone_column: soft_delete.tombstone_field.clone(),
            entity_scheme: holon_api::EntityName::new(spec.entity.clone()),
            tombstone_window: soft_delete.retention(),
            declared_columns,
            spec,
            key,
        }))
    }

    pub fn spec(&self) -> &ListSyncSpec {
        &self.spec
    }

    /// The URI scheme a row of this connection's mirror is addressed under: the
    /// declared type's own name. A local id is a typed REFERENCE, and the
    /// dispatcher refuses one whose scheme names something other than the
    /// entity it is being written to.
    pub fn entity_scheme(&self) -> &str {
        self.entity_scheme.as_str()
    }

    pub fn tombstone_column(&self) -> &str {
        &self.tombstone_column
    }

    pub fn tombstone_window(&self) -> Duration {
        self.tombstone_window
    }

    /// Whether the declared type has a column of this name.
    ///
    /// The declared type is the authority on which columns its rows have, so a
    /// column the peer serves that it does not declare is not part of the row —
    /// it must not reach `create` as a parameter, where it would land in the
    /// overflow bag as a property nobody declared.
    pub fn declares_column(&self, name: &str) -> bool {
        self.declared_columns.contains(name)
    }

    /// This row's reconciliation key, as the sidecar derives it.
    ///
    /// Exactly one output, because a key that is sometimes two values and
    /// sometimes none is not an identity. The row is named in the failure: a
    /// key expression that cannot read one record is what turns a complete
    /// snapshot into a partial one.
    pub fn key_of(&self, row: &serde_json::Value) -> Result<RowKey> {
        let emitted = self
            .key
            .map(row)
            .with_context(|| format!("deriving the key of the row {row}"))?;
        match emitted.as_slice() {
            [one] => Ok(RowKey(one.clone())),
            other => anyhow::bail!(
                "`list_sync.key` emitted {} values for the row {row}, and a key is exactly one",
                other.len()
            ),
        }
    }
}

/// One row's identity, as its connection derives it.
///
/// Opaque on purpose: a server id is a string, a content key is an array, and
/// nothing in the reconciler needs to tell them apart. Ordering is over the
/// JSON text so an outcome is deterministic whatever shape a peer chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowKey(serde_json::Value);

impl RowKey {
    pub fn as_json(&self) -> &serde_json::Value {
        &self.0
    }
}

impl PartialOrd for RowKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RowKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.to_string().cmp(&other.0.to_string())
    }
}

impl std::fmt::Display for RowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
