//! A block deleted or moved in Holon while an external editor saves the same
//! file with an edit to ANOTHER block, before the write-back that carries the
//! Holon change has landed. The saved file then still holds the block where
//! the Loro tree no longer does. The vault must converge: the edit reaches the
//! store, the Holon change stands, and the write-back carries it to disk.
//! When the editor also changed the block Holon changed — its text or only a
//! property — the block's file text it loses is disclosed as a condition
//! naming it.
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

/// What the editor's save changes on Beta, the block Holon changed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum BetaEdit {
    Untouched,
    Content,
    PropertyOnly,
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

/// Every conflict copy in the vault, with its text.
async fn conflict_copies(env: &TestEnvironment) -> Vec<(std::path::PathBuf, String)> {
    let scanned = holon_filesystem::FileSystem::scan_directory(env.org_fs.as_ref(), env.org_root())
        .await
        .expect("scan the vault");
    let mut copies = Vec::new();
    for path in scanned.files {
        if holon_core::conflict_copy::is_conflict_copy(&path) {
            let text = disk(env, &path).await;
            copies.push((path, text));
        }
    }
    copies.sort();
    copies
}

/// The `Debug` text of every current condition.
fn conditions(env: &TestEnvironment) -> Vec<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .map(|c| format!("{c:?}"))
        .collect()
}

async fn disk(env: &TestEnvironment, path: &std::path::Path) -> String {
    holon_filesystem::FileSystem::read_to_string(env.org_fs.as_ref(), path)
        .await
        .expect("read vault.org")
}

#[test]
fn a_delete_racing_an_external_edit_of_the_same_file_converges() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::DeleteBeta,
        BetaEdit::Untouched,
    ));
}

#[test]
fn a_delete_racing_an_external_edit_of_the_deleted_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::DeleteBeta,
        BetaEdit::Content,
    ));
}

#[test]
fn a_delete_racing_a_property_only_edit_of_the_deleted_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::DeleteBeta,
        BetaEdit::PropertyOnly,
    ));
}

#[test]
fn a_move_racing_an_external_edit_of_the_same_file_converges() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::IndentBetaUnderAlpha,
        BetaEdit::Untouched,
    ));
}

#[test]
fn a_move_racing_an_external_edit_of_the_moved_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::IndentBetaUnderAlpha,
        BetaEdit::Content,
    ));
}

#[test]
fn a_move_racing_a_property_only_edit_of_the_moved_block_discloses_the_lost_text() {
    let rt = runtime();
    rt.clone().block_on(holon_change_races_edit(
        rt,
        HolonChange::IndentBetaUnderAlpha,
        BetaEdit::PropertyOnly,
    ));
}

