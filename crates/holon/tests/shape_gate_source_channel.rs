//! The shape gate's keystroke exemption belongs to the editor's CHANNEL, not
//! to the `source_text` field: the same source line written through
//! `set_field("source_text")` is judged, whoever sends it, while
//! `OperationEngine::commit_keystroke` lands it (Model.md invariant 17).

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::SOURCE_TEXT_FIELD;
use holon_api::SourceKeystroke;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_frontend::OperationIntent;
use holon_frontend::execute_gesture;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const DECISION: &str = "block:d";

/// The production SqlOnly block wiring: the CRUD authority plus the structural
/// provider.
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread test
                    // runtime; block_in_place makes the bridge deadlock-free.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block provider")
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

const PAGE: &str = "block:page";

/// Seed a page holding an open decision with one option, as foreign input
/// (`Sync`), which the gate never judges.
async fn seed(engine: &BackendEngine) {
    let create = |id: &str, parent: &str, content: &str, props: &[(&str, &str)], tags: &[&str]| {
        let mut p: StorageEntity = HashMap::new();
        p.insert("id".into(), s(id));
        p.insert("parent_id".into(), s(parent));
        p.insert("content".into(), s(content));
        p.insert(
            "properties".into(),
            Value::Object(props.iter().map(|(k, v)| (k.to_string(), s(v))).collect()),
        );
        p.insert(
            "tags".into(),
            Value::Array(tags.iter().map(|t| s(t)).collect()),
        );
        p
    };
    for params in [
        create(PAGE, "sentinel:no_parent", "page", &[], &[]),
        {
            let mut d = create(DECISION, PAGE, "Which store?", &[], &["decision"]);
            d.insert("task_state".into(), s("?"));
            d.insert("sort_key".into(), s("A0"));
            d
        },
        create("block:d-a", DECISION, "Loro", &[("option", "a")], &[]),
    ] {
        engine
            .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("seed: {e:#}"));
    }
}

async fn task_state(engine: &BackendEngine) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT json_extract(properties, '$.task_state') AS ts FROM {BLOCK_WRITE_TABLE} \
                 WHERE id = '{DECISION}'"
            ),
            HashMap::new(),
        )
        .await
        .expect("read the decision");
    match rows.first().and_then(|r| r.get("ts")) {
        Some(Value::String(v)) if !v.is_empty() => Some(v.clone()),
        _ => None,
    }
}

fn source_write(source: &str) -> StorageEntity {
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), s(DECISION));
    p.insert("field".into(), s(SOURCE_TEXT_FIELD));
    p.insert("value".into(), s(source));
    p
}

/// Demoting the decision's keyword leaves no decision (B4).
const DEMOTED: &str = "Which store?";

#[tokio::test(flavor = "multi_thread")]
async fn a_source_text_write_by_an_agent_is_judged() {
    let engine = block_engine().await;
    seed(&engine).await;
    let agent = OpOrigin::Agent {
        session_id: "s".into(),
        tool_call_id: "t".into(),
    };
    let err = engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            source_write(DEMOTED),
            agent,
        )
        .await
        .expect_err("a demoting source write by an agent is refused");
    assert!(format!("{err:#}").contains("rule B4"), "{err:#}");
    assert_eq!(
        task_state(&engine).await.as_deref(),
        Some("?"),
        "nothing was written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_text_write_by_name_is_judged_even_as_the_user() {
    let engine = block_engine().await;
    seed(&engine).await;
    let err = engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            source_write(DEMOTED),
            OpOrigin::User,
        )
        .await
        .expect_err("the field name is no keystroke channel");
    assert!(format!("{err:#}").contains("rule B4"), "{err:#}");
    assert_eq!(task_state(&engine).await.as_deref(), Some("?"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_editor_keystroke_lands_even_when_it_breaks_the_shape() {
    let engine = block_engine().await;
    seed(&engine).await;
    engine
        .commit_keystroke(SourceKeystroke {
            id: DECISION.into(),
            source: DEMOTED.into(),
            write_seq: None,
        })
        .await
        .expect("a keystroke is never refused by a shape");
    assert_eq!(
        task_state(&engine).await,
        None,
        "the keystroke demoted the block"
    );
}

/// The web editor's flush crosses the worker boundary as a wire intent and
/// runs as the user's gesture there. Typing a keyword at the start of an open
/// decision's title is text on that path, so the gate has nothing to refuse.
#[tokio::test(flavor = "multi_thread")]
async fn web_typing_on_an_open_decision_lands() {
    let engine = block_engine().await;
    seed(&engine).await;
    let wire = OperationIntent::content_edit(DECISION, "DONE Which store?").to_wire();
    let intent = OperationIntent::from_wire(&wire).expect("the worker decodes the wire");
    execute_gesture(engine.as_ref(), intent)
        .await
        .expect("web typing is never refused by a shape");
    assert_eq!(task_state(&engine).await.as_deref(), Some("?"));
}

fn create(id: &str, parent: &str, props: &[(&str, &str)], after: Option<&str>) -> StorageEntity {
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), s(id));
    p.insert("parent_id".into(), s(parent));
    p.insert("content".into(), s("a note"));
    p.insert(
        "properties".into(),
        Value::Object(props.iter().map(|(k, v)| (k.to_string(), s(v))).collect()),
    );
    if let Some(after) = after {
        p.insert("after_block_id".into(), s(after));
    }
    p
}

