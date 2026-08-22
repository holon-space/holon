//! Reading and re-writing the `kvs` table of a LogSeq DB graph.
//!
//! A DB graph is one SQLite table, `kvs(addr INTEGER PRIMARY KEY, content
//! TEXT, addresses JSON)`, holding a persistent DataScript B+-tree. This
//! module parses that table into typed rows, guards the storage parameters
//! Holon's writer is pinned to, and writes the rows back out to a **new**
//! file. It carries no datom semantics: the tree is replayed as it was read.
//!
//! # How an edit is written: the tail, not the tree
//!
//! DataScript does not rewrite its B+-trees for a small transaction. Each
//! transaction's datoms are appended to a TAIL held at addr 1, and while the
//! tail's total datom count stays within the branching factor that is the ONLY
//! row written — the trees and addr 0 are untouched. On read, `restore-conn`
//! replays the tail over the restored trees, and that is the same
//! `restore-conn` LogSeq's own CLI path calls. So a tail append is not a
//! shortcut around LogSeq; it is what LogSeq does.
//!
//! DEFERRED AND REQUIRED: writing the trees themselves. A tail holds at most
//! [`PINNED_BRANCHING_FACTOR`] datoms, so a bulk push, or any push onto a graph
//! whose tail LogSeq has already filled, needs a full re-store — insert into
//! all three index trees with split and rebalance. That work is de-risked but
//! unwritten: persistent-sorted-set 0.1.2 (CLJS) hardcodes max-len 32, min-len
//! 16, avg-len 24, `shift` = depth − 1, and the branch invariant
//! `keys[i] == max(children[i].keys)`. Until it exists,
//! [`Tail::push_transaction`] refuses rather than overflowing.
//!
//! The `addresses` column is not a copy of anything in the content. LogSeq's
//! own storage layer removes `:addresses` from a node before encoding it and
//! puts the child pointers in the column instead, then re-attaches them on
//! restore. A node carrying both would therefore be read back with the
//! column's value silently winning, so [`RowError::AddressesInContent`]
//! refuses that shape rather than picking a side.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use libsql::Builder;
use libsql::OpenFlags;

use crate::TransitNode;
use crate::transit::decode_document;

/// The LogSeq schema version this writer is pinned to, EXACTLY.
///
/// Not a minimum and not major-only. Of the 13 shipped 65.x migrations, 10
/// rewrite user datoms rather than adding schema, so a graph one minor away
/// holds datoms whose meaning this build does not know. LogSeq itself will not
/// re-migrate it either — the version datom already reads the newer number —
/// so the obsolete meanings would simply persist.
///
/// Two ways a pin can go stale, and only one is visible here:
/// - SEMANTIC drift, a minor bump: caught by this guard, which turns it into
///   "Holon refuses to write until it is re-pinned".
/// - STRUCTURAL drift, the DataScript fork changing its storage layout: NOT
///   caught here, because nothing in a graph file records which fork rev wrote
///   it. The row-0 unknown-key refusal in [`RootNode::parse`] is the only
///   in-file signal, and the oracle run (`just lsqdb-oracle`) is the only real
///   check.
///
/// Operational cost, measured rather than guessed: minors ship at roughly
/// 1.9/month, so expect to re-pin about monthly. Re-pinning means moving the
/// oracle checkout to the matching rev, re-baselining on the fixture, and
/// re-running the legs — see docs/Testing/LogseqDbOracle.md. If that cadence
/// ever becomes unacceptable, the FW-1(a) ruling itself is what needs
/// revisiting, not this constant.
pub const PINNED_SCHEMA_VERSION: SchemaVersion = SchemaVersion {
    major: 65,
    minor: 33,
};

/// The `:branching-factor` this writer is pinned to.
pub const PINNED_BRANCHING_FACTOR: i64 = 32;
/// The `:ref-type` this writer is pinned to.
pub const PINNED_REF_TYPE: &str = "strong";

/// The addr-0 keys a graph this writer understands carries — no more, no less.
///
/// An unknown key is the only signal inside the file that the DataScript fork
/// changed its storage layout, which nothing else in the graph records. It is
/// therefore a hard stop, not a warning.
pub const ROOT_KEYS: &[&str] = &[
    "schema",
    "eavt",
    "aevt",
    "avet",
    "eavt-metadata",
    "aevt-metadata",
    "avet-metadata",
    "max-tx",
    "max-eid",
    "max-addr",
    "branching-factor",
    "ref-type",
];

