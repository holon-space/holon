//! A block deleted or moved in Holon while an external editor saves the same
//! file with an edit to ANOTHER block, before the write-back that carries the
//! Holon change has landed. The saved file then still holds the block where
//! the Loro tree no longer does. The vault must converge: the edit reaches the
//! store, the Holon change stands, and the write-back carries it to disk.
//! When the editor also changed the block Holon changed, the text it loses is
//! disclosed as a condition naming it.
//!
//! @pbt kind harness
//! @pbt covers delete-races-file-edit — a Holon delete or move racing an
//! external edit of the same file loses neither the edit nor the Holon change

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironment;

const VAULT: &str = "* Alpha
:PROPERTIES:
:ID: d91-alpha
:END:
* Beta
:PROPERTIES:
:ID: d91-beta
:END:
* Gamma
:PROPERTIES:
:ID: d91-gamma
:END:
";

#[derive(Debug, Clone, Copy)]
enum HolonChange {
    DeleteBeta,
    IndentBetaUnderAlpha,
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    )
}

/// A file whose ingest keeps the sync loop busy while the race is staged
/// behind it.
fn bulk_file() -> String {
    (0..200)
        .map(|i| format!("* bulk {i}\n:PROPERTIES:\n:ID: d91-bulk-{i}\n:END:\n"))
        .collect()
}

async fn row(env: &TestEnvironment, id: &str) -> Option<(String, String)> {
    let rows = env
        .query_sql(&format!(
            "SELECT content, parent_id FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .expect("query block_raw");
    rows.first().map(|r| {
        let column = |name: &str| {
            r.get(name)
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| panic!("{name} column of {id}"))
                .to_string()
        };
        (column("content"), column("parent_id"))
    })
}

/// The file text a `FileEditOverruled` condition on `id` discloses.
fn overruled_text(env: &TestEnvironment, id: &str) -> Option<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .find_map(|c| match c.reason {
            holon_api::ConditionKind::FileEditOverruled { file_text, .. } if c.subject == id => {
                Some(file_text)
            }
            _ => None,
        })
}

async fn disk(env: &TestEnvironment, path: &std::path::Path) -> String {
    holon_filesystem::FileSystem::read_to_string(env.org_fs.as_ref(), path)
        .await
        .expect("read vault.org")
}

#[test]
fn a_delete_racing_an_external_edit_of_the_same_file_converges() {
    let rt = runtime();
    rt.clone()
        .block_on(holon_change_races_edit(rt, HolonChange::DeleteBeta, false));
}

#[test]
fn a_delete_racing_an_external_edit_of_the_deleted_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone()
        .block_on(holon_change_races_edit(rt, HolonChange::DeleteBeta, true));
}

#[test]
fn a_move_racing_an_external_edit_of_the_same_file_converges() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::IndentBetaUnderAlpha,
        false,
    ));
}

#[test]
fn a_move_racing_an_external_edit_of_the_moved_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::IndentBetaUnderAlpha,
        true,
    ));
}

async fn holon_change_races_edit(
    runtime: Arc<tokio::runtime::Runtime>,
    change: HolonChange,
    edit_beta: bool,
) {
    holon_integration_tests::test_tracing::SpanCollector::global();
    let env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
    assert!(env.loro_enabled(), "this race needs the Loro wiring");
    let path = env
        .write_org_file("vault.org", VAULT)
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("start_app");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while row(&env, "block:d91-gamma").await.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the org scan never projected vault.org into SQL"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
    env.wait_for_org_files_stable(100, Duration::from_secs(10))
        .await;

    env.write_org_file("bulk.org", &bulk_file())
        .await
        .expect("write bulk.org");
    match change {
        HolonChange::DeleteBeta => env.delete_block("d91-beta").await.expect("delete Beta"),
        HolonChange::IndentBetaUnderAlpha => {
            env.execute_operation(
                "block",
                "move_block",
                HashMap::from([
                    ("id".to_string(), Value::String("block:d91-beta".into())),
                    (
                        "parent_id".to_string(),
                        Value::String("block:d91-alpha".into()),
                    ),
                ]),
            )
            .await
            .expect("move Beta under Alpha");
        }
    }
    let before = disk(&env, &path).await;
    assert!(
        before.contains("\n* Beta"),
        "the race was not staged: the write-back carried the {change:?} before the editor \
         saved:\n{before}"
    );
    let mut edited = before.replacen("* Alpha", "* Alpha edited outside", 1);
    assert_ne!(edited, before, "the editor's change must touch Alpha");
    if edit_beta {
        edited = edited.replacen("\n* Beta", "\n* Beta edited outside", 1);
        assert!(
            edited.contains("* Beta edited outside"),
            "the editor must edit Beta"
        );
    }
    env.write_org_file("vault.org", &edited)
        .await
        .expect("the editor saves vault.org");

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let alpha = row(&env, "block:d91-alpha").await;
        let beta = row(&env, "block:d91-beta").await;
        let on_disk = disk(&env, &path).await;
        let edit_landed = alpha.as_ref().map(|(c, _)| c.as_str()) == Some("Alpha edited outside")
            && on_disk.contains("* Alpha edited outside");
        let change_stands = match change {
            HolonChange::DeleteBeta => beta.is_none() && !on_disk.contains("d91-beta"),
            HolonChange::IndentBetaUnderAlpha => {
                beta.as_ref().map(|(c, p)| (c.as_str(), p.as_str()))
                    == Some(("Beta", "block:d91-alpha"))
                    && on_disk.contains("\n** Beta\n")
            }
        };
        let disclosed = overruled_text(&env, "block:d91-beta");
        let disclosure_right = if edit_beta {
            disclosed.as_deref() == Some("Beta edited outside")
        } else {
            disclosed.is_none()
        };
        if edit_landed && change_stands && disclosure_right {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the vault did not converge after the race with a {change:?}: Alpha in the store \
             {alpha:?} (want the editor's text), Beta in the store {beta:?}, Beta's overruled \
             file text disclosed {disclosed:?} (want {}), vault.org on disk:\n{on_disk}",
            if edit_beta {
                "the editor's Beta text"
            } else {
                "none"
            }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
