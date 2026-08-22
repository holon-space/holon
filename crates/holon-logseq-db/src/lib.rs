//! Read-only import of a LogSeq DB-version graph into Holon.
//!
//! A LogSeq DB graph is a standard SQLite file whose `kvs` table holds a
//! persistent DataScript B+-tree, each node a self-contained Transit-JSON
//! document. This crate decodes that tree into deduped datoms, projects the
//! datoms into Holon [`Block`]s, and (via the `ingest` boundary, stage-1
//! increment 4) enters them through `BlockOrdering`. It is **read-only**: no
//! code path here writes a LogSeq db file.
//!
//! Stage 1 scope and the identity-based acceptance gate are described in
//! `plan-lsqdb-import.md`; the fixture-driven amendments are its §9b.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use holon_api::Block;
use holon_api::EntityUri;

pub mod base;
mod datoms;
pub mod ingest;
pub mod kvs_writer;
mod project;
mod transit;

pub use datoms::DatomSet;
pub use datoms::DatomValue;
pub use datoms::EntityKind;
pub use datoms::LogseqAttr;
pub use datoms::LogseqDatom;
pub use datoms::Schema;
pub use datoms::Tx;
pub use datoms::read_datoms;
pub use project::Projection;
pub use project::project;
pub use transit::TransitError;
pub use transit::decode_document;
pub use transit::encode_document;

/// An `f64` compared and hashed by its IEEE-754 bit pattern.
///
/// [`TransitNode`] must be `Eq`/`Hash` so decoded datom values can key the
/// `(e, a, v, tx)` dedup set, but `f64` is neither. Bit-pattern identity is the
/// honest choice for stored numeric literals (amendment A1): two `NaN`s with
/// the same bits compare equal, and `+0.0`/`-0.0` are distinct — the exact
/// value LogSeq persisted round-trips rather than being coerced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct F64Bits(u64);

impl F64Bits {
    pub fn new(f: f64) -> Self {
        Self(f.to_bits())
    }
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

/// A LogSeq entity id — the `e` slot of a datom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Eid(pub i64);

/// A decoded Transit node: the general tree the Transit-JSON reader produces,
/// mirroring the native structure the spike's Python decoder returns. Datom
/// leaves are `Map`s; datom values are any node (scalars project to typed
/// values, collections carry opaque).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TransitNode {
    Nil,
    Bool(bool),
    Int(i64),
    Float(F64Bits),
    Str(String),
    Keyword(String),
    Symbol(String),
    Uuid(String),
    /// `~t` — an instant written as an ISO-8601 string.
    Instant(String),
    /// `~m` — an instant written as milliseconds since the epoch.
    ///
    /// Distinct from [`Instant`](Self::Instant) because the two are the same
    /// value in different ground types, and a writer that collapsed them would
    /// re-emit `~m1787218309858` as `~t1787218309858` — silently handing
    /// LogSeq an unparseable date string where a number stood.
    InstantMillis(String),
    List(Vec<TransitNode>),
    /// Map as ordered key/value pairs (Transit `["^ ", k0, v0, …]`); order is
    /// preserved so datom-leaf and property drawers replay as authored.
    Map(Vec<(TransitNode, TransitNode)>),
    Tagged(String, Box<TransitNode>),
}

/// Everything that can go wrong importing a LogSeq graph. Fail-loud house law:
/// nothing here is a silent skip — an unclassifiable datom errors rather than
/// being dropped.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("failed to open LogSeq db at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
    /// The db is under LogSeq's exclusive WAL lock (LogSeq is running).
    /// Never a retry loop, never a partial read (amendment A2): the caller must
    /// import a byte-copy snapshot instead.
    #[error(
        "LogSeq db at {path} is locked (LogSeq appears to be running); \
         copy it to a snapshot and import the copy — this stage never reads a live db"
    )]
    Locked { path: PathBuf },
    /// The file is not a database (SQLite `NOTADB`). Distinct from
    /// [`Locked`](Self::Locked) on purpose: "LogSeq is running, import a copy"
    /// is useless advice for a truncated or non-SQLite file, and saying it
    /// would hide the corruption.
    #[error("{path} is not a readable SQLite database (corrupt, truncated, or not a LogSeq graph)")]
    Corrupt { path: PathBuf },
    /// A Transit decode failure carrying the `kvs` row it came from, so a
    /// corrupt node names itself instead of surfacing as a count mismatch.
    #[error("Transit decode error in kvs addr {addr}: {source}")]
    Decode {
        addr: i64,
        #[source]
        source: TransitError,
    },
    /// A decode outside any `kvs` row (the standalone [`decode`] entry point).
    #[error("Transit decode error: {0}")]
    Transit(#[from] TransitError),
    #[error("unknown attribute {attr:?} is not declared in the schema node (addr 0)")]
    UnknownAttr { attr: String },
    /// The addr-0 root node is not the shape a DataScript schema node has.
    #[error("malformed schema node (kvs addr 0): {detail}")]
    MalformedSchema { detail: String },
    /// A datom tuple whose slots do not type-check. Never skipped: a tuple we
    /// cannot read is a graph we do not understand.
    #[error("malformed datom in kvs addr {addr}: {detail}")]
    MalformedDatom { addr: i64, detail: String },
    /// A reference pointing at an entity that is not a projectable block.
    /// Never repaired by dropping the edge — a lost parent silently reparents
    /// a subtree to the root.
    #[error("entity {from} references entity {to} through {attr}, which is not a block")]
    DanglingReference { from: i64, attr: String, to: i64 },
    /// Blocks that no chain of parents connects to a root — a missing parent
    /// or a parent cycle. Creating them anyway would reparent a subtree.
    #[error("{count} block(s) are not reachable from any root: {sample}")]
    UnreachableBlocks { count: usize, sample: String },
    /// Namespace pages (`a/b/c`) require page-under-page chain construction,
    /// which Holon's name-chain identity constrains; deferred to a fast-follow.
    /// Fail loud rather than silently flatten the slash into one page name.
    #[error(
        "namespace page {name:?} is unsupported in stage-1 (page-chain construction is a fast-follow)"
    )]
    NamespacePage { name: String },
}

