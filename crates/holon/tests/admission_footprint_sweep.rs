//! D3 of the admission-overlay plan, REPORT-ONLY: fire every catalog op once on
//! a fixture store, diff the store around it, and compare the observed effect
//! with the footprint the op's declared marking delta gives at admission.
//!
//! It writes `docs/Testing/AdmissionFootprintCensus.md` and asserts nothing
//! about the verdicts; Inc 3 turns the `outside` rows into failing tests.
//!
//! Footprint rule (SP8 overlay): a declared delta binds to ONE subject, the
//! entity its `id_column` param names, and writes the aspects whose flow moves
//! tokens. An undeclared op claims every entity URI in its params.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::OperationDescriptor;
use holon_api::PAGE_TAG;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::marking::ExistenceFlow;
use holon_api::marking::KindDelta;
use holon_api::marking::MarkingDelta;
use holon_api::marking::StructuralFlow;
use holon_api::marking::TextFlow;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";
const ROOT_PARENT: &str = "sentinel:no_parent";

/// Logs, projections and engine state: written as a consequence of any write,
/// never a token an op's footprint claims.
const BOOKKEEPING_TABLES: &[&str] = &[
    "block_derived",
    "block_history",
    "clock",
    "clock_reader",
    "file",
    "integration_cache",
    "integration_state",
    "local_ui_state",
    "operation",
    "sqlite_sequence",
    "sync_states",
    "undo_log",
    "watch_context",
];
const BOOKKEEPING_FIELDS: &[&str] = &["_change_origin", "created_at", "updated_at", "write_seq"];
const PLACEMENT_FIELDS: &[&str] = &["parent_id", "sort_key"];

/// The column that names the entity an id-less row belongs to.
const ROW_OWNER: &[(&str, &str)] = &[
    ("advice_suppressed", "anchor_id"),
    ("block_contributes_to", "block_id"),
    ("block_links", "source_block_id"),
    ("block_redirects", "from_id"),
    ("block_requires", "block_id"),
    ("block_tags", "block_id"),
    ("navigation_cursor", "region"),
];

const GESTURE_OPS: &[&str] = &[
    "create",
    "set_field",
    "split_block",
    "join_block",
    "indent",
    "outdent",
    "move_block",
    "delete",
    "delete_subtree",
    "delete_keep_children",
    "convert_block_to_page",
    "merge_blocks",
    "undo",
    "redo",
];

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    BLOCK.to_string(),
                    BLOCK.to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    BLOCK.to_string(),
                    BLOCK.to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
                    tokio::runtime::Handle::current()
                        .block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with the SqlOnly block providers")
}

/// The ids of one op's private fixture subtree, so every op runs on fresh
/// blocks inside one engine.
struct Fixture {
    prefix: String,
}

impl Fixture {
    fn id(&self, role: &str) -> String {
        format!("block:{}{role}", self.prefix)
    }