/// What can go wrong reading or writing a `kvs` table.
#[derive(Debug, thiserror::Error)]
pub enum RowError {
    #[error("failed to open LogSeq db at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
    #[error("failed to write LogSeq db at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
    #[error("Transit error in kvs addr {addr}: {source}")]
    Transit {
        addr: i64,
        #[source]
        source: crate::TransitError,
    },
    #[error("the kvs table has no addr-0 root node")]
    NoRoot,
    #[error("the addr-0 root node is not a Transit map")]
    RootNotAMap,
    /// The graph declares a storage key this writer has never seen. See
    /// [`ROOT_KEYS`] for why this is fatal rather than ignorable.
    #[error(
        "addr-0 declares unknown storage key {key:?}; this graph was written by a \
         DataScript layout this build does not understand, and writing it would corrupt the tree"
    )]
    UnknownRootKey { key: String },
    #[error("addr-0 is missing required storage key {key:?}")]
    MissingRootKey { key: String },
    #[error("addr-0 declares key {key:?} twice")]
    DuplicateRootKey { key: String },
    #[error("addr-0 key {key:?} holds {found}, expected {expected}")]
    RootKeyType {
        key: String,
        found: &'static str,
        expected: &'static str,
    },
    #[error(
        "graph declares :branching-factor {found}, but this writer is pinned to \
         {PINNED_BRANCHING_FACTOR}; node splits would land in the wrong places"
    )]
    BranchingFactor { found: i64 },
    #[error("graph declares :ref-type {found:?}, but this writer is pinned to {PINNED_REF_TYPE:?}")]
    RefType { found: String },
    #[error("kvs addr {addr}: addresses column is not a JSON array of integers: {text:?}")]
    BadAddresses { addr: i64, text: String },
    /// Child pointers in both the content and the column. LogSeq's restore
    /// path overwrites `:addresses` from the column, so the two disagreeing
    /// changes the tree's shape with no error anywhere.
    #[error(
        "kvs addr {addr}: node content carries :addresses, which LogSeq's storage layer \
         strips before encoding; the column is the only authority for child pointers"
    )]
    AddressesInContent { addr: i64 },
    /// The graph is a version this build does not know how to write.
    #[error(
        "graph is at LogSeq schema version {found}, but this build is pinned to \
         {PINNED_SCHEMA_VERSION}; a minor bump rewrites user datoms, so writing this graph \
         would edit data whose meaning has changed. Re-pin Holon to {found} (re-baseline the \
         oracle first — docs/Testing/LogseqDbOracle.md) rather than relaxing this check"
    )]
    SchemaVersionMismatch { found: SchemaVersion },
    /// No `:logseq.kv/schema-version` datom at all.
    ///
    /// LogSeq treats a missing version as 0 (migrate.cljs:551) and migrates the
    /// graph forward. Holon cannot: version 0 means every 65.x migration is
    /// still un-applied, so nothing in the graph means what this build assumes.
    #[error(
        "graph carries no :logseq.kv/schema-version datom, which LogSeq reads as version 0 \
         and would migrate forward; this build is pinned to {PINNED_SCHEMA_VERSION} and \
         will not write a graph whose migrations have not been applied. Open it in LogSeq \
         once to migrate it, then re-import"
    )]
    SchemaVersionMissing,
    /// The version datom exists but is not a `{:major :minor}` map.
    #[error("the :logseq.kv/schema-version datom is malformed: {detail}")]
    SchemaVersionMalformed { detail: String },
    /// Addr 1 is not the list-of-transactions the tail must be.
    #[error("the tail at kvs addr 1 is malformed: {detail}")]
    MalformedTail { detail: String },
    /// A tail entry is not `[e a v tx]`.
    ///
    /// Loud here because it is silent THERE: `db-with-tail-datoms` wraps its
    /// replay in `(catch :default _ db)`, so LogSeq drops a malformed tail
    /// transaction and loads the graph as though the edit never happened. A
    /// shape this writer emitted and did not check would therefore vanish with
    /// no error on either side.
    #[error(
        "tail datom {position} is not shaped [e a v tx]: {detail}. LogSeq silently ignores a \
         tail transaction it cannot replay, so this would have been lost without a word"
    )]
    MalformedTailDatom { position: String, detail: String },
    #[error("the kvs table has no addr-1 tail node")]
    NoTail,
    #[error("entity {entity} carries no :block/title datom, so there is nothing to replace")]
    NoTitle { entity: i64 },
    #[error("entity {entity}'s :block/title holds {found}, not a string")]
    TitleNotAString { entity: i64, found: &'static str },
    /// Replacing a cardinality-MANY value is a different operation: an assert
    /// adds to the set instead of superseding, so the retract must name the
    /// exact value and the caller must mean "remove this one".
    #[error(
        "attribute :{attribute} is cardinality-many; replacing a value there is not the same \
         operation as replacing a cardinality-one value and is out of this increment's scope"
    )]
    NotCardinalityOne { attribute: String },
    /// The tail cannot hold the new transaction.
    #[error(
        "the tail already holds {existing} datom(s) and this change adds {adding}, which \
         exceeds the branching factor of {limit}; LogSeq flushes the tail into the index \
         trees at that point and this build cannot yet write them. Open the graph in LogSeq \
         once to flush the tail, then retry"
    )]
    TailOverflow {
        existing: usize,
        adding: usize,
        limit: usize,
    },
}

/// A LogSeq schema version, as the `:logseq.kv/schema-version` datom carries
/// it: a `{:major :minor}` map, both halves meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersion {
    pub major: i64,
    pub minor: i64,
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Whether a datom asserts a value or retracts one.
///
/// Carried on the wire as the SIGN of the transaction id — `datom-added` is
/// literally `(pos? tx)` — so this enum and [`TxId`] together are the whole of
/// a tail datom's fourth slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatomOp {
    Assert,
    Retract,
}

/// A transaction id: always positive, because the sign slot is spoken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TxId(i64);

impl TxId {
    /// Refuses zero and negatives: a zero tx has no sign, so it could not say
    /// whether it asserts or retracts, and a negative one is already spoken
    /// for by [`DatomOp::Retract`].
    pub fn new(tx: i64) -> Result<Self, RowError> {
        if tx > 0 {
            Ok(Self(tx))
        } else {
            Err(RowError::MalformedTailDatom {
                position: "tx".to_string(),
                detail: format!("transaction id {tx} is not positive"),
            })
        }
    }

    pub fn get(self) -> i64 {
        self.0
    }

    /// The signed form LogSeq stores.
    fn signed(self, op: DatomOp) -> i64 {
        match op {
            DatomOp::Assert => self.0,
            DatomOp::Retract => -self.0,
        }
    }
}

/// One datom as the tail carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct TailDatom {
    pub entity: i64,
    /// The attribute ident WITHOUT its leading colon, matching [`TransitNode`].
    pub attribute: String,
    pub value: TransitNode,
    pub tx: TxId,
    pub op: DatomOp,
}

impl TailDatom {
    fn parse(node: &TransitNode, position: &str) -> Result<Self, RowError> {
        let malformed = |detail: String| RowError::MalformedTailDatom {
            position: position.to_string(),
            detail,
        };
        let TransitNode::List(slots) = node else {
            return Err(malformed(format!(
                "it is {}, not a 4-slot list",
                node_kind(node)
            )));
        };
        let [
            TransitNode::Int(entity),
            TransitNode::Keyword(attribute),
            value,
            TransitNode::Int(tx),
        ] = slots.as_slice()
        else {
            return Err(malformed(format!(
                "its {} slot(s) are not [integer, keyword, value, integer]",
                slots.len()
            )));
        };
        let op = if *tx > 0 {
            DatomOp::Assert
        } else {
            DatomOp::Retract
        };
        Ok(Self {
            entity: *entity,
            attribute: attribute.clone(),
            value: value.clone(),
            tx: TxId::new(tx.abs())?,
            op,
        })
    }

    fn to_node(&self) -> TransitNode {
        TransitNode::List(vec![
            TransitNode::Int(self.entity),
            TransitNode::Keyword(self.attribute.clone()),
            self.value.clone(),
            TransitNode::Int(self.tx.signed(self.op)),
        ])
    }
}

/// The transaction log at kvs addr 1, replayed over the trees on every load.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tail {
    transactions: Vec<Vec<TailDatom>>,
}

