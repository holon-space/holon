//! Boots that die before they have a bus, each in its own process: the
//! evidence of every one of them reaches the first boot that gets a bus.

use std::path::Path;
use std::process::Command;

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

fn previous_run_messages(config_dir: &Path) -> Vec<String> {
    holon_frontend::panic_record::install(config_dir)
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
    let name = "a_panic_record_survives_a_later_boot_that_exits_before_its_bus";
    let dir = tempfile::tempdir().expect("temp config dir");

    boot_and_die(name, dir.path(), "panic");
    boot_and_die(name, dir.path(), "exit");

    let shown = previous_run_messages(dir.path());
    assert!(
        shown.iter().any(|m| m.starts_with(PANIC_MESSAGE)),
        "the first boot's panic must reach the third boot's bus; shown: {shown:?}"
    );
}

#[test]
fn every_boot_that_died_before_its_bus_is_shown_by_the_next_bus() {
    die_before_the_bus();
    let name = "every_boot_that_died_before_its_bus_is_shown_by_the_next_bus";
    let dir = tempfile::tempdir().expect("temp config dir");

    boot_and_die(name, dir.path(), "panic");
    boot_and_die(name, dir.path(), "exit");
    boot_and_die(name, dir.path(), "boot-failed");

    let shown = previous_run_messages(dir.path());
    assert!(
        shown.iter().any(|m| m.starts_with(PANIC_MESSAGE))
            && shown.iter().any(|m| m == BOOT_FAILED_MESSAGE),
        "the first boot's panic and the failed boot must both be shown; shown: {shown:?}"
    );
    assert_eq!(
        previous_run_messages(dir.path()),
        Vec::<String>::new(),
        "a record is shown once"
    );
}
