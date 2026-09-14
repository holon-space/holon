//! The operation boundary parses entity references once (ruling D125.a).
//!
//! An operation parameter that addresses an entity is declared
//! `TypeHint::EntityId`, and the dispatcher parses every one of them into an
//! `EntityUri` before any provider sees the request. A value that carries no
//! scheme names no entity, so it is refused there — org files on disk store
//! bare ids and the org parser adds the scheme, which means a caller reaching
//! the dispatcher with a bare id never crossed that parse.
//!
//! These rungs drive the seam through production wiring: a real backend engine,
//! a real `SqlOperationProvider`, the real dispatcher.

use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const TABLE: &str = "test_item";
/// Entity names normalize `_` to `-` (URI-scheme safety), so this is also the
/// scheme every reference to the entity must carry.
const ENTITY: &str = "test-item";

/// A backend whose only write authority for `test-item` is the SQL provider,
/// with one row already in the table. `extra_first` is registered BEFORE that
/// authority, so a boundary that read the first matching provider rather than
/// the union would answer differently across the two calls.
async fn engine_with_one_row() -> Arc<BackendEngine> {
    let engine = holon::di::test_helpers::create_test_engine_with_providers(
        ":memory:".into(),
        move |module| {
            module.with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    TABLE.to_string(),
                    ENTITY.to_string(),
                    ENTITY.to_string(),
                )) as Arc<dyn holon_core::OperationProvider>
            })
        },
    )
    .await
    .expect("test engine boots");

    engine
        .db_handle()
        .execute_ddl(&format!(
            "CREATE TABLE {TABLE} (id TEXT PRIMARY KEY, content TEXT, completed BOOLEAN)"
        ))
        .await
        .expect("table created");
    engine
        .db_handle()
        .execute(
            &format!(
                "INSERT INTO {TABLE} (id, content, completed) VALUES ('{ENTITY}:item-1', 'Test \
                 task', 0)"
            ),
            vec![],
        )
        .await
        .expect("row inserted");
    engine
}

/// The production SqlOnly block wiring — a CRUD authority plus the structural
/// provider that owns `indent` — with one MORE authority for `indent` on top.
/// That is the registry shape production already has (`SqlBlockOperations` and
/// `LoroBlockOperations` both advertise the structural ops), which is why
/// `OperationDispatcher::operations` allowlists those duplicates rather than
/// refusing them.
async fn block_engine_with_a_second_indent_authority() -> Arc<BackendEngine> {
    holon::di::test_helpers::create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider(Arc::new(SecondIndentAuthority))
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    holon::storage::BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn holon_core::OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    holon::storage::BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = holon_api::block::Block::type_definition();
                block_raw_type_def.name = holon::storage::BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
                    handle.block_on(holon::core::queryable_cache::QueryableCache::<
                        holon_api::block::Block,
                    >::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(holon::core::sql_block_operations::SqlBlockOperations::new(
                    sql_ops,
                    Arc::new(cache),
                )) as Arc<dyn holon_core::OperationProvider>
            })
    })
    .await
    .expect("block engine boots")
}

/// A SECOND authority for `block/indent` — the shape production already has:
/// `SqlBlockOperations` and `LoroBlockOperations` both advertise the structural
/// block ops, which is why `OperationDispatcher::operations` carries a named
/// allowlist for exactly those duplicates instead of refusing them.
///
/// It declares one entity reference the other authority does NOT (`after_id`),
/// which is what makes the rung below a test of the UNION rather than of
/// routing: a boundary reading the first matching descriptor would see one
/// authority's parameters and be blind to the other's, whichever order the two
/// were registered in.
struct SecondIndentAuthority;