impl Tail {
    pub fn parse(node: &TransitNode) -> Result<Self, RowError> {
        let TransitNode::List(txs) = node else {
            return Err(RowError::MalformedTail {
                detail: format!("it is {}, not a list of transactions", node_kind(node)),
            });
        };
        let mut transactions = Vec::with_capacity(txs.len());
        for (i, tx) in txs.iter().enumerate() {
            let TransitNode::List(datoms) = tx else {
                return Err(RowError::MalformedTail {
                    detail: format!("transaction {i} is {}, not a list", node_kind(tx)),
                });
            };
            transactions.push(
                datoms
                    .iter()
                    .enumerate()
                    .map(|(j, d)| TailDatom::parse(d, &format!("transaction {i}, datom {j}")))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        Ok(Self { transactions })
    }

    pub fn to_node(&self) -> TransitNode {
        TransitNode::List(
            self.transactions
                .iter()
                .map(|tx| TransitNode::List(tx.iter().map(TailDatom::to_node).collect()))
                .collect(),
        )
    }

    /// Total datoms across every transaction — the figure LogSeq compares
    /// against the branching factor when deciding whether to flush.
    pub fn datom_count(&self) -> usize {
        self.transactions.iter().map(Vec::len).sum()
    }

    pub fn transactions(&self) -> &[Vec<TailDatom>] {
        &self.transactions
    }

    /// The highest transaction id anywhere in the tail.
    pub fn max_tx(&self) -> Option<TxId> {
        self.transactions.iter().flatten().map(|d| d.tx).max()
    }

    /// Append one transaction, refusing to exceed what LogSeq will flush.
    ///
    /// The bound is the branching factor, and the comparison is the same one
    /// `store-after-transact!` makes: LogSeq flushes when the total EXCEEDS it,
    /// so a tail filled exactly to the limit is still a tail. Past that point
    /// LogSeq would rewrite the index trees, which this build cannot do —
    /// hence a refusal rather than an over-long tail LogSeq never produces.
    ///
    /// Refuses before mutating, so a rejected push leaves the tail as it was.
    pub fn push_transaction(&mut self, datoms: Vec<TailDatom>) -> Result<(), RowError> {
        let limit = PINNED_BRANCHING_FACTOR as usize;
        let existing = self.datom_count();
        if existing + datoms.len() > limit {
            return Err(RowError::TailOverflow {
                existing,
                adding: datoms.len(),
                limit,
            });
        }
        self.transactions.push(datoms);
        Ok(())
    }
}

/// The per-index `{:count :shift}` metadata of the root node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexMeta {
    pub count: i64,
    pub shift: i64,
}

/// The addr-0 root node, parsed into the storage parameters it declares.
///
/// `schema` stays an opaque [`TransitNode`]: it is the graph's attribute
/// vocabulary, which this module replays rather than interprets.
#[derive(Debug, Clone, PartialEq)]
pub struct RootNode {
    pub schema: TransitNode,
    pub eavt: i64,
    pub aevt: i64,
    pub avet: i64,
    pub eavt_metadata: IndexMeta,
    pub aevt_metadata: IndexMeta,
    pub avet_metadata: IndexMeta,
    pub max_tx: i64,
    pub max_eid: i64,
    pub max_addr: i64,
    pub branching_factor: i64,
    pub ref_type: String,
    /// The order addr-0's keys were authored in, so re-encoding replays the
    /// document rather than imposing this struct's field order.
    key_order: Vec<String>,
}

/// One `kvs` row: its address, its decoded node, and its child pointers.
#[derive(Debug, Clone, PartialEq)]
pub struct KvsRow {
    pub addr: i64,
    pub node: TransitNode,
    /// `None` for a leaf node and for addr 0; `Some` for a branch node.
    pub addresses: Option<Vec<i64>>,
    /// The bytes this row held on disk, kept so a re-encode can report how
    /// much of the original emission it reproduced.
    pub original_content: String,
}

/// The kvs address of the transaction tail. Fixed by DataScript, not by us.
const TAIL_ADDR: i64 = 1;

/// A whole graph's `kvs` table, decoded and guarded.
#[derive(Debug, Clone, PartialEq)]
pub struct KvsGraph {
    pub root: RootNode,
    /// Every row including addr 0, ordered by address.
    pub rows: Vec<KvsRow>,
}

impl KvsGraph {
    fn tail_row(&self) -> Result<usize, RowError> {
        self.rows
            .iter()
            .position(|r| r.addr == TAIL_ADDR)
            .ok_or(RowError::NoTail)
    }

    /// The transaction tail at addr 1.
    pub fn tail(&self) -> Result<Tail, RowError> {
        Tail::parse(&self.rows[self.tail_row()?].node)
    }

    /// Replace the tail, leaving every other row untouched.
    pub fn set_tail(&mut self, tail: &Tail) -> Result<(), RowError> {
        let at = self.tail_row()?;
        self.rows[at].node = tail.to_node();
        Ok(())
    }
}

/// What a write reproduced.
///
/// `rows_byte_identical` is expected to equal `rows_written` for a graph
/// written back unchanged, and the round-trip test asserts exactly that. The
/// Transit write cache is emission-order dependent, so this was not obviously
/// achievable — but it is, because the cache is invertible: emitting a
/// back-reference wherever the reader would have cached the string reproduces
/// LogSeq's own bytes. Keeping it pinned makes a divergence visible as a
/// count instead of as a subtly different file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteReport {
    pub rows_written: usize,
    pub rows_byte_identical: usize,
}

fn keyword(node: &TransitNode) -> Option<&str> {
    match node {
        TransitNode::Keyword(k) => Some(k),
        _ => None,
    }
}

fn node_kind(node: &TransitNode) -> &'static str {
    match node {
        TransitNode::Nil => "nil",
        TransitNode::Bool(_) => "a boolean",
        TransitNode::Int(_) => "an integer",
        TransitNode::Float(_) => "a float",
        TransitNode::Str(_) => "a string",
        TransitNode::Keyword(_) => "a keyword",
        TransitNode::Symbol(_) => "a symbol",
        TransitNode::Uuid(_) => "a uuid",
        TransitNode::Instant(_) | TransitNode::InstantMillis(_) => "an instant",
        TransitNode::List(_) => "a list",
        TransitNode::Map(_) => "a map",
        TransitNode::Tagged(..) => "a tagged value",
    }
}

fn want_int(key: &str, node: &TransitNode) -> Result<i64, RowError> {
    match node {
        TransitNode::Int(i) => Ok(*i),
        other => Err(RowError::RootKeyType {
            key: key.to_string(),
            found: node_kind(other),
            expected: "an integer",
        }),
    }
}

fn want_meta(key: &str, node: &TransitNode) -> Result<IndexMeta, RowError> {
    let TransitNode::Map(pairs) = node else {
        return Err(RowError::RootKeyType {
            key: key.to_string(),
            found: node_kind(node),
            expected: "a map",
        });
    };
    let mut count = None;
    let mut shift = None;
    for (k, v) in pairs {
        match keyword(k) {
            Some("count") => count = Some(want_int(key, v)?),
            Some("shift") => shift = Some(want_int(key, v)?),
            _ => {
                return Err(RowError::UnknownRootKey {
                    key: format!("{key}/{}", keyword(k).unwrap_or("<non-keyword>")),
                });
            }
        }
    }
    match (count, shift) {
        (Some(count), Some(shift)) => Ok(IndexMeta { count, shift }),
        _ => Err(RowError::MissingRootKey {
            key: format!("{key}/count or {key}/shift"),
        }),
    }
}

