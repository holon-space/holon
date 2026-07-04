//! Generic sidecar smoke test.
//!
//! Loads a sidecar YAML, connects to its MCP server via the declared
//! transport+auth, then exercises every entity end-to-end:
//!
//! 1. Registers the foreign table on an in-memory Turso connection.
//! 2. Drains the FDW cursor (pagination loop runs for real).
//! 3. Calls the same MCP tool independently with first-page args for a sanity
//!    comparison (first-page count, reported total when present).
//! 4. For entities that declare `enumerate_from`, materialises the parent's
//!    rows into a real SQL table first so the child fan-out can read them.
//!
//! Exit code is non-zero if any entity returned 0 rows from the FDW.
//!
//! Usage:
//!   cargo run -p holon-mcp-client --example sidecar_smoke_test -- \
//!     ~/.config/holon/integrations/github.yaml [--limit N]
//!
//! `--limit N` caps the cursor drain at N rows per entity to keep the
//! probe fast on large datasets. The independent first-page call is
//! always issued so the report remains comparable.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::entity::FieldSchema;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::connect_mcp;
use holon_mcp_client::connect_mcp_child;
use holon_mcp_client::integration_config::ChildProcessTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_sidecar::EntityConfig;
use holon_mcp_client::mcp_vtable::McpForeignDataWrapper;
use holon_mcp_client::mcp_vtable::VtableConfig;
use rmcp::model::CallToolRequestParam;
use serde_json::Value as JsonValue;
use turso_core::Connection as CoreConnection;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::MemoryIO;
use turso_core::OpenFlags;
use turso_core::StepResult;
use turso_core::Value;
use turso_core::foreign::ForeignDataWrapper;
use turso_core::foreign::PushedConstraint;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let yaml_path: PathBuf = args
        .next()
        .context("usage: sidecar_smoke_test <path-to-sidecar.yaml> [--limit N]")?
        .into();
    let mut limit: Option<usize> = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--limit" => {
                let n: usize = args
                    .next()
                    .context("--limit expects a number")?
                    .parse()
                    .context("--limit value must be a usize")?;
                limit = Some(n);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let yaml = std::fs::read_to_string(&yaml_path)
        .with_context(|| format!("read {}", yaml_path.display()))?;
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml)
        .with_context(|| format!("parse {} as IntegrationFileConfig", yaml_path.display()))?;
    let prefix = cfg.entity_prefix.as_deref().unwrap_or("");

    println!("# Sidecar smoke test");
    println!("  file:           {}", yaml_path.display());
    println!("  entity_prefix:  {prefix:?}");
    println!("  entities:       {}", cfg.entities.len());

    let (peer, _service) = connect(&cfg).await?;
    let peer: Arc<dyn McpCallSurface> = Arc::new(peer);

    let order = topological_order(&cfg)?;
    println!("  order:          {order:?}");
    println!();

    let conn = open_memory_conn();
    let mut results: Vec<EntityResult> = Vec::new();

    for entity_name in &order {
        let entity = cfg.entities.get(entity_name).expect("present");
        let result = exercise_entity(
            entity_name,
            entity,
            prefix,
            peer.clone(),
            conn.clone(),
            limit,
        )
        .await
        .unwrap_or_else(|e| EntityResult {
            name: entity_name.clone(),
            fdw_count: 0,
            first_page_records: 0,
            reported_total: None,
            error: Some(format!("{e:#}")),
        });
        results.push(result);
    }

    println!();
    print_report(&results);

    let failed = results
        .iter()
        .any(|r| r.error.is_some() || r.fdw_count == 0);
    if failed {
        bail!("one or more entities returned 0 rows or errored");
    }
    Ok(())
}

async fn connect(
    cfg: &IntegrationFileConfig,
) -> Result<(
    rmcp::service::Peer<rmcp::RoleClient>,
    holon_mcp_client::McpRunningService,
)> {
    let token = cfg.auth.as_ref().and_then(|a| a.static_token.clone());
    if let Some(http) = &cfg.transport.http {
        connect_mcp(&http.uri, token.as_deref())
            .await
            .with_context(|| format!("connect_mcp({})", http.uri))
    } else if let Some(ChildProcessTransport { command, args, env }) = &cfg.transport.child_process
    {
        connect_mcp_child(command, args, env)
            .await
            .with_context(|| format!("connect_mcp_child({command})"))
    } else {
        bail!("transport must declare either `http` or `child_process`")
    }
}

#[derive(Debug, Default)]
struct EntityResult {
    name: String,
    fdw_count: usize,
    first_page_records: usize,
    reported_total: Option<u64>,
    error: Option<String>,
}

fn print_report(results: &[EntityResult]) {
    println!("entity                 fdw_count  first_page  total       status");
    println!("---------------------- ---------- ----------  ---------   --------");
    for r in results {
        let total = r
            .reported_total
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string());
        let status = match (&r.error, r.fdw_count) {
            (Some(e), _) => format!("ERROR: {e}"),
            (None, 0) => "EMPTY".to_string(),
            _ => "OK".to_string(),
        };
        println!(
            "{:22} {:>10}  {:>9}  {:>9}   {}",
            r.name, r.fdw_count, r.first_page_records, total, status
        );
    }
}

