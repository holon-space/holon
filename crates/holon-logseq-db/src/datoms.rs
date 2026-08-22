//! `kvs` rows → deduped, typed datoms.
//!
//! A LogSeq DB graph stores a persistent DataScript B+-tree in the `kvs`
//! table. Addr 0 is the schema/root node; every other addr is a tree node,
//! either a branch (no `:keys` datom list) or a leaf whose `:keys` entry holds
//! datom tuples. The same datom appears in all three index trees (EAV/AEV/AVE),
//! so the leaf stream is ~3.1x redundant and dedup on the full `(e, a, v, tx)`
//! tuple is what recovers the true datom set.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use libsql::Builder;
use libsql::OpenFlags;

use crate::Eid;
use crate::ImportError;
use crate::TransitNode;
use crate::kvs_writer::DatomOp;
use crate::kvs_writer::Tail;
use crate::transit::decode_document;

/// The transaction slot of a datom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tx(pub i64);

/// A datom attribute, parsed against the graph's own schema declaration.
///
/// The named variants are the spine the Block projection understands; `Raw`
/// carries an attribute the schema declares but the projection does not map,
/// which reaches a Block as a `_logseq_raw/*` property rather than being
/// dropped. An attribute the schema does not declare at all is not a `Raw` —
/// it is a loud [`ImportError::UnknownAttr`], because it means we are reading
/// a graph whose vocabulary we have not seen.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LogseqAttr {
    Uuid,
    Parent,
    Page,
    Order,
    Title,
    Name,
    Tags,
    Refs,
    Link,
    JournalDay,
    CreatedAt,
    UpdatedAt,
    Collapsed,
    /// `:db/ident` — the self-identifying name of a schema or config entity.
    DbIdent,
    /// `:kv/value` — the payload of a `:logseq.kv/*` config singleton.
    KvValue,
    Raw(String),
}

impl LogseqAttr {
    /// The original LogSeq keyword, leading `:` included.
    pub fn ident(&self) -> &str {
        match self {
            Self::Uuid => ":block/uuid",
            Self::Parent => ":block/parent",
            Self::Page => ":block/page",
            Self::Order => ":block/order",
            Self::Title => ":block/title",
            Self::Name => ":block/name",
            Self::Tags => ":block/tags",
            Self::Refs => ":block/refs",
            Self::Link => ":block/link",
            Self::JournalDay => ":block/journal-day",
            Self::CreatedAt => ":block/created-at",
            Self::UpdatedAt => ":block/updated-at",
            Self::Collapsed => ":block/collapsed?",
            Self::DbIdent => ":db/ident",
            Self::KvValue => ":kv/value",
            Self::Raw(ident) => ident,
        }
    }

    fn parse(ident: &str, schema: &Schema) -> Result<Self, ImportError> {
        Ok(match ident {
            ":block/uuid" => Self::Uuid,
            ":block/parent" => Self::Parent,
            ":block/page" => Self::Page,
            ":block/order" => Self::Order,
            ":block/title" => Self::Title,
            ":block/name" => Self::Name,
            ":block/tags" => Self::Tags,
            ":block/refs" => Self::Refs,
            ":block/link" => Self::Link,
            ":block/journal-day" => Self::JournalDay,
            ":block/created-at" => Self::CreatedAt,
            ":block/updated-at" => Self::UpdatedAt,
            ":block/collapsed?" => Self::Collapsed,
            ":db/ident" => Self::DbIdent,
            ":kv/value" => Self::KvValue,
            _ if schema.declares(ident) => Self::Raw(ident.to_string()),
            _ => {
                return Err(ImportError::UnknownAttr {
                    attr: ident.to_string(),
                });
            }
        })
    }
}

