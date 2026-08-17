//! Row identity of the SHIPPED `claude-history` sidecar
//! (`assets/integrations/claude-history.yaml`), on two levels:
//!
//!   1. Declaration — what each entity's schema says its identity is, and what
//!      `McpForeignDataWrapper` resolves that to. A row identity is the switch
//!      that decides whether a matview over the foreign table is incrementally
//!      maintained or a one-shot snapshot, so a schema edit that silently moves
//!      it must fail here rather than in a mirror nobody is watching.
//!   2. Truth — whether the declared identity is actually a KEY over the data
//!      the provider serves across an unpinned fan-out. A declaration the data
//!      contradicts is refused three times over: at CREATE MATERIALIZED VIEW
//!      (mirror PRIMARY KEY), at REFRESH (sweep guard), and at push (the
//!      driver's own guard) — so measuring it is what decides which entities
//!      can be retargeted onto the mirror at all.
//!
//! Level 2 drives the REAL `claude-code-history-mcp` binary when it is present,
//! and falls back to `tests/fixtures/claude_history_identity.json` — a
//! recording of that same measurement — when it is not. Both arms run the same
//! assertions and each names itself in its failure messages.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use holon::di::DbHandleCacheFactory;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpConnectionResult;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::SyncGate;
use holon_mcp_client::build_mcp_integration;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_sidecar::EntityConfig;
use holon_mcp_client::mcp_vtable::FetchContract;
use holon_mcp_client::mcp_vtable::McpForeignDataWrapper;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::service::ServiceError;
use turso_core::foreign::ForeignDataWrapper;

const SIDECAR_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/integrations/claude-history.yaml"
);
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/claude_history_identity.json"
);

fn sidecar() -> IntegrationFileConfig {
    let yaml = std::fs::read_to_string(SIDECAR_PATH)
        .unwrap_or_else(|e| panic!("read {SIDECAR_PATH}: {e}"));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {SIDECAR_PATH}: {e}"))
}

/// What the yaml INTENDS each entity's row identity to be, spelled out
/// independently of the resolution code so that a schema edit which moves an
/// identity fails here instead of silently changing what a mirror keys on.
const INTENDED_IDENTITY: &[(&str, &[&str])] = &[
    ("project", &["id"]),
    ("live_session", &["id"]),
    ("pending_question", &["id"]),
    ("session", &["id"]),
    ("task", &["id"]),
    // Composite: `id` alone is NOT unique across projects — a resume/fork
    // copies the subagents directory byte-identically. Measured below.
    ("agent", &["id", "project_id"]),
    ("message", &["uuid"]),
    ("agent_message", &["uuid"]),
];

/// A peer that fails loudly if anything calls it. Identity resolution is pure
/// config, so a wrapper built for this test must never reach the network.
#[derive(Debug)]
struct NeverCalledPeer;

#[async_trait]
impl McpCallSurface for NeverCalledPeer {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        panic!(
            "identity resolution must not call the MCP server (tool '{}')",
            params.name
        );
    }
    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        panic!(
            "identity resolution must not call the MCP server (resource '{}')",
            params.uri
        );
    }
}

fn build_wrapper(name: &str, entity: &EntityConfig, prefix: Option<&str>) -> McpForeignDataWrapper {
    let columns: Vec<(String, String)> = entity
        .schema
        .iter()
        .map(|f| (f.name.clone(), f.sql_type.clone()))
        .collect();
    McpForeignDataWrapper::new(
        &format!("{}{name}_fdw", prefix.unwrap_or("")),
        &columns,
        entity.vtable.as_ref().expect("entity declares a vtable"),
        Arc::new(NeverCalledPeer),
        Some((entity.id_column_or_default(), format!("cc-{name}"))),
        &entity.identity_columns(),
        None,
        tokio::runtime::Handle::current(),
        prefix,
        &HashMap::new(),
    )
}

// ---------------------------------------------------------------------------
// Level 1 — declaration
// ---------------------------------------------------------------------------

