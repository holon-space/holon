#![cfg(feature = "pbt")]
//! A keystone case booted from its key's template must be indistinguishable
//! from a fresh boot, whatever ran in the process before it.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::snapshot_boot::BootKey;
use holon_integration_tests::pbt::composed::snapshot_boot::BootTemplate;
use holon_integration_tests::pbt::composed::snapshot_boot::assert_obs_eq;
use holon_integration_tests::pbt::composed::snapshot_boot::observe;
use holon_integration_tests::pbt::composed::snapshot_boot::template_for;
use holon_integration_tests::pbt::composed::snapshot_boot::why_not_restorable;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::wide_init_state;
use holon_integration_tests::pbt::frontend_slice::components::StoreSnapshot;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use holon_integration_tests::pbt::hand_authored::load_cases;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::BootFault;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use proptest::prelude::Just;
use proptest::strategy::Strategy;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

fn restorable(state: &ReferenceState) -> bool {
    why_not_restorable(state).is_none()
}

fn restorable_init_state() -> impl Strategy<Value = ReferenceState> {
    wide_init_state(
        Just(IntegrationConnectTiming::Instant).boxed(),
        Just(BootFault::None).boxed(),
    )
    .prop_filter("a session boot", restorable)
}

fn sidecar_initial_state(case: &HandAuthoredCase) -> ReferenceState {
    match &case.initial_state {
        Some(pinned) => pinned
            .clone()
            .into_reference_state(&format!("snapshot boot example {:?}", case.name)),
        None => wide_e2e_ref(),
    }
}

/// One state per distinct key among the hand-authored cases.
fn sidecar_states() -> Vec<ReferenceState> {
    let mut seen = std::collections::HashSet::new();
    load_cases()
        .iter()
        .map(sidecar_initial_state)
        .filter(|state| seen.insert(BootKey::of(state)))
        .collect()
}

/// A lane run keeps each test under half the 120 s nextest cap; a run with
/// `PROPTEST_CASES` set takes every hand-authored key and that many cases.
fn full_run() -> bool {
    std::env::var_os("PROPTEST_CASES").is_some()
}

fn runner(lane_cases: u32) -> TestRunner {
    let config = Config::default();
    TestRunner::new(Config {
        cases: if full_run() { config.cases } else { lane_cases },
        failure_persistence: None,
        ..config
    })
}

fn restore_matches_fresh(state: &ReferenceState) {
    let fresh = ComposedSut::<WideE2E>::init_test(state);
    let fresh_obs = observe(&fresh);
    drop(fresh);
    let template = template_for(state);
    let restored = ComposedSut::<WideE2E>::init_restored(&template, state);
    assert_obs_eq(
        &fresh_obs,
        &observe(&restored),
        &format!("restore of {} vs a fresh boot", BootKey::of(state)),
    );
}

#[test]
fn a_restored_boot_observes_what_a_fresh_boot_observes() {
    let states: Vec<_> = sidecar_states().into_iter().filter(restorable).collect();
    let keys = if full_run() { states.len() } else { 5 };
    for state in &states[..keys.min(states.len())] {
        restore_matches_fresh(state);
    }
    runner(2)
        .run(&restorable_init_state(), |state| {
            restore_matches_fresh(&state);
            Ok(())
        })
        .unwrap();
}

/// Runs `transitions` on a restored SUT, then checks that a second restore of
/// the same key still observes the template.
fn restore_after(init: &ReferenceState, transitions: &[E2ETransition]) {
    let template = template_for(init);
    let mut ref_state = init.clone();
    let mut sut = ComposedSut::<WideE2E>::init_restored(&template, init);
    for transition in transitions {
        ref_state = WideE2EMachine::apply(ref_state, transition);
        sut = ComposedSut::<WideE2E>::apply(sut, &ref_state, transition.clone());
    }
    drop(sut);
    let again = ComposedSut::<WideE2E>::init_restored(&template, init);
    assert_obs_eq(
        template.obs(),
        &observe(&again),
        &format!(
            "restore of {} after {} transition(s)",
            template.key(),
            transitions.len()
        ),
    );
}

fn first_case_with(pick: fn(&E2ETransition) -> bool) -> HandAuthoredCase {
    load_cases()
        .into_iter()
        .find(|case| case.transitions.iter().any(pick))
        .expect("the sidecar holds a case with this transition")
}