    async fn seed(&self, engine: &BackendEngine) {
        let page = self.id("page");
        let rows: [(&str, &str, &str, &[&str]); 5] = [
            ("page", ROOT_PARENT, "Page", &[PAGE_TAG]),
            ("a", &page, "alpha text", &["old"]),
            ("b", &page, "bravo", &[]),
            ("b1", &self.id("b"), "bravo child", &[]),
            ("c", &page, "charlie", &[]),
        ];
        for (role, parent, content, tags) in rows {
            let mut params: StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String(self.id(role)));
            params.insert("parent_id".into(), Value::String(parent.to_string()));
            let content = if role == "page" {
                format!("Page {}", self.prefix.trim_end_matches('-'))
            } else {
                content.to_string()
            };
            params.insert("content".into(), Value::String(content));
            if !tags.is_empty() {
                params.insert(
                    "tags".into(),
                    Value::Array(tags.iter().map(|t| Value::String(t.to_string())).collect()),
                );
            }
            engine
                .execute_operation(&EntityName::new(BLOCK), "create", params, OpOrigin::Sync)
                .await
                .unwrap_or_else(|e| panic!("seed {role} for {}: {e:#}", self.prefix));
        }
    }

    /// One binding per op: the gesture a user or agent most plausibly fires.
    fn params(&self, d: &OperationDescriptor) -> StorageEntity {
        let s = |v: &str| Value::String(v.to_string());
        let id = |r: &str| Value::String(self.id(r));
        let entity = d.entity_name.as_str();
        let pairs: Vec<(&str, Value)> = match (entity, d.name.as_str()) {
            ("block", "set_field") => vec![
                ("id", id("b")),
                ("field", s("content")),
                ("value", s("bravo edited")),
            ],
            ("block", "create") => vec![
                ("id", id("n")),
                ("parent_id", id("page")),
                ("content", s("new")),
            ],
            ("block", "update") => vec![("id", id("b")), ("content", s("bravo updated"))],
            ("block", "delete") => vec![("id", id("c"))],
            ("block", "cycle_task_state") => vec![("id", id("a"))],
            ("block", "create_page_from_link") => {
                vec![("target", s("Linked page"))]
            }
            ("block", "rewrite_link_resolution") => vec![("from", id("a")), ("to", id("b"))],
            ("block", "restore_link_resolution") => vec![("rows", s("[]"))],
            ("block", "block_to_page_plan") => vec![("target", id("b"))],
            ("block", "merge_blocks_plan") => vec![("canonical", id("a")), ("duplicate", id("c"))],
            ("block", "dismiss_advice") => vec![("anchor_id", id("a")), ("lesson_id", id("b"))],
            ("block", "add_tag") => vec![("id", id("b")), ("tag", s("fresh"))],
            ("block", "remove_tag") => vec![("id", id("a")), ("tag", s("old"))],
            ("block", "indent") => vec![("id", id("b"))],
            ("block", "outdent") => vec![("id", id("b1"))],
            ("block", "move_to_position") => vec![("id", id("c")), ("parent_id", id("a"))],
            ("block", "move_block") => vec![("id", id("c")), ("parent_id", id("a"))],
            ("block", "split_block") => vec![("id", id("a")), ("position", Value::Integer(5))],
            ("block", "join_block") => vec![("id", id("b")), ("position", Value::Integer(0))],
            ("block", "restore_split") => vec![
                ("target_id", id("a")),
                ("target_content", s("alpha text restored")),
                ("block_id", id("c")),
                ("block_content", s("charlie")),
                ("block_parent", Value::String(self.id("page"))),
            ],
            ("block", "restore_join") => vec![
                ("target_id", id("a")),
                ("target_content", s("alpha")),
                ("deleted_id", id("j")),
            ],
            ("block", "move_up") => vec![("id", id("b"))],
            ("block", "move_down") => vec![("id", id("a"))],
            ("block", "embed_entity") => vec![("id", id("a")), ("target_uri", id("c"))],
            ("block", "delete_subtree") => vec![("id", id("b"))],
            ("block", "delete_keep_children") => vec![("id", id("b"))],
            ("block", "instantiate_template") => vec![
                ("template_id", id("b")),
                ("target_parent", id("page")),
                ("context_key", s("k")),
            ],
            ("block", "convert_block_to_page") => vec![("target", id("b"))],
            ("block", "merge_blocks") => vec![("canonical", id("a")), ("duplicate", id("c"))],
            ("navigation", "focus" | "focus_pin" | "open_tab") => {
                vec![("region", s("main")), ("block_id", id("a"))]
            }
            ("navigation", "close") => vec![("history_id", Value::Integer(1))],
            ("navigation", "activate") => {
                vec![("region", s("main")), ("history_id", Value::Integer(1))]
            }
            ("navigation", _) => vec![("region", s("main"))],
            ("identity", "merge_entities") => vec![
                ("canonical_a", s("identity-a")),
                ("canonical_b", s("identity-b")),
            ],
            ("identity", _) => vec![
                ("id", s("identity-a")),
                ("kind", s("person")),
                ("evidence_json", s("{}")),
                ("created_at", Value::Integer(0)),
                ("status", s("open")),
                ("primary_label", s("A")),
                ("merged_into_id", s("identity-b")),
                ("alias_keys_json", s("[]")),
            ],
            (_, "create") => vec![
                ("id", Value::String(format!("{entity}:{}x", self.prefix))),
                ("name", s("x")),
            ],
            (_, "consume") => vec![
                ("id", Value::String(format!("{entity}:{}x", self.prefix))),
                ("quantity", Value::Integer(1)),
                ("unit", s("g")),
            ],
            (_, "set_field") => vec![
                ("id", Value::String(format!("{entity}:{}x", self.prefix))),
                ("field", s("name")),
                ("value", s("y")),
            ],
            (_, _) => vec![("id", Value::String(format!("{entity}:{}x", self.prefix)))],
        };
        pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
    }
}

