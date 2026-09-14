//! D116.a: the undo history does not survive a restart, and says so.
//!
//! The CRDT's text-undo manager is rebuilt empty at every boot, so a
//! text-epoch marker from a previous session stands for typing nothing can
//! take back. Keeping the journal's operation entries while dropping its
//! typing would replay out of the order the user worked in, so the whole
//! journal goes — and the boot discloses it, because a person who typed before
//! the restart would otherwise press cmd-z and get nothing with no
//! explanation.

use std::collections::HashSet;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_frontend::FrontendSession;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const VAULT_ORG: &str = "\
* Restart probe page
:PROPERTIES:
:ID: restart-probe-page
:END:
** A child block
:PROPERTIES:
:ID: restart-probe-child
:END:
Some text to edit.
";

struct Booted {
    engine: Arc<BackendEngine>,
    injector: fluxdi::Injector,
    _session: Arc<FrontendSession>,
}

/// Boot the shared production wiring over `dir`. Booting twice over the SAME
/// directory is the restart: the replica database, and therefore the persisted
/// journal, is the one the first boot left behind.
async fn boot(dir: &std::path::Path) -> Booted {
    let config = HolonConfig {
        db_path: Some(dir.join("restart.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, injector) = holon_app::new_from_config_with_di(
        config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        |_injector| Ok(()),
        |injector| injector.clone(),
    )
    .await
    .expect("the shared wiring must boot a session");
    Booted {
        engine,
        injector,
        _session: session,
    }
}

async fn journal_one_user_edit(booted: &Booted, value: &str) {
    let mut params: holon_api::StorageEntity = std::collections::HashMap::new();
    params.insert(
        "id".into(),
        Value::String("restart-probe-child".to_string()),
    );
    params.insert("field".into(), Value::String("content".to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    booted
        .engine
        .execute_operation(
            &holon_api::EntityName::from("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
        .expect("a user set_field through the production dispatcher");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_clears_the_journal_and_discloses_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("probe.org"), VAULT_ORG).expect("write the probe vault");

    // First session: leave undo history behind.
    {
        let first = boot(dir.path()).await;
        journal_one_user_edit(&first, "edited before the restart").await;
        assert!(
            first.engine.can_undo().await,
            "the first session must leave something to undo, or the restart proves nothing"
        );
        assert_eq!(
            first.engine.undo_entries_discarded_at_boot(),
            0,
            "a fresh replica discards nothing"
        );
    }

    // Second session over the SAME replica: the restart.
    let second = boot(dir.path()).await;

    assert!(
        second.engine.undo_entries_discarded_at_boot() > 0,
        "the restart found no journal to discard, so either the first session persisted nothing \
         or the clear is not happening where this test can see it"
    );
    assert!(
        !second.engine.can_undo().await,
        "undo history survived a restart; a marker from the previous session stands for typing \
         the rebuilt manager cannot take back"
    );
    assert_eq!(
        second.engine.text_epoch_count().await,
        0,
        "a text-epoch marker survived the restart — the state the loud refusal exists to catch \
         must be unreachable, not merely refused"
    );

    // The disclosure reached the bus.
    let bus = second.injector.resolve::<Arc<holon_api::ConditionBus>>();
    // `subscribe` snapshots the conditions currently in effect, so a condition
    // raised during boot is still observable after it.
    let raised: Vec<String> = bus
        .subscribe()
        .current
        .iter()
        .map(|c| c.condition_key().kind.to_string())
        .collect();
    assert!(
        raised
            .iter()
            .any(|k| k == holon_api::ConditionKind::UNDO_HISTORY_CLEARED_AT_BOOT),
        "the boot cleared the journal without disclosing it; a silent loss is exactly what the \
         error policy forbids. Conditions raised: {raised:?}"
    );
}
