//! A warm reboot over an unchanged schema must keep every materialized view the
//! schema modules own, not drop and rebuild it.
//!
//! Rebuilding `block_with_path` (a recursive CTE over every block) cost
//! 2.5–2.8 s of each warm boot at 10k blocks and 40 s at 50k. The oracle is
//! the DDL the actor executes (its `holon_latency` `matview_ddl` events), not
//! wall time.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use holon_core::storage::Resource;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::*;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::layer::SubscriberExt;

/// The object name of every `matview_ddl` event the actor emitted.
#[derive(Clone, Default)]
struct DdlTargets(Arc<Mutex<Vec<String>>>);

impl DdlTargets {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[derive(Default)]
struct LatencyFields {
    stage: Option<String>,
    view: Option<String>,
}

impl Visit for LatencyFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "stage" => self.stage = Some(value.to_string()),
            "view" => self.view = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl<S: tracing::Subscriber> Layer<S> for DdlTargets {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "holon_latency" {
            return;
        }
        let mut fields = LatencyFields::default();
        event.record(&mut fields);
        if fields.stage.as_deref() == Some("matview_ddl") {
            self.0
                .lock()
                .unwrap()
                .push(fields.view.expect("matview_ddl event names its object"));
        }
    }
}

fn modules() -> Vec<Box<dyn SchemaModule>> {
    vec![
        Box::new(CoreSchemaModule),
        Box::new(BlockSchemaModule),
        Box::new(BlockMatviewSchemaModule),
        Box::new(BlockRequirementEdgesSchemaModule),
        Box::new(TrustProposalsSchemaModule),
        Box::new(BlockHierarchySchemaModule),
        Box::new(NavigationSchemaModule),
        Box::new(SyncStateSchemaModule),
        Box::new(IntegrationStateSchemaModule),
        Box::new(OperationsSchemaModule),
        Box::new(HistorySchemaModule),
        Box::new(BlockDerivedSchemaModule),
        Box::new(AutomationsJournalSchemaModule),
        Box::new(JournalDayPagesSchemaModule),
        Box::new(JournalFeedSchemaModule),
        Box::new(LinkSchemaModule),
        Box::new(IdentitySchemaModule),
    ]
}

/// Every module in dependency order, as the DI graph orders them at boot.
async fn ensure_all(handle: &DbHandle) {
    let mut pending = modules();
    let mut ready: HashSet<Resource> = HashSet::new();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .position(|m| m.requires().iter().all(|r| ready.contains(r)))
            .unwrap_or_else(|| {
                panic!(
                    "no schema module is runnable; unmet requirements: {:?}",
                    pending
                        .iter()
                        .map(|m| (m.name().to_string(), m.requires()))
                        .collect::<Vec<_>>()
                )
            });
        let module = pending.remove(next);
        module
            .ensure_schema(handle)
            .await
            .unwrap_or_else(|e| panic!("ensure_schema {}: {e}", module.name()));
        module
            .initialize_data(handle)
            .await
            .unwrap_or_else(|e| panic!("initialize_data {}: {e}", module.name()));
        ready.extend(module.provides());
    }
}

async fn open(path: &Path) -> (TursoBackend, DbHandle) {
    let db = TursoBackend::open_database(path).expect("open db file");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

async fn view_sql(handle: &DbHandle) -> HashMap<String, String> {
    handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type = 'view'",
            HashMap::new(),
        )
        .await
        .expect("read sqlite_master")
        .into_iter()
        .map(|row| match (row.get("name"), row.get("sql")) {
            (Some(holon_api::Value::String(n)), Some(holon_api::Value::String(s))) => {
                (n.clone(), s.clone())
            }
            other => panic!("view row without name/sql: {other:?}"),
        })
        .collect()
}

async fn paths(handle: &DbHandle) -> Vec<(String, String)> {
    handle
        .query(
            "SELECT id, path FROM block_with_path ORDER BY id",
            HashMap::new(),
        )
        .await
        .expect("read block_with_path")
        .into_iter()
        .map(|row| match (row.get("id"), row.get("path")) {
            (Some(holon_api::Value::String(i)), Some(holon_api::Value::String(p))) => {
                (i.clone(), p.clone())
            }
            other => panic!("block_with_path row without id/path: {other:?}"),
        })
        .collect()
}