#[async_trait::async_trait]
impl holon_core::OperationProvider for SecondIndentAuthority {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        let block_ref = |name: &str, description: &str| holon_api::OperationParam {
            name: name.to_string(),
            type_hint: holon_api::TypeHint::EntityId {
                entity_name: EntityName::new("block"),
            },
            description: description.to_string(),
        };
        vec![holon_api::OperationDescriptor {
            entity_name: "block".into(),
            entity_short_name: "block".to_string(),
            id_column: "id".to_string(),
            name: "indent".to_string(),
            display_name: "Indent".to_string(),
            description: "A second authority for one structural block op".to_string(),
            // `after_id` ONLY. The other authority declares `id` and not this,
            // so neither descriptor is a superset of the other: whichever one
            // a first-match scan reached, it would be blind to the other's
            // parameter, and one of the two directions asserted below would
            // pass a bare reference through.
            required_params: vec![block_ref("after_id", "The sibling it lands under")],
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }]
    }

    async fn execute_operation(
        &self,
        _entity_name: &EntityName,
        _op_name: &str,
        _params: StorageEntity,
    ) -> holon_core::Result<holon_core::OperationResult> {
        panic!("the boundary must refuse before any authority runs")
    }
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    let mut p = StorageEntity::new();
    for (key, value) in pairs {
        p.insert((*key).into(), value.clone());
    }
    p
}

