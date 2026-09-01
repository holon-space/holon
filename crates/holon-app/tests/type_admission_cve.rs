//! CV-E at declaration time: a type may not declare a `computed_persisted`
//! field against a home that cannot persist the kind it produces (ruling
//! D54.a).
//!
//! Driven through the production admission seat
//! (`holon_app::type_admission::declare_type_admitted`), not through the pure
//! check, so what is pinned is that the refusal is REACHABLE from where
//! production declares a type — the same reason `move_guard_policy.rs` drives
//! `move_block` rather than the guard's predicate.
//!
//! @pbt kind harness
//! @pbt covers cve-declaration-refusal — a computed_persisted field whose kind
//! a home cannot persist is refused when the type is declared, and the refusal
//! names the field, the home, and the reason
//! @pbt covers cve-unknown-home — a `home:` no profile answers for is refused
//! whether or not the type has computed fields
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone's datatype axis
//! declares only Text-kinded computed fields against no home, so it cannot
//! draw a refusable declaration (see lane-report-i3-2-p3.md §Keystone-rung
//! feasibility)

use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::ComputedSpec;
use holon_api::ComputedTier;
use holon_api::FieldLifetime;
use holon_api::FieldSchema;
use holon_api::HomeProfileId;
use holon_api::TypeDefinition;
use holon_capability::ProfileRegistry;
use holon_loro_wiring::EventInfraModule;
use holon_profiles::TypeRegistry;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build a runtime"),
    )
}

async fn boot_fresh_db(
    db_path: std::path::PathBuf,
) -> Arc<holon::api::backend_engine::BackendEngine> {
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
        // Contributed here exactly as `turso_seams.rs` contributes it in
        // production — the same `provide_into_set` of the same provider. The
        // sibling `rehome_entity_op.rs` wires its provider the same way.
        injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root_async(
            |resolver| async move {
                let profiles = Arc::new(
                    holon_capability::registry::shipped_profiles().expect("profiles parse"),
                );
                Arc::new(holon_app::type_admission::TypeAdmissionProvider::new(
                    resolver, profiles,
                )) as Arc<dyn holon_core::OperationProvider>
            },
        ));
        Ok(())
    })
    .await
    .expect("fresh-db lazy DI graph must build")
}

/// A type with one BOOLEAN-producing `computed_persisted` field, homed where
/// the caller says.
///
/// `flag = a == b` is the discriminating shape: a comparison infers to
/// [`holon_api::computation::FieldKind::Boolean`], which `string_only` refuses.
/// A concatenation — the only shape the keystone's datatype axis draws — would
/// infer to Text and be offered by every home, so it could not tell an enforced
/// check from an absent one.
fn boolean_computed_type(name: &str, home: &str) -> TypeDefinition {
    let fields = vec![
        FieldSchema::new("id", "TEXT").primary_key(),
        FieldSchema::new("a", "TEXT").nullable(),
        FieldSchema::new("b", "TEXT").nullable(),
    ];
    let mut type_def = TypeDefinition::new(name, fields);
    type_def.home = Some(HomeProfileId::parse(home).expect("a well-formed profile id"));

    let declared = type_def.field_types();
    let spec = ComputedSpec::parse(
        "flag",
        "a == b",
        ComputedTier::ComputedPersisted,
        &declared,
        &holon_api::bounded_engine(),
    )
    .expect("`a == b` lowers to SQL, so the persisted tier accepts it");
    type_def.fields.push(FieldSchema {
        name: "flag".to_string(),
        sql_type: "BOOLEAN".to_string(),
        lifetime: FieldLifetime::Computed { spec },
        ..Default::default()
    });
    type_def
}

