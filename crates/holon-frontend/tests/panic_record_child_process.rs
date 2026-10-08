//! The panic hook's file write, observed from outside a process that died of
//! the panic. A PBT cannot do this: its own process must survive the panic.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::RECORD_FILE;

const CHILD_ENV: &str = "HOLON_PANIC_RECORD_CHILD_DIR";
const CHILD_MESSAGE: &str = "panic-record child probe";
const TEST_NAME: &str = "a_panic_that_kills_the_process_leaves_its_record";

#[test]
fn a_panic_that_kills_the_process_leaves_its_record() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        holon_frontend::panic_record::install(
            Path::new(&dir),
            Arc::new(holon_api::ConditionBus::new()),
        );
        panic!("{CHILD_MESSAGE}");
    }

    let dir = tempfile::tempdir().expect("temp config dir");
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir.path())
        .output()
        .expect("spawn the child test process");
    assert!(
        !output.status.success(),
        "the child must die of its panic; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let path = dir.path().join(RECORD_FILE);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "the dead child left no panic record at {}: {e}; its stderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let record: PanicRecord = serde_json::from_slice(&bytes).expect("the record parses");
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
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(CHILD_MESSAGE),
        "the chained default hook still prints the panic"
    );
}