/// The schema's `primary_key` flags are the identity declaration. Reading it
/// off `id_column_or_default()` instead would answer `"id"` for the uuid-keyed
/// message tables, naming a column that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn every_claude_history_entity_declares_the_identity_its_yaml_intends() {
    let cfg = sidecar();
    let listed: HashSet<&str> = INTENDED_IDENTITY.iter().map(|(n, _)| *n).collect();

    for (name, intended) in INTENDED_IDENTITY {
        let entity = cfg
            .entities
            .get(*name)
            .unwrap_or_else(|| panic!("the shipped sidecar declares entity '{name}'"));
        assert_eq!(
            entity.identity_columns(),
            intended.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "entity '{name}' resolves a different row identity than this test declares \
             intended; a mirror keys on this, so the change must be deliberate"
        );
    }

    let unlisted: Vec<&String> = cfg
        .entities
        .iter()
        .filter(|(name, e)| e.vtable.is_some() && !listed.contains(name.as_str()))
        .map(|(name, _)| name)
        .collect();
    assert!(
        unlisted.is_empty(),
        "these entities declare a vtable but no intended identity in this test, so nothing \
         checks what their mirrors key on: {unlisted:?}"
    );
}

/// The wrapper is where a declaration becomes the column indices the engine
/// keys mirrors on. `None` there means snapshot semantics and no incremental
/// maintenance at all, which is exactly the failure this level catches.
#[tokio::test(flavor = "multi_thread")]
async fn the_wrapper_resolves_each_declaration_to_its_schema_column_indices() {
    let cfg = sidecar();
    let prefix = cfg.entity_prefix.as_deref();

    for (name, intended) in INTENDED_IDENTITY {
        let entity = cfg.entities.get(*name).expect("entity present");
        if entity.vtable.is_none() {
            continue;
        }
        let expected: Vec<u32> = intended
            .iter()
            .map(|col| {
                entity
                    .schema
                    .iter()
                    .position(|f| f.name == *col)
                    .unwrap_or_else(|| panic!("entity '{name}' has no column '{col}'"))
                    as u32
            })
            .collect();
        let fdw = build_wrapper(name, entity, prefix);
        assert_eq!(
            fdw.identity_columns(),
            Some(&expected[..]),
            "entity '{name}' must resolve its identity to the schema indices of {intended:?}; \
             None would keep every matview over it a one-shot snapshot"
        );
    }
}

// ---------------------------------------------------------------------------
// Level 2 — truth against the data the provider actually serves
// ---------------------------------------------------------------------------

/// One entity's identity tuples as fetched over an unpinned fan-out.
struct Measurement {
    entity: &'static str,
    identity: Vec<String>,
    tuples: Vec<Vec<Option<String>>>,
}

/// What a measurement is allowed to show. Anything else is a defect in either
/// the declaration or the provider, and must not pass silently.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// The declared identity is a key: unique and fully non-NULL.
    IsAKey,
    /// It is not — the duplicated tuples are listed so the failure names them.
    NotAKey {
        duplicates: Vec<Vec<Option<String>>>,
    },
    /// Some row carries a NULL in an identity column.
    HasNullIdentity,
}

fn verdict(m: &Measurement) -> Verdict {
    if m.tuples.iter().any(|t| t.iter().any(|v| v.is_none())) {
        return Verdict::HasNullIdentity;
    }
    let mut counts: HashMap<&Vec<Option<String>>, usize> = HashMap::new();
    for t in &m.tuples {
        *counts.entry(t).or_default() += 1;
    }
    let mut duplicates: Vec<Vec<Option<String>>> = counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(t, _)| t.clone())
        .collect();
    if duplicates.is_empty() {
        Verdict::IsAKey
    } else {
        duplicates.sort();
        Verdict::NotAKey { duplicates }
    }
}

fn assert_measured(arm: &str, m: &Measurement, expected: Verdict) {
    assert!(
        !m.tuples.is_empty(),
        "[{arm}] '{}' produced no rows at all — a fan-out that fetched nothing cannot \
         measure anything, and passing here would be a false green",
        m.entity
    );
    let got = verdict(m);
    assert_eq!(
        got,
        expected,
        "[{arm}] '{}' over identity {:?}: {} rows measured",
        m.entity,
        m.identity,
        m.tuples.len()
    );
}

// --- fixture arm -----------------------------------------------------------