impl RootNode {
    /// Parse addr 0, refusing any storage parameter this build is not pinned
    /// to.
    pub fn parse(root: &TransitNode) -> Result<Self, RowError> {
        let TransitNode::Map(pairs) = root else {
            return Err(RowError::RootNotAMap);
        };

        let mut seen = BTreeSet::new();
        let mut key_order = Vec::with_capacity(pairs.len());
        for (k, _) in pairs {
            let Some(name) = keyword(k) else {
                return Err(RowError::RootKeyType {
                    key: format!("{k:?}"),
                    found: node_kind(k),
                    expected: "a keyword",
                });
            };
            if !ROOT_KEYS.contains(&name) {
                return Err(RowError::UnknownRootKey {
                    key: name.to_string(),
                });
            }
            if !seen.insert(name.to_string()) {
                return Err(RowError::DuplicateRootKey {
                    key: name.to_string(),
                });
            }
            key_order.push(name.to_string());
        }
        for required in ROOT_KEYS {
            if !seen.contains(*required) {
                return Err(RowError::MissingRootKey {
                    key: (*required).to_string(),
                });
            }
        }

        let get = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| keyword(k) == Some(name))
                .map(|(_, v)| v)
                .expect("every ROOT_KEYS entry was just proven present")
        };

        let branching_factor = want_int("branching-factor", get("branching-factor"))?;
        if branching_factor != PINNED_BRANCHING_FACTOR {
            return Err(RowError::BranchingFactor {
                found: branching_factor,
            });
        }
        let ref_type = match get("ref-type") {
            TransitNode::Keyword(k) => k.clone(),
            other => {
                return Err(RowError::RootKeyType {
                    key: "ref-type".to_string(),
                    found: node_kind(other),
                    expected: "a keyword",
                });
            }
        };
        if ref_type != PINNED_REF_TYPE {
            return Err(RowError::RefType { found: ref_type });
        }

        Ok(Self {
            schema: get("schema").clone(),
            eavt: want_int("eavt", get("eavt"))?,
            aevt: want_int("aevt", get("aevt"))?,
            avet: want_int("avet", get("avet"))?,
            eavt_metadata: want_meta("eavt-metadata", get("eavt-metadata"))?,
            aevt_metadata: want_meta("aevt-metadata", get("aevt-metadata"))?,
            avet_metadata: want_meta("avet-metadata", get("avet-metadata"))?,
            max_tx: want_int("max-tx", get("max-tx"))?,
            max_eid: want_int("max-eid", get("max-eid"))?,
            max_addr: want_int("max-addr", get("max-addr"))?,
            branching_factor,
            ref_type,
            key_order,
        })
    }

    /// The storage keys addr 0 declared, in the order it authored them.
    pub fn declared_keys(&self) -> &[String] {
        &self.key_order
    }

    /// Rebuild the addr-0 Transit map, replaying the authored key order.
    pub fn to_node(&self) -> TransitNode {
        let meta = |m: &IndexMeta| {
            TransitNode::Map(vec![
                (
                    TransitNode::Keyword("count".into()),
                    TransitNode::Int(m.count),
                ),
                (
                    TransitNode::Keyword("shift".into()),
                    TransitNode::Int(m.shift),
                ),
            ])
        };
        let pairs = self
            .key_order
            .iter()
            .map(|name| {
                let value = match name.as_str() {
                    "schema" => self.schema.clone(),
                    "eavt" => TransitNode::Int(self.eavt),
                    "aevt" => TransitNode::Int(self.aevt),
                    "avet" => TransitNode::Int(self.avet),
                    "eavt-metadata" => meta(&self.eavt_metadata),
                    "aevt-metadata" => meta(&self.aevt_metadata),
                    "avet-metadata" => meta(&self.avet_metadata),
                    "max-tx" => TransitNode::Int(self.max_tx),
                    "max-eid" => TransitNode::Int(self.max_eid),
                    "max-addr" => TransitNode::Int(self.max_addr),
                    "branching-factor" => TransitNode::Int(self.branching_factor),
                    "ref-type" => TransitNode::Keyword(self.ref_type.clone()),
                    other => unreachable!("key_order holds only ROOT_KEYS, got {other:?}"),
                };
                (TransitNode::Keyword(name.clone()), value)
            })
            .collect();
        TransitNode::Map(pairs)
    }
}

fn parse_addresses(addr: i64, text: &str) -> Result<Vec<i64>, RowError> {
    let bad = || RowError::BadAddresses {
        addr,
        text: text.to_string(),
    };
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| bad())?;
    let serde_json::Value::Array(items) = value else {
        return Err(bad());
    };
    items
        .iter()
        .map(|i| i.as_i64().ok_or_else(bad))
        .collect::<Result<Vec<_>, _>>()
}

fn carries_addresses(node: &TransitNode) -> bool {
    match node {
        TransitNode::Map(pairs) => pairs.iter().any(|(k, _)| keyword(k) == Some("addresses")),
        _ => false,
    }
}

/// The datom tuples a node carries, or `&[]` for a node that holds none.
///
/// Both leaf and branch nodes carry `:keys`; a branch node's entries are
/// separators copied from the leaves below it, so scanning every node finds
/// the same datom more than once. That is harmless for a lookup by ident.
fn datom_tuples(node: &TransitNode) -> &[TransitNode] {
    let TransitNode::Map(pairs) = node else {
        return &[];
    };
    for (k, v) in pairs {
        if keyword(k) == Some("keys") {
            if let TransitNode::List(tuples) = v {
                return tuples;
            }
        }
    }
    &[]
}

/// A datom tuple's `(entity, attribute)` pair plus its value, when it has the
/// `[e a v …]` shape. Anything else is not a datom and is skipped by callers.
fn tuple_parts(tuple: &TransitNode) -> Option<(i64, &str, &TransitNode)> {
    let TransitNode::List(slots) = tuple else {
        return None;
    };
    let [TransitNode::Int(e), attr, value, ..] = slots.as_slice() else {
        return None;
    };
    Some((*e, keyword(attr)?, value))
}

