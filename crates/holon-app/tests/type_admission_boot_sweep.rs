//! The CV-E admission sweep is ON the real boot path, and it aborts.
//!
//! `type_admission_cve.rs` pins the sweep FUNCTION. This file pins its
//! WIRING — that `holon_app::new_from_config_with_di`, the entry every native
//! frontend and the headless keystone boot through, actually runs it. Without
//! this, `sweep_registry` could be deleted from `session.rs` and every test in
//! the sibling file would stay green.
//!
//! The ordering claim is what the ABORT arm proves: admission fails, and the
//! call returns `Err` INSTEAD of a session. A caller that never receives the
//! session and engine handles has no route to dispatch a write, so no
//! caller-served write can precede the verdict. (Write authorities are already
//! registered by then — `FreeStandingTypeViews` derives them during engine
//! construction — which is why the refusal aborts startup rather than
//! unwinding them: there is no undeclare.)
//!
//! @pbt kind harness
//! @pbt covers cve-boot-sweep-wired — the real boot entry runs admission over
//! the whole type registry
//! @pbt covers cve-boot-sweep-aborts — a type the registry holds whose home
//! cannot carry it aborts startup with a named error instead of returning a
//! session
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone boots only
//! admissible types, so it cannot observe the abort

use std::collections::HashSet;
use std::sync::Arc;

use holon_api::ComputedSpec;
use holon_api::ComputedTier;
use holon_api::FieldLifetime;
use holon_api::FieldSchema;
use holon_api::HomeProfileId;
use holon_api::TypeDefinition;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build test runtime"),
    )
}

/// A type whose `computed_persisted` field produces a Boolean — refused by any
/// `string_only` home, admitted by `holon-native`.
fn boolean_computed_type(name: &str, home: &str) -> TypeDefinition {
    let mut type_def = TypeDefinition::new(
        name,
        vec![
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("a", "TEXT").nullable(),
            FieldSchema::new("b", "TEXT").nullable(),
        ],
    );
    type_def.home = Some(HomeProfileId::parse(home).expect("a well-formed profile id"));
    let declared = type_def.field_types();
    let spec = ComputedSpec::parse(
        "flag",
        "a == b",
        ComputedTier::ComputedPersisted,
        &declared,
        &holon_api::bounded_engine(),
    )
    .expect("`a == b` lowers to SQL");
    type_def.fields.push(FieldSchema {
        name: "flag".to_string(),
        sql_type: "BOOLEAN".to_string(),
        lifetime: FieldLifetime::Computed { spec },
        ..Default::default()
    });
    type_def
}

/// Boot the real shared wiring, optionally seeding one extra type into the
/// registry the way any other seeding door would.
async fn boot(
    dir: &std::path::Path,
    seed: Option<TypeDefinition>,
) -> anyhow::Result<Arc<holon_frontend::FrontendSession>> {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("sweep.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()).without_wait(),
        dir.to_path_buf(),
        HashSet::new(),
        move |injector| {
            if let Some(type_def) = seed {
                // Exactly what a seeding door does: register into the SHARED
                // registry. No admission call of its own — that is the point.
                injector
                    .resolve::<holon_profiles::TypeRegistry>()
                    .register(type_def)?;
            }
            Ok(())
        },
        |_| (),
    )
    .await
    .map(|(session, _engine, ())| session)
}

/// The tree as authored boots. This is the arm that would fail if
/// `person.yaml` lost its `home:` — the sweep runs over the real bundled types.
#[test]
fn the_real_boot_path_admits_the_bundled_registry() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        boot(dir.path(), None)
            .await
            .expect("every bundled type must pass admission on the real boot path");
    });
}

/// A type seeded by any door, whose home cannot carry it, ABORTS the boot —
/// and the caller gets an error instead of a session.
#[test]
fn a_seeded_type_its_home_cannot_carry_aborts_the_boot() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let err = boot(
            dir.path(),
            Some(boolean_computed_type("gen_boot_refused", "org")),
        )
        .await
        .err()
        .expect("a Boolean computed_persisted field cannot live in `org`, so boot must abort");

        let text = format!("{err:#}");
        assert!(
            text.contains("gen_boot_refused") && text.contains("flag"),
            "the startup refusal must name the offending type and field: {text}"
        );
        assert!(
            text.contains("refusing to start"),
            "the refusal must read as a startup abort, not an incidental error: {text}"
        );
    });
}
