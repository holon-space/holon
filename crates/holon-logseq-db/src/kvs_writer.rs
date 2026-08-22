//! Reading and re-writing the `kvs` table of a LogSeq DB graph.
//!
//! A DB graph is one SQLite table, `kvs(addr INTEGER PRIMARY KEY, content
//! TEXT, addresses JSON)`, holding a persistent DataScript B+-tree. This
//! module parses that table into typed rows, guards the storage parameters
//! Holon's writer is pinned to, and writes the rows back out to a **new**
//! file. It carries no datom semantics: the tree is replayed as it was read.
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

/// A whole graph's `kvs` table, decoded and guarded.
#[derive(Debug, Clone, PartialEq)]
pub struct KvsGraph {
    pub root: RootNode,
    /// Every row including addr 0, ordered by address.
    pub rows: Vec<KvsRow>,
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