/// A datom value. Reference-typed attributes carry an entity id rather than a
/// bare integer, so the projection cannot confuse a parent pointer with a
/// number — the schema decides which is which, not the call site.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DatomValue {
    Ref(Eid),
    Node(TransitNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogseqDatom {
    pub e: Eid,
    pub a: LogseqAttr,
    pub v: DatomValue,
    /// Absent on datoms whose leaf tuple carries only three slots.
    pub tx: Option<Tx>,
}

/// The graph's own attribute declarations, read from the addr-0 schema node.
#[derive(Debug)]
pub struct Schema {
    declared: HashSet<String>,
    ref_typed: HashSet<String>,
    cardinality_many: HashSet<String>,
}

/// DataScript's meta-attributes describe the schema itself and so are never
/// listed inside it. They appear as datoms on property-definition entities, so
/// the vocabulary check must admit them or a healthy graph fails to import.
const SELF_DESCRIBING_NAMESPACE: &str = ":db/";

impl Schema {
    fn declares(&self, ident: &str) -> bool {
        self.declared.contains(ident) || ident.starts_with(SELF_DESCRIBING_NAMESPACE)
    }

    fn is_ref(&self, ident: &str) -> bool {
        self.ref_typed.contains(ident)
    }

    /// Whether an attribute may hold several values at once. DataScript's
    /// default is cardinality-one, and an undeclared attribute (the `:db/*`
    /// meta-vocabulary) is one as well.
    ///
    /// This distinction is load-bearing, not bookkeeping: a cardinality-ONE
    /// attribute can still carry several datoms, one per transaction that
    /// changed it (`:block/updated-at` does, on the fixture). Only the
    /// highest-tx datom is the current value, so treating cardinality-one as
    /// "there is exactly one datom" silently resurrects stale content.
    pub fn is_cardinality_many(&self, ident: &str) -> bool {
        self.cardinality_many.contains(ident)
    }

    /// Parse the addr-0 root node. Its `:schema` map holds one entry per
    /// declared attribute (keyword key → attribute definition map) plus an
    /// integer intern table (id → attribute keyword) that we do not need,
    /// since datom attributes arrive as keywords.
    fn parse(root: &TransitNode) -> Result<Self, ImportError> {
        let TransitNode::Map(entries) = root else {
            return Err(ImportError::MalformedSchema {
                detail: format!("addr-0 node is {}, expected a map", node_kind(root)),
            });
        };
        let schema = entries
            .iter()
            .find(|(k, _)| matches!(k, TransitNode::Keyword(name) if name == "schema"))
            .map(|(_, v)| v)
            .ok_or_else(|| ImportError::MalformedSchema {
                detail: "addr-0 node has no :schema entry".to_string(),
            })?;
        let TransitNode::Map(declarations) = schema else {
            return Err(ImportError::MalformedSchema {
                detail: format!(":schema is {}, expected a map", node_kind(schema)),
            });
        };

        let mut declared = HashSet::new();
        let mut ref_typed = HashSet::new();
        let mut cardinality_many = HashSet::new();
        for (key, definition) in declarations {
            let TransitNode::Keyword(name) = key else {
                continue; // the integer intern table
            };
            let ident = format!(":{name}");
            if declares(definition, "db/valueType", "db.type/ref") {
                ref_typed.insert(ident.clone());
            }
            if declares(definition, "db/cardinality", "db.cardinality/many") {
                cardinality_many.insert(ident.clone());
            }
            declared.insert(ident);
        }
        Ok(Self {
            declared,
            ref_typed,
            cardinality_many,
        })
    }
}

/// Whether an attribute definition sets `field` to `expected`.
fn declares(definition: &TransitNode, field: &str, expected: &str) -> bool {
    let TransitNode::Map(fields) = definition else {
        return false;
    };
    fields.iter().any(|(k, v)| {
        matches!(k, TransitNode::Keyword(name) if name == field)
            && matches!(v, TransitNode::Keyword(name) if name == expected)
    })
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

/// How an entity is classified by the datoms it carries. The three arms
/// partition the entity set exactly — nothing falls outside, which is what the
/// keystone's totality assertion checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// Carries `:block/uuid` — projects to a Holon Block.
    Block,
    /// A uuid-less `:logseq.kv/*` config singleton (`:db/ident` + `:kv/value`).
    KvSingleton,
    /// Carries a uuid, but LogSeq calls the entity its OWN — a property
    /// definition, a class page, a `$$$`-hidden system page, a shipped file.
    ///
    /// Deliberately a KIND rather than a filter applied later: the projection
    /// emits a Block for every `Block`-kind entity, so making built-ins a
    /// different kind is what makes "a built-in materialized as a user block"
    /// unrepresentable instead of merely unwanted. They are still READ — the
    /// datoms stay in the set, so property and class names still resolve.
    ///
    /// See docs/Testing/bugfunnel/entries/
    /// 2026-08-22-importer-materializes-logseq-built-ins-as-blocks.md.
    BuiltIn,
    /// Uuid-less and not a config singleton: LogSeq's own half-created
    /// remnants (a bare `:block/created-at`, sometimes an empty title). They
    /// carry no identity, so they cannot become Blocks — recorded, not
    /// dropped, and never silently folded in with the config singletons.
    Orphan,
}

/// The deduped datom set of one graph, plus the entity partition.
#[derive(Debug)]
pub struct DatomSet {
    /// Deduped and ordered — `BTreeSet` collection makes the sequence a
    /// function of the graph alone, so downstream projection is reproducible.
    pub datoms: Vec<LogseqDatom>,
    /// Leaf tuples seen before dedup (index-tree redundancy ≈ 3.1x).
    pub leaf_datoms: usize,
    pub entities: HashMap<Eid, EntityKind>,
    /// What the PAGE leg of the built-in rule excluded — disclosed, never
    /// inferred from a count.
    pub built_in_exclusions: BuiltInExclusions,
    /// The graph's own attribute declarations — the authority the projection
    /// consults for cardinality rather than assuming per attribute.
    pub schema: Schema,
}

impl DatomSet {
    pub fn count_kind(&self, kind: EntityKind) -> usize {
        self.entities.values().filter(|k| **k == kind).count()
    }

    pub fn distinct_attrs(&self) -> usize {
        self.datoms
            .iter()
            .map(|d| d.a.ident())
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn uuid_datoms(&self) -> usize {
        self.datoms
            .iter()
            .filter(|d| d.a == LogseqAttr::Uuid)
            .count()
    }
}

/// Read every `kvs` row of the graph at `path`, opened **read-only**.
///
/// The read-only flag is the enforcement of the stage-1 no-write rule, not a
/// hint: SQLite itself refuses a write on this handle. It also means a live
/// LogSeq graph (hot WAL, which needs a writable `-shm`) fails to open — the
/// caller must import a snapshot copy, which [`ImportError::Locked`] says.
async fn read_kvs_rows(path: &Path) -> Result<Vec<(i64, String)>, ImportError> {
    let open_error = |source: libsql::Error| {
        // A hot WAL or an exclusive lock is not a corrupt file; it is LogSeq
        // running. Say so instead of surfacing a raw sqlite code.
        if matches!(
            source,
            libsql::Error::SqliteFailure(5 | 6 | 8, _) // BUSY | LOCKED | READONLY
        ) {
            ImportError::Locked {
                path: path.to_path_buf(),
            }
        // NOTADB is the opposite diagnosis: the bytes are not a database at
        // all. Folding it into `Locked` would send the reader off to make a
        // snapshot copy, which cannot help, and would hide real corruption.
        } else if matches!(source, libsql::Error::SqliteFailure(26, _)) {
            ImportError::Corrupt {
                path: path.to_path_buf(),
            }
        } else {
            ImportError::Open {
                path: path.to_path_buf(),
                source: source.into(),
            }
        }
    };

    let db = Builder::new_local(path)
        .flags(OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .map_err(open_error)?;
    let conn = db.connect().map_err(open_error)?;
    let mut rows = conn
        .query("SELECT addr, content FROM kvs ORDER BY addr", ())
        .await
        .map_err(open_error)?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(open_error)? {
        let addr: i64 = row.get(0).map_err(open_error)?;
        let content: String = row.get(1).map_err(open_error)?;
        out.push((addr, content));
    }
    Ok(out)
}

/// Decode the graph at `path` into its deduped datom set.
pub async fn read_datoms(path: &Path) -> Result<DatomSet, ImportError> {
    let rows = read_kvs_rows(path).await?;
    let (root_addr, root_content) =
        rows.first().filter(|(addr, _)| *addr == 0).ok_or_else(|| {
            ImportError::MalformedSchema {
                detail: "the kvs table has no addr-0 root node".to_string(),
            }
        })?;
    let root = decode_document(root_content).map_err(|source| ImportError::Decode {
        addr: *root_addr,
        source,
    })?;
    let schema = Schema::parse(&root)?;

    let mut deduped = BTreeSet::new();
    let mut leaf_datoms = 0usize;
    // Addr 1 is the transaction tail, not a tree node — it is replayed OVER the
    // trees below, so reading it here as well would double-count it.
    for (addr, content) in rows
        .iter()
        .filter(|(addr, _)| *addr > 0 && *addr != TAIL_ADDR)
    {
        let node = decode_document(content).map_err(|source| ImportError::Decode {
            addr: *addr,
            source,
        })?;
        for tuple in leaf_tuples(&node) {
            leaf_datoms += 1;
            deduped.insert(parse_datom(tuple, &schema, *addr)?);
        }
    }

    apply_tail(&mut deduped, &rows, &schema)?;

    let datoms: Vec<LogseqDatom> = deduped.into_iter().collect();
    let (entities, built_in_exclusions) = classify_entities(&datoms)?;
    Ok(DatomSet {
        datoms,
        leaf_datoms,
        entities,
        built_in_exclusions,
        schema,
    })
}

/// The kvs address of the transaction tail. Fixed by DataScript.
const TAIL_ADDR: i64 = 1;

/// Replay the transaction tail over the datoms restored from the trees.
///
/// DataScript does not rewrite its index trees for a small transaction: the
/// datoms go into a log at addr 1, and every reader — `restore-conn`, and
/// therefore LogSeq itself — applies that log on top of the trees. A reader
/// that skips it sees the graph as it was before the most recent edits, which
/// is stale rather than wrong-looking, and so shows up as nothing at all.
///
/// The tail is ordered, and later entries win: a retraction removes the datom
/// with that `(e, a, v)` whatever transaction first asserted it, and an
/// assertion adds one under the tail's own transaction id.
fn apply_tail(
    deduped: &mut BTreeSet<LogseqDatom>,
    rows: &[(i64, String)],
    schema: &Schema,
) -> Result<(), ImportError> {
    let Some((_, content)) = rows.iter().find(|(addr, _)| *addr == TAIL_ADDR) else {
        // No tail row at all is a graph LogSeq has never written through its
        // storage layer; the trees are the whole story.
        return Ok(());
    };
    let node = decode_document(content).map_err(|source| ImportError::Decode {
        addr: TAIL_ADDR,
        source,
    })?;
    let tail = Tail::parse(&node).map_err(|source| ImportError::Tail { source })?;

    for transaction in tail.transactions() {
        for entry in transaction {
            // Rebuilt with a POSITIVE tx: the sign carried the operation, which
            // `entry.op` now holds, and `parse_datom` wants the id itself.
            let tuple = TransitNode::List(vec![
                TransitNode::Int(entry.entity),
                TransitNode::Keyword(entry.attribute.clone()),
                entry.value.clone(),
                TransitNode::Int(entry.tx.get()),
            ]);
            let datom = parse_datom(&tuple, schema, TAIL_ADDR)?;
            match entry.op {
                DatomOp::Assert => {
                    deduped.insert(datom);
                }
                DatomOp::Retract => {
                    deduped.retain(|held| {
                        !(held.e == datom.e && held.a == datom.a && held.v == datom.v)
                    });
                }
            }
        }
    }
    Ok(())
}

/// The datom tuples of one tree node. A node without a `:keys` list is a
/// branch node of the B+-tree and holds no datoms — the only structural skip
/// in this pipeline, and it drops nothing, because branch nodes carry
/// separators rather than data.
fn leaf_tuples(node: &TransitNode) -> &[TransitNode] {
    let TransitNode::Map(entries) = node else {
        return &[];
    };
    entries
        .iter()
        .find(|(k, _)| matches!(k, TransitNode::Keyword(name) if name == "keys"))
        .and_then(|(_, v)| match v {
            TransitNode::List(items) => Some(items.as_slice()),
            _ => None,
        })
        .unwrap_or(&[])
}

fn parse_datom(
    tuple: &TransitNode,
    schema: &Schema,
    addr: i64,
) -> Result<LogseqDatom, ImportError> {
    let TransitNode::List(slots) = tuple else {
        return Err(ImportError::MalformedDatom {
            addr,
            detail: format!("datom tuple is {}, expected a list", node_kind(tuple)),
        });
    };
    if slots.len() < 3 {
        return Err(ImportError::MalformedDatom {
            addr,
            detail: format!("datom tuple has {} slots, expected at least 3", slots.len()),
        });
    }
    let TransitNode::Int(e) = slots[0] else {
        return Err(ImportError::MalformedDatom {
            addr,
            detail: format!(
                "entity slot is {}, expected an integer",
                node_kind(&slots[0])
            ),
        });
    };
    let TransitNode::Keyword(ref name) = slots[1] else {
        return Err(ImportError::MalformedDatom {
            addr,
            detail: format!(
                "attribute slot is {}, expected a keyword",
                node_kind(&slots[1])
            ),
        });
    };
    let ident = format!(":{name}");
    let a = LogseqAttr::parse(&ident, schema)?;

    let v = if schema.is_ref(&ident) {
        match slots[2] {
            TransitNode::Int(target) => DatomValue::Ref(Eid(target)),
            ref other => {
                return Err(ImportError::MalformedDatom {
                    addr,
                    detail: format!(
                        "{ident} is reference-typed but its value is {}",
                        node_kind(other)
                    ),
                });
            }
        }
    } else {
        DatomValue::Node(slots[2].clone())
    };

    let tx = match slots.get(3) {
        None => None,
        Some(TransitNode::Int(tx)) => Some(Tx(*tx)),
        Some(other) => {
            return Err(ImportError::MalformedDatom {
                addr,
                detail: format!("tx slot is {}, expected an integer", node_kind(other)),
            });
        }
    };

    Ok(LogseqDatom {
        e: Eid(e),
        a,
        v,
        tx,
    })
}

fn classify_entities(
    datoms: &[LogseqDatom],
) -> Result<(HashMap<Eid, EntityKind>, BuiltInExclusions), ImportError> {
    let mut has_uuid: HashSet<Eid> = HashSet::new();
    let mut kv_ident: HashSet<Eid> = HashSet::new();
    let mut built_in: HashSet<Eid> = HashSet::new();
    let mut page_of: HashMap<Eid, Eid> = HashMap::new();
    let mut children_of: HashMap<Eid, Vec<Eid>> = HashMap::new();
    let mut all: HashSet<Eid> = HashSet::new();
    for datom in datoms {
        all.insert(datom.e);
        match (&datom.a, &datom.v) {
            (LogseqAttr::Uuid, _) => {
                has_uuid.insert(datom.e);
            }
            (LogseqAttr::DbIdent, DatomValue::Node(TransitNode::Keyword(name)))
                if name.starts_with("logseq.kv/") =>
            {
                kv_ident.insert(datom.e);
                built_in.insert(datom.e);
            }
            // The same three legs as `kvs_writer::is_built_in`, read off
            // LogSeq's datoms instead of the writer's trees. The namespace
            // rule itself is NOT restated — it is called — so the two readers
            // cannot drift on the part that was hardest to get right.
            (LogseqAttr::DbIdent, DatomValue::Node(TransitNode::Keyword(name)))
                if crate::kvs_writer::is_internal_ident(name) =>
            {
                built_in.insert(datom.e);
            }
            (LogseqAttr::Raw(ident), DatomValue::Node(TransitNode::Bool(true)))
                if ident == ":logseq.property/built-in?" =>
            {
                built_in.insert(datom.e);
            }
            (LogseqAttr::Raw(ident), _) if ident == ":file/path" => {
                built_in.insert(datom.e);
            }
            (LogseqAttr::Page, DatomValue::Ref(target)) => {
                page_of.insert(datom.e, *target);
            }
            (LogseqAttr::Parent, DatomValue::Ref(target)) => {
                children_of.entry(*target).or_default().push(datom.e);
            }
            _ => {}
        }
    }
    // An entity whose containing PAGE is LogSeq's is LogSeq's too. Measured:
    // the fixture's `$$$views` page holds four per-view UI records
    // (`:logseq.property.view/feature-type :linked-references` and friends)
    // that carry no built-in flag of their own — nobody authored a block
    // called "Linked references", LogSeq minted one per view it rendered.
    //
    // This is also what keeps the importer's fail-loud parent rule intact:
    // without it the projection cannot resolve those blocks' parent and
    // refuses the whole import with a DanglingReference.
    let mut excluded_under_built_in_page: Vec<Eid> = page_of
        .iter()
        .filter(|(e, page)| built_in.contains(page) && !built_in.contains(e))
        .map(|(e, _)| *e)
        .collect();
    excluded_under_built_in_page.sort();

    // Excluding a page-owned entity that has CHILDREN would drop whatever
    // hangs beneath it. Zero such entities exist today (measured), and if a
    // future graph grows one we refuse the import rather than lose content.
    for e in &excluded_under_built_in_page {
        if let Some(children) = children_of.get(e) {
            if !children.is_empty() {
                let mut children = children.clone();
                children.sort();
                return Err(ImportError::BuiltInPageChildHasChildren {
                    entity: e.0,
                    page: page_of[e].0,
                    children: children.iter().map(|c| c.0).collect(),
                });
            }
        }
    }
    built_in.extend(excluded_under_built_in_page.iter().copied());

    let kinds: HashMap<Eid, EntityKind> = all
        .into_iter()
        .map(|e| {
            // BuiltIn outranks Block and ONLY Block: a uuid-bearing entity
            // LogSeq owns never becomes a user block. KvSingleton and Orphan
            // are untouched — they were already not projected, and folding
            // them in here would lose a distinction the importer uses.
            let kind = if has_uuid.contains(&e) {
                if built_in.contains(&e) {
                    EntityKind::BuiltIn
                } else {
                    EntityKind::Block
                }
            } else if kv_ident.contains(&e) {
                EntityKind::KvSingleton
            } else {
                EntityKind::Orphan
            };
            (e, kind)
        })
        .collect();
    Ok((
        kinds,
        BuiltInExclusions {
            under_built_in_page: excluded_under_built_in_page,
        },
    ))
}

/// Which entities the PAGE leg excluded, so the import can disclose it.
///
/// Disclosed rather than silent: dropping content is exactly the kind of
/// thing that must be visible in the summary, not inferred from a count that
/// came out lower than expected.
#[derive(Debug, Clone, Default)]
pub struct BuiltInExclusions {
    pub under_built_in_page: Vec<Eid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of(doc: &str) -> Schema {
        Schema::parse(&decode_document(doc).expect("decode")).expect("parse schema")
    }

    #[test]
    fn schema_reads_declarations_and_ref_types() {
        let schema = schema_of(
            r#"["^ ","~:schema",["^ ",
                 "~:block/parent",["^ ","~:db/valueType","~:db.type/ref"],
                 "~:block/title",["^ ","~:db/valueType","~:db.type/string"],
                 "~i32","~:block/created-at"]]"#,
        );
        assert!(schema.declares(":block/parent"));
        assert!(schema.declares(":block/title"));
        assert!(schema.is_ref(":block/parent"));
        assert!(!schema.is_ref(":block/title"));
        // The integer intern table is not a declaration.
        assert!(!schema.declares(":block/created-at"));
    }

    #[test]
    fn db_namespace_is_declared_without_being_listed() {
        let schema = schema_of(r#"["^ ","~:schema",["^ "]]"#);
        assert!(schema.declares(":db/valueType"));
        assert!(schema.declares(":db/cardinality"));
        assert!(!schema.declares(":user.property/whatever"));
    }

    #[test]
    fn undeclared_attribute_is_a_loud_error() {
        let schema = schema_of(r#"["^ ","~:schema",["^ "]]"#);
        let err = LogseqAttr::parse(":user.property/nope", &schema)
            .expect_err("an undeclared attribute must stop the import");
        assert!(
            matches!(err, ImportError::UnknownAttr { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn declared_but_unmapped_attribute_becomes_a_raw_carrier() {
        let schema = schema_of(
            r#"["^ ","~:schema",["^ ","~:logseq.property/status",["^ ","~:db/index",true]]]"#,
        );
        assert_eq!(
            LogseqAttr::parse(":logseq.property/status", &schema).expect("declared"),
            LogseqAttr::Raw(":logseq.property/status".to_string())
        );
    }

    #[test]
    fn reference_typed_values_parse_to_entity_ids() {
        let schema = schema_of(
            r#"["^ ","~:schema",["^ ","~:block/parent",["^ ","~:db/valueType","~:db.type/ref"]]]"#,
        );
        let tuple = decode_document(r#"["~i203","~:block/parent","~i193","~i536871022"]"#)
            .expect("decode tuple");
        let datom = parse_datom(&tuple, &schema, 7).expect("parse datom");
        assert_eq!(datom.e, Eid(203));
        assert_eq!(datom.a, LogseqAttr::Parent);
        assert_eq!(datom.v, DatomValue::Ref(Eid(193)));
        assert_eq!(datom.tx, Some(Tx(536871022)));
    }

    #[test]
    fn a_three_slot_tuple_has_no_tx() {
        let schema = schema_of(r#"["^ ","~:schema",["^ "]]"#);
        let tuple = decode_document(r#"["~i1","~:block/title","hello"]"#).expect("decode tuple");
        let datom = parse_datom(&tuple, &schema, 7).expect("parse datom");
        assert_eq!(datom.tx, None);
        assert_eq!(
            datom.v,
            DatomValue::Node(TransitNode::Str("hello".to_string()))
        );
    }

    #[test]
    fn a_reference_attribute_with_a_non_integer_value_is_a_loud_error() {
        let schema = schema_of(
            r#"["^ ","~:schema",["^ ","~:block/parent",["^ ","~:db/valueType","~:db.type/ref"]]]"#,
        );
        let tuple = decode_document(r#"["~i1","~:block/parent","not-an-eid"]"#).expect("decode");
        let err = parse_datom(&tuple, &schema, 7).expect_err("a dangling ref must stop the import");
        assert!(
            matches!(err, ImportError::MalformedDatom { addr: 7, .. }),
            "got {err:?}"
        );
    }

    /// Dedup keys on the FULL `(e, a, v, tx)` tuple. The fixture cannot prove
    /// this: it holds zero `(e, a, v)` groups spanning more than one
    /// transaction, so 3-tuple and 4-tuple dedup both yield 2631 there and
    /// every asserted number stays green if `tx` is dropped from the key.
    /// This states the rule directly instead.
    #[test]
    fn two_transactions_of_the_same_value_are_two_datoms() {
        let at = |tx: i64| LogseqDatom {
            e: Eid(1),
            a: LogseqAttr::Title,
            v: DatomValue::Node(TransitNode::Str("same".into())),
            tx: Some(Tx(tx)),
        };
        let deduped: BTreeSet<LogseqDatom> = [at(100), at(200)].into_iter().collect();
        assert_eq!(
            deduped.len(),
            2,
            "datoms differing only in tx are distinct — dropping tx from the \
             dedup key would silently merge a value's history"
        );
        // ... and a genuine duplicate still collapses.
        let collapsed: BTreeSet<LogseqDatom> = [at(100), at(100)].into_iter().collect();
        assert_eq!(collapsed.len(), 1);
    }

    #[test]
    fn branch_nodes_yield_no_datoms() {
        let branch = decode_document(r#"["^ ","~:pointers",["~i1","~i2"]]"#).expect("decode");
        assert!(leaf_tuples(&branch).is_empty());
    }

    #[test]
    fn entities_partition_into_blocks_singletons_and_orphans() {
        let datoms = vec![
            LogseqDatom {
                e: Eid(1),
                a: LogseqAttr::Uuid,
                v: DatomValue::Node(TransitNode::Uuid("u".into())),
                tx: None,
            },
            LogseqDatom {
                e: Eid(2),
                a: LogseqAttr::DbIdent,
                v: DatomValue::Node(TransitNode::Keyword("logseq.kv/db-type".into())),
                tx: None,
            },
            LogseqDatom {
                e: Eid(3),
                a: LogseqAttr::CreatedAt,
                v: DatomValue::Node(TransitNode::Int(1)),
                tx: None,
            },
        ];
        let (kinds, _) = classify_entities(&datoms).expect("classifies");
        assert_eq!(kinds[&Eid(1)], EntityKind::Block);
        assert_eq!(kinds[&Eid(2)], EntityKind::KvSingleton);
        assert_eq!(kinds[&Eid(3)], EntityKind::Orphan);
    }
}
