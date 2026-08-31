//! A `computed_persisted` field declared on a type is a real matview column
//! after a production boot.
//!
//! `person.display_name` is the acceptance case: the registry compiles it into
//! a `Computation`, `TypeDefinition::persisted_derived_plan` lowers it to a
//! `PlantedColumn`, and `TursoAdapter`'s matview module must plant that column
//! so a reader of the `person` matview sees the derived value. Nothing else in
//! the tree observes this end to end — the plan is otherwise only exercised by
//! hand-built views in `holon/tests/computed_tier_dual_path.rs`, which plant
//! the column themselves and so cannot tell whether registration does.
//!
//! @pbt kind harness
//! @pbt covers computed-persisted-boot-column — a declared computed_persisted
//! field materializes as a matview column with correct values on a real boot
//! @pbt overlaps general_e2e_composed_pbt — kept: keystone declares no
//! computed_persisted field and never reads a planted column

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::Value;
use holon_loro_wiring::EventInfraModule;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

/// Boot a fresh file-backed DB through the lazy-DI entry the frontends use.
/// `FreeStandingTypeViews` is an eager schema root, so returning from here
/// means every free-standing type — `person` among them — has been registered
/// through `TursoAdapter::register`.
async fn boot_fresh_db(
    db_path: std::path::PathBuf,
) -> Arc<holon::api::backend_engine::BackendEngine> {
    assert!(!db_path.exists(), "db file must not pre-exist");
    holon::di::create_backend_engine(db_path, |injector| {
        EventInfraModule
            .configure(injector)
            .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
        injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
            |resolver| {
                let db = resolver
                    .resolve::<dyn holon::di::DbHandleProvider>()
                    .handle();
                Arc::new(holon::core::SqlOperationProvider::new(
                    db,
                    holon::storage::BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                )) as Arc<dyn holon_core::OperationProvider>
            },
        ));
        Ok(())
    })
    .await
    .expect("fresh-db lazy DI graph must build")
}

#[test]
fn declared_computed_persisted_field_is_a_matview_column_after_boot() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let db = engine.db_handle();

        db.execute(
            "INSERT INTO person_raw (id, email, role) VALUES \
             ('person:chef', 'a@b.c', 'Chef'), ('person:plain', 'd@e.f', NULL)",
            vec![],
        )
        .await
        .expect("insert into person_raw");

        let rows = db
            .query(
                "SELECT id, display_name FROM person ORDER BY id",
                HashMap::new(),
            )
            .await
            .expect(
                "the `person` matview must expose the declared computed_persisted column \
                 `display_name` — a 'no such column' error here means registration did not \
                 consume TypeDefinition::persisted_derived_plan",
            );

        let names: Vec<Option<&Value>> = rows.iter().map(|r| r.get("display_name")).collect();
        assert_eq!(
            names,
            vec![
                Some(&Value::String("Chef — a@b.c".into())),
                Some(&Value::String("d@e.f".into())),
            ],
            "planted display_name must equal the declaration's value for both the \
             role-present and role-absent branches"
        );
    });
}