async fn parent_of(engine: &BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT parent_id FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read the block");
    match rows.first().and_then(|r| r.get("parent_id")) {
        Some(Value::String(p)) => p.clone(),
        other => panic!("{id} has parent {other:?}"),
    }
}

/// The SQL authority answers the gate's "is anything tagged near this write"
/// with one query. A block whose previous sibling is the decision is near it:
/// the indent makes it an option, here with a key the decision already has.
#[tokio::test(flavor = "multi_thread")]
async fn indenting_a_sibling_into_a_decision_is_judged_over_the_sql_authority() {
    let engine = block_engine().await;
    seed(&engine).await;
    for params in [
        {
            let mut p = create("block:after", PAGE, &[("option", "a")], Some(DECISION));
            p.insert("sort_key".into(), s("A5"));
            p
        },
        create("block:p", "sentinel:no_parent", &[], None),
        create("block:x", "block:p", &[], None),
        create("block:y", "block:p", &[], Some("block:x")),
    ] {
        engine
            .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("seed: {e:#}"));
    }
    let indent = |id: &str| {
        let mut p: StorageEntity = HashMap::new();
        p.insert("id".into(), s(id));
        p
    };
    let err = engine
        .execute_operation(
            &EntityName::new("block"),
            "indent",
            indent("block:after"),
            OpOrigin::User,
        )
        .await
        .expect_err("the decision would hold option a twice");
    assert!(format!("{err:#}").contains("rule DC1"), "{err:#}");
    assert_eq!(parent_of(&engine, "block:after").await, PAGE);
    engine
        .execute_operation(
            &EntityName::new("block"),
            "indent",
            indent("block:y"),
            OpOrigin::User,
        )
        .await
        .expect("an indent far from every decision lands");
    assert_eq!(parent_of(&engine, "block:y").await, "block:x");
}

/// A write to a decision's child is judged: the SQL authority's neighbourhood
/// read places the block under a tagged one.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_only_option_is_judged_over_the_sql_authority() {
    let engine = block_engine().await;
    seed(&engine).await;
    let err = engine
        .execute_operation(
            &EntityName::new("block"),
            "delete",
            HashMap::from([("id".into(), s("block:d-a"))]),
            OpOrigin::User,
        )
        .await
        .expect_err("the decision would have no option");
    assert!(format!("{err:#}").contains("rule DC1"), "{err:#}");
    assert_eq!(parent_of(&engine, "block:d-a").await, DECISION);
}

/// A plan legal only as a whole that stops part way leaves its prefix
/// applied: the SqlOnly authority has no rollback. Moving the only option out
/// leaves the decision with none until the new option lands, and the plan
/// fails before it does.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_that_stops_part_way_names_the_decision_it_left_broken() {
    let engine = block_engine().await;
    seed(&engine).await;
    let move_out: StorageEntity = HashMap::from([
        ("id".into(), s("block:d-a")),
        ("parent_id".into(), s(PAGE)),
        ("after_block_id".into(), s(DECISION)),
    ]);
    let replacement = create("block:d-b", DECISION, &[("option", "b")], None);
    let as_op = |name: &str, params: &StorageEntity| {
        holon_api::Operation::new(
            "block",
            name,
            name,
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    };
    let plan = [
        as_op("move_block", &move_out),
        as_op("create", &replacement),
    ];
    let untouched = engine
        .execute_judged::<_, ()>(&plan, &OpOrigin::User, async {
            anyhow::bail!("the move failed")
        })
        .await
        .expect_err("the plan failed at its first op");
    assert_eq!(format!("{untouched:#}"), "the move failed");
    let err = engine
        .execute_judged::<_, ()>(&plan, &OpOrigin::User, async {
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "move_block",
                    move_out.clone(),
                    OpOrigin::User,
                )
                .await?;
            anyhow::bail!("the create failed")
        })
        .await
        .expect_err("the plan failed at its second op");
    let err = format!("{err:#}");
    assert!(
        err.contains("the create failed") && err.contains("rule DC1"),
        "the failure names the decision its applied ops left broken: {err}"
    );
    assert_eq!(parent_of(&engine, "block:d-a").await, PAGE);
}