/// One store row, keyed so a before/after pair can be matched.
type Rows = BTreeMap<(String, String), BTreeMap<String, String>>;

async fn snapshot(engine: &BackendEngine) -> Rows {
    let tables = engine
        .db_handle()
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '__turso_internal%' AND name NOT LIKE 'sqlite_%'",
            HashMap::new(),
        )
        .await
        .expect("list tables");
    let mut rows = Rows::new();
    for t in tables {
        let table = t["name"].as_string().expect("table name").to_string();
        if BOOKKEEPING_TABLES.contains(&table.as_str()) {
            continue;
        }
        for row in engine
            .db_handle()
            .query(&format!("SELECT * FROM {table}"), HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("snapshot {table}: {e:#}"))
        {
            let row: BTreeMap<String, String> = row
                .into_iter()
                .map(|(k, v)| (k.to_string(), render(&v)))
                .collect();
            let key = match row.get("id") {
                Some(id) => id.clone(),
                None => format!("{row:?}"),
            };
            rows.insert((table.clone(), key), row);
        }
    }
    rows
}

/// Map-valued columns come back as hash maps, so their key order is sorted
/// here; otherwise two reads of one row differ.
fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Json(s) => render_json(
            &serde_json::from_str(s).unwrap_or_else(|e| panic!("stored Json {s:?}: {e}")),
        ),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(render).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => {
            let sorted: BTreeMap<&String, String> =
                map.iter().map(|(k, v)| (k, render(v))).collect();
            format!("{sorted:?}")
        }
        other => format!("{other:?}"),
    }
}

fn render_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, String> =
                map.iter().map(|(k, v)| (k, render_json(v))).collect();
            format!("{sorted:?}")
        }
        serde_json::Value::Array(items) => {
            format!(
                "[{}]",
                items.iter().map(render_json).collect::<Vec<_>>().join(",")
            )
        }
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Aspect {
    Existence,
    Structural,
    Text,
}

/// One observed change: which entity, which aspect, and the column (or edge
/// table) that showed it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Effect {
    entity: String,
    aspect: Aspect,
    field: String,
}

fn owner(table: &str, row: &BTreeMap<String, String>) -> String {
    if let Some(id) = row.get("id") {
        return if id.contains(':') {
            id.clone()
        } else {
            format!("{table}:{id}")
        };
    }
    let column = ROW_OWNER
        .iter()
        .find(|(t, _)| *t == table)
        .map(|(_, c)| *c)
        .unwrap_or_else(|| panic!("table {table} has no `id` and no ROW_OWNER entry: {row:?}"));
    row.get(column)
        .unwrap_or_else(|| panic!("ROW_OWNER says {table}.{column}, row has {row:?}"))
        .clone()
}

fn diff(before: &Rows, after: &Rows) -> BTreeSet<Effect> {
    let mut effects = BTreeSet::new();
    let entity_table =
        |t: &str| t == BLOCK_WRITE_TABLE || t.ends_with("_raw") || t == "navigation_history";
    for ((table, key), row) in after {
        match before.get(&(table.clone(), key.clone())) {
            None if entity_table(table) => {
                effects.insert(Effect {
                    entity: owner(table, row),
                    aspect: Aspect::Existence,
                    field: format!("{table}+"),
                });
            }
            None => {
                effects.insert(Effect {
                    entity: owner(table, row),
                    aspect: Aspect::Text,
                    field: format!("{table}+"),
                });
            }
            Some(was) => {
                for (col, value) in row {
                    if was.get(col) == Some(value) || BOOKKEEPING_FIELDS.contains(&col.as_str()) {
                        continue;
                    }
                    let aspect = if PLACEMENT_FIELDS.contains(&col.as_str()) {
                        Aspect::Structural
                    } else {
                        Aspect::Text
                    };
                    effects.insert(Effect {
                        entity: owner(table, row),
                        aspect,
                        field: col.clone(),
                    });
                }
            }
        }
    }
    for ((table, key), row) in before {
        if !after.contains_key(&(table.clone(), key.clone())) {
            let aspect = if entity_table(table) {
                Aspect::Existence
            } else {
                Aspect::Text
            };
            effects.insert(Effect {
                entity: owner(table, row),
                aspect,
                field: format!("{table}-"),
            });
        }
    }
    effects
}

