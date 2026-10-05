//! A vault file the user deletes right after its first ingest stays deleted,
//! and its blocks leave the database.

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const PROFILE_ORG: &str = "\
* Late profile
:PROPERTIES:
:ID: late-profile
:END:
#+begin_src holon_entity_profile_yaml
entity_name: person_late
computed:
  display_name: '\"x\"'
#+end_src
";

const DEADLINE: Duration = Duration::from_secs(15);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
}

async fn profile_rows(engine: &holon::api::BackendEngine) -> usize {
    engine
        .db_handle()
        .query(
            "SELECT id FROM block WHERE source_language = 'holon_entity_profile_yaml'",
            HashMap::new(),
        )
        .await
        .expect("query profile blocks")
        .len()
}

/// One boot, one write, one delete. `Err` names what survived the delete.
async fn delete_right_after_ingest() -> Result<(), String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let holon_config = HolonConfig {
        db_path: Some(dir.path().join("delete-after-ingest.db")),
        vault: VaultConfig {
            root: Some(dir.path().to_path_buf()),
        },
        ..Default::default()
    };
    let (_session, engine, ()) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.path().to_path_buf(),
        HashSet::new(),
        |_| Ok(()),
        |_| (),
    )
    .await
    .expect("session boots");
    let file = dir.path().join("profiles.org");
    std::fs::write(&file, PROFILE_ORG).expect("write the vault file");
    let start = Instant::now();
    while profile_rows(&engine).await == 0 {
        assert!(start.elapsed() < DEADLINE, "the file was never ingested");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tracing::info!("[REPRO] ingested after {:?}; deleting", start.elapsed());
    std::fs::remove_file(&file).expect("delete the vault file");

    let start = Instant::now();
    loop {
        let rows = profile_rows(&engine).await;
        let exists = file.exists();
        if rows == 0 && !exists {
            return Ok(());
        }
        if start.elapsed() > DEADLINE {
            let bytes = std::fs::read_to_string(&file).unwrap_or_default();
            return Err(format!(
                "after the delete: {rows} profile row(s), file exists again: {exists}, \
                 bytes: {bytes:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
#[ignore = "red until D70.d: the normalization write-back can still overwrite a delete (M2, bug 2026-10-05-deleted-vault-org-file-is-recreated-by-projection-writer)"]
fn a_vault_file_deleted_right_after_ingest_stays_deleted() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let iterations: usize = std::env::var("REPRO_ITERS")
        .map(|v| v.parse().expect("REPRO_ITERS is a count"))
        .unwrap_or(1);
    let mut failures = Vec::new();
    for i in 0..iterations {
        let outcome = runtime().block_on(delete_right_after_ingest());
        eprintln!("[REPRO] iteration {i}: {outcome:?}");
        if let Err(e) = outcome {
            failures.push(format!("iteration {i}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {iterations} deletes did not stick:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
