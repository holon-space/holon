//! In a Loro mode the Loro store is the write authority of every free-standing
//! typed entity, and the projector sink is the ONLY writer of their Turso
//! tables. An operation that writes a type table directly bypasses Loro: the
//! table then holds a value Loro does not, and the next projection of that
//! entity silently writes Loro's value back over it.
//!
//! The census drives every operation the catalog offers on a Loro-held type
//! beside the type's own write authority (a "companion" operation such as the
//! pantry's `consume`), and requires the changed entity to read the same in
//! Loro and in its table once the projection settles. A companion operation the
//! census has no driver for fails it, so a new one cannot escape the check.
//!
//! @pbt kind harness
//! @pbt covers loro-typed-writer-census — in a Loro mode no SQL typed writer
//! exists besides the projector sink
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone drives only the
//! write authority's own create/set_field/delete, no companion operation

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::FieldSchema;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;

const SETTLE: Duration = Duration::from_secs(20);

/// One companion operation the census can run: the entity it seeds first and
/// the params of the operation itself.
struct Driver {
    entity: &'static str,
    op: &'static str,
    seed: fn(&str) -> Vec<(&'static str, Value)>,
    params: fn(&str) -> Vec<(&'static str, Value)>,
}

const DRIVERS: &[Driver] = &[Driver {
    entity: "pantry_item",
    op: "consume",
    seed: |id| {
        vec![
            ("id", Value::String(id.to_string())),
            ("name", Value::String("flour".into())),
            ("quantity", Value::Float(500.0)),
            ("unit", Value::String("g".into())),
        ]
    },
    params: |id| {
        vec![
            ("id", Value::String(id.to_string())),
            ("quantity", Value::Float(200.0)),
            ("unit", Value::String("g".into())),
        ]
    },
}];

fn params(pairs: Vec<(&'static str, Value)>) -> HashMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// The columns both stores hold the same way: every persisted field but the
/// overflow pair, whose shape is each store's own.
fn compared_columns(type_def: &TypeDefinition) -> Vec<String> {
    type_def
        .persistent_fields()
        .into_iter()
        .map(|f| f.name.clone())
        .filter(|name| {
            name != FieldSchema::OVERFLOW_PROPERTIES && name != FieldSchema::OVERFLOW_PROPERTY_KINDS
        })
        .collect()
}

async fn loro_row(env: &TestEnvironment, type_def: &TypeDefinition, id: &str) -> Vec<Value> {
    let entity = EntityName::new(type_def.name.as_str());
    let doc = env
        .injector()
        .expect("a booted environment has an injector")
        .resolve::<LoroDocumentStore>()
        .get_doc(DocScope::Global)
        .await
        .expect("the Global Loro doc");
    let key = id
        .strip_prefix(&format!("{}:", entity.as_str()))
        .unwrap_or_else(|| panic!("{id} is not an id of {entity}"));
    let stored = doc
        .with_read(|d| {
            Ok(holon_loro::loro_entity_operations::read_entity(
                d,
                &holon_loro::loro_entity_operations::entity_root_name(&entity),
                &entity,
                key,
            ))
        })
        .expect("reading the Loro doc")
        .unwrap_or_else(|| panic!("Loro holds no {entity} '{id}'"));
    assert!(stored.live, "Loro holds {id} as deleted");
    compared_columns(type_def)
        .iter()
        .map(|c| {
            stored
                .fields
                .get(c.as_str())
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect()
}

async fn sql_row(env: &TestEnvironment, type_def: &TypeDefinition, id: &str) -> Vec<Value> {
    let table = format!(
        "{}_raw",
        EntityName::new(type_def.name.as_str()).table_name()
    );
    let rows = env
        .query_sql(&format!(
            "SELECT * FROM {table} WHERE id = '{}'",
            id.replace('\'', "''")
        ))
        .await
        .unwrap_or_else(|e| panic!("reading {table}: {e}"));
    assert_eq!(
        rows.len(),
        1,
        "{table} must hold exactly one row {id}: {rows:?}"
    );
    compared_columns(type_def)
        .iter()
        .map(|c| rows[0].get(c).cloned().unwrap_or(Value::Null))
        .collect()
}

/// Every Loro-held type: free-standing (`is_free_standing`), not an integration
/// mirror, served.
fn loro_held_types(env: &TestEnvironment) -> Vec<TypeDefinition> {
    let injector = env
        .injector()
        .expect("a booted environment has an injector");
    let dispatcher = env.engine().get_dispatcher();
    injector
        .resolve::<holon_profiles::TypeRegistry>()
        .all()
        .into_iter()
        .filter(|t| {
            t.name != "block"
                && t.id_references.is_none()
                && !t.persistent_fields().is_empty()
                && t.owning_integration().is_none()
                && dispatcher.has_provider(&t.name)
        })
        .collect()
}

/// The `(entity, op)` pairs the catalog offers on `types` beside each type's
/// own write authority.
fn companion_operations(
    env: &TestEnvironment,
    types: &[TypeDefinition],
) -> BTreeSet<(String, String)> {
    let injector = env
        .injector()
        .expect("a booted environment has an injector");
    let authority = injector.resolve::<dyn holon_core::TypeWriteAuthority>();
    let catalog = env.engine().get_dispatcher().catalog();
    let mut out = BTreeSet::new();
    for type_def in types {
        let entity = EntityName::new(type_def.name.as_str());
        let own: BTreeSet<String> = authority
            .for_type(type_def)
            .unwrap_or_else(|e| panic!("the write authority of {}: {e}", type_def.name))
            .operations()
            .into_iter()
            .map(|d| d.name)
            .collect();
        for descriptor in catalog.iter().filter(|d| d.entity_name == entity) {
            if !own.contains(&descriptor.name) {
                out.insert((type_def.name.clone(), descriptor.name.clone()));
            }
        }
    }
    out
}

#[test]
fn in_a_loro_mode_no_sql_typed_writer_exists_besides_the_projector_sink() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        assert!(env.loro_enabled(), "the census needs a Loro mode");
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(SETTLE).await;

        let types = loro_held_types(&env);
        let companions = companion_operations(&env, &types);
        assert!(
            !companions.is_empty(),
            "no companion operation on a Loro-held type: the census would be vacuous"
        );

        for (n, (entity, op)) in companions.iter().enumerate() {
            let driver = DRIVERS
                .iter()
                .find(|d| d.entity == entity && d.op == op)
                .unwrap_or_else(|| {
                    panic!(
                        "{entity}.{op} is offered on a Loro-held type, but the census has no \
                         driver for it. Add one to DRIVERS so the census checks it writes \
                         through the Loro authority"
                    )
                });
            let type_def = types.iter().find(|t| &t.name == entity).unwrap();
            let id = format!("{}:census-{n}", EntityName::new(entity.as_str()).as_str());
            env.execute_operation(entity, "create", params((driver.seed)(&id)))
                .await
                .unwrap_or_else(|e| panic!("seeding {id} for {entity}.{op}: {e:#}"));
            env.wait_for_loro_quiescence(SETTLE).await;
            let before = loro_row(&env, type_def, &id).await;

            env.execute_operation(entity, op, params((driver.params)(&id)))
                .await
                .unwrap_or_else(|e| panic!("{entity}.{op} on {id}: {e:#}"));
            env.wait_for_loro_quiescence(SETTLE).await;

            let loro = loro_row(&env, type_def, &id).await;
            let sql = sql_row(&env, type_def, &id).await;
            assert_eq!(
                sql,
                loro,
                "{entity}.{op} left the {entity} table holding a row Loro does not hold \
                 (columns {:?}): a writer other than the projector sink wrote it",
                compared_columns(type_def)
            );
            assert_ne!(
                loro, before,
                "{entity}.{op} did not change {id} in Loro, the write authority"
            );
        }
    });
}

#[test]
fn consume_in_a_loro_mode_changes_quantity_in_loro_and_the_projection_and_keeps_its_refusals() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(SETTLE).await;

        let type_def = loro_held_types(&env)
            .into_iter()
            .find(|t| t.name == "pantry_item")
            .expect("pantry_item is a Loro-held type");
        let quantity = compared_columns(&type_def)
            .iter()
            .position(|c| c == "quantity")
            .unwrap();
        let id = "pantry-item:rye";
        env.execute_operation(
            "pantry_item",
            "create",
            params(vec![
                ("id", Value::String(id.into())),
                ("name", Value::String("rye".into())),
                ("quantity", Value::Float(500.0)),
                ("unit", Value::String("g".into())),
            ]),
        )
        .await
        .expect("stocking rye");
        env.wait_for_loro_quiescence(SETTLE).await;

        let consume = |amount: f64, unit: &str| {
            params(vec![
                ("id", Value::String(id.into())),
                ("quantity", Value::Float(amount)),
                ("unit", Value::String(unit.into())),
            ])
        };
        env.execute_operation("pantry_item", "consume", consume(200.0, "g"))
            .await
            .expect("consuming 200 g of 500 g");
        env.wait_for_loro_quiescence(SETTLE).await;
        assert_eq!(
            loro_row(&env, &type_def, id).await[quantity],
            Value::Float(300.0),
            "consume must change quantity in Loro, the write authority"
        );
        assert_eq!(
            sql_row(&env, &type_def, id).await[quantity],
            Value::Float(300.0),
            "the projection must carry the consumed quantity"
        );

        let past_empty = env
            .execute_operation("pantry_item", "consume", consume(400.0, "g"))
            .await
            .expect_err("consuming past empty must be refused");
        assert!(
            past_empty.to_string().contains("cannot go negative"),
            "{past_empty:#}"
        );
        let other_unit = env
            .execute_operation("pantry_item", "consume", consume(1.0, "kg"))
            .await
            .expect_err("consuming in an unconvertible unit must be refused");
        assert!(
            other_unit.to_string().contains("no conversion factor"),
            "{other_unit:#}"
        );
        let missing = env
            .execute_operation(
                "pantry_item",
                "consume",
                params(vec![
                    ("id", Value::String("pantry-item:nothing".into())),
                    ("quantity", Value::Float(1.0)),
                    ("unit", Value::String("g".into())),
                ]),
            )
            .await
            .expect_err("consuming an item nobody stocked must be refused");
        assert!(
            missing.to_string().contains("no pantry item"),
            "{missing:#}"
        );

        env.wait_for_loro_quiescence(SETTLE).await;
        assert_eq!(
            loro_row(&env, &type_def, id).await[quantity],
            Value::Float(300.0),
            "a refused consume must not change Loro"
        );
        assert_eq!(
            sql_row(&env, &type_def, id).await[quantity],
            Value::Float(300.0),
            "a refused consume must not change the projection"
        );
    });
}
