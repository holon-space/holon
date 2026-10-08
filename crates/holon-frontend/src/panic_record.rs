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
use std::sync::MutexGuard;
use std::sync::Once;
use std::sync::PoisonError;
use std::sync::mpsc;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::condition_bus::PANIC_CONDITIONS_SUBJECT;
use serde::Deserialize;
use serde::Serialize;

/// The record of the last panic, in the config dir.
pub const RECORD_FILE: &str = "last-panic.json";

/// Where a disclosed record is moved, so it is shown once.
pub const SEEN_RECORD_FILE: &str = "last-panic.seen.json";

/// Appended to the message of a record whose panic reached no bus.
pub const NOT_SHOWN_NOTE: &str = "not shown while Holon ran: no thread carried it to the app";

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

fn record_unwritable(record_dir: &Path, error: &std::io::Error) -> Condition {
    Condition {
        subject: record_dir.display().to_string(),
        reason: ConditionKind::PanicRecordUnwritable {
            reason: error.to_string(),
        },
    }
}

struct Target {
    record_dir: PathBuf,
    /// The panicking thread may hold the bus's own lock, so it never emits:
    /// its conditions go through this channel to a forwarder thread.
    raised: mpsc::Sender<Condition>,
    /// The channel's other end until [`install`] gives it a forwarder;
    /// conditions raised before then wait in the channel.
    unforwarded: Option<mpsc::Receiver<Condition>>,
}

static TARGET: Mutex<Option<Target>> = Mutex::new(None);

fn target() -> MutexGuard<'static, Option<Target>> {
    TARGET.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Make every later panic in this process write its record into
/// `record_dir`, creating it if missing, and take the previous run's record
/// out of it for [`install`] to disclose. An entry point calls this as soon
/// as it knows the dir, before it has a bus.
///
/// The hook is installed once per process and chains the hook installed
/// before it. A call for another dir, or after [`install`], re-targets it.
pub fn arm(record_dir: &Path) {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record(info);
            previous(info);
        }));
    });
    if target()
        .as_ref()
        .is_some_and(|t| t.record_dir == record_dir && t.unforwarded.is_some())
    {
        return;
    }

    let (raised, unforwarded) = mpsc::channel();
    match std::fs::create_dir_all(record_dir) {
        Ok(()) => take_previous(record_dir, &raised),
        Err(e) => {
            eprintln!(
                "panic record: cannot create {}, so a crash is not disclosed at the next start: {e}",
                record_dir.display()
            );
            raised
                .send(record_unwritable(record_dir, &e))
                .expect("the receiver is held here");
        }
    }
    *target() = Some(Target {
        record_dir: record_dir.to_path_buf(),
        raised,
        unforwarded: Some(unforwarded),
    });
}

/// [`arm`] for `config_dir`, then return a new bus that holds every condition
/// raised since and receives every later one. The session must run on this
/// bus. The last call in a process is the one whose bus shows panics.
pub fn install(config_dir: &Path) -> Arc<ConditionBus> {
    install_with(config_dir, spawn_forwarder)
}

type SpawnForwarder = fn(mpsc::Receiver<Condition>, Arc<ConditionBus>) -> std::io::Result<()>;

fn install_with(config_dir: &Path, spawn: SpawnForwarder) -> Arc<ConditionBus> {
    arm(config_dir);
    let unforwarded = target()
        .as_mut()
        .and_then(|t| t.unforwarded.take())
        .expect("arm leaves the receiver for install");
    let conditions = Arc::new(ConditionBus::new());
    for condition in unforwarded.try_iter() {
        conditions.emit(condition);
    }
    if let Err(e) = spawn(unforwarded, conditions.clone()) {
        eprintln!("panic record: cannot start the thread that shows panics while Holon runs: {e}");
        conditions.emit(Condition {
            subject: PANIC_CONDITIONS_SUBJECT.to_string(),
            reason: ConditionKind::PanicConditionsUnavailable {
                reason: format!("its thread did not start: {e}"),
            },
        });
    }
    conditions
}

fn spawn_forwarder(
    forward: mpsc::Receiver<Condition>,
    conditions: Arc<ConditionBus>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("panic-conditions".to_string())
        .spawn(move || {
            for condition in forward {
                conditions.emit(condition);
            }
        })
        .map(drop)
}

