//! A source-line write claims only the block it writes: typing in one block
//! never waits behind a write on another.

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
use holon_api::Operation;
use holon_api::SOURCE_TEXT_FIELD;
use holon_api::SourceKeystroke;
use holon_api::Value;
use holon_api::admission::AdmissionCensus;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const PAGE: &str = "block:page";
const HELD: &str = "block:held";
const TYPED: &str = "block:typed";

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

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).to_string())))
        .collect()
}

async fn seed(engine: &BackendEngine) {
    for (id, parent) in [(PAGE, "sentinel:no_parent"), (HELD, PAGE), (TYPED, PAGE)] {
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                params(&[("id", id), ("parent_id", parent), ("content", id)]),
                OpOrigin::Sync,
            )
            .await
            .unwrap_or_else(|e| panic!("seed {id}: {e:#}"));
    }
}

/// Park a user's `set_field` on [`HELD`] inside its claim.
async fn hold_a_write(engine: &Arc<BackendEngine>) -> tokio::task::JoinHandle<()> {
    hold(
        engine,
        "set_field",
        &[("id", HELD), ("field", "content"), ("value", "held")],
    )
    .await
}

/// Park a user's `op` with `pairs` inside its claim.
async fn hold(
    engine: &Arc<BackendEngine>,
    op: &'static str,
    pairs: &[(&str, &str)],
) -> tokio::task::JoinHandle<()> {
    engine.dispatch_hold().hold_next("block", op);
    let held = tokio::spawn(engine.execute_operation(
        &EntityName::new("block"),
        op,
        params(pairs),
        OpOrigin::User,
    ));
    let parked = tokio::time::timeout(Duration::from_secs(10), async {
        while engine.dispatch_hold().parked_runs("block", op) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(parked.is_ok(), "the held {op} never reached the hold");
    tokio::spawn(async move {
        held.await
            .expect("the held write's task")
            .expect("the held write lands once released");
    })
}

async fn content(engine: &BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read the block");
    rows[0]["content"]
        .as_string()
        .expect("content is text")
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_does_not_wait_behind_a_held_write_on_another_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let keystroke = engine.commit_keystroke(SourceKeystroke {
        id: TYPED.into(),
        source: "typed while another block is written".into(),
        write_seq: None,
    });
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 2,
            waiting: 0
        },
        "the keystroke on {TYPED} waits behind the held write on {HELD}"
    );
    tokio::time::timeout(Duration::from_secs(10), keystroke)
        .await
        .expect("the keystroke finishes while the other write is held")
        .expect("the keystroke lands");
    assert_eq!(
        content(&engine, TYPED).await,
        "typed while another block is written"
    );

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_line_write_does_not_wait_behind_a_held_write_on_another_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let write = engine.execute_operation(
        &EntityName::new("block"),
        "set_field",
        params(&[
            ("id", TYPED),
            ("field", SOURCE_TEXT_FIELD),
            ("value", "TODO written while another block is written"),
        ]),
        OpOrigin::User,
    );
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 2,
            waiting: 0
        },
        "the source-line write on {TYPED} waits behind the held write on {HELD}"
    );
    tokio::time::timeout(Duration::from_secs(10), write)
        .await
        .expect("the source-line write finishes while the other write is held")
        .expect("the source-line write lands");

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
}

/// Text that parses as an absolute URI is content, not a reference: two
/// blocks holding the same link do not serialize.
#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_of_a_uri_does_not_wait_behind_a_held_write_of_the_same_uri() {
    for text in ["https://example.com/x", "mailto:a@b.c", "doi:10.1/2"] {
        let engine = block_engine().await;
        seed(&engine).await;
        let held = hold(
            &engine,
            "set_field",
            &[("id", HELD), ("field", "content"), ("value", text)],
        )
        .await;

        let keystroke = engine.commit_keystroke(SourceKeystroke {
            id: TYPED.into(),
            source: text.into(),
            write_seq: None,
        });
        assert_eq!(
            engine.admission().census().waiting,
            0,
            "the keystroke of {text:?} on {TYPED} waits behind the held write of the same text \
             on {HELD}"
        );
        tokio::time::timeout(Duration::from_secs(10), keystroke)
            .await
            .expect("the keystroke finishes while the other write is held")
            .expect("the keystroke lands");
        assert_eq!(content(&engine, TYPED).await, text);

        engine
            .dispatch_hold()
            .release(1, Duration::from_secs(5))
            .await
            .expect("release the held write");
        held.await.expect("the held write completes");
    }
}

/// A create claims the row it creates and the parent it lands under, not the
/// whole relation.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_create_claims_its_row_and_parent_only() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold(
        &engine,
        "create",
        &[("id", "block:new"), ("parent_id", HELD), ("content", "new")],
    )
    .await;

    let unrelated = engine.commit_keystroke(SourceKeystroke {
        id: TYPED.into(),
        source: "typed during a create elsewhere".into(),
        write_seq: None,
    });
    assert_eq!(
        engine.admission().census().waiting,
        0,
        "the keystroke on {TYPED} waits behind a held create under {HELD}"
    );
    tokio::time::timeout(Duration::from_secs(10), unrelated)
        .await
        .expect("the keystroke finishes while the create is held")
        .expect("the keystroke lands");

    let on_parent = tokio::spawn(engine.commit_keystroke(SourceKeystroke {
        id: HELD.into(),
        source: "typed into the parent".into(),
        write_seq: None,
    }));
    assert_eq!(
        engine.admission().census().waiting,
        1,
        "a keystroke on the parent {HELD} runs past the held create under it"
    );

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held create");
    held.await.expect("the held create completes");
    on_parent
        .await
        .expect("the parent keystroke's task")
        .expect("the parent keystroke lands");
}