async fn exercise_entity(
    entity_name: &str,
    entity: &EntityConfig,
    prefix: &str,
    peer: Arc<dyn McpCallSurface>,
    conn: Arc<CoreConnection>,
    limit: Option<usize>,
) -> Result<EntityResult> {
    let Some(vtable) = entity.vtable.as_ref() else {
        return Ok(EntityResult {
            name: entity_name.to_string(),
            error: Some("no vtable declared".to_string()),
            ..Default::default()
        });
    };

    let table_name = format!("{prefix}{entity_name}");
    create_cache_table(&conn, &table_name, &entity.schema)?;
    let columns: Vec<(String, String)> = entity
        .schema
        .iter()
        .map(|f| (f.name.clone(), f.sql_type.clone()))
        .collect();
    let fdw = McpForeignDataWrapper::new(
        &table_name,
        &columns,
        vtable,
        peer.clone(),
        None,
        &entity.identity_columns(),
        None,
        tokio::runtime::Handle::current(),
        Some(prefix),
        &std::collections::HashMap::new(),
    );

    // ----- 1. Drain the FDW cursor (pagination + fan-out happen for real) -----
    let fdw_rows = tokio::task::spawn_blocking({
        let fdw = std::sync::Arc::new(fdw);
        let conn = conn.clone();
        move || -> Result<Vec<Vec<Value>>> {
            let mut cursor = fdw
                .open_cursor(conn)
                .map_err(|e| anyhow::anyhow!("open_cursor: {e}"))?;
            let mut has = cursor
                .filter(&[] as &[PushedConstraint])
                .map_err(|e| anyhow::anyhow!("filter: {e}"))?;
            let mut out = Vec::new();
            while has {
                let row: Vec<Value> = (0..columns_len(&fdw))
                    .map(|i| cursor.column(i).expect("column"))
                    .collect();
                out.push(row);
                if let Some(l) = limit
                    && out.len() >= l
                {
                    break;
                }
                has = cursor.next().map_err(|e| anyhow::anyhow!("next: {e}"))?;
            }
            Ok(out)
        }
    })
    .await
    .context("spawn_blocking cursor drain")??;

    // Persist into the cache table so child entities can enumerate against it.
    insert_rows_into_cache(&conn, &table_name, &columns, &fdw_rows)?;

    // ----- 2. Independent comparison call (first page only, no fan-out) -----
    let (first_page_records, reported_total) = first_page_probe(&peer, vtable).await?;

    Ok(EntityResult {
        name: entity_name.to_string(),
        fdw_count: fdw_rows.len(),
        first_page_records,
        reported_total,
        error: None,
    })
}

fn columns_len(fdw: &McpForeignDataWrapper) -> usize {
    // We approximate by reading schema_sql column count from the FDW. The
    // FDW doesn't expose its column list directly, but ForeignDataWrapper
    // gives us KeyColumn which only covers pushdown columns. Simpler: count
    // commas between the parens of `schema_sql()`.
    let ddl = fdw.schema_sql();
    let inner = ddl
        .split_once('(')
        .and_then(|(_, rest)| rest.rsplit_once(')'))
        .map(|(inner, _)| inner)
        .unwrap_or(&ddl);
    inner
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .filter(|s| !s.trim_start().to_uppercase().starts_with("PRIMARY"))
        .count()
}

/// Independent direct call to the same MCP tool for sanity. Uses the YAML's
/// `static_args`, asks for one page, and pulls `total_count` (if present)
/// out of the response. Does NOT enumerate parents — for child entities
/// it issues a single call with whatever static_args contains (which is
/// usually enough to get a non-error response back from GitHub even without
/// owner/repo, because the server returns a clean error structure).
async fn first_page_probe(
    peer: &Arc<dyn McpCallSurface>,
    vtable: &VtableConfig,
) -> Result<(usize, Option<u64>)> {
    let Some(tool) = vtable.search_tool.as_ref() else {
        return Ok((0, None));
    };
    let mut params = vtable.static_args.clone();
    // Best-effort page-size hint.
    params.insert("perPage".to_string(), serde_json::json!(5));

    // For entities with enumerate_from, the tool requires owner+repo. Skip
    // the probe rather than panic — the FDW count already covered that.
    let needs_enumeration = vtable
        .filter_mapping
        .values()
        .any(|fc| fc.enumerate_from.is_some());
    if needs_enumeration {
        return Ok((0, None));
    }

    let result = peer
        .call_tool(CallToolRequestParam {
            name: Cow::Owned(tool.clone()),
            arguments: Some(params),
        })
        .await
        .with_context(|| format!("first_page_probe({tool})"))?;

    if result.is_error == Some(true) {
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("MCP tool error during first_page_probe: {text}");
    }
    let body_text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("");
    let body: JsonValue =
        serde_json::from_str(&body_text).with_context(|| format!("parse {tool} response JSON"))?;

    let records = match vtable.extract_path.as_deref() {
        Some(path) => body
            .get(path)
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        None => body.as_array().map(|a| a.len()).unwrap_or(0),
    };
    let total = match &vtable.pagination {
        Some(holon_mcp_client::mcp_vtable::PaginationConfig::PageTotal {
            total_response_path,
            ..
        }) => body
            .as_object()
            .and_then(|m| resolve_dotted_u64(m, total_response_path)),
        _ => body
            .as_object()
            .and_then(|m| resolve_dotted_u64(m, "totalCount")),
    };
    Ok((records, total))
}

