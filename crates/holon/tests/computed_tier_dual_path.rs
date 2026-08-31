//! The acceptance case for the computed tier: `person_profile.display_name`,
//! as the production registry compiles it, produces the same value in memory
//! (`Computation::eval`) and as a planted matview column evaluated by SQLite.
//!
//! The registry supplies the column plan; this file plants it. Schema
//! registration does not yet consume `persisted_derived_plan`, so a booted
//! vault has no `display_name` matview column.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::computation::Computation;
use holon_api::computation::Context;
use holon_api::computation::DerivedFieldPlan;
use holon_profiles::create_default_registry;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;

/// `display_name` exactly as the boot registry compiled it.
fn display_name_computation() -> Computation {
    let registry = create_default_registry().expect("default registry boots");
    let td = registry.get("person").expect("person registered");
    td.computed_spec("display_name")
        .expect("display_name declared on person")
        .computation()
        .clone()
}

fn person_plan() -> DerivedFieldPlan {
    let registry = create_default_registry().expect("default registry boots");
    let td = registry.get("person").expect("person registered");
    td.persisted_derived_plan()
}

/// How `role` is supplied — the three states the guard must collapse.
#[derive(Debug, Clone)]
enum Role {
    Absent,
    Unit,
    Present(String),
}

fn role_strategy() -> impl Strategy<Value = Role> {
    prop_oneof![
        1 => Just(Role::Absent),
        1 => Just(Role::Unit),
        3 => "[a-zA-Z ]{0,12}".prop_map(Role::Present),
    ]
}

fn eval_ctx(role: &Role, email: &str) -> Context {
    let mut ctx = Context::new();
    match role {
        Role::Absent => {}
        Role::Unit => {
            ctx.insert("role".into(), Value::Null);
        }
        Role::Present(s) => {
            ctx.insert("role".into(), Value::String(s.clone()));
        }
    }
    ctx.insert("email".into(), Value::String(email.to_string()));
    ctx
}

fn role_sql_value(role: &Role) -> turso::Value {
    match role {
        Role::Absent | Role::Unit => turso::Value::Null,
        Role::Present(s) => turso::Value::Text(s.clone()),
    }
}

async fn setup_person_view(plan: &DerivedFieldPlan) -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE person (id TEXT PRIMARY KEY, role TEXT, email TEXT)")
        .await
        .expect("create person");

    let cols: String = plan
        .sql_planted
        .iter()
        .map(|c| format!(", {}", c.select_expr()))
        .collect();
    let select = format!("SELECT id{cols} FROM person");
    reconcile_named_view(&handle, "person_derived", &select)
        .await
        .unwrap_or_else(|e| panic!("planted DDL `{select}` must succeed: {e}"));
    handle
}

async fn insert_person(handle: &DbHandle, i: usize, role: &Role, email: &str) {
    handle
        .execute(
            "INSERT INTO person (id, role, email) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text(format!("p{i}")),
                role_sql_value(role),
                turso::Value::Text(email.to_string()),
            ],
        )
        .await
        .expect("insert person");
}

async fn read_display_name(handle: &DbHandle, i: usize) -> Value {
    let rows = handle
        .query(
            &format!("SELECT display_name FROM person_derived WHERE id = 'p{i}'"),
            HashMap::new(),
        )
        .await
        .expect("query derived view");
    rows.first()
        .and_then(|r| r.get("display_name").cloned())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn declared_persisted_field_is_planted_and_read_back_from_the_matview() {
    let plan = person_plan();
    assert_eq!(
        plan.sql_planted.len(),
        1,
        "display_name must be SQL-planted; stage: {:?}",
        plan.stage_evaluated
    );
    assert_eq!(plan.sql_planted[0].name.as_str(), "display_name");

    let handle = setup_person_view(&plan).await;
    insert_person(&handle, 0, &Role::Present("Chef".into()), "a@b.c").await;
    insert_person(&handle, 1, &Role::Absent, "d@e.f").await;

    assert_eq!(
        read_display_name(&handle, 0).await,
        Value::String("Chef — a@b.c".into())
    );
    assert_eq!(
        read_display_name(&handle, 1).await,
        Value::String("d@e.f".into())
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn eval_and_planted_sql_agree_on_display_name(
        rows in prop::collection::vec(
            (role_strategy(), "[a-z]{1,6}@[a-z]{1,6}\\.[a-z]{2,3}"),
            1..6,
        )
    ) {
        let comp = display_name_computation();
        let expected: Vec<Value> = rows
            .iter()
            .map(|(role, email)| {
                comp.eval(&eval_ctx(role, email))
                    .unwrap_or_else(|e| panic!("eval failed for {role:?}/{email}: {e}"))
            })
            .collect();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let actual: Vec<Value> = rt.block_on(async {
            let handle = setup_person_view(&person_plan()).await;
            for (i, (role, email)) in rows.iter().enumerate() {
                insert_person(&handle, i, role, email).await;
            }
            let mut out = Vec::with_capacity(rows.len());
            for i in 0..rows.len() {
                out.push(read_display_name(&handle, i).await);
            }
            out
        });

        prop_assert_eq!(&actual, &expected, "eval vs planted SQL for rows {:?}", rows);
    }
}