/// The aspects a declared delta WRITES on its subject.
fn written_aspects(k: &KindDelta) -> BTreeSet<Aspect> {
    let mut w = BTreeSet::new();
    if matches!(
        k.structural,
        StructuralFlow::Produces | StructuralFlow::Consumes | StructuralFlow::Relocates
    ) {
        w.insert(Aspect::Structural);
    }
    if k.text == TextFlow::Produces {
        w.insert(Aspect::Text);
    }
    if k.existence == ExistenceFlow::Produces {
        w.insert(Aspect::Existence);
    }
    w
}

fn delta_text(delta: &MarkingDelta) -> String {
    let kinds = |ks: &[KindDelta]| {
        ks.iter()
            .map(|k| {
                format!(
                    "{}(s={:?} t={:?} e={:?})",
                    k.kind, k.structural, k.text, k.existence
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    match delta {
        MarkingDelta::Undeclared => "none".to_string(),
        MarkingDelta::Static { kinds: ks } => kinds(ks),
        MarkingDelta::Envelope {
            kinds: ks,
            varies_by,
        } => {
            format!("{} envelope by {}", kinds(ks), varies_by.join(","))
        }
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 36
        && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-')
        && s.chars()
            .filter(|c| *c != '-')
            .all(|c| c.is_ascii_hexdigit())
}

/// Renders entity ids stably: the fixture prefix is dropped and a minted
/// uuid is named by the content it was minted with.
struct Namer<'a> {
    prefix: &'a str,
    contents: BTreeMap<String, String>,
}

impl Namer<'_> {
    fn new<'a>(prefix: &'a str, before: &Rows, after: &Rows) -> Namer<'a> {
        let mut contents = BTreeMap::new();
        for rows in [before, after] {
            for ((table, key), row) in rows {
                if table == BLOCK_WRITE_TABLE {
                    contents.insert(key.clone(), row.get("content").cloned().unwrap_or_default());
                }
            }
        }
        Namer { prefix, contents }
    }

    fn name(&self, entity: &str) -> String {
        let local = entity.split_once(':').map(|(_, l)| l).unwrap_or(entity);
        if is_uuid(local) {
            return format!(
                "minted({:?})",
                self.contents.get(entity).map(String::as_str).unwrap_or("?")
            );
        }
        entity.replace(self.prefix, "")
    }

    fn mask(&self, text: &str) -> String {
        text.split(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ',')
            .fold(text.replace(self.prefix, ""), |acc, word| {
                let local = word.split_once(':').map(|(_, l)| l).unwrap_or(word);
                if is_uuid(local) {
                    acc.replace(local, "<uuid>")
                } else {
                    acc
                }
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Inside,
    Outside,
    Undeclared,
}

struct Row {
    entity: String,
    op: String,
    declared: String,
    observed: String,
    verdict: Verdict,
    outside: String,
    subject_claim: String,
    unreported: String,
}

enum Fired {
    Dispatched(Vec<holon_core::FieldDelta>),
    Engine,
    Failed(String),
}

fn effects_text(namer: &Namer, effects: &BTreeSet<Effect>) -> String {
    if effects.is_empty() {
        return "(none)".to_string();
    }
    let mut by_entity: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in effects {
        by_entity
            .entry(namer.name(&e.entity))
            .or_default()
            .push(format!("{:?}:{}", e.aspect, e.field).to_lowercase());
    }
    by_entity
        .into_iter()
        .map(|(entity, fs)| format!("{entity}[{}]", fs.join(" ")))
        .collect::<Vec<_>>()
        .join("; ")
}

fn uris_in(params: &StorageEntity) -> BTreeSet<String> {
    params
        .values()
        .filter_map(|v| v.as_string())
        .filter(|s| s.contains(':') && !s.contains(' '))
        .map(str::to_string)
        .collect()
}

fn judge(
    d: &OperationDescriptor,
    params: &StorageEntity,
    prefix: &str,
    before: &Rows,
    after: &Rows,
    fired: Fired,
) -> Row {
    let namer = Namer::new(prefix, before, after);
    let effects = diff(before, after);
    let observed = match &fired {
        Fired::Failed(msg) => format!("ERROR: {}", namer.mask(msg)),
        _ => effects_text(&namer, &effects),
    };

    let claimed = uris_in(params);
    let escapes: BTreeSet<Effect> = effects
        .iter()
        .filter(|e| !claimed.contains(&e.entity))
        .cloned()
        .collect();
    let subject_claim = if escapes.is_empty() {
        "covers".to_string()
    } else {
        format!("misses {}", effects_text(&namer, &escapes))
    };

    let unreported = match &fired {
        Fired::Dispatched(changes) => {
            let reported: BTreeSet<(String, String)> = changes
                .iter()
                .map(|c| (c.entity_id.clone(), c.field.clone()))
                .collect();
            let reported_entities: BTreeSet<&String> = reported.iter().map(|(e, _)| e).collect();
            let missing: BTreeSet<Effect> = effects
                .iter()
                .filter(|e| match e.aspect {
                    Aspect::Existence => !reported_entities.contains(&e.entity),
                    _ => !reported.contains(&(e.entity.clone(), e.field.clone())),
                })
                .cloned()
                .collect();
            if missing.is_empty() {
                "-".to_string()
            } else {
                effects_text(&namer, &missing)
            }
        }
        Fired::Engine => "n/a (engine compound)".to_string(),
        Fired::Failed(_) => "-".to_string(),
    };

    let (verdict, outside) = match d
        .marking_delta
        .for_kind(&holon_api::arcs::ArcRelation::block())
    {
        _ if matches!(d.marking_delta, MarkingDelta::Undeclared) => {
            (Verdict::Undeclared, String::new())
        }
        None => panic!(
            "{}.{} declares a delta without a block kind",
            d.entity_name, d.name
        ),
        Some(k) => {
            assert!(
                !matches!(fired, Fired::Failed(_)),
                "declared op {}.{} failed on its fixture: {observed}",
                d.entity_name,
                d.name
            );
            let subject = params
                .get(d.id_column.as_str())
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| {
                    panic!(
                        "{}.{} binding lacks `{}`",
                        d.entity_name, d.name, d.id_column
                    )
                })
                .to_string();
            let writes = written_aspects(k);
            let out: BTreeSet<Effect> = effects
                .iter()
                .filter(|e| !(e.entity == subject && writes.contains(&e.aspect)))
                .cloned()
                .collect();
            if out.is_empty() {
                (Verdict::Inside, String::new())
            } else {
                (Verdict::Outside, effects_text(&namer, &out))
            }
        }
    };

    Row {
        entity: d.entity_name.to_string(),
        op: d.name.clone(),
        declared: delta_text(&d.marking_delta),
        observed,
        verdict,
        outside,
        subject_claim,
        unreported,
    }
}

fn census(rows: &[Row]) -> String {
    let mut out = String::new();
    let count = |v: Verdict| rows.iter().filter(|r| r.verdict == v).count();
    writeln!(out, "# Admission footprint census (D3, report-only)\n").unwrap();
    writeln!(
        out,
        "Generated by `crates/holon/tests/admission_footprint_sweep.rs`; do not edit by hand.\n\n\
         - Fixture: the SqlOnly block wiring (`SqlOperationProvider` + `SqlBlockOperations`), one fresh\n\
           subtree per op (`page` > `a`(tag old), `b` > `b1`, `c`). Observed = before/after diff of every\n\
           base table except {BOOKKEEPING_TABLES:?}, ignoring {BOOKKEEPING_FIELDS:?}.\n\
         - Footprint = the declared delta's written aspects on ONE subject (the `id_column` param).\n\
         - `inside`: every observed effect is on the subject in a written aspect. `outside`: some is not.\n\
         - `subject claim`: the overlay's fallback footprint (every entity URI in params).\n\
         - `unreported`: observed effects missing from `result.changes` (what D1 alone would miss).\n\
         - Not covered: the Loro write leg (Text/Mark ops such as `insert_text` are not in this catalog).\n"
    )
    .unwrap();
    writeln!(
        out,
        "Counts: {} ops; inside {}; outside {}; undeclared {} (of which {} failed on the fixture binding, so their effect is unmeasured).\n",
        rows.len(),
        count(Verdict::Inside),
        count(Verdict::Outside),
        count(Verdict::Undeclared),
        rows.iter().filter(|r| r.observed.starts_with("ERROR")).count()
    )
    .unwrap();
    let table = |out: &mut String, rows: &mut dyn Iterator<Item = &Row>| {
        writeln!(
            out,
            "| op | declared delta | observed effect | verdict | outside the footprint | subject claim | unreported |\n|---|---|---|---|---|---|---|"
        )
        .unwrap();
        for r in rows {
            writeln!(
                out,
                "| {}.{} | {} | {} | {:?} | {} | {} | {} |",
                r.entity,
                r.op,
                r.declared,
                r.observed.replace('|', "/"),
                r.verdict,
                r.outside,
                r.subject_claim,
                r.unreported
            )
            .unwrap();
        }
    };
    writeln!(out, "## Gesture ops of the slot\n").unwrap();
    table(
        &mut out,
        &mut GESTURE_OPS.iter().filter_map(|g| {
            rows.iter()
                .find(|r| r.op == *g && (r.entity == BLOCK || r.entity == "engine"))
        }),
    );
    writeln!(out, "\n## Every op\n").unwrap();
    table(&mut out, &mut rows.iter());
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn footprint_census_report_only() {
    let engine = block_engine().await;
    let dispatcher = engine.get_dispatcher();
    let routed: BTreeSet<(String, String)> = dispatcher
        .operations()
        .into_iter()
        .map(|d| (d.entity_name.to_string(), d.name))
        .collect();
    let mut catalog = engine.operation_catalog().expect("catalog");
    catalog
        .sort_by(|a, b| (a.entity_name.as_str(), &a.name).cmp(&(b.entity_name.as_str(), &b.name)));

    let mut rows = Vec::new();
    for (i, d) in catalog.iter().enumerate() {
        let fixture = Fixture {
            prefix: format!("o{i}-"),
        };
        fixture.seed(&engine).await;
        let params = fixture.params(d);
        let before = snapshot(&engine).await;
        let fired = if routed.contains(&(d.entity_name.to_string(), d.name.clone())) {
            match dispatcher
                .execute_operation(&d.entity_name, &d.name, params.clone())
                .await
            {
                Ok(result) => Fired::Dispatched(result.changes),
                Err(e) => Fired::Failed(format!("{e:#}").lines().next().unwrap_or("").to_string()),
            }
        } else {
            match engine
                .execute_operation(&d.entity_name, &d.name, params.clone(), OpOrigin::User)
                .await
            {
                Ok(_) => Fired::Engine,
                Err(e) => Fired::Failed(format!("{e:#}").lines().next().unwrap_or("").to_string()),
            }
        };
        let after = snapshot(&engine).await;
        rows.push(judge(d, &params, &fixture.prefix, &before, &after, fired));
    }

    let fixture = Fixture {
        prefix: format!("o{}-", catalog.len()),
    };
    fixture.seed(&engine).await;
    let mut created: StorageEntity = HashMap::new();
    created.insert("id".into(), Value::String(fixture.id("n")));
    created.insert("parent_id".into(), Value::String(fixture.id("page")));
    created.insert("content".into(), Value::String("typed".into()));
    engine
        .execute_operation(
            &EntityName::new(BLOCK),
            "create",
            created.clone(),
            OpOrigin::User,
        )
        .await
        .expect("create before undo");
    let fence = OperationDescriptor {
        entity_name: EntityName::new("engine"),
        ..catalog[0].clone()
    };
    for step in ["undo", "redo"] {
        let before = snapshot(&engine).await;
        let outcome = match step {
            "undo" => engine.undo().await,
            _ => engine.redo().await,
        };
        let fired = match outcome {
            Ok(_) => Fired::Engine,
            Err(e) => Fired::Failed(format!("{e:#}")),
        };
        let after = snapshot(&engine).await;
        let d = OperationDescriptor {
            name: step.to_string(),
            marking_delta: MarkingDelta::Undeclared,
            ..fence.clone()
        };
        let mut row = judge(&d, &created, &fixture.prefix, &before, &after, fired);
        row.declared = "none (fence)".to_string();
        rows.push(row);
    }

    let text = census(&rows);
    println!("{text}");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/Testing/AdmissionFootprintCensus.md");
    std::fs::write(&path, &text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
