//! Every block operation, run, undone and redone on blocks whose parent is the
//! ROOT. Their inverses name that parent, and the root is the stored sentinel,
//! so each position an inverse writes it into must declare that it admits the
//! root — the operation boundary refuses it everywhere else.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const ROOT: &str = "sentinel:no_parent";

/// The SqlOnly block wiring: the CRUD authority plus the structural provider.
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

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}

fn id(value: &str) -> Value {
    Value::String(value.to_string())
}

/// Three top-level blocks `a`, `b`, `c` in that order, and `d` under `c`.
async fn seed(engine: &BackendEngine) {
    let mut after: Option<&str> = None;
    for (block, parent) in [
        ("block:a", ROOT),
        ("block:b", ROOT),
        ("block:c", ROOT),
        ("block:d", "block:c"),
    ] {
        let mut fields = params(&[
            ("id", id(block)),
            ("parent_id", id(parent)),
            ("content", id(&block[6..].repeat(2))),
        ]);
        if parent == ROOT
            && let Some(prev) = after
        {
            fields.insert("after_block_id".into(), id(prev));
        }
        engine
            .execute_operation(&EntityName::new("block"), "create", fields, OpOrigin::Sync)
            .await
            .unwrap_or_else(|e| panic!("seed {block}: {e:#}"));
        if parent == ROOT {
            after = Some(block);
        }
    }
}

/// What an operation on a top-level block does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// Runs, undoes and redoes.
    RoundTrip,
    /// Runs, and declares that nothing undoes it.
    Irreversible,
    /// Refuses the top-level subject in its own words.
    Refused(&'static str),
}

/// What `op` did on a fresh seeded engine.
async fn observe(op: &str, pairs: &[(&str, Value)]) -> Result<Expect, String> {
    let engine = block_engine().await;
    seed(&engine).await;
    if let Err(e) = engine
        .execute_operation(&EntityName::new("block"), op, params(pairs), OpOrigin::User)
        .await
    {
        return Err(format!("{e:#}"));
    }
    match engine.undo().await {
        Ok(UndoOutcome::Applied) => {}
        Ok(UndoOutcome::Empty) => return Ok(Expect::Irreversible),
        other => return Err(format!("undo: {other:?}")),
    }
    match engine.redo().await {
        Ok(UndoOutcome::Applied) => Ok(Expect::RoundTrip),
        other => Err(format!("redo: {other:?}")),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_block_operation_undoes_and_redoes_at_the_top_level() {
    let cases: Vec<(&str, Vec<(&str, Value)>, Expect)> = vec![
        (
            "indent",
            vec![("id", id("block:b"))],
            Expect::Refused("Cannot indent root block"),
        ),
        (
            "outdent",
            vec![("id", id("block:d"))],
            Expect::Refused("parent is already at root level"),
        ),
        (
            "move_up",
            vec![("id", id("block:b"))],
            Expect::Refused("Cannot move root block"),
        ),
        (
            "move_down",
            vec![("id", id("block:b"))],
            Expect::Refused("Cannot move root block"),
        ),
        (
            "move_block",
            vec![("id", id("block:b")), ("parent_id", id("block:c"))],
            Expect::RoundTrip,
        ),
        (
            "move_to_position",
            vec![("id", id("block:b")), ("parent_id", id(ROOT))],
            Expect::Irreversible,
        ),
        (
            "split_block",
            vec![("id", id("block:b")), ("position", Value::Integer(1))],
            Expect::RoundTrip,
        ),
        (
            "join_block",
            vec![("id", id("block:b")), ("position", Value::Integer(0))],
            Expect::Refused("no previous sibling and no parent"),
        ),
        // What the undo of a top-level join dispatches.
        (
            "restore_split",
            vec![
                ("target_id", id("block:a")),
                ("target_content", id("aa")),
                ("block_id", id("block:x")),
                ("block_content", id("xx")),
                ("block_parent", id(ROOT)),
                ("after_id", id("block:a")),
            ],
            Expect::RoundTrip,
        ),
        (
            "restore_join",
            vec![
                ("target_id", id("block:a")),
                ("target_content", id("aabb")),
                ("deleted_id", id("block:b")),
            ],
            Expect::RoundTrip,
        ),
        (
            "delete_subtree",
            vec![("id", id("block:c"))],
            Expect::Irreversible,
        ),
        (
            "delete_keep_children",
            vec![("id", id("block:c"))],
            Expect::Refused("Cannot delete_keep_children on a root block"),
        ),
        (
            "embed_entity",
            vec![("id", id("block:b")), ("target_uri", id("block:a"))],
            Expect::RoundTrip,
        ),
    ];

    let covered: BTreeSet<&str> = cases.iter().map(|(op, _, _)| *op).collect();
    let declared: Vec<String> = holon_core::__operations_block_operations::block_operations(
        "block", "block", "block", "id",
    )
    .into_iter()
    .map(|descriptor| descriptor.name)
    .collect();
    let declared: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    assert_eq!(
        covered, declared,
        "every BlockOperations op needs a top-level case here"
    );

    let mut wrong = Vec::new();
    for (op, pairs, expected) in &cases {
        let observed = observe(op, pairs).await;
        let as_expected = match (&observed, expected) {
            (Ok(got), want) => got == want,
            (Err(e), Expect::Refused(said)) => {
                e.contains(said) && !e.contains("operation boundary")
            }
            (Err(_), _) => false,
        };
        if !as_expected {
            wrong.push(format!("{op}: expected {expected:?}, got {observed:?}"));
        }
    }
    assert!(wrong.is_empty(), "at the top level:\n{}", wrong.join("\n"));
}
