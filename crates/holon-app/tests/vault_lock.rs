//! One writer per vault (Model.md invariant 4, at the process boundary).
//!
//! Two sessions on one vault each load `{vault}/.loro` at boot and each save
//! the whole document, so the last saver drops the other's edits (bugfunnel
//! `2026-09-29-two-instances-on-one-vault-overwrite-each-others-loro-snapshot`).
//! The shared boot path therefore takes an exclusive lock on the vault, and a
//! second writer refuses to start by name.
//!
//! @pbt kind harness
//! @pbt covers vault-single-writer — a second session on a held vault refuses
//! to boot and names the holder; the holder's teardown lets the next writer in

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;

const VAULT_ORG: &str = "\
* Vault lock page
:PROPERTIES:
:ID: vault-lock-page
:END:
";

struct Booted {
    injector: fluxdi::Injector,
    _session: Arc<holon_frontend::FrontendSession>,
}

/// Boot the shared wiring over `vault`, with its DB and config in `state` —
/// one state dir per instance, as each build keeps its own DB.
async fn boot(vault: &Path, state: &Path) -> anyhow::Result<Booted> {
    let holon_config = HolonConfig {
        db_path: Some(state.join("holon.db")),
        vault: VaultConfig {
            root: Some(vault.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, _engine, injector) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        state.to_path_buf(),
        HashSet::new(),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            Ok(())
        },
        |injector| injector.clone(),
    )
    .await?;
    Ok(Booted {
        injector,
        _session: session,
    })
}

fn vault_with_page() -> tempfile::TempDir {
    let vault = tempfile::tempdir().expect("create the vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the vault page");
    vault
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_writer_on_a_held_vault_is_refused_by_name() {
    let vault = vault_with_page();
    let state_a = tempfile::tempdir().expect("state dir A");
    let state_b = tempfile::tempdir().expect("state dir B");

    let a = boot(vault.path(), state_a.path())
        .await
        .expect("the first writer boots");

    let refused = boot(vault.path(), state_b.path()).await;
    let err = match refused {
        Ok(b) => {
            holon_app::shutdown_session(&b.injector)
                .await
                .expect("tear down the second writer");
            panic!(
                "a second session booted on a vault that another session holds: both now \
                 write {}/.loro and the org files",
                vault.path().display()
            )
        }
        Err(e) => format!("{e:#}"),
    };
    let pid = std::process::id().to_string();
    assert!(
        err.contains(&vault.path().display().to_string()) && err.contains(&pid),
        "the refusal must name the vault and the holder's pid ({pid}); got: {err}"
    );

    holon_app::shutdown_session(&a.injector)
        .await
        .expect("tear down the first writer");
}

fn listing(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("list the vault") {
        let path = entry.expect("read a vault entry").path();
        if path.is_dir() {
            out.extend(listing(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_writer_creates_nothing_in_the_vault() {
    let vault = vault_with_page();
    let state_a = tempfile::tempdir().expect("state dir A");
    let a = boot(vault.path(), state_a.path())
        .await
        .expect("the first writer boots");
    let before = listing(vault.path());

    // The second writer's DB lives inside the vault, so opening it before the
    // lock would leave a file behind.
    let refused = boot(vault.path(), vault.path()).await;
    let after = listing(vault.path());
    if let Ok(b) = refused {
        holon_app::shutdown_session(&b.injector)
            .await
            .expect("tear down the second writer");
        panic!("a second session booted on a held vault");
    }
    let created: Vec<_> = after.iter().filter(|p| !before.contains(p)).collect();
    assert!(
        created.is_empty(),
        "a refused writer created files in the vault it may not write: {created:?}"
    );

    holon_app::shutdown_session(&a.injector)
        .await
        .expect("tear down the first writer");
}

/// The holder's teardown is what lets the next writer in, while the holder's
/// container is still alive: a lock only the container drop releases would
/// keep every restart path (the keystone `Reboot`, a quit that leaks the
/// container) locked out.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_writer_boots_once_the_holder_has_shut_down() {
    let vault = vault_with_page();
    let state_a = tempfile::tempdir().expect("state dir A");
    let state_b = tempfile::tempdir().expect("state dir B");

    let a = boot(vault.path(), state_a.path())
        .await
        .expect("the first writer boots");
    holon_app::shutdown_session(&a.injector)
        .await
        .expect("tear down the first writer");

    let b = boot(vault.path(), state_b.path())
        .await
        .expect("the next writer boots once the holder has shut down");
    holon_app::shutdown_session(&b.injector)
        .await
        .expect("tear down the next writer");
    drop(a);
}
