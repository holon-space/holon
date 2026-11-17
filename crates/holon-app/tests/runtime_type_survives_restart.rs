//! A type declared at runtime survives a restart: the next boot declares it
//! again through the admission seat, so its stored rows take writes. A stored
//! definition the seat refuses does not stop the boot or the other types; the
//! boot report names it and why.
//!
//! @pbt kind harness
//! @pbt covers runtime-type-redeclared-at-boot — a declared type takes writes
//! again after a restart, with its rows kept, under the Loro write authority
//! @pbt covers refused-stored-type-disclosed — a stored definition the seat
//! refuses at boot is a failed boot step naming the type and the refusal
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone cannot store a
//! definition the seat refuses, and its typed reboot cases draw SQL authority

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::EntityName;
use holon_api::FieldSchema;
use holon_api::HomeProfileId;
use holon_api::StorageEntity;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_frontend::platform::BootStep;

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    session: Arc<FrontendSession>,
}

async fn boot(dir: &Path) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join("holon.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(
            holon_api::UiInfo::permissive(),
            std::sync::Arc::new(holon_api::SystemClock),
        ),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(holon_conditions::ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring must boot a session");
    Booted {
        engine,
        injector,
        session,
    }
}

fn params(pairs: &[(&str, &str)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), Value::String((*v).into())))
        .collect()
}

async fn dispatch(
    booted: &Booted,
    entity: &str,
    op: &str,
    p: StorageEntity,
) -> holon_core::Result<()> {
    booted
        .engine
        .get_dispatcher()
        .execute_operation(&EntityName::new(entity), op, p)
        .await
        .map(|_| ())
}

async fn authority(booted: &Booted) -> Arc<dyn holon_core::TypeWriteAuthority> {
    booted
        .injector
        .resolve_async::<dyn holon_core::TypeWriteAuthority>()
        .await
}

fn kept_type() -> TypeDefinition {
    TypeDefinition::new(
        "gen_kept",
        [
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("title", "TEXT"),
        ]
        .into_iter()
        .chain(FieldSchema::overflow_pair())
        .collect(),
    )
}

#[test]
fn a_declared_type_takes_writes_after_a_restart_and_a_refused_one_is_disclosed() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("create tempdir");
        let kept = kept_type();
        let mut refused = TypeDefinition::new(
            "gen_refused",
            [FieldSchema::new("id", "TEXT").primary_key()]
                .into_iter()
                .chain(FieldSchema::overflow_pair())
                .collect(),
        );
        refused.home =
            Some(HomeProfileId::parse("holon-nativ").expect("well-formed, merely wrong"));
        {
            let first = boot(dir.path()).await;
            dispatch(
                &first,
                holon_app::type_admission::TYPE_ENTITY,
                holon_app::type_admission::DECLARE_TYPE_OP,
                params(&[(
                    "definition",
                    &serde_json::to_string(&kept).expect("a TypeDefinition serializes"),
                )]),
            )
            .await
            .expect("declare gen_kept");
            dispatch(
                &first,
                "gen_kept",
                "create",
                params(&[("id", "gen-kept:k1"), ("title", "one")]),
            )
            .await
            .expect("create a gen_kept row");
            // Past the seat, as a definition stored by a build whose profiles
            // admitted it would be.
            authority(&first)
                .await
                .store_definition(&refused)
                .await
                .expect("store a definition the seat refuses");
            holon_app::shutdown_session(&first.injector)
                .await
                .expect("shut the first session down");
        }

        let second = boot(dir.path()).await;
        let failed = second.session.boot_report().failed();
        assert_eq!(
            failed.iter().map(|(step, _)| *step).collect::<Vec<_>>(),
            vec![BootStep::RedeclareStoredTypes],
            "only the redeclaration step fails: {failed:?}"
        );
        let why = &failed[0].1;
        assert!(
            why.contains("gen_refused") && why.contains("holon-nativ") && !why.contains("gen_kept"),
            "the failure names the refused type and its refusal, and only it: {why}"
        );

        dispatch(
            &second,
            "gen_kept",
            "set_field",
            params(&[("id", "gen-kept:k1"), ("field", "title"), ("value", "uno")]),
        )
        .await
        .expect("the redeclared type takes writes on a stored row");
        dispatch(
            &second,
            "gen_kept",
            "create",
            params(&[("id", "gen-kept:k2"), ("title", "two")]),
        )
        .await
        .expect("the redeclared type takes new rows");
        let mut titles: Vec<String> = authority(&second)
            .await
            .read_all(&kept)
            .await
            .expect("read gen_kept")
            .iter()
            .map(|row| {
                row.get("title")
                    .and_then(|v| v.as_string())
                    .expect("a title")
                    .to_string()
            })
            .collect();
        titles.sort();
        assert_eq!(titles, vec!["two".to_string(), "uno".to_string()]);
        assert!(
            dispatch(
                &second,
                "gen_refused",
                "create",
                params(&[("id", "gen-refused:r1")])
            )
            .await
            .is_err(),
            "a refused type takes no writes"
        );
        holon_app::shutdown_session(&second.injector)
            .await
            .expect("shut the second session down");
    });
}