/// The graph's `:logseq.kv/schema-version`, read from its datoms.
///
/// Found by ident rather than by entity id: a fresh graph also carries
/// `:logseq.kv/graph-initial-schema-version`, whose value is identical at
/// creation and diverges later, so guessing the entity would silently read the
/// wrong one on every graph that has ever been migrated.
pub fn schema_version(rows: &[KvsRow]) -> Result<SchemaVersion, RowError> {
    let mut version_entity = None;
    for row in rows {
        for tuple in datom_tuples(&row.node) {
            if let Some((e, "db/ident", TransitNode::Keyword(ident))) = tuple_parts(tuple) {
                if ident == "logseq.kv/schema-version" {
                    version_entity = Some(e);
                }
            }
        }
    }
    let version_entity = version_entity.ok_or(RowError::SchemaVersionMissing)?;

    for row in rows {
        for tuple in datom_tuples(&row.node) {
            let Some((e, "kv/value", value)) = tuple_parts(tuple) else {
                continue;
            };
            if e != version_entity {
                continue;
            }
            let TransitNode::Map(pairs) = value else {
                return Err(RowError::SchemaVersionMalformed {
                    detail: format!("its :kv/value is {}, not a map", node_kind(value)),
                });
            };
            let mut major = None;
            let mut minor = None;
            for (k, v) in pairs {
                match (keyword(k), v) {
                    (Some("major"), TransitNode::Int(i)) => major = Some(*i),
                    (Some("minor"), TransitNode::Int(i)) => minor = Some(*i),
                    _ => {}
                }
            }
            return match (major, minor) {
                (Some(major), Some(minor)) => Ok(SchemaVersion { major, minor }),
                _ => Err(RowError::SchemaVersionMalformed {
                    detail: "its :kv/value has no integer :major and :minor".to_string(),
                }),
            };
        }
    }
    Err(RowError::SchemaVersionMalformed {
        detail: format!(
            "entity {version_entity} declares :db/ident :logseq.kv/schema-version \
             but carries no :kv/value datom"
        ),
    })
}

/// Refuse to proceed unless the graph is EXACTLY [`PINNED_SCHEMA_VERSION`].
///
/// Equality, not a floor. A graph BEHIND the pin is refused as firmly as one
/// ahead: LogSeq would migrate it forward on open, rewriting user datoms, and
/// this build has no way to do that — so writing to it would edit data that is
/// about to be rewritten underneath the edit.
pub fn assert_pinned_schema_version(rows: &[KvsRow]) -> Result<SchemaVersion, RowError> {
    let found = schema_version(rows)?;
    if found == PINNED_SCHEMA_VERSION {
        Ok(found)
    } else {
        Err(RowError::SchemaVersionMismatch { found })
    }
}

/// The attribute this increment can edit.
const TITLE: &str = "block/title";

/// Whether the root schema declares `attribute` as cardinality-many.
fn is_cardinality_many(schema: &TransitNode, attribute: &str) -> bool {
    let TransitNode::Map(attrs) = schema else {
        return false;
    };
    attrs
        .iter()
        .find(|(k, _)| keyword(k) == Some(attribute))
        .and_then(|(_, definition)| match definition {
            TransitNode::Map(fields) => fields
                .iter()
                .find_map(|(k, v)| (keyword(k) == Some("db/cardinality")).then_some(keyword(v))),
            _ => None,
        })
        .flatten()
        == Some("db.cardinality/many")
}

/// The entity id LogSeq gave the block with this uuid.
///
/// Holon addresses blocks by uuid and the tail addresses them by entity id, so
/// something has to bridge the two; this is that. Returns `None` rather than an
/// error because "no such block" is a question a caller may legitimately ask.
pub fn entity_by_uuid(graph: &KvsGraph, uuid: &str) -> Option<i64> {
    for row in &graph.rows {
        for tuple in datom_tuples(&row.node) {
            if let Some((e, "block/uuid", TransitNode::Uuid(found))) = tuple_parts(tuple) {
                if found == uuid {
                    return Some(e);
                }
            }
        }
    }
    None
}

/// What one title replacement did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleEdit {
    pub entity: i64,
    pub old_title: String,
    pub new_title: String,
    pub tx: TxId,
}

/// The value `entity`'s `:block/title` currently resolves to.
///
/// The trees hold the stored state and the tail is replayed over them, so the
/// tail must win — otherwise a second edit in one session would retract a value
/// that is no longer current, and LogSeq would replay a retraction that matches
/// nothing.
fn current_title(graph: &KvsGraph, entity: i64) -> Result<String, RowError> {
    let mut best: Option<(i64, &TransitNode)> = None;
    for row in &graph.rows {
        for tuple in datom_tuples(&row.node) {
            let Some((e, attr, value)) = tuple_parts(tuple) else {
                continue;
            };
            if e != entity || attr != TITLE {
                continue;
            }
            // Branch nodes repeat the leaves' datoms as separators, so the same
            // datom is seen several times; highest transaction wins.
            let tx = match tuple {
                TransitNode::List(slots) => match slots.get(3) {
                    Some(TransitNode::Int(tx)) => *tx,
                    _ => 0,
                },
                _ => 0,
            };
            if best.is_none_or(|(best_tx, _)| tx >= best_tx) {
                best = Some((tx, value));
            }
        }
    }

    let mut current = match best {
        Some((_, TransitNode::Str(s))) => Some(s.clone()),
        Some((_, other)) => {
            return Err(RowError::TitleNotAString {
                entity,
                found: node_kind(other),
            });
        }
        None => None,
    };

    for tx in graph.tail()?.transactions() {
        for datom in tx {
            if datom.entity != entity || datom.attribute != TITLE {
                continue;
            }
            current = match datom.op {
                DatomOp::Assert => match &datom.value {
                    TransitNode::Str(s) => Some(s.clone()),
                    other => {
                        return Err(RowError::TitleNotAString {
                            entity,
                            found: node_kind(other),
                        });
                    }
                },
                DatomOp::Retract => None,
            };
        }
    }

    current.ok_or(RowError::NoTitle { entity })
}

