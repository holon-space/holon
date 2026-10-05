//! A judged write claims the decision subtree its judgement shapes and is
//! judged again under that claim, so two writes with disjoint admission
//! claims cannot together leave a decision no reader parses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::OpOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const PAGE: &str = "block:page";
const DECISION: &str = "block:d";
const OPTION_A: &str = "block:d-a";
const OPTION_B: &str = "block:d-b";

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

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs.iter().map(|(k, v)| ((*k).into(), s(v))).collect()
}

/// The params of a `create` seeded as foreign input.
fn created(
    id: &str,
    parent: &str,
    content: &str,
    props: &[(&str, &str)],
    tags: &[&str],
) -> StorageEntity {
    let mut p = params(&[("id", id), ("parent_id", parent), ("content", content)]);
    p.insert(
        "properties".into(),
        Value::Object(props.iter().map(|(k, v)| (k.to_string(), s(v))).collect()),
    );
    p.insert(
        "tags".into(),
        Value::Array(tags.iter().map(|t| s(t)).collect()),
    );
    p
}

/// Writes `ops` as foreign input (`Sync`), which the gate never judges.
async fn foreign(engine: &BackendEngine, ops: Vec<(&str, StorageEntity)>) {
    for (op, p) in ops {
        engine
            .execute_operation(&EntityName::new("block"), op, p, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("seed {op}: {e:#}"));
    }
}

/// A page holding a decision decided for option a, of options a, b and c.
async fn seed(engine: &BackendEngine) {
    let mut decision = created(
        DECISION,
        PAGE,
        "Which store?",
        &[
            ("chosen", "a"),
            ("decider", "person:martin"),
            ("decided", "2026-09-29T10:00:00Z"),
        ],
        &["decision"],
    );
    decision.insert("task_state".into(), s("DONE"));
    foreign(
        engine,
        vec![
            (
                "create",
                created(PAGE, "sentinel:no_parent", "page", &[], &[]),
            ),
            ("create", decision),
            (
                "create",
                created(OPTION_A, DECISION, "Loro", &[("option", "a")], &[]),
            ),
            (
                "create",
                created(OPTION_B, DECISION, "Turso", &[("option", "b")], &[]),
            ),
            (
                "create",
                created("block:d-c", DECISION, "SQLite", &[("option", "c")], &[]),
            ),
        ],
    )
    .await;
}

fn spawn_write(
    engine: &BackendEngine,
    op: &str,
    pairs: &[(&str, &str)],
) -> tokio::task::JoinHandle<anyhow::Result<OpOutcome>> {
    tokio::spawn(engine.execute_operation(
        &EntityName::new("block"),
        op,
        params(pairs),
        OpOrigin::User,
    ))
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting until {what}"));
}

async fn finished(
    write: tokio::task::JoinHandle<anyhow::Result<OpOutcome>>,
) -> anyhow::Result<OpOutcome> {
    tokio::time::timeout(Duration::from_secs(10), write)
        .await
        .expect("the write finishes")
        .expect("the write's task")
}

async fn read(engine: &BackendEngine, id: &str, expr: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT {expr} AS v FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read the block");
    match rows.first().and_then(|r| r.get("v")) {
        Some(Value::String(v)) => Some(v.clone()),
        Some(Value::Null) | None => None,
        Some(other) => panic!("{id}.{expr} is {other:?}"),
    }
}

async fn chosen(engine: &BackendEngine) -> Option<String> {
    read(engine, DECISION, "json_extract(properties, '$.chosen')").await
}

async fn waiting(engine: &BackendEngine, n: usize, what: &str) -> bool {
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.admission().census().waiting != n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| eprintln!("never {n} waiting: {what}"))
    .is_ok()
}

/// W1 re-decides for b and is held after its judgement; W2 deletes option b.
/// Their admission claims are disjoint. W2 must wait for W1's effect and be
/// judged against it: with `chosen: b` stored, deleting b breaks DC2.
#[tokio::test(flavor = "multi_thread")]
async fn two_disjoint_judged_ops_cannot_break_a_decision_together() {
    let engine = block_engine().await;
    seed(&engine).await;
    let hold = engine.dispatch_hold();

    hold.hold_next_judged("block", "set_field");
    let w1 = spawn_write(
        &engine,
        "set_field",
        &[("id", DECISION), ("field", "chosen"), ("value", "b")],
    );
    until("W1 is held after its judgement", || {
        hold.parked_runs("block", "set_field") == 1
    })
    .await;
    let w2 = spawn_write(&engine, "delete", &[("id", OPTION_B)]);
    let w2_waited = waiting(&engine, 1, "the delete of option b behind W1's shape claim").await;
    hold.release(1, Duration::from_secs(10))
        .await
        .expect("release W1");

    finished(w1).await.expect("W1 re-decides for b");
    let w2 = finished(w2).await;
    let refused = w2.expect_err(
        "deleting option b while the decision is decided for b breaks DC2; both writes landed",
    );
    assert!(format!("{refused:#}").contains("DC2"), "{refused:#}");
    assert!(
        w2_waited,
        "the delete ran without waiting for W1's shape claim"
    );
    assert_eq!(chosen(&engine).await.as_deref(), Some("b"));
    assert_eq!(
        read(&engine, OPTION_B, "id").await.as_deref(),
        Some(OPTION_B),
        "option b stays"
    );
}