/// (a) The refusal. `org` declares `computed_persisted: string_only`, and a
/// Boolean column is not a string.
#[test]
fn a_boolean_computed_persisted_field_is_refused_against_a_string_only_home() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
        let types = TypeRegistry::new();

        let err = holon_app::type_admission::declare_type_admitted(
            &profiles,
            &boolean_computed_type("gen_refused", "org"),
            engine.db_handle(),
            &types,
            &engine.get_dispatcher(),
        )
        .await
        .expect_err("a Boolean computed_persisted field has no representation in `org`")
        .to_string();

        assert!(
            err.contains("flag") && err.contains("org"),
            "the refusal must name the field and the home: {err}"
        );
        assert!(
            err.contains("string"),
            "the refusal must carry the profile's own reason: {err}"
        );
        assert!(
            !types.contains("gen_refused"),
            "a refused declaration must leave the registry untouched — declaration is one-way, \
             so a half-declared name is unrecoverable"
        );
    });
}

/// (c) The anti-over-refusal arm. `holon-native` is `full_algebra`, so the very
/// same declaration is admitted — without this a check that refused everything
/// would pass (a).
#[test]
fn the_same_field_is_admitted_against_a_full_algebra_home() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
        let types = TypeRegistry::new();

        holon_app::type_admission::declare_type_admitted(
            &profiles,
            &boolean_computed_type("gen_admitted", "holon-native"),
            engine.db_handle(),
            &types,
            &engine.get_dispatcher(),
        )
        .await
        .expect("`holon-native` persists a computed field of any kind (full_algebra)");

        assert!(
            types.contains("gen_admitted"),
            "an admitted declaration must actually declare"
        );
    });
}

/// (c, second arm) The type the tree really ships, admitted under the home its
/// OWN yaml declares — nothing patched in by the test.
///
/// The previous version of this test set `person.home` itself before checking,
/// which injected the very field whose absence was the defect: `person.yaml`
/// carried no `home:`, so the real boot registry would have been refused while
/// this test passed.
#[test]
fn the_bundled_person_type_is_admitted_under_the_home_its_yaml_declares() {
    let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
    let registry = holon_profiles::create_default_registry().expect("bundled registry builds");
    let person = registry.get("person").expect("`person` is a bundled type");

    assert!(
        person.fields.iter().any(|f| matches!(
            &f.lifetime,
            FieldLifetime::Computed { spec } if spec.tier() == ComputedTier::ComputedPersisted
        )),
        "this test is only meaningful while `person` still carries a computed_persisted field"
    );
    assert!(
        person.home.is_some(),
        "`person` declares a computed_persisted field, so its yaml must name a home — \
         admission refuses one that does not"
    );

    holon_capability::check_computed_persisted(
        &profiles,
        &person,
        &holon_capability::HomeSeat::Declaration,
    )
    .expect("the bundled person type must stay declarable as authored");
}

/// The whole boot registry passes admission as authored.
///
/// This is the arm that would have caught the real defect: `admits()` was never
/// on the registry-seeding path, so a bundled type with a
/// `computed_persisted` field and no home shipped unchecked. Sweeping the
/// registry — rather than a list of known seeding call sites — is what makes a
/// future door (an MCP sidecar, `holon_kitchen::register_kitchen_types`)
/// covered by construction.
#[test]
fn the_whole_default_registry_passes_admission_as_authored() {
    let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
    let registry = holon_profiles::create_default_registry().expect("bundled registry builds");

    holon_app::type_admission::sweep_registry(&profiles, &registry)
        .expect("every bundled type must pass admission as authored");
}

/// A type registered by any OTHER door is swept too — the property that makes
/// the sweep door-agnostic. `register_kitchen_types` shares the registry, so
/// the sweep sees its types without naming it.
#[test]
fn a_type_seeded_after_the_bundled_loop_is_still_swept() {
    let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
    let registry = holon_profiles::create_default_registry().expect("bundled registry builds");

    registry
        .register(boolean_computed_type("gen_late_door", "org"))
        .expect("registering is not where admission happens");

    let err = holon_app::type_admission::sweep_registry(&profiles, &registry)
        .expect_err("a Boolean computed_persisted field cannot live in `org`");
    assert!(
        err.contains("gen_late_door") && err.contains("flag"),
        "the boot refusal must name the offending type and field: {err}"
    );
}