/// Replace one existing block's `:block/title`, as a tail transaction.
///
/// Emits the retract of the old value and the assert of the new one under ONE
/// new transaction id, which is what LogSeq's own transactor would write.
///
/// ONLY addr 1 changes, and the reason is simply that this is what LogSeq
/// does: its `ldb/transact!` writes the same tail shape and leaves addr 0
/// alone, so rewriting the root would be a divergence, not tidiness.
///
/// The root's `:max-tx` is therefore left stale on disk. On restore LogSeq
/// does NOT read `:max-tx` out of the tail: measured across four graphs, the
/// restored `max-tx` is always `root + 1` whatever ids the tail carries.
///
/// This writer's id and LogSeq's DIFFER, measured on a pristine empty-tail
/// copy at root 536871022: LogSeq's next edit takes 536871024 (`root + 2`),
/// this writer takes 536871023 (`root + 1`). The mechanism is that restore
/// consumes `root + 1`, so the next transaction takes the one after it — which
/// means the id chosen here is one LogSeq considers already spent. `+ 2` is
/// not conditional on the tail being non-empty: three consecutive LogSeq edits
/// each took `+ 2`.
///
/// Left as is, because a transaction id in the tail is NOT a uniqueness
/// guarantee — LogSeq reuses one across consecutive tail edits — and nothing
/// here may treat one as if it were.
pub fn replace_block_title(
    graph: &mut KvsGraph,
    entity: i64,
    new_title: &str,
) -> Result<TitleEdit, RowError> {
    assert_pinned_schema_version(&graph.rows)?;
    if is_cardinality_many(&graph.root.schema, TITLE) {
        return Err(RowError::NotCardinalityOne {
            attribute: TITLE.to_string(),
        });
    }

    let old_title = current_title(graph, entity)?;
    let mut tail = graph.tail()?;
    let tx = TxId::new(
        graph
            .root
            .max_tx
            .max(tail.max_tx().map_or(0, TxId::get))
            .saturating_add(1),
    )?;

    let datom = |value: &str, op| TailDatom {
        entity,
        attribute: TITLE.to_string(),
        value: TransitNode::Str(value.to_string()),
        tx,
        op,
    };
    tail.push_transaction(vec![
        datom(&old_title, DatomOp::Retract),
        datom(new_title, DatomOp::Assert),
    ])?;
    graph.set_tail(&tail)?;

    Ok(TitleEdit {
        entity,
        old_title,
        new_title: new_title.to_string(),
        tx,
    })
}

