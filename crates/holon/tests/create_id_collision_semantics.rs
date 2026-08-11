//! Collision semantics of a `create` whose `id` is ALREADY HELD (task #8,
//! ruled 2026-08-12), driven through the PRODUCTION write path
//! (`SqlOperationProvider`) against the REAL `block_raw` schema.
//!
//! The ruling, per collision case:
//! - SAME title  → idempotent re-create: the create asserts existence, not the
//!   whole row. It writes only the fields it SUPPLIES and only where they
//!   differ; a re-create that supplies nothing new writes nothing at all.
//! - TITLE-LESS  → refused loudly: a create carrying no title cannot be
//!   recognized against the id's holder, so it must not land over it.
//! - DIFFERENT title → the ADR 0029 D1b `IdentityCollision` refusal
//!   (unchanged).
//! - NO collision → unchanged.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::storage::schema_module::EdgeFieldDescriptor;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::Value;
use holon_core::BatchOp;
use holon_core::EventOrigin;
use holon_core::OperationProvider;
use holon_core::OriginTaggedWrites;
use holon_turso::schema_modules::CoreSchemaModule;

const ENTITY: &str = "block";
const TABLE: &str = "block_raw";
const HELD: &str = "block:held";

fn tags_descriptor() -> EdgeFieldDescriptor {
    EdgeFieldDescriptor {
        entity: ENTITY.to_string(),
        field: "tags".to_string(),
        join_table: "block_tags".to_string(),
        source_col: "block_id".to_string(),
        target_col: "tag".to_string(),
    }
}

async fn setup() -> (DbHandle, Arc<SqlOperationProvider>) {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    // The PRODUCTION core schema (block_raw + the `sentinel:no_parent` FK anchor).
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (
                block_id TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (block_id, tag),
                FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
            )",
        )
        .await
        .expect("block_tags table");
    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![tags_descriptor()],
    ));
    (handle, provider)
}

/// One column value, key-order-canonicalized. The `properties` column comes
/// back DECODED as an object whose key order is not stable across reads, so
/// comparing its `Debug` form would report a change where none happened.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(_) => {
            let json: serde_json::Value = v.clone().into();
            let sorted: BTreeMap<String, String> = json
                .as_object()
                .expect("an Object converts to a JSON object")
                .iter()
                .map(|(k, x)| (k.clone(), x.to_string()))
                .collect();
            format!("{sorted:?}")
        }
        other => format!("{other:?}"),
    }
}

/// Every column of the row, plus its tag junction set — the "byte-identical"
/// oracle. Absent row = empty map.
async fn snapshot(handle: &DbHandle, id: &str) -> BTreeMap<String, String> {
    let sql = format!("SELECT * FROM {TABLE} WHERE id = '{id}'");
    let rows = handle.query(&sql, HashMap::new()).await.expect("row query");
    let mut out: BTreeMap<String, String> = rows
        .first()
        .map(|r| {
            r.iter()
                .map(|(k, v)| (k.to_string(), canonical(v)))
                .collect()
        })
        .unwrap_or_default();
    let tags = handle
        .query(
            &format!("SELECT tag FROM block_tags WHERE block_id = '{id}' ORDER BY tag"),
            HashMap::new(),
        )
        .await
        .expect("tag query");
    let tags: Vec<String> = tags
        .into_iter()
        .filter_map(|r| r.get("tag").and_then(|v| v.as_string()).map(String::from))
        .collect();
    out.insert("«tags»".to_string(), tags.join(","));
    out
}

/// A rich holder row: properties, a tag, a real order key, and timestamps far
/// enough in the past that any re-stamp is unmistakable.
fn held_params() -> holon_api::StorageEntity {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(HELD.to_string()));
    p.insert("content".into(), Value::String("Held Note".to_string()));
    p.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    p.insert("sort_key".into(), Value::String("A5".to_string()));
    p.insert("collapsed".into(), Value::Integer(1));
    p.insert("created_at".into(), Value::Integer(1_000));
    p.insert("updated_at".into(), Value::Integer(1_000));
    p.insert("custom_a".into(), Value::String("one".to_string()));
    p.insert("custom_b".into(), Value::String("two".to_string()));
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Held".to_string())]),
    );
    p
}

async fn seed_held(provider: &SqlOperationProvider) {
    let entity: EntityName = ENTITY.to_string().into();
    provider
        .execute_operation(&entity, "create", held_params())
        .await
        .expect("seed the held row");
}