/// Counts that gate the identity check. Populated from the deduped datom set;
/// the acceptance gate asserts `uuid_datoms == uuid_entities` and that the
/// block projection preserves them (see the keystone).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportStats {
    /// Distinct `(e, a, v, tx)` datoms after deduping the 3 index trees.
    pub unique_datoms: usize,
    /// Raw datom-leaf tuples before dedup (index redundancy ≈ 3.12×).
    pub leaf_datoms: usize,
    /// Distinct entity ids across all datoms.
    pub distinct_entities: usize,
    /// Distinct attribute idents across all datoms.
    pub distinct_attrs: usize,
    /// `#(:block/uuid datoms)`.
    pub uuid_datoms: usize,
    /// `#(entities carrying a :block/uuid)`.
    pub uuid_entities: usize,
    /// Uuid-less `:logseq.kv/*` config singletons (not blocks).
    pub kv_singletons: usize,
    /// Uuid-less entities that are not config singletons — LogSeq's own
    /// half-created remnants. Counted so the entity partition is provably
    /// total; see [`EntityKind::Orphan`].
    pub orphan_entities: usize,
}

/// The result of a read-only import: projected blocks + the per-parent sibling
/// order (fracdex-sorted, to be re-minted at the store boundary) + the identity
/// stats.
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    pub blocks: Vec<Block>,
    pub stats: ImportStats,
    ordered_children: HashMap<EntityUri, Vec<EntityUri>>,
}

impl ImportResult {
    /// Find a projected block by its bare LogSeq uuid.
    pub fn block_by_uuid(&self, uuid: &str) -> Option<&Block> {
        let want = EntityUri::block(uuid);
        self.blocks.iter().find(|b| b.id == want)
    }

    /// The intended sibling order of `parent`'s children (fracdex-sorted).
    pub fn ordered_children(&self, parent: &EntityUri) -> &[EntityUri] {
        self.ordered_children.get(parent).map_or(&[], Vec::as_slice)
    }
}

/// Read-only importer for a LogSeq DB-version graph.
#[derive(Debug, Default)]
pub struct LogseqDbImporter;

impl LogseqDbImporter {
    pub fn new() -> Self {
        Self
    }

    /// Import the LogSeq graph at `path` (a snapshot copy — never a live db).
    pub async fn import(&self, path: &Path) -> Result<ImportResult, ImportError> {
        let datoms = read_datoms(path).await?;
        let projection = project(&datoms)?;
        let stats = ImportStats {
            unique_datoms: datoms.datoms.len(),
            leaf_datoms: datoms.leaf_datoms,
            distinct_entities: datoms.entities.len(),
            distinct_attrs: datoms.distinct_attrs(),
            uuid_datoms: datoms.uuid_datoms(),
            uuid_entities: datoms.count_kind(EntityKind::Block),
            kv_singletons: datoms.count_kind(EntityKind::KvSingleton),
            orphan_entities: datoms.count_kind(EntityKind::Orphan),
        };
        Ok(ImportResult {
            blocks: projection.blocks,
            stats,
            ordered_children: projection.ordered_children,
        })
    }
}

/// Decode one Transit-JSON document string into a [`TransitNode`].
pub fn decode(doc: &str) -> Result<TransitNode, ImportError> {
    Ok(decode_document(doc)?)
}