fn resolve_dotted_u64(obj: &serde_json::Map<String, JsonValue>, path: &str) -> Option<u64> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let mut cur = obj.get(first)?.clone();
    for seg in parts {
        cur = match cur {
            JsonValue::Object(m) => m.get(seg)?.clone(),
            _ => return None,
        };
    }
    cur.as_u64()
}

/// Build a CREATE TABLE for the entity's cache from FieldSchema rows.
fn create_cache_table(
    conn: &Arc<CoreConnection>,
    name: &str,
    schema: &[FieldSchema],
) -> Result<()> {
    let cols: Vec<String> = schema
        .iter()
        .map(|f| {
            let null_clause = if !f.nullable { " NOT NULL" } else { "" };
            format!("{} {}{}", quote_ident(&f.name), f.sql_type, null_clause)
        })
        .collect();
    let pks: Vec<String> = schema
        .iter()
        .filter(|f| f.primary_key)
        .map(|f| quote_ident(&f.name))
        .collect();
    let pk_clause = if pks.is_empty() {
        String::new()
    } else {
        format!(", PRIMARY KEY ({})", pks.join(", "))
    };
    let ddl = format!(
        "CREATE TABLE IF NOT EXISTS {} ({}{})",
        quote_ident(name),
        cols.join(", "),
        pk_clause,
    );
    execute_sql(conn, &ddl)?;
    Ok(())
}

fn insert_rows_into_cache(
    conn: &Arc<CoreConnection>,
    name: &str,
    columns: &[(String, String)],
    rows: &[Vec<Value>],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let col_idents = columns
        .iter()
        .map(|(n, _)| quote_ident(n))
        .collect::<Vec<_>>()
        .join(", ");
    for row in rows {
        let literals = row
            .iter()
            .map(value_to_sql_literal)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR REPLACE INTO {} ({col_idents}) VALUES ({literals})",
            quote_ident(name)
        );
        execute_sql(conn, &sql)?;
    }
    Ok(())
}

fn value_to_sql_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Text(t) => format!("'{}'", t.as_str().replace('\'', "''")),
        Value::Numeric(turso_core::Numeric::Integer(i)) => i.to_string(),
        Value::Numeric(turso_core::Numeric::Float(f)) => format!("{}", **f),
        Value::Blob(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("x'{hex}'")
        }
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn execute_sql(conn: &Arc<CoreConnection>, sql: &str) -> Result<()> {
    let mut stmt = conn
        .query(sql)
        .with_context(|| format!("query setup `{sql}`"))?
        .with_context(|| format!("no statement for `{sql}`"))?;
    loop {
        match stmt.step().with_context(|| format!("step `{sql}`"))? {
            StepResult::Row | StepResult::IO => continue,
            StepResult::Done => return Ok(()),
            other => bail!("unexpected StepResult for `{sql}`: {other:?}"),
        }
    }
}

fn open_memory_conn() -> Arc<CoreConnection> {
    let io = Arc::new(MemoryIO::new());
    let opts = DatabaseOpts::default().with_views(true);
    let db = Database::open_file_with_flags(
        io,
        ":memory:",
        OpenFlags::default(),
        opts,
        None,
        Arc::new(turso_core::SqliteDialect),
    )
    .expect("open in-memory database");
    db.connect().expect("connect")
}

/// Topologically order entities so parents (referenced via `enumerate_from`)
/// come before children. Cycles → error.
fn topological_order(cfg: &IntegrationFileConfig) -> Result<Vec<String>> {
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
    for (name, ent) in &cfg.entities {
        let mut d = HashSet::new();
        if let Some(v) = ent.vtable.as_ref() {
            for fc in v.filter_mapping.values() {
                if let Some(ef) = fc.enumerate_from.as_ref() {
                    d.insert(ef.entity.clone());
                }
            }
        }
        deps.insert(name.clone(), d);
    }
    let mut out: Vec<String> = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();
    while placed.len() < deps.len() {
        let next = deps
            .iter()
            .filter(|(n, _)| !placed.contains(*n))
            .find(|(_, d)| {
                d.iter()
                    .all(|p| placed.contains(p) || !deps.contains_key(p))
            })
            .map(|(n, _)| n.clone());
        match next {
            Some(n) => {
                placed.insert(n.clone());
                out.push(n);
            }
            None => bail!("entity dependency cycle in sidecar"),
        }
    }
    Ok(out)
}