fn fixture_measurement(entity: &'static str, key: &str) -> Measurement {
    let raw = std::fs::read_to_string(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("read {FIXTURE_PATH}: {e}"));
    let doc: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {FIXTURE_PATH}: {e}"));
    let e = doc
        .get("entities")
        .and_then(|m| m.get(key))
        .unwrap_or_else(|| panic!("{FIXTURE_PATH} records no entity '{key}'"));
    let identity = e["identity"]
        .as_array()
        .expect("identity is an array")
        .iter()
        .map(|v| v.as_str().expect("identity column is a string").to_string())
        .collect();
    let tuples = e["sample_identities"]
        .as_array()
        .expect("sample_identities is an array")
        .iter()
        .map(|row| {
            row.as_array()
                .expect("each sample is an array")
                .iter()
                .map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .collect();
    Measurement {
        entity,
        identity,
        tuples,
    }
}

// --- real-provider arm -----------------------------------------------------

struct NoTokens;

#[async_trait]
impl SyncTokenStore for NoTokens {
    async fn save_token(&self, _: &str, _: StreamPosition) -> holon_core::Result<()> {
        Ok(())
    }
    async fn load_token(&self, _: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(None)
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        Ok(())
    }
}

/// The provider binary the shipped sidecar names, when it exists on this
/// machine. Absent ⇒ the fixture arm runs instead.
fn real_provider_command(cfg: &IntegrationFileConfig) -> Option<String> {
    let cmd = cfg.transport.child_process.as_ref()?.command.clone();
    std::path::Path::new(&cmd).exists().then_some(cmd)
}

/// Connect and run the sync pass. `project` is SYNC-only, and every fan-out
/// below enumerates it, so without this the scans have no parents to fan out
/// over and measure nothing.
async fn connect_real(cfg: IntegrationFileConfig, db: &DbHandle) {
    let mcp_config = cfg
        .into_mcp_config("claude-history".to_string())
        .expect("sidecar into_mcp_config");
    let result = build_mcp_integration(
        mcp_config,
        db.clone(),
        Arc::new(DbHandleCacheFactory::new(db.clone())),
        Arc::new(NoTokens),
        &PendingOAuthFlows::new(),
        SyncGate::opened(),
    )
    .await
    .expect("connect the real claude-history provider");
    match result {
        McpConnectionResult::Connected(integration) => {
            integration
                .sync_engine
                .sync_all()
                .await
                .expect("warm the sync-only cache tables the fan-outs enumerate");
            std::mem::forget(integration);
        }
        McpConnectionResult::NeedsAuth { provider_name, .. } => {
            panic!("unexpected NeedsAuth for '{provider_name}'")
        }
    }
}

/// Scan the foreign table unpinned, so the enumeration fans out exactly the way
/// a `SELECT * FROM <table>_fdw` in prod would.
///
/// The scanned columns come from the sidecar's own declaration, so weakening a
/// declaration cannot leave this measurement quietly checking the old one.
async fn scan_identity(
    db: &DbHandle,
    cfg: &IntegrationFileConfig,
    entity: &'static str,
    table: &str,
) -> Measurement {
    let declared = cfg
        .entities
        .get(entity)
        .unwrap_or_else(|| panic!("sidecar declares '{entity}'"))
        .identity_columns();
    let identity: Vec<&str> = declared.iter().map(|s| s.as_str()).collect();
    let sql = format!("SELECT {} FROM {table}", identity.join(", "));
    let rows = db
        .query(&sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("scan {table}: {e}"));
    let tuples = rows
        .iter()
        .map(|r| {
            identity
                .iter()
                .map(|c| match r.get(*c) {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Null) | None => None,
                    other => panic!("{table}.{c}: unexpected value {other:?}"),
                })
                .collect()
        })
        .collect();
    Measurement {
        entity,
        identity: identity.iter().map(|s| s.to_string()).collect(),
        tuples,
    }
}

/// The two uuid-keyed entities, from the recording. They are not scannable
/// unpinned on any machine right now (see the characterization test below), so
/// both arms read them from the same place.
fn assert_recorded_message_and_agent_message() {
    assert_measured(
        "recorded",
        &fixture_measurement("agent_message", "agent_message"),
        Verdict::IsAKey,
    );
    let single = fixture_measurement("agent", "agent_single_column_id");
    assert!(
        matches!(verdict(&single), Verdict::NotAKey { .. }),
        "the recording must keep showing WHY `agent` needs a composite identity: `id` alone \
         duplicated across projects. If this ever passes, the composite is no longer justified \
         by the data and the yaml should be revisited."
    );
    let message = fixture_measurement("message", "message");
    assert_eq!(
        verdict(&message),
        Verdict::NotAKey {
            duplicates: vec![vec![Some(String::new())]]
        },
        "'message' is the Increment-4 blocker, and the ONLY duplicate its declared identity may \
         have is the empty string the provider emits for its non-message rows (type=mode, \
         last-prompt, …). A different duplicate would be a real uuid collision; no duplicate at \
         all would mean the provider was fixed and `message` became retargetable."
    );
}

/// `session` and `agent` fan out over the cached projects; `message` and
/// `agent_message` fan out over the sessions and agents those scans cached, so
/// the order here is the dependency order of the watermarks.
///
/// The measured truth (real provider, `CLAUDE_DATA_DIR=~/.claude`, recorded in
/// the fixture):
///
/// | entity        | rows  | declared identity | verdict |
/// |---------------|-------|-------------------|---------|
/// | session       |   274 | `id`              | key |
/// | agent         |  1990 | `id`              | **NOT a key** — 3 ids under 2 projects each |
/// | agent         |  1990 | `id, project_id`  | key (the ruled composite) |
/// | message       | 21298 | `uuid`            | **NOT a key** — 8235 provider rows carry `uuid: ""` |
/// | agent_message |  3920 | `uuid`            | key |
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_identity_is_a_key_over_an_unpinned_fan_out() {
    let cfg = sidecar();
    match real_provider_command(&cfg) {
        Some(cmd) => {
            eprintln!("[real-provider arm] driving {cmd}");
            let (backend, db) = TursoBackend::new_in_memory().await.expect("in-memory db");
            std::mem::forget(backend);
            connect_real(cfg.clone(), &db).await;

            let session = scan_identity(&db, &cfg, "session", "cc_session_fdw").await;
            assert_measured("real-provider", &session, Verdict::IsAKey);

            let agent = scan_identity(&db, &cfg, "agent", "cc_agent_fdw").await;
            assert_measured("real-provider", &agent, Verdict::IsAKey);

            // `message` and `agent_message` cannot be scanned unpinned yet —
            // see `an_unpinned_scan_of_a_uuid_keyed_entity_fails_loud_today`.
            // Their measurements come from the recording until that is fixed.
            assert_recorded_message_and_agent_message();
        }
        None => {
            eprintln!("[fixture arm] provider binary absent; replaying {FIXTURE_PATH}");
            assert_measured(
                "fixture",
                &fixture_measurement("session", "session"),
                Verdict::IsAKey,
            );
            assert_measured(
                "fixture",
                &fixture_measurement("agent", "agent"),
                Verdict::IsAKey,
            );
            assert_recorded_message_and_agent_message();
        }
    }
}

/// The recording and the sidecar must not drift apart: whatever identity the
/// fixture was recorded over has to still be the declared one, or the fixture
/// arm silently measures a column nobody keys on any more.
#[tokio::test(flavor = "multi_thread")]
async fn the_recorded_measurement_covers_the_declared_identities() {
    let cfg = sidecar();
    for entity in ["session", "agent", "message", "agent_message"] {
        let declared = cfg
            .entities
            .get(entity)
            .expect("entity present")
            .identity_columns();
        let recorded = fixture_measurement("x", entity).identity;
        assert_eq!(
            declared, recorded,
            "{FIXTURE_PATH} recorded '{entity}' over a different identity than the sidecar \
             now declares — re-record it"
        );
    }
}

// ---------------------------------------------------------------------------
// Connect-time guard
// ---------------------------------------------------------------------------

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock-mcp-server");

async fn connect_fixture(fixture: &str) -> anyhow::Result<McpConnectionResult> {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut cfg: IntegrationFileConfig =
        serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    let cp = cfg
        .transport
        .child_process
        .as_mut()
        .expect("fixture declares child_process transport");
    cp.command = MOCK_BIN.to_string();
    cp.env
        .insert("MOCK_MCP_SCENARIO".to_string(), "happy".to_string());

    let (backend, db) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    let mcp_config = cfg.into_mcp_config("mock".to_string())?;
    build_mcp_integration(
        mcp_config,
        db.clone(),
        Arc::new(DbHandleCacheFactory::new(db.clone())),
        Arc::new(NoTokens),
        &PendingOAuthFlows::new(),
        SyncGate::opened(),
    )
    .await
}

/// A table with no declared identity keeps SNAPSHOT semantics: the engine
/// builds no mirror for it, so a matview reading it can never be maintained and
/// simply stops updating. Nothing about that is visible in the UI — the view
/// renders, it is just frozen. The misconfiguration is fully knowable at
/// connect, so it must fail there instead of becoming a stale panel.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_query_over_an_identity_less_fdw_table_is_refused_at_connect() {
    let err = match connect_fixture("vtable_no_identity.yaml").await {
        Err(e) => e.to_string(),
        Ok(_) => panic!(
            "connect must refuse a live_query reading an `_fdw` table whose wrapper declared no \
             row identity — the view it feeds would silently never update"
        ),
    };
    assert!(
        err.contains("mock_note_fdw") && err.contains("identity"),
        "the refusal must name the offending table and say what is missing, got: {err}"
    );
}