#[tokio::test(flavor = "multi_thread")]
async fn same_title_recreate_writes_nothing_it_does_not_supply() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;
    let before = snapshot(&handle, HELD).await;

    // The minimal colliding create: same id, same title, nothing else — the
    // shape a bare `block.create` action or a cache-missed re-observation
    // produces. It asserts that the block exists; it asserts NOTHING about
    // properties, order, collapse state, or when the block was created.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(HELD.to_string()));
    p.insert("content".into(), Value::String("Held Note".to_string()));
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("an idempotent re-create must succeed, not fail");

    assert_eq!(
        snapshot(&handle, HELD).await,
        before,
        "an idempotent same-title re-create must leave the row byte-identical — it supplied \
         only the id and the title it already had"
    );
}

/// WARN-level tracing capture, dependency-free (same shape as
/// `holon-orgmode/tests/name_chain_error_propagation.rs`).
#[derive(Clone, Default)]
struct WarnCapture(Arc<std::sync::Mutex<Vec<String>>>);

impl WarnCapture {
    fn warnings(&self) -> Vec<String> {
        self.0.lock().expect("capture lock").clone()
    }
}

struct MsgVisitor<'a>(&'a mut String);
impl tracing::field::Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if *event.metadata().level() == tracing::Level::WARN {
            let mut buf = String::new();
            event.record(&mut MsgVisitor(&mut buf));
            self.0.lock().expect("capture lock").push(buf);
        }
    }
}