/// A move claims the sibling it is placed after: the anchor's position is
/// what the move reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_on_the_anchor_waits_behind_a_held_move() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold(
        &engine,
        "move_block",
        &[("id", HELD), ("parent_id", PAGE), ("after_block_id", TYPED)],
    )
    .await;

    let keystroke = tokio::spawn(engine.commit_keystroke(SourceKeystroke {
        id: TYPED.into(),
        source: "typed into the anchor".into(),
        write_seq: None,
    }));
    assert_eq!(
        engine.admission().census().waiting,
        1,
        "a keystroke on the anchor {TYPED} runs past the held move placed after it"
    );

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held move");
    held.await.expect("the held move completes");
    keystroke
        .await
        .expect("the keystroke's task")
        .expect("the keystroke lands");
}

/// Hold `op` with `pairs`, then type into `subject`: the keystroke must wait.
async fn assert_a_keystroke_on_waits_behind(
    subject: &str,
    op: &'static str,
    pairs: &[(&str, &str)],
) {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold(&engine, op, pairs).await;

    let keystroke = tokio::spawn(engine.commit_keystroke(SourceKeystroke {
        id: subject.into(),
        source: format!("typed into {subject}"),
        write_seq: None,
    }));
    assert_eq!(
        engine.admission().census().waiting,
        1,
        "a keystroke on {subject} runs past the held {op} {pairs:?}"
    );

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("release the held {op}: {e:#}"));
    held.await.expect("the held write completes");
    keystroke
        .await
        .expect("the keystroke's task")
        .expect("the keystroke lands");
}

/// A restore recreates a block under its parent: the parent is a subject.
#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_on_the_restore_parent_waits_behind_a_held_restore_split() {
    assert_a_keystroke_on_waits_behind(
        HELD,
        "restore_split",
        &[
            ("target_id", TYPED),
            ("target_content", "kept"),
            ("block_id", "block:restored"),
            ("block_content", "restored"),
            ("block_parent", HELD),
        ],
    )
    .await;
}

/// An update that places its row after a sibling claims that sibling.
#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_on_the_update_anchor_waits_behind_a_held_update() {
    assert_a_keystroke_on_waits_behind(
        HELD,
        "update",
        &[
            ("id", TYPED),
            ("content", "moved"),
            ("after_block_id", HELD),
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keystroke_waits_behind_a_held_write_on_its_own_block() {
    let engine = block_engine().await;
    seed(&engine).await;
    let held = hold_a_write(&engine).await;

    let keystroke = tokio::spawn(engine.commit_keystroke(SourceKeystroke {
        id: HELD.into(),
        source: "typed after the held write".into(),
        write_seq: None,
    }));
    assert_eq!(
        engine.admission().census(),
        AdmissionCensus {
            released: 1,
            waiting: 1
        }
    );
    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the held write");
    held.await.expect("the held write completes");
    keystroke
        .await
        .expect("the keystroke's task")
        .expect("the keystroke lands after the held write");
    assert_eq!(content(&engine, HELD).await, "typed after the held write");
}

fn operation(op: &str, pairs: &[(&str, &str)]) -> Operation {
    Operation::from_params(
        "block",
        op,
        op,
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string()))),
    )
}

/// A 40-op patch, three creates then 37 retitles, held before its fourth op.
/// It claims only the rows it names, so a keystroke on another block neither
/// waits for it nor is held back by it.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_keystroke_during_a_held_40_row_patch_finishes() {
    let engine = block_engine().await;
    seed(&engine).await;
    let rows: Vec<String> = (0..37).map(|j| format!("block:row-{j}")).collect();
    for row in &rows {
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                params(&[("id", row), ("parent_id", PAGE), ("content", row)]),
                OpOrigin::Sync,
            )
            .await
            .unwrap_or_else(|e| panic!("seed {row}: {e:#}"));
    }
    let ops: Vec<Operation> = (0..3)
        .map(|j| {
            operation(
                "create",
                &[
                    ("id", &format!("block:new-{j}")),
                    ("parent_id", PAGE),
                    ("content", "new"),
                ],
            )
        })
        .chain(rows.iter().map(|row| {
            operation(
                "set_field",
                &[("id", row), ("field", "content"), ("value", "patched")],
            )
        }))
        .collect();
    assert_eq!(ops.len(), 40);

    engine.dispatch_hold().hold_next("block", "set_field");
    let patch = {
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
    let parked = tokio::time::timeout(Duration::from_secs(10), async {
        while engine.dispatch_hold().parked_runs("block", "set_field") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(parked.is_ok(), "the patch never reached its fourth op");

    let keystroke = engine.commit_keystroke(SourceKeystroke {
        id: TYPED.into(),
        source: "typed during the patch".into(),
        write_seq: None,
    });
    assert_eq!(
        engine.admission().census().waiting,
        0,
        "the keystroke on {TYPED} waits behind the held patch, which never names it"
    );
    tokio::time::timeout(Duration::from_secs(10), keystroke)
        .await
        .expect("the keystroke finishes while the patch is held")
        .expect("the keystroke lands");
    assert_eq!(content(&engine, TYPED).await, "typed during the patch");

    engine
        .dispatch_hold()
        .release(1, Duration::from_secs(5))
        .await
        .expect("release the patch");
    patch
        .await
        .expect("the patch's task")
        .expect("the patch lands");
    assert_eq!(content(&engine, "block:row-36").await, "patched");
}