/// Runs inside the panic hook, so it must not panic: a panic here aborts the
/// process. Every failure goes to stderr.
fn record(info: &std::panic::PanicHookInfo<'_>) {
    let mut record = PanicRecord::from_hook(info);
    let target = target();
    let Some(target) = target.as_ref() else {
        eprintln!(
            "panic record: no target installed for the panic at {}",
            record.location
        );
        return;
    };
    // The forwarder ends only when `ConditionBus::emit` panics; a new one
    // would meet the same broken bus, so the record says what was lost.
    if target.raised.send(record.task_panicked()).is_err() {
        eprintln!(
            "panic record: the panic at {} reaches no bus; the next start shows it",
            record.location
        );
        record.message = format!("{} ({NOT_SHOWN_NOTE})", record.message);
    }
    if let Err(e) = record.write_to(&target.record_dir) {
        eprintln!(
            "panic record: cannot write {} in {}: {e}",
            RECORD_FILE,
            target.record_dir.display()
        );
        if target
            .raised
            .send(record_unwritable(&target.record_dir, &e))
            .is_err()
        {
            eprintln!("panic record: that the record is missing reaches no bus either");
        }
    }
}

fn take_previous(record_dir: &Path, raised: &mpsc::Sender<Condition>) {
    let path = record_dir.join(RECORD_FILE);
    let record = match std::fs::read(&path) {
        Ok(bytes) => {
            let mut record = serde_json::from_slice::<PanicRecord>(&bytes)
                .unwrap_or_else(|e| unreadable_record(&path, &e.to_string()));
            if let Err(e) = std::fs::rename(&path, record_dir.join(SEEN_RECORD_FILE)) {
                record.message = format!(
                    "{} (Holon could not move {} aside, so it shows this again at the next start: {e})",
                    record.message,
                    path.display()
                );
            }
            record
        }
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => unreadable_record(&path, &e.to_string()),
    };
    raised
        .send(record.previous_run_panicked())
        .expect("the receiver is held by arm");
}

fn unreadable_record(path: &Path, error: &str) -> PanicRecord {
    PanicRecord {
        message: format!("the panic record could not be read: {error}"),
        location: path.display().to_string(),
        thread: "an unknown thread".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    /// The hook and its target are process-wide.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn panic_on_a_thread(message: &'static str) {
        let joined = std::thread::spawn(move || panic!("{message}")).join();
        assert!(joined.is_err(), "the thread must die of its panic");
    }

    fn recorded(dir: &Path) -> PanicRecord {
        let bytes = std::fs::read(dir.join(RECORD_FILE))
            .unwrap_or_else(|e| panic!("no panic record in {}: {e}", dir.display()));
        serde_json::from_slice(&bytes).expect("the record parses")
    }

    fn kinds(bus: &ConditionBus) -> Vec<&'static str> {
        bus.current()
            .iter()
            .map(|c| c.reason.condition_kind())
            .collect()
    }

    fn eventually_shows_panic(bus: &ConditionBus, message: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if bus.current().iter().any(|c| {
                matches!(&c.reason, ConditionKind::TaskPanicked { message: m, .. } if m == message)
            }) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_forwarder_that_cannot_start_still_leaves_the_record_and_says_so() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp config dir");

        let bus = install_with(dir.path(), |_, _| {
            Err(std::io::Error::other("no thread for you"))
        });
        assert_eq!(
            kinds(&bus),
            vec![ConditionKind::PANIC_CONDITIONS_UNAVAILABLE],
            "install must disclose that panics cannot reach the bus"
        );

        panic_on_a_thread("panic without a forwarder");
        let record = recorded(dir.path());
        assert_eq!(
            record.message,
            format!("panic without a forwarder ({NOT_SHOWN_NOTE})")
        );
    }

    #[test]
    fn a_panic_after_the_forwarder_died_is_marked_as_not_shown() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp config dir");
        let bus = install_with(dir.path(), |forward, _| {
            drop(forward);
            Ok(())
        });

        panic_on_a_thread("panic after the forwarder died");
        assert_eq!(
            recorded(dir.path()).message,
            format!("panic after the forwarder died ({NOT_SHOWN_NOTE})")
        );
        assert!(kinds(&bus).is_empty());
    }

    #[test]
    fn a_dir_that_cannot_be_created_is_disclosed_on_the_bus() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"").expect("write a plain file");

        let bus = install(&file.join("config"));
        assert_eq!(
            kinds(&bus),
            vec![ConditionKind::PANIC_RECORD_UNWRITABLE],
            "a config dir under a plain file cannot be created"
        );
    }

    #[test]
    fn conditions_raised_between_arm_and_install_reach_the_bus() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp config dir");
        PanicRecord {
            message: "the previous run".to_string(),
            location: "prior.rs:1:1".to_string(),
            thread: "main".to_string(),
        }
        .write_to(dir.path())
        .expect("seed a previous record");

        arm(dir.path());
        panic_on_a_thread("panic before the bus exists");
        assert_eq!(recorded(dir.path()).message, "panic before the bus exists");
        let bus = install(dir.path());

        let mut raised = kinds(&bus);
        raised.sort_unstable();
        assert_eq!(
            raised,
            vec![
                ConditionKind::PREVIOUS_RUN_PANICKED,
                ConditionKind::TASK_PANICKED
            ]
        );
        panic_on_a_thread("panic after install");
        assert!(eventually_shows_panic(&bus, "panic after install"));
    }
}