/// B (held before its judgement) holds the decision; A, a judged write on
/// option a, claims the subtree and waits on B; B's own shape claim then
/// needs A's block: a wait cycle. B has written nothing, so it is admitted
/// again behind A, and both land.
#[tokio::test(flavor = "multi_thread")]
async fn a_wait_cycle_between_two_judged_writes_retries_one_and_both_land() {
    let engine = block_engine().await;
    seed(&engine).await;
    let hold = engine.dispatch_hold();

    hold.hold_next("block", "set_field");
    let b = spawn_write(
        &engine,
        "set_field",
        &[("id", DECISION), ("field", "chosen"), ("value", "c")],
    );
    until("B is held before its judgement", || hold.parked() == 1).await;
    let a = spawn_write(
        &engine,
        "set_field",
        &[("id", OPTION_A), ("field", "task_state"), ("value", "TODO")],
    );
    assert!(
        waiting(&engine, 1, "A's shape claim behind B").await,
        "A's shape claim did not wait on B"
    );
    hold.release(1, Duration::from_secs(10))
        .await
        .expect("release B");

    finished(a).await.expect("A lands");
    finished(b).await.expect("B lands after its retry");
    assert_eq!(chosen(&engine).await.as_deref(), Some("c"));
    assert_eq!(
        read(
            &engine,
            OPTION_A,
            "json_extract(properties, '$.task_state')"
        )
        .await
        .as_deref(),
        Some("TODO")
    );
}

/// A user's write whose shape claim is refused on every attempt fails with
/// an error that names the refusals, and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_refused_its_shape_claim_three_times_fails_by_name_and_writes_nothing() {
    let engine = block_engine().await;
    seed(&engine).await;
    engine
        .dispatch_hold()
        .refuse_next_nests("block", "set_field", 3);

    let err = finished(spawn_write(
        &engine,
        "set_field",
        &[("id", DECISION), ("field", "chosen"), ("value", "c")],
    ))
    .await
    .expect_err("every attempt's shape claim is refused");
    let text = format!("{err:#}");
    assert!(text.contains("refused 3 times"), "{text}");
    assert!(text.contains("could not claim the tagged blocks"), "{text}");
    assert_eq!(chosen(&engine).await.as_deref(), Some("a"));
}

/// P writes keyword-headed text on option a, which lands as its content and
/// then its `task_state`. W, a judged write on the decision, claims the
/// subtree and waits on P. P's claim then closes a cycle with W. P must not
/// end with its text landed and its keyword refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_converged_keyword_write_never_lands_half_when_its_claim_closes_a_cycle() {
    let engine = block_engine().await;
    seed(&engine).await;
    let hold = engine.dispatch_hold();

    hold.hold_next("block", "set_field");
    let p = spawn_write(
        &engine,
        "set_field",
        &[
            ("id", OPTION_A),
            ("field", "content"),
            ("value", "TODO Loro store"),
        ],
    );
    until("P is held before its judgement", || hold.parked() == 1).await;
    let w = spawn_write(
        &engine,
        "set_field",
        &[("id", DECISION), ("field", "chosen"), ("value", "c")],
    );
    assert!(
        waiting(&engine, 1, "W's shape claim behind P").await,
        "W's shape claim did not wait on P"
    );
    hold.release(1, Duration::from_secs(10))
        .await
        .expect("release P");

    let p = finished(p).await;
    finished(w).await.expect("W lands");
    let content = read(&engine, OPTION_A, "content").await;
    let keyword = read(
        &engine,
        OPTION_A,
        "json_extract(properties, '$.task_state')",
    )
    .await;
    p.unwrap_or_else(|e| {
        panic!("P failed with content {content:?} and task_state {keyword:?} stored: {e:#}")
    });
    assert_eq!(content.as_deref(), Some("Loro store"));
    assert_eq!(keyword.as_deref(), Some("TODO"));
    assert_eq!(chosen(&engine).await.as_deref(), Some("c"));
}