/// The negative control: the same sidecar with the identity declared connects.
/// Without this the guard could be "refuse every `_fdw` live_query" and still
/// look right.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_query_over_an_identified_fdw_table_connects() {
    let result = connect_fixture("vtable_identity_declared.yaml")
        .await
        .expect("a declared identity makes the same live_query maintainable");
    match result {
        McpConnectionResult::Connected(integration) => std::mem::forget(integration),
        McpConnectionResult::NeedsAuth { provider_name, .. } => {
            panic!("unexpected NeedsAuth for '{provider_name}'")
        }
    }
}

/// A view on the mirror is maintained from re-fetches, and how much of a
/// re-fetch's ABSENCE means "deleted" is a property of the source that cannot
/// be derived from any response — only declared. Left undeclared, the next
/// author inherits a silent assumption whose wrong value loses data, so connect
/// refuses. (Same fixture as the identity guard's control, minus the contract:
/// the identity is fine, only the promise is missing.)
#[tokio::test(flavor = "multi_thread")]
async fn a_live_query_over_an_fdw_table_without_a_fetch_contract_is_refused_at_connect() {
    let err = match connect_fixture("vtable_no_fetch_contract.yaml").await {
        Err(e) => e.to_string(),
        Ok(_) => panic!(
            "connect must refuse a live_query over an `_fdw` table whose entity never declared \
             what its source promises about a fetch"
        ),
    };
    assert!(
        err.contains("mock_note_fdw") && err.contains("fetch_contract"),
        "the refusal must name the table and the missing declaration, got: {err}"
    );
}