#[test]
fn a_restore_does_not_see_the_cases_before_it() {
    for case in [
        first_case_with(|t| matches!(t, E2ETransition::CrashRestart(_))),
        first_case_with(|t| matches!(t, E2ETransition::RebootWhileDatabaseHeld(_))),
    ] {
        restore_after(&sidecar_initial_state(&case), &case.transitions);
    }
    runner(2)
        .run(
            &WideE2EMachine::sequential_strategy(1..6)
                .prop_filter("a session boot", |(init, _, _)| restorable(init)),
            |(init, transitions, _)| {
                restore_after(&init, &transitions);
                Ok(())
            },
        )
        .unwrap();
}

fn panic_message(run: impl FnOnce()) -> String {
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .expect_err("the restore must refuse");
    match panic.downcast::<String>() {
        Ok(message) => *message,
        Err(panic) => panic
            .downcast_ref::<&str>()
            .expect("a panic message")
            .to_string(),
    }
}

fn first_restorable_sidecar_state() -> ReferenceState {
    sidecar_states()
        .into_iter()
        .find(restorable)
        .expect("a hand-authored case boots a restorable session")
}

/// A template restored for a case of another key, an env flag included,
/// fails the key check. A restore always observes its own template, so only
/// property R can catch a key that misses a boot input.
#[test]
fn a_template_of_another_key_is_refused() {
    let state = first_restorable_sidecar_state();
    let template = template_for(&state);
    // SAFETY: nextest runs each test of this target in its own process.
    unsafe { std::env::set_var("HOLON_SOAK_SEED_BLOCKS", "4") };
    let keyed = panic_message(|| {
        ComposedSut::<WideE2E>::init_restored(&template, &state);
    });
    assert!(keyed.contains("template built for"), "{keyed}");
    // SAFETY: as above.
    unsafe { std::env::remove_var("HOLON_SOAK_SEED_BLOCKS") };

    runner(8)
        .run(
            &(restorable_init_state(), restorable_init_state())
                .prop_filter("two keys", |(a, b)| BootKey::of(a) != BootKey::of(b)),
            |(a, b)| {
                let of_a = template_for(&a);
                let keyed = panic_message(|| {
                    ComposedSut::<WideE2E>::init_restored(&of_a, &b);
                });
                assert!(keyed.contains("template built for"), "{keyed}");
                Ok(())
            },
        )
        .unwrap();
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flat_map(|entry| {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

/// A change the restored boot decodes, so the boot succeeds and only the
/// `Obs` check can see it.
fn change_loro_doc(snapshot: &StoreSnapshot) {
    let path = files_under(snapshot.dir())
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "holon_tree.loro"))
        .expect("the store holds the Global Loro snapshot");
    let (doc, _) =
        holon_loro::update_log::read_persisted(&path).expect("read the persisted Loro doc");
    // A new peer would fail the boot's peer check before the `Obs` check runs.
    let peers: Vec<u64> = doc.oplog_vv().keys().copied().collect();
    let [peer] = peers[..] else {
        panic!("the template's Loro doc holds ops of one peer, not {peers:?}");
    };
    doc.set_peer_id(peer).expect("write as the template's peer");
    doc.get_map("changed")
        .insert("key", "value")
        .expect("change the Loro doc");
    doc.commit();
    let bytes = doc
        .export(loro::ExportMode::Snapshot)
        .expect("export the Loro doc");
    std::fs::write(&path, bytes).expect("write the Loro snapshot");
    holon_loro::update_log::reset(&holon_loro::update_log::log_path(&path))
        .expect("reset the Loro update log");
}

fn change_org_line(snapshot: &StoreSnapshot) {
    let fs = snapshot.org_fs();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        use holon_filesystem::FileSystem;
        let vault_files = files_under_fs(fs, snapshot.org_root())
            .await
            .into_iter()
            .filter(|p| !p.components().any(|c| c.as_os_str() == ".holon"));
        for path in vault_files {
            let text = fs.read_to_string(&path).await.expect("read a vault file");
            if let Some(at) = text.find("\n* ") {
                let line_end = text[at + 1..].find('\n').map_or(text.len(), |n| at + 1 + n);
                let changed = format!("{} changed{}", &text[..line_end], &text[line_end..]);
                fs.write(&path, changed.as_bytes())
                    .await
                    .expect("write a vault file");
                return;
            }
        }
        panic!("no vault file holds a headline");
    });
}

async fn files_under_fs(
    fs: &holon_filesystem::InMemoryFileSystem,
    root: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let formats = holon_orgmode::file_sync_controller::org_only_format_registry();
    holon_orgmode::file_watcher::scan_directory(fs, root, &formats)
        .await
        .expect("scan the vault copy")
        .files
}