/// A new child of the decision typed as keyword-headed text lands as its
/// content and then its `task_state`. The block is new, so the claim taken
/// before the create cannot name it; the keyword write must still not need a
/// claim of its own, which could be refused after the create landed.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_child_never_lands_without_its_converged_keyword() {
    let engine = block_engine().await;
    seed(&engine).await;
    engine
        .dispatch_hold()
        .refuse_next_nests("block", "set_field", 1);

    let created = finished(spawn_write(
        &engine,
        "create",
        &[
            ("id", "block:d-n"),
            ("parent_id", DECISION),
            ("content", "TODO benchmark both"),
        ],
    ))
    .await;
    let content = read(&engine, "block:d-n", "content").await;
    let keyword = read(
        &engine,
        "block:d-n",
        "json_extract(properties, '$.task_state')",
    )
    .await;
    created.unwrap_or_else(|e| {
        panic!(
            "the create failed with content {content:?} and task_state {keyword:?} stored: {e:#}"
        )
    });
    assert_eq!(content.as_deref(), Some("benchmark both"));
    assert_eq!(keyword.as_deref(), Some("TODO"));
}

/// A page whose `#+TODO:` ring is `ring`.
fn page(id: &str, ring: &[holon_api::TaskState]) -> Vec<(&'static str, StorageEntity)> {
    let ring = serde_json::to_string(ring).expect("TaskState serializes");
    vec![
        ("create", created(id, "sentinel:no_parent", id, &[], &[])),
        (
            "set_field",
            params(&[("id", id), ("field", "todo_keywords"), ("value", &ring)]),
        ),
        ("add_tag", params(&[("id", id), ("tag", "Page")])),
    ]
}

/// Moving a block into a document whose ring classifies a keyword otherwise
/// re-derives the category of every task below it, after the move landed.
/// The move itself shapes no decision, but the category write on a child of
/// the decision below it does; it must not need a claim of its own, which
/// could be refused after the move landed.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_never_lands_without_the_categories_it_re_derives() {
    use holon_api::TaskState;

    let engine = block_engine().await;
    let mut ops = page(
        "block:closing",
        &[
            TaskState::active("?"),
            TaskState::active("TODO"),
            TaskState::done("CANCELLED"),
        ],
    );
    ops.extend(page(
        "block:triage",
        &[
            TaskState::active("?"),
            TaskState::active("TODO"),
            TaskState::active("CANCELLED"),
            TaskState::done("DONE"),
        ],
    ));
    let mut note = created("block:dx-n", "block:dx", "Ask the team", &[], &[]);
    note.insert("task_state".into(), s("CANCELLED"));
    let mut question = created("block:dx", "block:x", "Which store?", &[], &["decision"]);
    question.insert("task_state".into(), s("?"));
    ops.extend([
        (
            "create",
            created("block:x", "block:closing", "Storage", &[], &[]),
        ),
        ("create", question),
        (
            "create",
            created("block:dx-a", "block:dx", "Loro", &[("option", "a")], &[]),
        ),
        (
            "create",
            created("block:dx-b", "block:dx", "Turso", &[("option", "b")], &[]),
        ),
        ("create", note),
    ]);
    foreign(&engine, ops).await;
    let category = |engine: Arc<BackendEngine>| async move {
        read(
            &engine,
            "block:dx-n",
            "json_extract(properties, '$.task_state_category')",
        )
        .await
    };
    assert_eq!(category(Arc::clone(&engine)).await.as_deref(), Some("done"));
    engine
        .dispatch_hold()
        .refuse_next_nests("block", "set_field", 1);

    let moved = finished(spawn_write(
        &engine,
        "move_block",
        &[("id", "block:x"), ("parent_id", "block:triage")],
    ))
    .await;
    let parent = read(&engine, "block:x", "parent_id").await;
    let category = category(Arc::clone(&engine)).await;
    moved.unwrap_or_else(|e| {
        panic!("the move failed with parent {parent:?} and category {category:?} stored: {e:#}")
    });
    assert_eq!(parent.as_deref(), Some("block:triage"));
    assert_eq!(category.as_deref(), Some("active"));
}

