//! Leaving the terminal must not take the panic record's hook with it: the
//! session shuts down after stderr is restored.

use holon_frontend::panic_record::RECORD_FILE;
use holon_tui::stderr_to_log::StderrToLog;

const MESSAGE: &str = "panic after the stderr restore";

#[test]
fn a_panic_after_the_stderr_restore_still_leaves_its_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_dir = dir.path().join("config");
    let stderr = StderrToLog::redirect(&dir.path().join("tui.log")).expect("redirect stderr");
    let _bus = holon_frontend::panic_record::install(&config_dir);
    stderr.restore().expect("restore stderr");

    let joined = std::thread::spawn(|| panic!("{MESSAGE}")).join();
    assert!(joined.is_err(), "the probe thread must die of its panic");

    let path = config_dir.join(RECORD_FILE);
    let record = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "no panic record at {} after the restore: {e}",
            path.display()
        )
    });
    assert!(
        record.contains(MESSAGE),
        "the record names another panic: {record}"
    );
}