/// (d) A typo'd home fails on the day it is authored, not on the day someone
/// adds the first computed field to that type.
///
/// Driven THROUGH the seat, not through `check_declared_homes_exist` directly.
/// The library-level version of this test left a surviving mutant: deleting the
/// `check_declared_homes_exist` call from `admits()` kept the whole suite
/// green, because nothing asserted the SEAT consulted it.
#[test]
fn an_unknown_home_is_refused_through_the_seat_even_with_no_computed_fields() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
        let types = TypeRegistry::new();

        let mut plain = TypeDefinition::new(
            "gen_plain",
            vec![FieldSchema::new("id", "TEXT").primary_key()],
        );
        plain.home = Some(HomeProfileId::parse("holon-nativ").expect("well-formed, merely wrong"));

        let err = holon_app::type_admission::declare_type_admitted(
            &profiles,
            &plain,
            engine.db_handle(),
            &types,
            &engine.get_dispatcher(),
        )
        .await
        .expect_err("`holon-nativ` names no registered profile")
        .to_string();
        assert!(
            err.contains("holon-nativ") && err.contains("holon-native"),
            "the refusal must name the typo AND the known ids that could have been meant: {err}"
        );
        assert!(
            !types.contains("gen_plain"),
            "a type refused for an unknown home must not be declared"
        );
    });
}

/// The same refusal through the PUBLIC op surface, so the guard cannot be
/// bypassed by an agent driving `execute_operation`.
#[test]
fn an_unknown_home_is_refused_through_the_operation_surface() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let dispatcher = engine.get_dispatcher();

        let mut plain = TypeDefinition::new(
            "gen_plain_op",
            vec![FieldSchema::new("id", "TEXT").primary_key()],
        );
        plain.home = Some(HomeProfileId::parse("holon-nativ").expect("well-formed, merely wrong"));

        let mut params = holon_core::storage::types::StorageEntity::new();
        params.insert(
            "definition".into(),
            holon_api::Value::String(
                serde_json::to_string(&plain).expect("a TypeDefinition serializes"),
            ),
        );

        let err = holon_core::OperationProvider::execute_operation(
            dispatcher.as_ref(),
            &holon_api::EntityName::new(holon_app::type_admission::TYPE_ENTITY),
            holon_app::type_admission::DECLARE_TYPE_OP,
            params,
        )
        .await
        .expect_err("`holon-nativ` names no registered profile")
        .to_string();
        assert!(
            err.contains("holon-nativ"),
            "the refusal must survive the op surface intact: {err}"
        );
    });
}

/// A `computed_persisted` field with no home at all is refused rather than
/// silently defaulted — a default would make the whole check vacuous.
#[test]
fn a_computed_persisted_field_with_no_declared_home_is_refused() {
    let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
    let mut homeless = boolean_computed_type("gen_homeless", "org");
    homeless.home = None;

    let err = holon_capability::check_computed_persisted(
        &profiles,
        &homeless,
        &holon_capability::HomeSeat::Declaration,
    )
    .expect_err("a persisted computed field cannot be checked against a home nobody stated")
    .to_string();
    assert!(
        err.contains("flag") && err.contains("names no home"),
        "the refusal must say WHICH field lacks a home: {err}"
    );
}

