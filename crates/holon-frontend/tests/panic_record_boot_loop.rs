//! Boots that die before the user saw their bus, each in its own process: the
//! evidence of every one of them reaches the next bus.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use holon_api::ConditionBus;
use holon_api::ConditionKind;

const DIR_ENV: &str = "HOLON_BOOT_LOOP_DIR";
const DEATH_ENV: &str = "HOLON_BOOT_LOOP_DEATH";
const PANIC_MESSAGE: &str = "boot loop: the first boot panics";
const BOOT_FAILED_MESSAGE: &str = "boot loop: a later boot fails";

/// In the child (`DIR_ENV` set): arm the record on that dir as a mobile entry
/// point does, then die the way `DEATH_ENV` names before any bus exists.
fn die_before_the_bus() {
    let Ok(dir) = std::env::var(DIR_ENV) else {
        return;
    };
    holon_frontend::panic_record::arm(Path::new(&dir));
    match std::env::var(DEATH_ENV)
        .expect("the parent names the death")
        .as_str()
    {
        "panic" => panic!("{PANIC_MESSAGE}"),
        "exit" => std::process::exit(1),
        "exit-after-install" => {
            holon_frontend::panic_record::install(Path::new(&dir));
            std::process::exit(1)
        }
        "boot-failed" => {
            holon_frontend::panic_record::record_exit(BOOT_FAILED_MESSAGE.to_string())
                .expect("write the failed boot's record");
            std::process::exit(1)
        }
        other => panic!("unknown death {other}"),
    }
}

fn boot_and_die(test_name: &str, config_dir: &Path, death: &str) {
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(DIR_ENV, config_dir)
        .env(DEATH_ENV, death)
        .output()
        .expect("spawn the child boot");
    assert!(
        !output.status.success(),
        "the child boot must die ({death}); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `install` keeps process-wide state, and the tests of this file share a
/// process under `cargo test`.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn previous_run_messages(conditions: &ConditionBus) -> Vec<String> {
    conditions
        .current()
        .into_iter()
        .filter_map(|c| match c.reason {
            ConditionKind::PreviousRunPanicked { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn a_panic_record_survives_a_later_boot_that_exits_before_its_bus() {
    die_before_the_bus();
    let _serial = serial();
    let name = "a_panic_record_survives_a_later_boot_that_exits_before_its_bus";
    let dir = tempfile::tempdir().expect("temp config dir");

    boot_and_die(name, dir.path(), "panic");
    boot_and_die(name, dir.path(), "exit");

    let bus = holon_frontend::panic_record::install(dir.path());
    let shown = previous_run_messages(&bus);
    assert!(
        shown.iter().any(|m| m.starts_with(PANIC_MESSAGE)),
        "the first boot's panic must reach the third boot's bus; shown: {shown:?}"
    );
}

#[test]
fn a_bus_that_no_frontend_drew_leaves_the_records_unshown() {
    die_before_the_bus();
    let _serial = serial();
    let name = "a_bus_that_no_frontend_drew_leaves_the_records_unshown";
    let dir = tempfile::tempdir().expect("temp config dir");

    boot_and_die(name, dir.path(), "panic");
    boot_and_die(name, dir.path(), "exit-after-install");

    let bus = holon_frontend::panic_record::install(dir.path());
    let shown = previous_run_messages(&bus);
    assert!(
        shown.iter().any(|m| m.starts_with(PANIC_MESSAGE)),
        "the second boot's bus was never drawn, so the third boot must show the panic; \
         shown: {shown:?}"
    );
}

#[test]
fn every_boot_that_died_before_its_bus_is_shown_by_the_next_bus() {
    die_before_the_bus();
    let _serial = serial();
    let name = "every_boot_that_died_before_its_bus_is_shown_by_the_next_bus";
    let dir = tempfile::tempdir().expect("temp config dir");

    boot_and_die(name, dir.path(), "panic");
    boot_and_die(name, dir.path(), "exit");
    boot_and_die(name, dir.path(), "boot-failed");

    let bus = holon_frontend::panic_record::install(dir.path());
    let shown = previous_run_messages(&bus);
    assert!(
        shown.iter().any(|m| m.starts_with(PANIC_MESSAGE))
            && shown.iter().any(|m| m == BOOT_FAILED_MESSAGE),
        "the first boot's panic and the failed boot must both be shown; shown: {shown:?}"
    );
    holon_frontend::panic_record::seen_on(&bus);
    assert_eq!(
        previous_run_messages(&holon_frontend::panic_record::install(dir.path())),
        Vec::<String>::new(),
        "a record the user has seen is not shown again"
    );
}
