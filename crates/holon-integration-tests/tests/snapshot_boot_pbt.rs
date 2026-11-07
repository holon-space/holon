#![cfg(feature = "pbt")]
//! A keystone case booted from its key's template must be indistinguishable
//! from a fresh boot, whatever ran in the process before it.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::snapshot_boot::BootKey;
use holon_integration_tests::pbt::composed::snapshot_boot::BootTemplate;
use holon_integration_tests::pbt::composed::snapshot_boot::why_not_restorable;
use holon_integration_tests::pbt::frontend_slice::components::StoreSnapshot;
use holon_integration_tests::pbt::composed::snapshot_boot::assert_obs_eq;
use holon_integration_tests::pbt::composed::snapshot_boot::observe;
use holon_integration_tests::pbt::composed::snapshot_boot::template_for;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::wide_init_state;
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

fn runner() -> TestRunner {
    TestRunner::new(Config {
        cases: 8,
        failure_persistence: None,
        ..Config::default()
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
    for state in sidecar_states().iter().filter(|s| restorable(s)) {
        restore_matches_fresh(state);
    }
    runner()
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
    runner()
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

    runner()
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

fn flip_loro_byte(snapshot: &StoreSnapshot) {
    let path = files_under(snapshot.dir())
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "holon_tree.loro"))
        .expect("the store holds the Global Loro snapshot");
    let mut bytes = std::fs::read(&path).expect("read the Loro snapshot");
    let ix = bytes.len() / 2;
    bytes[ix] ^= 0xff;
    std::fs::write(&path, bytes).expect("write the Loro snapshot");
}

fn change_org_line(snapshot: &StoreSnapshot) {
    let fs = snapshot.org_fs();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        use holon_filesystem::FileSystem;
        for path in files_under_fs(fs, snapshot.org_root()).await {
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

fn change_block_row(snapshot: &StoreSnapshot) {
    let db = holon_turso::turso::TursoBackend::open_database(snapshot.dir().join("test.db"))
        .expect("open the store's database");
    let conn = db
        .connect(holon_turso::turso::ConnectOpts::new())
        .expect("connect to the store's database");
    conn.execute(
        "UPDATE block SET content = content || ' changed' \
         WHERE id = (SELECT id FROM block WHERE content <> '' ORDER BY id LIMIT 1)",
    )
    .expect("change one block row");
}

/// A template store changed in one Loro byte, one vault line or one block
/// row restores to an `Obs` that differs from the template's, and the check
/// names the field.
#[test]
fn a_changed_template_store_fails_the_obs_check() {
    let state = first_restorable_sidecar_state();
    for (change, field) in [
        (flip_loro_byte as fn(&StoreSnapshot), "loro_tree"),
        (change_org_line, "vault_file"),
        (change_block_row, "block_row"),
    ] {
        let template = BootTemplate::build(&state);
        change(template.snapshot());
        let message = panic_message(|| {
            ComposedSut::<WideE2E>::init_restored(&template, &state);
        });
        assert!(
            message.contains("item(s) differ") && message.contains(&format!("\n{field}[")),
            "a changed {field} store: {message}"
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