/// Read and guard the whole `kvs` table of the graph at `path`.
///
/// Opened read-only, so this can never be the leg that damages a graph.
pub async fn read_graph(path: &Path) -> Result<KvsGraph, RowError> {
    let open_error = |source: libsql::Error| RowError::Open {
        path: path.to_path_buf(),
        source: source.into(),
    };

    let db = Builder::new_local(path)
        .flags(OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .map_err(open_error)?;
    let conn = db.connect().map_err(open_error)?;
    let mut sql_rows = conn
        .query("SELECT addr, content, addresses FROM kvs ORDER BY addr", ())
        .await
        .map_err(open_error)?;

    let mut rows = Vec::new();
    while let Some(row) = sql_rows.next().await.map_err(open_error)? {
        let addr: i64 = row.get(0).map_err(open_error)?;
        let content: String = row.get(1).map_err(open_error)?;
        let addresses: Option<String> = row.get(2).map_err(open_error)?;

        let node =
            decode_document(&content).map_err(|source| RowError::Transit { addr, source })?;
        if carries_addresses(&node) {
            return Err(RowError::AddressesInContent { addr });
        }
        let addresses = addresses
            .map(|text| parse_addresses(addr, &text))
            .transpose()?;
        rows.push(KvsRow {
            addr,
            node,
            addresses,
            original_content: content,
        });
    }

    let root_row = rows
        .first()
        .filter(|r| r.addr == 0)
        .ok_or(RowError::NoRoot)?;
    let root = RootNode::parse(&root_row.node)?;
    Ok(KvsGraph { root, rows })
}

/// Write `graph` to a new SQLite file at `dest`.
///
/// Addr 0 is re-emitted from the parsed [`RootNode`] rather than replayed from
/// the node it was read as, so the storage parameters this writer claims to
/// understand are the ones that actually reach the file. Every other row is
/// re-encoded from its decoded value.
///
/// The whole table lands in one transaction: a graph missing some of its rows
/// is a broken B+-tree, which is worse than no file at all.
pub async fn write_graph(graph: &KvsGraph, dest: &Path) -> Result<WriteReport, RowError> {
    let write_error = |source: libsql::Error| RowError::Write {
        path: dest.to_path_buf(),
        source: source.into(),
    };

    // Before any file exists: a version this build does not understand must not
    // leave a half-written graph behind to explain away.
    assert_pinned_schema_version(&graph.rows)?;

    let db = Builder::new_local(dest)
        .build()
        .await
        .map_err(write_error)?;
    let conn = db.connect().map_err(write_error)?;
    conn.execute(
        "create table if not exists kvs (addr INTEGER primary key, content TEXT, addresses JSON)",
        (),
    )
    .await
    .map_err(write_error)?;

    let txn = conn.transaction().await.map_err(write_error)?;
    let mut report = WriteReport::default();
    for row in &graph.rows {
        let node = if row.addr == 0 {
            graph.root.to_node()
        } else {
            row.node.clone()
        };
        let content = crate::transit::encode_document(&node);
        if content == row.original_content {
            report.rows_byte_identical += 1;
        }
        let addresses = row
            .addresses
            .as_ref()
            .map(|a| serde_json::to_string(a).expect("a Vec<i64> always serializes"));
        txn.execute(
            "INSERT INTO kvs (addr, content, addresses) VALUES (?1, ?2, ?3)",
            libsql::params![row.addr, content, addresses],
        )
        .await
        .map_err(write_error)?;
        report.rows_written += 1;
    }
    txn.commit().await.map_err(write_error)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transit::decode_document;

    /// A minimal addr-0 document with `overrides` splice into its key list.
    fn root_doc(extra: &str) -> String {
        format!(
            r#"["^ ","~:schema",["^ "],"~:eavt",10,"~:aevt",20,"~:avet",30,\
"~:eavt-metadata",["^ ","~:count",1,"~:shift",2],\
"~:aevt-metadata",["^ ","~:count",1,"~:shift",2],\
"~:avet-metadata",["^ ","~:count",1,"~:shift",2],\
"~:max-tx",5,"~:max-eid",6,"~:max-addr",7{extra}]"#
        )
        .replace("\\\n", "")
    }

    fn parse(extra: &str) -> Result<RootNode, RowError> {
        RootNode::parse(&decode_document(&root_doc(extra)).expect("test document decodes"))
    }

    const PINNED: &str = r#","~:branching-factor",32,"~:ref-type","~:strong""#;

    #[test]
    fn a_pinned_root_node_parses() {
        let root = parse(PINNED).expect("the pinned shape is accepted");
        assert_eq!(root.branching_factor, 32);
        assert_eq!(root.ref_type, "strong");
        assert_eq!(root.max_addr, 7);
        assert_eq!(root.eavt_metadata, IndexMeta { count: 1, shift: 2 });
    }

    #[test]
    fn a_different_branching_factor_is_refused() {
        let err = parse(r#","~:branching-factor",64,"~:ref-type","~:strong""#)
            .expect_err("a 64-way tree is not the tree this writer builds");
        assert!(
            matches!(err, RowError::BranchingFactor { found: 64 }),
            "got {err:?}"
        );
    }

    #[test]
    fn a_weak_ref_type_is_refused() {
        let err = parse(r#","~:branching-factor",32,"~:ref-type","~:weak""#)
            .expect_err("only :strong refs are understood");
        assert!(matches!(err, RowError::RefType { .. }), "got {err:?}");
    }

    /// The one in-file signal that the DataScript storage layout moved.
    #[test]
    fn an_unknown_root_key_is_refused() {
        let err =
            parse(r#","~:branching-factor",32,"~:ref-type","~:strong","~:bloom-filter",["^ "]"#)
                .expect_err("an unrecognised storage key must stop the writer");
        match err {
            RowError::UnknownRootKey { key } => assert_eq!(key, "bloom-filter"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_missing_root_key_is_refused() {
        let err = parse(r#","~:branching-factor",32"#).expect_err("ref-type is required");
        match err {
            RowError::MissingRootKey { key } => assert_eq!(key, "ref-type"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_root_key_of_the_wrong_type_is_refused() {
        let doc = root_doc(PINNED).replace(r#""~:max-eid",6"#, r#""~:max-eid","six""#);
        let err = RootNode::parse(&decode_document(&doc).expect("decodes"))
            .expect_err("max-eid must be an integer");
        assert!(matches!(err, RowError::RootKeyType { .. }), "got {err:?}");
    }

    /// Re-emitting addr 0 replays the authored key order, not this struct's
    /// field order — the tree does not care, but a reordered root is a
    /// gratuitous diff against LogSeq's own bytes on every write.
    ///
    /// Asserted as node identity plus a round-trip rather than against a
    /// hand-written document: the encoder emits `^N` back-references for the
    /// repeated `:count`/`:shift` keys, so hand-written bytes would only test
    /// how faithfully the fixture was transcribed. Byte-agreement with LogSeq
    /// is measured on the real graph instead.
    #[test]
    fn the_root_node_re_emits_in_its_authored_key_order() {
        let node = decode_document(&root_doc(PINNED)).expect("decodes");
        let root = RootNode::parse(&node).expect("parses");
        assert_eq!(root.to_node(), node, "re-emission must replay the document");

        let re_encoded = crate::transit::encode_document(&root.to_node());
        assert_eq!(decode_document(&re_encoded).expect("re-decodes"), node);

        let TransitNode::Map(pairs) = &root.to_node() else {
            panic!("addr 0 is a map")
        };
        let keys: Vec<&str> = pairs
            .iter()
            .filter_map(|(k, _)| keyword(k))
            .take(4)
            .collect();
        assert_eq!(keys, ["schema", "eavt", "aevt", "avet"]);
    }

    #[test]
    fn an_addresses_column_that_is_not_an_integer_array_is_refused() {
        assert!(parse_addresses(9, "[1,2,3]").is_ok());
        for bad in [r#"{"a":1}"#, "[1,\"two\"]", "not json", "[1.5]"] {
            let err = parse_addresses(9, bad)
                .expect_err("only a JSON array of integers is a child-pointer list");
            assert!(matches!(err, RowError::BadAddresses { addr: 9, .. }));
        }
    }

    // ----------------------------------------------------------- the tail

    fn datom(entity: i64, tx: i64, op: DatomOp) -> TailDatom {
        TailDatom {
            entity,
            attribute: "block/title".to_string(),
            value: TransitNode::Str(format!("v{entity}")),
            tx: TxId::new(tx).expect("positive"),
            op,
        }
    }

    fn tail_of(datoms: usize) -> Tail {
        let mut tail = Tail::default();
        tail.push_transaction(
            (0..datoms)
                .map(|i| datom(i as i64, 100, DatomOp::Assert))
                .collect(),
        )
        .expect("building the starting state is not the thing under test");
        tail
    }

    #[test]
    fn an_empty_tail_round_trips_through_transit() {
        let empty = decode_document("[]").expect("decodes");
        let tail = Tail::parse(&empty).expect("an empty list is an empty tail");
        assert_eq!(tail.datom_count(), 0);
        assert_eq!(tail.to_node(), empty, "an empty tail re-emits as []");
    }

    /// The sign of tx IS the assert/retract flag, so it must survive a round
    /// trip in both directions or a retraction silently becomes an assertion.
    #[test]
    fn the_sign_of_tx_carries_assert_versus_retract() {
        let doc = r#"[[[5,"~:block/title","old",-7],[5,"~:block/title","new",7]]]"#;
        let node = decode_document(doc).expect("decodes");
        let tail = Tail::parse(&node).expect("parses");
        let tx = &tail.transactions()[0];
        assert_eq!(tx[0].op, DatomOp::Retract);
        assert_eq!(tx[0].tx.get(), 7, "the magnitude is the transaction id");
        assert_eq!(tx[1].op, DatomOp::Assert);
        // Structural, not textual: the encoder emits `^0` for the repeated
        // keyword, so re-emitted BYTES differ from this hand-written document
        // while denoting the same thing. The signed tx slots are what matter.
        assert_eq!(
            tail.to_node(),
            node,
            "re-emission must reproduce the signed form"
        );
    }

    /// Exactly at the branching factor is still writable — LogSeq flushes when
    /// the count EXCEEDS it, not when it reaches it.
    #[test]
    fn a_tail_filled_to_the_branching_factor_is_accepted() {
        let limit = PINNED_BRANCHING_FACTOR as usize;
        let mut tail = tail_of(limit - 1);
        tail.push_transaction(vec![datom(99, 101, DatomOp::Assert)])
            .expect("reaching the branching factor exactly is allowed");
        assert_eq!(tail.datom_count(), limit);
    }

    #[test]
    fn a_tail_that_would_exceed_the_branching_factor_is_refused() {
        let limit = PINNED_BRANCHING_FACTOR as usize;
        let mut tail = tail_of(limit);
        let err = tail
            .push_transaction(vec![datom(99, 101, DatomOp::Assert)])
            .expect_err("one datom past the branching factor must refuse");
        match err {
            RowError::TailOverflow {
                existing,
                adding,
                limit: reported,
            } => {
                assert_eq!((existing, adding, reported), (limit, 1, limit));
                let text = RowError::TailOverflow {
                    existing,
                    adding,
                    limit: reported,
                }
                .to_string();
                assert!(text.contains("Open the graph in LogSeq once"), "{text}");
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(
            tail.datom_count(),
            limit,
            "a refused push must not have half-applied"
        );
    }

    /// The realistic case: LogSeq left datoms in the tail before Holon arrived.
    #[test]
    fn an_already_non_empty_tail_counts_against_the_budget() {
        let limit = PINNED_BRANCHING_FACTOR as usize;
        let mut tail = tail_of(limit - 1);
        assert_eq!(tail.datom_count(), limit - 1);
        let err = tail
            .push_transaction(vec![
                datom(98, 101, DatomOp::Retract),
                datom(99, 101, DatomOp::Assert),
            ])
            .expect_err("a 2-datom edit does not fit in 1 remaining slot");
        assert!(
            matches!(err, RowError::TailOverflow { existing, adding, .. } if existing == limit - 1 && adding == 2),
            "got {err:?}"
        );
    }

    /// The silent-loss hazard, pinned. LogSeq's `db-with-tail-datoms` wraps its
    /// replay in `(catch :default _ db)`, so a tail it cannot parse is dropped
    /// with no error anywhere. Holon must refuse the shape instead.
    #[test]
    fn a_tail_datom_of_the_wrong_shape_is_refused_rather_than_silently_lost() {
        for (doc, why) in [
            (r#"[[[5,"~:block/title","new"]]]"#, "three slots"),
            (r#"[[[5,"block/title","new",7]]]"#, "attribute is a string"),
            (
                r#"[[["five","~:block/title","new",7]]]"#,
                "entity is a string",
            ),
            (r#"[[[5,"~:block/title","new",0]]]"#, "tx has no sign"),
            (
                r#"[[5,"~:block/title","new",7]]"#,
                "datom not wrapped in a tx",
            ),
        ] {
            let node = decode_document(doc).expect("the document itself is valid Transit");
            assert!(
                Tail::parse(&node).is_err(),
                "{why}: {doc} must be refused, or LogSeq would drop it in silence"
            );
        }
    }

    // ------------------------------------------- the schema-version refusal

    fn row(addr: i64, doc: &str) -> KvsRow {
        KvsRow {
            addr,
            node: decode_document(doc).expect("test document decodes"),
            addresses: None,
            original_content: doc.to_string(),
        }
    }

    /// A graph carrying one kv entity whose version map is `major.minor`.
    ///
    /// Entity 8 is `:logseq.kv/graph-initial-schema-version` and is left at
    /// the pinned version throughout, so every test below proves the guard
    /// read the entity it was asked for rather than whichever came first.
    fn graph_at(major: i64, minor: i64) -> Vec<KvsRow> {
        let pinned = PINNED_SCHEMA_VERSION;
        vec![row(
            1000,
            &format!(
                r#"["^ ","~:keys",[[8,"~:db/ident","~:logseq.kv/graph-initial-schema-version",1],[8,"~:kv/value",["^ ","~:major",{},"~:minor",{}],1],[7,"~:db/ident","~:logseq.kv/schema-version",1],[7,"~:kv/value",["^ ","~:major",{major},"~:minor",{minor}],1]]]"#,
                pinned.major, pinned.minor
            )
            .replace("\\\n", ""),
        )]
    }

    #[test]
    fn the_pinned_version_is_accepted() {
        let rows = graph_at(PINNED_SCHEMA_VERSION.major, PINNED_SCHEMA_VERSION.minor);
        assert_eq!(schema_version(&rows).expect("reads"), PINNED_SCHEMA_VERSION);
        assert_eq!(
            assert_pinned_schema_version(&rows).expect("the pinned version is writable"),
            PINNED_SCHEMA_VERSION
        );
    }

    /// A minor bump is NOT cosmetic: most shipped 65.x migrations rewrite user
    /// datoms, so this must refuse as hard as a major bump does.
    #[test]
    fn one_minor_ahead_is_refused() {
        let rows = graph_at(PINNED_SCHEMA_VERSION.major, PINNED_SCHEMA_VERSION.minor + 1);
        let err = assert_pinned_schema_version(&rows).expect_err("a newer minor must refuse");
        match err {
            RowError::SchemaVersionMismatch { found } => {
                assert_eq!(found.minor, PINNED_SCHEMA_VERSION.minor + 1);
                // The message must name BOTH versions, or the operator cannot
                // tell what to re-pin to.
                let text = RowError::SchemaVersionMismatch { found }.to_string();
                assert!(text.contains(&found.to_string()), "{text}");
                assert!(text.contains(&PINNED_SCHEMA_VERSION.to_string()), "{text}");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn one_major_ahead_is_refused() {
        let rows = graph_at(PINNED_SCHEMA_VERSION.major + 1, PINNED_SCHEMA_VERSION.minor);
        let err = assert_pinned_schema_version(&rows).expect_err("a newer major must refuse");
        assert!(
            matches!(err, RowError::SchemaVersionMismatch { found } if found.major == PINNED_SCHEMA_VERSION.major + 1),
            "got {err:?}"
        );
    }

    /// An OLDER graph is refused too. The guard is equality, not a floor:
    /// LogSeq would migrate it forward, and this build cannot.
    #[test]
    fn one_minor_behind_is_also_refused() {
        let rows = graph_at(PINNED_SCHEMA_VERSION.major, PINNED_SCHEMA_VERSION.minor - 1);
        assert!(
            matches!(
                assert_pinned_schema_version(&rows),
                Err(RowError::SchemaVersionMismatch { .. })
            ),
            "an older graph must refuse as well, or the pin is a floor"
        );
    }

    /// No version datom = LogSeq's version 0 (migrate.cljs:551), i.e. every
    /// 65.x migration still un-applied.
    #[test]
    fn a_graph_without_the_version_datom_is_refused() {
        let rows = vec![row(
            1000,
            r#"["^ ","~:keys",[[1,"~:block/title","hello",1]]]"#,
        )];
        assert!(
            matches!(
                assert_pinned_schema_version(&rows),
                Err(RowError::SchemaVersionMissing)
            ),
            "a graph with no version datom must refuse"
        );
    }

    #[test]
    fn a_version_datom_that_is_not_a_major_minor_map_is_refused() {
        let rows = vec![row(
            1000,
            r#"["^ ","~:keys",[[7,"~:db/ident","~:logseq.kv/schema-version",1],[7,"~:kv/value","65.33",1]]]"#,
        )];
        let err = assert_pinned_schema_version(&rows).expect_err("a string version is malformed");
        assert!(
            matches!(err, RowError::SchemaVersionMalformed { .. }),
            "got {err:?}"
        );
    }

    /// Child pointers in the content AND the column: LogSeq's restore lets the
    /// column win, so the two disagreeing silently reshapes the tree.
    #[test]
    fn a_node_carrying_addresses_in_its_content_is_detected() {
        let with = decode_document(r#"["^ ","~:addresses",[1,2]]"#).expect("decodes");
        assert!(carries_addresses(&with));
        let without = decode_document(r#"["^ ","~:keys",[]]"#).expect("decodes");
        assert!(!carries_addresses(&without));
    }
}