async fn seed(handle: &DbHandle) {
    for (id, parent) in [
        ("block:root", "sentinel:no_parent"),
        ("block:child", "block:root"),
    ] {
        handle
            .execute(
                "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, 'x')",
                vec![
                    turso::Value::Text(id.to_string()),
                    turso::Value::Text(parent.to_string()),
                ],
            )
            .await
            .expect("seed block_raw");
    }
}

fn expected_paths() -> Vec<(String, String)> {
    vec![
        (
            "block:child".to_string(),
            "/block:root/block:child".to_string(),
        ),
        ("block:root".to_string(), "/block:root".to_string()),
    ]
}

/// One boot running every schema module: the DDL targets of that pass, the
/// stored views and the `block_with_path` rows.
async fn boot(
    path: &Path,
    ddl: &DdlTargets,
) -> (Vec<String>, HashMap<String, String>, Vec<(String, String)>) {
    let (backend, handle) = open(path).await;
    ddl.take();
    ensure_all(&handle).await;
    let ran = ddl.take();
    let views = view_sql(&handle).await;
    let paths = paths(&handle).await;
    handle.shutdown().await.expect("shutdown");
    drop(backend);
    (ran, views, paths)
}

fn rebuilt_views<'a>(ran: &'a [String], existing: &HashMap<String, String>) -> Vec<&'a String> {
    let mut rebuilt: Vec<&String> = ran.iter().filter(|n| existing.contains_key(*n)).collect();
    rebuilt.sort();
    rebuilt.dedup();
    rebuilt
}

#[tokio::test(flavor = "multi_thread")]
async fn warm_reboot_rebuilds_no_matview_and_a_changed_base_rebuilds_its_dependents() {
    let ddl = DdlTargets::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(ddl.clone()))
        .expect("install the DDL recorder");

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("warm.db");

    let (_, cold_views, _) = boot(&path, &ddl).await;
    assert!(
        cold_views.contains_key("block_with_path"),
        "the cold boot must create block_with_path, or this test proves nothing: {:?}",
        cold_views.keys()
    );
    {
        let (backend, handle) = open(&path).await;
        seed(&handle).await;
        handle.shutdown().await.expect("shutdown seeding");
        drop(backend);
    }

    let (warm_ddl, warm_views, warm_paths) = boot(&path, &ddl).await;
    let rebuilt = rebuilt_views(&warm_ddl, &cold_views);
    assert!(
        rebuilt.is_empty(),
        "a warm reboot over an unchanged schema ran DDL against these existing matviews: \
         {rebuilt:?}\nstored definitions:\n{}",
        rebuilt
            .iter()
            .map(|v| format!("  {v}: {}", cold_views[*v]))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(
        warm_views, cold_views,
        "the warm boot changed the stored view set"
    );
    assert_eq!(warm_paths, expected_paths(), "warm block_with_path rows");

    // A database written by a release whose `block` definition differed: the
    // stale `block` carries a `block_with_path` built over it.
    {
        let (backend, handle) = open(&path).await;
        reconcile_named_view(
            &handle,
            "block",
            "SELECT * FROM block_raw b WHERE b.id != 'sentinel:no_parent'",
        )
        .await
        .expect("plant a stale block definition");
        reconcile_named_view(
            &handle,
            "block_with_path",
            include_str!("../sql/schema/blocks_with_paths.sql"),
        )
        .await
        .expect("build block_with_path over the stale block");
        handle.shutdown().await.expect("shutdown stale boot");
        drop(backend);
    }
    let (upgrade_ddl, upgrade_views, upgrade_paths) = boot(&path, &ddl).await;
    for view in ["block", "block_with_path"] {
        assert!(
            upgrade_ddl.iter().any(|n| n == view),
            "a changed `block` definition must rebuild {view} on the boot that sees it; DDL \
             ran: {upgrade_ddl:?}"
        );
    }
    assert_eq!(
        upgrade_views, cold_views,
        "the upgrade boot must converge on the current definitions"
    );
    assert_eq!(
        upgrade_paths,
        expected_paths(),
        "block_with_path rows after the cascade rebuild"
    );

    let (settled_ddl, _, _) = boot(&path, &ddl).await;
    let rebuilt = rebuilt_views(&settled_ddl, &cold_views);
    assert!(
        rebuilt.is_empty(),
        "the boot after an upgrade must rebuild nothing again: {rebuilt:?}"
    );
}
