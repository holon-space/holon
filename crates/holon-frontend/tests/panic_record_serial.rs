//! `panic_record::install` keeps one record dir and one bus for the whole
//! process, and every test of this binary shares that process.

use std::path::Path;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use holon_frontend::panic_record::PanicRecord;
use holon_frontend::panic_record::RECORD_FILE;

/// Held by every test that installs the record or panics on purpose, so no
/// other test's panic reaches the installed dir or bus.
pub fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The record in `config_dir` if this thread's panic left it: a panic that a
/// test outside [`serial`] raises may still leave its own.
pub fn own_record(config_dir: &Path) -> Option<PanicRecord> {
    let bytes = match std::fs::read(config_dir.join(RECORD_FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("read the panic record in {}: {e}", config_dir.display()),
    };
    let record: PanicRecord = serde_json::from_slice(&bytes).expect("the record parses");
    let thread = std::thread::current();
    (Some(record.thread.as_str()) == thread.name()).then_some(record)
}
