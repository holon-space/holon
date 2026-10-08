//! Every panic leaves an indication: a [`ConditionKind::TaskPanicked`] on the
//! core bus now, and a record in the config dir that the next start discloses
//! as [`ConditionKind::PreviousRunPanicked`]. The record is written on the
//! panicking thread before anything else, so a panic that still aborts (one
//! that crosses `extern "C"`) leaves it too.

use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::PoisonError;
use std::sync::mpsc;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use serde::Deserialize;
use serde::Serialize;

/// The record of the last panic, in the config dir.
pub const RECORD_FILE: &str = "last-panic.json";

/// Where a disclosed record is moved, so it is shown once.
pub const SEEN_RECORD_FILE: &str = "last-panic.seen.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanicRecord {
    pub message: String,
    /// `file:line:column` of the panic.
    pub location: String,
    pub thread: String,
}

impl PanicRecord {
    pub fn from_hook(info: &std::panic::PanicHookInfo<'_>) -> Self {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with a non-text payload".to_string());
        let location = info.location().map_or_else(
            || "an unknown location".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        let thread = std::thread::current()
            .name()
            .unwrap_or("an unnamed thread")
            .to_string();
        Self {
            message,
            location,
            thread,
        }
    }

    /// Write this record as the config dir's [`RECORD_FILE`], replacing an
    /// earlier one: the last panic before a crash is the one that matters.
    pub fn write_to(&self, config_dir: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(config_dir.join(RECORD_FILE), bytes)
    }

    fn task_panicked(&self) -> Condition {
        Condition {
            subject: self.location.clone(),
            reason: ConditionKind::TaskPanicked {
                message: self.message.clone(),
                thread: self.thread.clone(),
            },
        }
    }

    fn previous_run_panicked(&self) -> Condition {
        Condition {
            subject: self.location.clone(),
            reason: ConditionKind::PreviousRunPanicked {
                message: self.message.clone(),
                thread: self.thread.clone(),
            },
        }
    }
}

struct Target {
    config_dir: PathBuf,
    /// Drained by a forwarder thread: the panicking thread may hold the
    /// bus's own lock, so it never emits itself.
    raised: mpsc::Sender<Condition>,
}

static TARGET: Mutex<Option<Target>> = Mutex::new(None);

/// Disclose the previous run's panic record on `conditions` (moving it aside,
/// so it is shown once), then make every later panic in this process write
/// its record into `config_dir` and raise `TaskPanicked` on `conditions`.
///
/// The hook is installed once per process and chains the hook installed
/// before it. A later call re-targets it: the last booted app is the one
/// whose window shows the condition.
pub fn install(config_dir: &Path, conditions: Arc<ConditionBus>) {
    disclose_previous(config_dir, &conditions);

    let (raised, forward) = mpsc::channel::<Condition>();
    std::thread::Builder::new()
        .name("panic-conditions".to_string())
        .spawn(move || {
            for condition in forward {
                conditions.emit(condition);
            }
        })
        .unwrap_or_else(|e| panic!("cannot start the panic-condition forwarder thread: {e}"));
    *TARGET.lock().unwrap_or_else(PoisonError::into_inner) = Some(Target {
        config_dir: config_dir.to_path_buf(),
        raised,
    });

    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record(info);
            previous(info);
        }));
    });
}

/// Runs inside the panic hook, so it must not panic: a panic here aborts the
/// process. Every failure goes to stderr.
fn record(info: &std::panic::PanicHookInfo<'_>) {
    let record = PanicRecord::from_hook(info);
    let target = TARGET.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(target) = target.as_ref() else {
        eprintln!(
            "panic record: no target installed for the panic at {}",
            record.location
        );
        return;
    };
    if let Err(e) = record.write_to(&target.config_dir) {
        eprintln!(
            "panic record: cannot write {} in {}: {e}",
            RECORD_FILE,
            target.config_dir.display()
        );
    }
    if target.raised.send(record.task_panicked()).is_err() {
        eprintln!(
            "panic record: the condition forwarder is gone; the panic at {} is not shown",
            record.location
        );
    }
}

fn disclose_previous(config_dir: &Path, conditions: &ConditionBus) {
    let path = config_dir.join(RECORD_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            conditions.emit(unreadable_record(&path, &e.to_string()).previous_run_panicked());
            return;
        }
    };
    let mut record = serde_json::from_slice::<PanicRecord>(&bytes)
        .unwrap_or_else(|e| unreadable_record(&path, &e.to_string()));
    if let Err(e) = std::fs::rename(&path, config_dir.join(SEEN_RECORD_FILE)) {
        record.message = format!(
            "{} (Holon could not move {} aside, so it shows this again at the next start: {e})",
            record.message,
            path.display()
        );
    }
    conditions.emit(record.previous_run_panicked());
}

fn unreadable_record(path: &Path, error: &str) -> PanicRecord {
    PanicRecord {
        message: format!("the panic record could not be read: {error}"),
        location: path.display().to_string(),
        thread: "an unknown thread".to_string(),
    }
}