async fn dispatch(
    engine: &Arc<BackendEngine>,
    op: &str,
    pairs: &[(&str, Value)],
) -> Result<(), String> {
    engine
        .execute_operation(&EntityName::new(ENTITY), op, params(pairs), OpOrigin::User)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn assert_refusal_names_the_scheme(what: &str, outcome: Result<(), String>, bare: &str) {
    let message = outcome.expect_err(&format!(
        "{what}: the operation boundary accepted the unschemed entity reference {bare:?}"
    ));
    assert!(
        message.contains("unschemed entity reference") || message.contains("carries no scheme"),
        "{what}: refusal does not name the defect: {message}"
    );
    assert!(
        message.contains(&format!("{ENTITY}:{bare}")),
        "{what}: refusal must suggest the REFERENCED entity's scheme: {message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_refuses_a_bare_entity_reference() {
    let engine = engine_with_one_row().await;
    let outcome = dispatch(
        &engine,
        "set_field",
        &[
            ("id", Value::String("item-1".to_string())),
            ("field", Value::String("completed".to_string())),
            ("value", Value::Boolean(true)),
        ],
    )
    .await;
    assert_refusal_names_the_scheme("test-item/set_field", outcome, "item-1");
}

/// `block/indent` is advertised by TWO authorities, and each names a reference
/// the other does not. Both must be parsed, so neither authority's declaration
/// can be the one the boundary happens to reach first.
///
/// The two directions together pin the union without depending on registration
/// order: one bare value is caught only if the SECOND authority's declaration
/// was read, the other only if the FIRST one was.
#[tokio::test(flavor = "multi_thread")]
async fn both_authorities_declarations_are_parsed_for_one_operation() {
    let engine = block_engine_with_a_second_indent_authority().await;

    let bare_extra = engine
        .execute_operation(
            &EntityName::new("block"),
            "indent",
            params(&[
                ("id", Value::String("block:b-1".to_string())),
                ("after_id", Value::String("b-2".to_string())),
            ]),
            OpOrigin::User,
        )
        .await
        .expect_err("`after_id` is declared by the second authority and must be parsed");
    assert!(
        bare_extra.to_string().contains("after_id"),
        "the refusal must name the parameter only the second authority declares: {bare_extra}"
    );

    let bare_subject = engine
        .execute_operation(
            &EntityName::new("block"),
            "indent",
            params(&[
                ("id", Value::String("b-1".to_string())),
                ("after_id", Value::String("block:b-2".to_string())),
            ]),
            OpOrigin::User,
        )
        .await
        .expect_err("`id` is declared by both and must be parsed");
    assert!(
        bare_subject.to_string().contains("'id'"),
        "the refusal must name the subject: {bare_subject}"
    );
}

/// A scheme that merely EXISTS is not a reference to the right thing: both of
/// these parse as URIs, then match no row, and the write reports success having
/// changed nothing.
#[tokio::test(flavor = "multi_thread")]
async fn set_field_refuses_a_reference_to_another_entity() {
    let engine = engine_with_one_row().await;

    for foreign in ["https://example.com/x", "block:item-1"] {
        let outcome = dispatch(
            &engine,
            "set_field",
            &[
                ("id", Value::String(foreign.to_string())),
                ("field", Value::String("completed".to_string())),
                ("value", Value::Boolean(true)),
            ],
        )
        .await;
        let message = outcome.expect_err(&format!(
            "test-item/set_field accepted {foreign:?}, which names another entity"
        ));
        assert!(
            message.contains(ENTITY) && message.contains("names the entity"),
            "the refusal must name both the entity found and the one expected: {message}"
        );
    }

    let rows = engine
        .execute_query(
            format!("SELECT completed FROM {TABLE} WHERE id = '{ENTITY}:item-1'"),
            std::collections::HashMap::new(),
            None,
        )
        .await
        .expect("read back");
    assert_eq!(
        rows[0].get("completed").expect("column present"),
        &Value::Integer(0),
        "a refused write must not have changed the row"
    );
}

/// A `create` that SUPPLIES its id is refused on the same terms: the id would
/// otherwise be scheme-stamped here and stored under a spelling the caller
/// never wrote.
#[tokio::test(flavor = "multi_thread")]
async fn create_refuses_a_bare_supplied_id() {
    let engine = engine_with_one_row().await;
    let outcome = dispatch(
        &engine,
        "create",
        &[
            ("id", Value::String("item-2".to_string())),
            ("content", Value::String("Fresh".to_string())),
        ],
    )
    .await;
    assert_refusal_names_the_scheme("test-item/create", outcome, "item-2");
}

/// The ROOT is a value, not a wildcard. `move_block` declares its `parent_id`
/// as a position the root may occupy; a write's SUBJECT never is one, so the
/// sentinel there names no row and is refused like any other foreign reference.
#[tokio::test(flavor = "multi_thread")]
async fn set_field_refuses_the_root_sentinel_as_its_subject() {
    let engine = engine_with_one_row().await;
    let outcome = dispatch(
        &engine,
        "set_field",
        &[
            (
                "id",
                Value::String(holon_api::EntityUri::no_parent().to_string()),
            ),
            ("field", Value::String("completed".to_string())),
            ("value", Value::Boolean(true)),
        ],
    )
    .await;
    let message = outcome
        .expect_err("the root sentinel names no row, so it cannot be the subject of a write");
    assert!(
        message.contains("sentinel") && message.contains(ENTITY),
        "the refusal must name both the sentinel and the entity expected: {message}"
    );
}

/// The seam refuses unschemed references; it does not stand between a
/// scheme-qualified one and its provider.
#[tokio::test(flavor = "multi_thread")]
async fn set_field_accepts_a_scheme_qualified_reference() {
    let engine = engine_with_one_row().await;
    dispatch(
        &engine,
        "set_field",
        &[
            ("id", Value::String(format!("{ENTITY}:item-1"))),
            ("field", Value::String("completed".to_string())),
            ("value", Value::Boolean(true)),
        ],
    )
    .await
    .expect("a scheme-qualified reference reaches the provider");

    let rows = engine
        .execute_query(
            format!("SELECT completed FROM {TABLE} WHERE id = '{ENTITY}:item-1'"),
            std::collections::HashMap::new(),
            None,
        )
        .await
        .expect("read back");
    assert_eq!(rows.len(), 1, "the write landed on exactly one row");
    match rows[0].get("completed").expect("column present") {
        Value::Integer(i) => assert_eq!(*i, 1),
        Value::Boolean(b) => assert!(b),
        other => panic!("unexpected value type for completed: {other:?}"),
    }
}