/// Adopting a holder whose title is SQL NULL must be DISCLOSED, exactly like
/// adopting one whose title is blank.
///
/// `block_raw.content` is `NOT NULL`, so the NULL shape needs a table that
/// permits it — the recognition predicate reads the same either way: no name
/// held, so the create adopts. The bug this pins is that the NULL spelling
/// took the silent arm while `""` took the warning one.
#[tokio::test]
async fn adopting_a_null_titled_holder_is_disclosed() {
    use tracing_subscriber::layer::SubscriberExt;

    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    handle
        .execute_ddl(
            "CREATE TABLE nullable_block (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT)",
        )
        .await
        .expect("nullable table");
    let provider = Arc::new(SqlOperationProvider::new(
        handle.clone(),
        "nullable_block".to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
    ));
    handle
        .execute(
            "INSERT INTO nullable_block (id, parent_id, content) VALUES ('block:nulltitled', \
             'sentinel:no_parent', NULL)",
            vec![],
        )
        .await
        .expect("seed a NULL-titled holder");

    let cap = WarnCapture::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(cap.clone()));

    let entity: EntityName = ENTITY.to_string().into();
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:nulltitled".to_string()));
    p.insert("content".into(), Value::String("Adopted".to_string()));
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("a create over a nameless holder ADOPTS it, it does not collide");

    let rows = handle
        .query(
            "SELECT content FROM nullable_block WHERE id = 'block:nulltitled'",
            HashMap::new(),
        )
        .await
        .expect("read back");
    assert_eq!(
        rows.first().and_then(|r| r.get("content")),
        Some(&Value::String("Adopted".to_string())),
        "the adoption must complete the placeholder with the requested title"
    );

    let warnings = cap.warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("block:nulltitled") && w.contains("UNNAMED placeholder")),
        "adopting a NULL-titled holder must be DISCLOSED, not silent — captured warnings: \
         {warnings:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cdc_lagged_recreate_of_an_unchanged_page_writes_nothing() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;
    let before = snapshot(&handle, HELD).await;

    // The CDC-lag shape (`verify-c1-fix` hazard 1): `LiveDocumentManager`
    // pre-checks LiveData ONLY, so a row already in SQL but not yet mirrored
    // takes the write path and `insert_page` re-creates it with the FULL
    // param set `build_block_params` emits. Distinct from the minimal
    // re-create above: every field is supplied, and every one of them
    // happens to match. Incoming must not "win" over state nobody changed.
    provider
        .execute_operation(&entity, "create", held_params())
        .await
        .expect("a full-param re-create of an unchanged row must succeed");

    assert_eq!(
        snapshot(&handle, HELD).await,
        before,
        "a re-create supplying every field, all of them unchanged, must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn same_title_recreate_writes_only_the_fields_it_supplies() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;
    let before = snapshot(&handle, HELD).await;

    // A re-create that DOES carry new state: the supplied column lands (the
    // authority re-asserts it), everything it did not supply is untouched.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(HELD.to_string()));
    p.insert("content".into(), Value::String("Held Note".to_string()));
    p.insert("collapsed".into(), Value::Integer(0));
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("re-create carrying a changed field must land");

    let after = snapshot(&handle, HELD).await;
    assert_eq!(
        after.get("collapsed").map(String::as_str),
        Some("Integer(0)"),
        "the supplied field must be written"
    );
    for key in [
        "created_at",
        "updated_at",
        "sort_key",
        "properties",
        "«tags»",
    ] {
        assert_eq!(
            after.get(key),
            before.get(key),
            "{key} was not supplied by the re-create and must not change"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn title_less_create_over_a_held_id_is_refused() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;
    let before = snapshot(&handle, HELD).await;

    // No `content`: nothing to recognize the id's holder by, so this create
    // cannot know whether it is the same entity or a rival one.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(HELD.to_string()));
    p.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    p.insert("custom_a".into(), Value::String("clobbered".to_string()));
    let err = provider
        .execute_operation(&entity, "create", p)
        .await
        .expect_err("a title-less create over a held id must be refused, not landed");
    let msg = err.to_string();
    assert!(
        msg.contains(HELD) && msg.to_lowercase().contains("no title"),
        "the refusal must name the id and say WHY it was refused; got: {msg}"
    );
    assert_eq!(
        snapshot(&handle, HELD).await,
        before,
        "a refused create must leave the held row untouched"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn different_title_create_still_collides_with_the_d1b_marker() {
    let (_handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;

    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(HELD.to_string()));
    p.insert("content".into(), Value::String("A Rival Note".to_string()));
    let err = provider
        .execute_operation(&entity, "create", p)
        .await
        .expect_err("a different-title create over a held id must collide (ADR 0029 D1b)");
    assert!(
        err.to_string()
            .contains(holon_api::IDENTITY_COLLISION_MARKER),
        "the D1b arm must be unchanged and keep its stable marker; got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_create_at_a_free_id_is_unchanged() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;

    let mut p = held_params();
    p.insert("id".into(), Value::String("block:fresh".to_string()));
    p.insert("content".into(), Value::String("Fresh Note".to_string()));
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("a create at a free id must land");

    let row = snapshot(&handle, "block:fresh").await;
    assert_eq!(
        row.get("content").map(String::as_str),
        Some("String(\"Fresh Note\")"),
        "a non-colliding create must write its row in full: {row:?}"
    );
    assert_eq!(
        row.get("«tags»").map(String::as_str),
        Some("Held"),
        "its edge fields must land too: {row:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_batch_seam_obeys_the_same_collision_semantics() {
    let (handle, provider) = setup().await;
    let entity: EntityName = ENTITY.to_string().into();
    seed_held(&provider).await;
    let before = snapshot(&handle, HELD).await;

    // The batch seam (`execute_batch_with_origin`) is what bulk org ingest and
    // the Loro→SQL projection write through. A cold boot re-derives create vs
    // update from a cache that is EMPTY, so an existing row arrives here as a
    // "create" — the live collision window.
    let mut same: holon_api::StorageEntity = HashMap::new();
    same.insert("id".into(), Value::String(HELD.to_string()));
    same.insert("content".into(), Value::String("Held Note".to_string()));
    provider
        .execute_batch_with_origin(
            &entity,
            vec![BatchOp::data("create", same)],
            EventOrigin::Org,
        )
        .await
        .expect("an idempotent batch re-create must succeed");
    assert_eq!(
        snapshot(&handle, HELD).await,
        before,
        "the batch seam must not clobber a row it merely re-observed"
    );

    let mut title_less: holon_api::StorageEntity = HashMap::new();
    title_less.insert("id".into(), Value::String(HELD.to_string()));
    title_less.insert("custom_a".into(), Value::String("clobbered".to_string()));
    let err = provider
        .execute_batch_with_origin(
            &entity,
            vec![BatchOp::data("create", title_less)],
            EventOrigin::Org,
        )
        .await
        .expect_err("a title-less batch create over a held id must be refused");
    assert!(
        err.to_string().contains(HELD),
        "the batch refusal must name the id; got: {err}"
    );
    assert_eq!(
        snapshot(&handle, HELD).await,
        before,
        "a refused batch must leave the held row untouched"
    );
}