/// Every shipped `claude-history` vtable entity declares its contract, and all
/// of them are `scoped_snapshot`: each fan-out is bounded by an
/// `enumerate_from` watermark, so no fetch of theirs can witness a deletion.
/// That single fact is what makes the upsert-only push path sound.
#[tokio::test(flavor = "multi_thread")]
async fn every_shipped_vtable_entity_declares_a_scoped_snapshot_contract() {
    let cfg = sidecar();
    let mut checked = 0;
    for (name, entity) in &cfg.entities {
        let Some(vtable) = entity.vtable.as_ref() else {
            continue;
        };
        assert_eq!(
            vtable.fetch_contract,
            Some(FetchContract::ScopedSnapshot),
            "entity '{name}' must declare `scoped_snapshot`: its fetch is watermark-bounded, so \
             a row it does not return may simply be out of scope"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 5,
        "the shipped sidecar has 5 vtable entities; a new one must be reviewed here, not \
         silently inherit a contract"
    );
}

// ---------------------------------------------------------------------------
// Known blocker, pinned as characterization
// ---------------------------------------------------------------------------

/// An unpinned scan of a uuid-keyed write-through entity fails today, and the
/// mirror-as-storage retarget makes unpinned scans the normal case.
///
/// The cause is the same class ADR-adjacent to the identity work: the cursor
/// takes the stale-row-deletion column from `id_column_or_default()`, whose
/// `"id"` is an answer to a question `message`/`agent_message` never asked —
/// they declare `uuid`. A PINNED query supplies the parent key from WHERE, runs
/// no enumeration, and never reaches the deletion path, which is why prod has
/// not hit this: every shipped render pins.
///
/// Pinned as characterization so the day it is fixed, this test fails and says
/// so — do not delete it, invert it.
#[tokio::test(flavor = "multi_thread")]
async fn an_unpinned_scan_of_a_uuid_keyed_entity_fails_loud_today() {
    let cfg = sidecar();
    let Some(_) = real_provider_command(&cfg) else {
        eprintln!("[skipped] provider binary absent; this pin needs a real fan-out");
        return;
    };
    let (backend, db) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    connect_real(cfg, &db).await;
    // Warm cc_agent so agent_message has parents to enumerate.
    db.query("SELECT id FROM cc_agent_fdw", HashMap::new())
        .await
        .expect("the agent fan-out works — its id column really is `id`");

    let err = db
        .query("SELECT uuid FROM cc_agent_message_fdw", HashMap::new())
        .await
        .expect_err(
            "KNOWN BLOCKER: if this now succeeds, the uuid-keyed stale-deletion bug is fixed \
             — move `agent_message` (and `message`) into the live measurement above",
        )
        .to_string();
    assert!(
        err.contains("id column 'id' not in schema columns"),
        "the failure must still be the id_column_or_default() default naming a column the \
         uuid-keyed schema does not have, got: {err}"
    );
}