async fn holon_change_races_edit(
    runtime: Arc<tokio::runtime::Runtime>,
    change: HolonChange,
    beta_edit: BetaEdit,
) {
    holon_integration_tests::test_tracing::SpanCollector::global();
    let mut env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
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
    let lost_marker = match beta_edit {
        BetaEdit::Untouched => None,
        BetaEdit::Content => {
            edited = edited.replacen("\n* Beta", "\n* Beta edited outside", 1);
            Some("* Beta edited outside")
        }
        BetaEdit::PropertyOnly => {
            edited = edited.replacen(":ID: d91-beta\n", ":ID: d91-beta\n:OWNER: editor\n", 1);
            Some(":OWNER: editor")
        }
    };
    if let Some(marker) = lost_marker {
        assert!(edited.contains(marker), "the editor must edit Beta");
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
        let disclosure_right = match (lost_marker, &disclosed) {
            (Some(marker), Some(text)) => {
                let text = text.to_lowercase();
                text.contains(&marker.to_lowercase()) && text.contains("d91-beta")
            }
            (None, None) => true,
            _ => false,
        };
        if edit_landed && change_stands && disclosure_right {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the vault did not converge after the race with a {change:?}: Alpha in the store \
             {alpha:?} (want the editor's text), Beta in the store {beta:?}, Beta's overruled \
             file text disclosed {disclosed:?} (want {}), vault.org on disk:\n{on_disk}",
            match lost_marker {
                Some(marker) => format!("Beta's org text holding {marker:?}"),
                None => "none".to_string(),
            }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let copies = conflict_copies(&env).await;
    if lost_marker.is_none() {
        assert!(
            copies.is_empty(),
            "an editor change Holon did not overrule left conflict copies: {copies:?}"
        );
        return;
    }
    let [(copy, copy_text)] = copies.as_slice() else {
        panic!(
            "the overruling write-back must leave exactly one conflict copy of the editor's \
             save, found {copies:?}"
        );
    };
    assert_eq!(
        copy_text, &edited,
        "the conflict copy must hold the editor's save byte-equal"
    );
    let named = conditions(&env);
    assert!(
        named
            .iter()
            .any(|c| c.contains("FileEditOverruled") && c.contains(&copy.display().to_string())),
        "the overruled-edit condition must name the conflict copy {}: {named:?}",
        copy.display()
    );

    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
    env.wait_for_org_files_stable(100, Duration::from_secs(10))
        .await;
    env.stop_app().await.expect("stop_app");
    env.start_app(true).await.expect("restart");
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
    env.wait_for_org_files_stable(100, Duration::from_secs(10))
        .await;
    let after_restart = conflict_copies(&env).await;
    assert_eq!(
        after_restart, copies,
        "the conflict copy must survive a restart unchanged"
    );
    let beta = row(&env, "block:d91-beta").await;
    let stands = match change {
        HolonChange::DeleteBeta => beta.is_none(),
        HolonChange::IndentBetaUnderAlpha => {
            beta.as_ref().map(|(c, p)| (c.as_str(), p.as_str()))
                == Some(("Beta", "block:d91-alpha"))
        }
    };
    assert!(
        stands,
        "after a restart Holon's {change:?} must still stand and the conflict copy must not be \
         ingested: Beta in the store {beta:?}"
    );
    let named = conditions(&env);
    assert!(
        !named.iter().any(|c| c.contains(".conflict-")),
        "after a restart no condition may come from ingesting the conflict copy: {named:?}"
    );
}

const PARENT_VAULT: &str = "* Alpha
:PROPERTIES:
:ID: d92-alpha
:END:
** Child
:PROPERTIES:
:ID: d92-child
:END:
* Gamma
:PROPERTIES:
:ID: d92-gamma
:END:
";

/// Holon deletes a parent and keeps its child, and the editor saves the file
/// that still holds the child, edited, under that parent. The child stays live
/// in Holon, the parent stays deleted, the editor's other edit lands, and the
/// child's lost edit is disclosed as overruled by a move, not by a delete.
#[test]
#[ignore = "D97 write-back state machine: per-block classification needs the bound base; round-4 rule mislabels a kept child as Deleted"]
fn a_parent_delete_keeping_its_child_racing_an_external_edit_keeps_the_child() {
    let rt = runtime();
    rt.clone().block_on(async move {
        holon_integration_tests::test_tracing::SpanCollector::global();
        let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "this race needs the Loro wiring");
        let path = env
            .write_org_file("vault.org", PARENT_VAULT)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while row(&env, "block:d92-gamma").await.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the org scan never projected vault.org into SQL"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        env.wait_for_org_files_stable(100, Duration::from_secs(10))
            .await;
        let doc = row(&env, "block:d92-alpha").await.expect("Alpha row").1;

        env.write_org_file("bulk.org", &bulk_file())
            .await
            .expect("write bulk.org");
        env.execute_operation(
            "block",
            "delete_keep_children",
            HashMap::from([("id".to_string(), Value::String("block:d92-alpha".into()))]),
        )
        .await
        .expect("delete Alpha, keep Child");
        let before = disk(&env, &path).await;
        assert!(
            before.contains("* Alpha\n") && before.contains("** Child\n"),
            "the race was not staged: the write-back carried the delete before the editor \
             saved:\n{before}"
        );
        let edited = before
            .replacen("* Gamma", "* Gamma edited outside", 1)
            .replacen("** Child", "** Child edited outside", 1);
        env.write_org_file("vault.org", &edited)
            .await
            .expect("the editor saves vault.org");

        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let alpha = row(&env, "block:d92-alpha").await;
            let child = row(&env, "block:d92-child").await;
            let gamma = row(&env, "block:d92-gamma").await;
            let on_disk = disk(&env, &path).await;
            let disclosed = env
                .injector()
                .expect("injector")
                .resolve::<Arc<holon_api::ConditionBus>>()
                .current()
                .into_iter()
                .find_map(|c| match c.reason {
                    holon_api::ConditionKind::FileEditOverruled { change, .. }
                        if c.subject == "block:d92-child" =>
                    {
                        Some(change)
                    }
                    _ => None,
                });
            if disclosed == Some(holon_api::HolonChange::Moved)
                && gamma.as_ref().map(|(c, _)| c.as_str()) == Some("Gamma edited outside")
                && on_disk.contains("* Gamma edited outside")
                && alpha.is_none()
                && !on_disk.contains("d92-alpha")
                && child.as_ref().map(|(c, p)| (c.as_str(), p.as_str()))
                    == Some(("Child", doc.as_str()))
                && on_disk.contains("\n* Child\n")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the vault did not converge after a parent delete keeping its child raced an \
                 edit: Alpha {alpha:?} (want none), Child {child:?} (want under {doc}), Gamma \
                 {gamma:?} (want the editor's text), Child's lost edit disclosed as overruled by \
                 {disclosed:?} (want Moved), vault.org on disk:\n{on_disk}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

const DEMOTE_VAULT: &str = "* Alpha
:PROPERTIES:
:ID: d93-alpha
:END:
* Child
:PROPERTIES:
:ID: d93-child
:END:
* Gamma
:PROPERTIES:
:ID: d93-gamma
:END:
";

/// Holon deletes a root block, and the editor saves the file with another
/// root block demoted under it and a third block edited. The demoted block is
/// live in Holon and must not be re-parented under the deleted one: the ingest
/// lands the editor's other edit, the deleted block stays deleted, and the
/// demoted block stays at the root.
#[test]
fn a_demote_under_a_holon_deleted_parent_converges() {
    let rt = runtime();
    rt.clone().block_on(async move {
        holon_integration_tests::test_tracing::SpanCollector::global();
        let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "this race needs the Loro wiring");
        let path = env
            .write_org_file("vault.org", DEMOTE_VAULT)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while row(&env, "block:d93-gamma").await.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the org scan never projected vault.org into SQL"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        env.wait_for_org_files_stable(100, Duration::from_secs(10))
            .await;
        let doc = row(&env, "block:d93-alpha").await.expect("Alpha row").1;

        env.write_org_file("bulk.org", &bulk_file())
            .await
            .expect("write bulk.org");
        env.execute_operation(
            "block",
            "delete",
            HashMap::from([("id".to_string(), Value::String("block:d93-alpha".into()))]),
        )
        .await
        .expect("delete Alpha");
        let before = disk(&env, &path).await;
        assert!(
            before.contains("d93-alpha"),
            "the race was not staged: the write-back carried the delete before the editor \
             saved:\n{before}"
        );
        let edited = before
            .replacen("* Gamma", "* Gamma edited outside", 1)
            .replacen("* Child", "** Child", 1);
        assert!(
            edited.contains("** Child\n"),
            "the editor must demote Child"
        );
        env.write_org_file("vault.org", &edited)
            .await
            .expect("the editor saves vault.org");

        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let alpha = row(&env, "block:d93-alpha").await;
            let child = row(&env, "block:d93-child").await;
            let gamma = row(&env, "block:d93-gamma").await;
            let on_disk = disk(&env, &path).await;
            if gamma.as_ref().map(|(c, _)| c.as_str()) == Some("Gamma edited outside")
                && on_disk.contains("* Gamma edited outside")
                && alpha.is_none()
                && !on_disk.contains("d93-alpha")
                && child.as_ref().map(|(c, p)| (c.as_str(), p.as_str()))
                    == Some(("Child", doc.as_str()))
                && on_disk.contains("\n* Child\n")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the vault did not converge after a demote under a Holon-deleted parent raced \
                 the delete: Alpha {alpha:?} (want none), Child {child:?} (want under {doc}), \
                 Gamma {gamma:?} (want the editor's text), vault.org on disk:\n{on_disk}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

/// Holon deletes Beta and the write-back removes it from the file. The app
/// restarts, so the base is seeded from the store, which cannot hold Beta. An
/// editor then restores the file with Beta still in it and Alpha edited. The
/// edit lands and Beta stays deleted.
#[test]
#[ignore = "D97 write-back state machine Inc 2 (A11): seed-from-store path does not ask ever_seen"]
fn a_restart_then_a_file_still_holding_a_holon_deleted_block_keeps_it_deleted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        holon_integration_tests::test_tracing::SpanCollector::global();
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "this hole needs the Loro wiring");
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
        let with_beta = disk(&env, &path).await;

        env.delete_block("d91-beta").await.expect("delete Beta");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while disk(&env, &path).await.contains("d91-beta") {
            assert!(
                std::time::Instant::now() < deadline,
                "the write-back never removed Beta from vault.org"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_org_files_stable(100, Duration::from_secs(10))
            .await;

        env.stop_app().await.expect("stop_app");
        let restored = with_beta.replacen("* Alpha", "* Alpha edited outside", 1);
        assert_ne!(restored, with_beta, "the editor must touch Alpha");
        env.write_org_file("vault.org", &restored)
            .await
            .expect("the editor restores vault.org");
        env.start_app(true).await.expect("restart");

        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let alpha = row(&env, "block:d91-alpha").await;
            if alpha.as_ref().map(|(c, _)| c.as_str()) == Some("Alpha edited outside") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the editor's edit never reached the store: Alpha {alpha:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        env.wait_for_org_files_stable(100, Duration::from_secs(10))
            .await;
        let beta = row(&env, "block:d91-beta").await;
        let on_disk = disk(&env, &path).await;
        assert!(
            beta.is_none() && !on_disk.contains("d91-beta"),
            "Beta, deleted in Holon, was resurrected by a file that still held it after a \
             restart: Beta in the store {beta:?}, vault.org on disk:\n{on_disk}"
        );
    });
}