/// W re-decides for b and is held after its judgement, holding its shape
/// claim on the decision and its children. A keystroke into option b must
/// wait for W, so W's writes land on the state W judged.
#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_into_a_child_of_a_claimed_decision_waits_for_the_held_write() {
    let engine = block_engine().await;
    seed(&engine).await;
    let hold = engine.dispatch_hold();

    hold.hold_next_judged("block", "set_field");
    let w = spawn_write(
        &engine,
        "set_field",
        &[("id", DECISION), ("field", "chosen"), ("value", "b")],
    );
    until("W is held after its judgement", || {
        hold.parked_runs("block", "set_field") == 1
    })
    .await;
    let keystroke = tokio::spawn(engine.commit_keystroke(holon_api::SourceKeystroke {
        id: OPTION_B.to_string(),
        source: "Turso db".to_string(),
        write_seq: None,
    }));
    let waited = waiting(
        &engine,
        1,
        "the keystroke into option b behind W's shape claim",
    )
    .await;
    hold.release(1, Duration::from_secs(10))
        .await
        .expect("release W");

    finished(w).await.expect("W re-decides for b");
    finished(keystroke).await.expect("the keystroke lands");
    assert!(waited, "the keystroke ran inside W's judged window");
    assert_eq!(chosen(&engine).await.as_deref(), Some("b"));
    assert_eq!(
        read(&engine, OPTION_B, "content").await.as_deref(),
        Some("Turso db")
    );
}

/// A plan adds option d and re-decides for b; it is held after its first op.
/// Three foreign writes reach into the decision: a create under it (its claim
/// names the decision), an indent of its next sibling into it and an outdent
/// of option b (their claims name neither; only their judgements find the
/// decision). Each must wait for the whole plan and be judged against its
/// result: with `chosen: b` stored, moving option b out breaks DC2.
#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_write_into_a_claimed_subtree_waits_and_the_decision_stays_valid() {
    let engine = block_engine().await;
    seed(&engine).await;
    foreign(
        &engine,
        vec![(
            "create",
            created("block:x", PAGE, "next to the decision", &[], &[]),
        )],
    )
    .await;
    let hold = engine.dispatch_hold();

    let to_op = |op: &str, p: StorageEntity| {
        holon_api::Operation::from_params(
            "block",
            op,
            op,
            p.into_iter().map(|(k, v)| (k.to_string(), v)),
        )
    };
    let ops = vec![
        to_op(
            "create",
            created("block:d-d", DECISION, "DuckDB", &[("option", "d")], &[]),
        ),
        to_op(
            "set_field",
            params(&[("id", DECISION), ("field", "chosen"), ("value", "b")]),
        ),
    ];
    hold.hold_next("block", "set_field");
    let plan = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .execute_judged(&ops, &OpOrigin::User, async {
                    for op in &ops {
                        engine
                            .execute_operation(
                                &op.entity_name,
                                &op.op_name,
                                op.params
                                    .iter()
                                    .map(|(k, v)| (k.as_str().into(), v.clone()))
                                    .collect(),
                                OpOrigin::User,
                            )
                            .await?;
                    }
                    Ok(())
                })
                .await
        })
    };
    until("the plan is held after its first op", || {
        hold.parked_runs("block", "set_field") == 1
    })
    .await;

    let create = spawn_write(
        &engine,
        "create",
        &[
            ("id", "block:d-note"),
            ("parent_id", DECISION),
            ("content", "a note"),
        ],
    );
    let indent = spawn_write(&engine, "indent", &[("id", "block:x")]);
    let outdent = spawn_write(&engine, "outdent", &[("id", OPTION_B)]);
    let waited = waiting(&engine, 3, "the three foreign writes behind the plan").await;
    hold.release(1, Duration::from_secs(10))
        .await
        .expect("release the plan");

    let plan = tokio::time::timeout(Duration::from_secs(10), plan)
        .await
        .expect("the plan finishes")
        .expect("the plan's task");
    let create = finished(create).await;
    let indent = finished(indent).await;
    let outdent = finished(outdent).await;
    assert!(
        waited,
        "a foreign write ran inside the plan's judged window"
    );
    plan.expect("the plan adds option d and re-decides for b");
    create.expect("a note under the decision is legal after the plan");
    indent.expect("indenting a plain block into the decision is legal after the plan");
    let refused = outdent.expect_err(
        "moving option b out while the decision is decided for b breaks DC2; it was judged \
         against the state before the plan",
    );
    assert!(format!("{refused:#}").contains("DC2"), "{refused:#}");
    assert_eq!(chosen(&engine).await.as_deref(), Some("b"));
    for (child, why) in [
        (OPTION_B, "option b stays"),
        ("block:d-d", "the plan's option"),
        ("block:d-note", "the create"),
        ("block:x", "the indent"),
    ] {
        assert_eq!(
            read(&engine, child, "parent_id").await.as_deref(),
            Some(DECISION),
            "{why}"
        );
    }
}
