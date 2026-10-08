//! The panic hook's file write, observed from outside a process that died of
//! the panic. A PBT cannot do this: its own process must survive the panic.

use std::path::Path;
use std::process::Command;

use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::RECORD_FILE;

const CHILD_ENV: &str = "HOLON_PANIC_RECORD_CHILD_DIR";
const CHILD_MESSAGE: &str = "panic-record child probe";

/// In the child (`CHILD_ENV` set): install on that dir and die of a panic.
/// In the parent: run `test_name` as that child on `config_dir` and return the
/// record it left there.
fn record_of_a_child_that_panics(test_name: &str, config_dir: &Path) -> PanicRecord {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        holon_frontend::panic_record::install(Path::new(&dir));
        panic!("{CHILD_MESSAGE}");
    }

    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, config_dir)
        .output()
        .expect("spawn the child test process");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the child must die of its panic; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(CHILD_MESSAGE),
        "the chained default hook still prints the panic"
    );

    let path = config_dir.join(RECORD_FILE);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "the dead child left no panic record at {}: {e}; its stderr:\n{stderr}",
            path.display()
        )
    });
    serde_json::from_slice(&bytes).expect("the record parses")
}

#[test]
fn a_panic_that_kills_the_process_leaves_its_record() {
    let dir = tempfile::tempdir().expect("temp config dir");
    let record = record_of_a_child_that_panics(
        "a_panic_that_kills_the_process_leaves_its_record",
        dir.path(),
    );
    assert_eq!(record.message, CHILD_MESSAGE);
    let this_file = Path::new(file!())
        .file_name()
        .expect("file! names a file")
        .to_string_lossy()
        .into_owned();
    assert!(
        record.location.contains(&this_file),
        "the record must name where the child panicked (in {this_file}); got {}",
        record.location
    );
}

#[test]
fn a_config_dir_that_does_not_exist_yet_still_gets_the_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let record = record_of_a_child_that_panics(
        "a_config_dir_that_does_not_exist_yet_still_gets_the_record",
        &dir.path().join("not").join("created"),
    );
    assert_eq!(record.message, CHILD_MESSAGE);
}