/// A navigation row, not a block row: the cold boot rebuilds every block row
/// from the Loro doc or the vault, so a changed one never reaches the `Obs`.
fn change_navigation_row(snapshot: &StoreSnapshot) {
    let db = holon_turso::turso::TursoBackend::open_database(snapshot.dir().join("test.db"))
        .expect("open the store's database");
    let conn = db
        .connect(holon_turso::turso::ConnectOpts::new())
        .expect("connect to the store's database");
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) \
         SELECT 'changed', id FROM block_raw ORDER BY id LIMIT 1",
    )
    .expect("add one navigation row");
}

/// A template store changed in its Loro doc, one vault line or one
/// navigation row fails the restore. A changed Loro doc or row fails the `Obs`
/// check, which names the field; a changed vault file is ingested again at the
/// restore, which the ingest check refuses before the `Obs` check runs.
#[test]
fn a_changed_template_store_fails_the_obs_check() {
    let state = first_restorable_sidecar_state();
    for (change, store, expected) in [
        (
            change_loro_doc as fn(&StoreSnapshot),
            "Loro",
            ["item(s) differ", "\nloro_tree["],
        ),
        (change_org_line, "vault", ["the restore ingested", ""]),
        (
            change_navigation_row,
            "navigation",
            ["item(s) differ", "\nnav["],
        ),
    ] {
        let template = BootTemplate::build(&state);
        change(template.snapshot());
        let message = panic_message(|| {
            ComposedSut::<WideE2E>::init_restored(&template, &state);
        });
        assert!(
            expected.iter().all(|part| message.contains(part)),
            "a changed {store} store: {message}"
        );
    }
}

#[test]
fn a_key_with_a_boot_fault_has_no_template() {
    let mut state = first_restorable_sidecar_state();
    state.boot_fault = BootFault::TaskPanic;
    let message = panic_message(|| {
        BootTemplate::build(&state);
    });
    assert!(message.contains("has a boot fault"), "{message}");
}

#[test]
fn a_template_store_that_cannot_be_cloned_is_refused() {
    let state = first_restorable_sidecar_state();
    let template = BootTemplate::build(&state);
    std::fs::remove_dir_all(template.snapshot().dir()).expect("remove the template store");
    let message = panic_message(|| {
        ComposedSut::<WideE2E>::init_restored(&template, &state);
    });
    assert!(message.contains("failed"), "{message}");
}

#[test]
fn a_template_vault_with_a_fault_armed_is_refused() {
    let state = first_restorable_sidecar_state();
    let template = BootTemplate::build(&state);
    template.snapshot().org_fs().fail_next_write_commit();
    let message = panic_message(|| {
        ComposedSut::<WideE2E>::init_restored(&template, &state);
    });
    assert!(message.contains("a fault is armed"), "{message}");
}

/// An edit the disk refuses to write back fails the shutdown, so the template
/// would copy a store whose files miss that edit.
#[test]
fn a_template_whose_shutdown_fails_is_refused() {
    use holon_filesystem::FileSystem;
    let state = first_restorable_sidecar_state();
    let message = panic_message(|| {
        BootTemplate::build_before_shutdown(&state, async |frontend| {
            let path = frontend
                .org_paths()
                .first()
                .expect("the template has an org file")
                .clone();
            let text = frontend
                .org_fs()
                .read_to_string(&path)
                .await
                .expect("read the template's org file");
            let page = text
                .lines()
                .find_map(|line| line.trim().strip_prefix(":ID:"))
                .unwrap_or_else(|| panic!("{} has no :ID: line:\n{text}", path.display()))
                .trim()
                .to_string();
            frontend.org_fs().arm_write_failure(&path);
            frontend
                .create_block("refused-edit", &page, "refused edit")
                .await;
        });
    });
    assert!(
        message.contains("the template's shutdown failed"),
        "{message}"
    );
}

/// The template boot keeps the fresh boot's error check: an error logged
/// while it boots fails the build, so every case of the key goes red.
#[test]
fn a_template_boot_that_logs_an_error_is_refused() {
    let state = first_restorable_sidecar_state();
    let message = panic_message(|| {
        BootTemplate::build_after_boot(&state, || {
            tracing::error!("planted error in the template boot");
        });
    });
    assert!(
        message.contains("[snapshot boot] the template boot of")
            && message.contains("planted error in the template boot"),
        "{message}"
    );
}