/// The rehome seat (`HomeSeat::Destination`) applies the DESTINATION to every
/// field. A field-level home is a declaration-time default and must never
/// exempt a field from the check a move performs.
///
/// Library-level only: no production op moves a `computed_persisted`-bearing
/// entity between homes today (see docs/Plans/BlockGeneralization.md §I3-2).
#[test]
fn a_field_level_home_does_not_exempt_a_field_from_the_destination_check() {
    let profiles = holon_capability::registry::shipped_profiles().expect("shipped profiles");
    let mut type_def = boolean_computed_type("gen_moving", "holon-native");
    for field in &mut type_def.fields {
        if field.name == "flag" {
            field.home = Some(HomeProfileId::parse("holon-native").expect("well-formed"));
        }
    }

    // Declaring it is fine: both the type and the field name a lossless home.
    holon_capability::check_computed_persisted(
        &profiles,
        &type_def,
        &holon_capability::HomeSeat::Declaration,
    )
    .expect("holon-native persists a Boolean computed field");

    let err = holon_capability::check_computed_persisted(
        &profiles,
        &type_def,
        &holon_capability::HomeSeat::Destination(holon_capability::CapabilityProfileId::new("org")),
    )
    .expect_err("the destination governs, so the field's own `holon-native` must not exempt it")
    .to_string();
    assert!(
        err.contains("flag") && err.contains("org"),
        "the refusal must name the field and the DESTINATION: {err}"
    );
}

// ---------------------------------------------------------------------------
// D57 — the seat as a PN action, reachable through the GENERIC op surface.
// ---------------------------------------------------------------------------

/// The op is discoverable and executable by name alone, with no bespoke MCP
/// tool — that reachability is the whole point of registering it.
///
/// @pbt covers declare-type-as-pn-action — `declare_type` is listed by the
/// generic operation surface, refuses a lossy declaration through it, and
/// declares a lossless one
#[test]
fn declare_type_is_reachable_through_the_generic_operation_surface() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let engine = boot_fresh_db(dir.path().join("fresh.db")).await;
        let dispatcher = engine.get_dispatcher();

        let listed = holon_core::OperationProvider::operations(dispatcher.as_ref());
        assert!(
            listed
                .iter()
                .any(|op| op.name == holon_app::type_admission::DECLARE_TYPE_OP
                    && op.entity_name == holon_app::type_admission::TYPE_ENTITY),
            "the generic surface must list `declare_type`; it listed: {:?}",
            listed
                .iter()
                .map(|o| (&o.entity_name, &o.name))
                .collect::<Vec<_>>()
        );

        let call = |type_def: &TypeDefinition| {
            let mut params = holon_core::storage::types::StorageEntity::new();
            params.insert(
                "definition".into(),
                holon_api::Value::String(
                    serde_json::to_string(type_def).expect("a TypeDefinition serializes"),
                ),
            );
            params
        };

        let refused = holon_core::OperationProvider::execute_operation(
            dispatcher.as_ref(),
            &holon_api::EntityName::new(holon_app::type_admission::TYPE_ENTITY),
            holon_app::type_admission::DECLARE_TYPE_OP,
            call(&boolean_computed_type("gen_pn_refused", "org")),
        )
        .await
        .expect_err("a Boolean computed_persisted field has no representation in `org`")
        .to_string();
        assert!(
            refused.contains("flag") && refused.contains("org"),
            "the refusal must survive the op surface intact: {refused}"
        );

        let outcome = holon_core::OperationProvider::execute_operation(
            dispatcher.as_ref(),
            &holon_api::EntityName::new(holon_app::type_admission::TYPE_ENTITY),
            holon_app::type_admission::DECLARE_TYPE_OP,
            call(&boolean_computed_type("gen_pn_ok", "holon-native")),
        )
        .await
        .expect("`holon-native` is full_algebra, so this declaration is admitted");

        // Declaration is one-way, and the op says so rather than leaving undo
        // UNDECLARED for the engine to reject with no reason.
        match outcome.undo {
            holon_core::UndoAction::DeclaredIrreversible(reason) => assert!(
                reason.contains("one-way"),
                "the irreversibility must name its reason: {reason}"
            ),
            other => panic!("declare_type must declare itself irreversible, got {other:?}"),
        }
    });
}
