//! Every panic leaves an indication: a [`ConditionKind::TaskPanicked`] on the
//! core bus now, and a record in the config dir that a later start discloses
//! as [`ConditionKind::PreviousRunPanicked`]. The record is written on the
//! panicking thread before anything else, so a panic that still aborts (one
//! that crosses `extern "C"`) leaves it too.
//!
//! A record leaves [`UNSHOWN_DIR`] only after a bus has shown it, so a run
//! that dies before its bus exists keeps the records of the runs before it.

use std::io::ErrorKind;
use std::panic::Location;
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

/// The record of this run's last panic, in the config dir.
pub const RECORD_FILE: &str = "last-panic.json";

/// Records of earlier runs that no bus has shown yet, as `<n>.json` in the
/// order the runs ended.
pub const UNSHOWN_DIR: &str = "unshown-panics";

/// Where a shown record is moved, so it is shown once.
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
        Self {
            message,
            location,
            thread: current_thread_name(),
        }
    }

    /// Write this record as the config dir's [`RECORD_FILE`], replacing an
    /// earlier one: the last panic before a crash is the one that matters.
    pub fn write_to(&self, config_dir: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(config_dir.join(RECORD_FILE), bytes)
    }

    fn not_shown(&self) -> Self {
        Self {
            message: format!("{} ({NOT_SHOWN_NOTE})", self.message),
            ..self.clone()
        }
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

fn current_thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("an unnamed thread")
        .to_string()
}

fn record_unwritable(subject: &Path, reason: String) -> Condition {
    Condition {
        subject: subject.display().to_string(),
        reason: ConditionKind::PanicRecordUnwritable { reason },
    }
}

struct Target {
    record_dir: PathBuf,
    /// The panicking thread may hold the bus's own lock, so it never emits:
    /// its conditions go through this channel. Only [`install`] replaces it,
    /// handing the new receiver to a forwarder thread.
    raised: mpsc::Sender<Condition>,
    delivery: Delivery,
}

enum Delivery {
    /// No bus yet: conditions wait in the channel for [`install`].
    Queued {
        waiting: mpsc::Receiver<Condition>,
        /// The last record written while no bus existed, without the note it
        /// carries on disk, and its dir.
        noted: Option<(PathBuf, PanicRecord)>,
    },
    /// A forwarder thread owns the receiver and emits onto the bus.
    Forwarded,
}

impl Target {
    /// `arm` for `record_dir` already started this run, and no bus has
    /// taken its conditions yet.
    fn armed_before_its_bus(&self, record_dir: &Path) -> bool {
        self.record_dir == record_dir && matches!(self.delivery, Delivery::Queued { .. })
    }
}

static TARGET: Mutex<Option<Target>> = Mutex::new(None);

fn target() -> MutexGuard<'static, Option<Target>> {
    TARGET.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Make every later panic in this process write its record into
/// `record_dir`, creating it if missing, and keep the previous run's record
/// for [`install`] to disclose. An entry point calls this as soon as it knows
/// the dir, before it has a bus.
///
/// The hook is installed once per process and chains the hook installed
/// before it. A call for another dir, or after [`install`], re-targets it and
/// starts a new run there.
pub fn arm(record_dir: &Path) {
    hook_once();
    let mut target = target();
    match target.as_mut() {
        Some(t) if t.armed_before_its_bus(record_dir) => {}
        Some(t) => {
            t.record_dir = record_dir.to_path_buf();
            start_run(record_dir, &t.raised);
        }
        None => {
            let (raised, waiting) = mpsc::channel();
            start_run(record_dir, &raised);
            *target = Some(Target {
                record_dir: record_dir.to_path_buf(),
                raised,
                delivery: Delivery::Queued {
                    waiting,
                    noted: None,
                },
            });
        }
    }
}

fn hook_once() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record(info);
            previous(info);
        }));
    });
}

fn start_run(record_dir: &Path, raised: &mpsc::Sender<Condition>) {
    let prepared = std::fs::create_dir_all(record_dir)
        .map_err(|e| {
            format!("cannot create it, so a crash is not disclosed at the next start: {e}")
        })
        .and_then(|()| {
            keep_unshown(record_dir)
                .map_err(|e| format!("cannot keep the previous run's record for the next bus: {e}"))
        });
    if let Err(reason) = prepared {
        eprintln!("panic record: {}: {reason}", record_dir.display());
        if raised.send(record_unwritable(record_dir, reason)).is_err() {
            eprintln!("panic record: that reaches no bus either");
        }
    }
}

/// Move the previous run's record into [`UNSHOWN_DIR`], out of the way of
/// this run's panics.
fn keep_unshown(record_dir: &Path) -> std::io::Result<()> {
    let record = record_dir.join(RECORD_FILE);
    match std::fs::symlink_metadata(&record) {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    let unshown_dir = record_dir.join(UNSHOWN_DIR);
    std::fs::create_dir_all(&unshown_dir)?;
    let next = unshown_records(record_dir)?
        .last()
        .map_or(1, |(n, _)| n + 1);
    std::fs::rename(&record, unshown_dir.join(format!("{next}.json")))
}

/// The files in [`UNSHOWN_DIR`], oldest first.
fn unshown_records(record_dir: &Path) -> std::io::Result<Vec<(u64, PathBuf)>> {
    let unshown_dir = record_dir.join(UNSHOWN_DIR);
    let entries = match std::fs::read_dir(&unshown_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let not_a_record = || {
            std::io::Error::other(format!(
                "{} is not a record Holon wrote (<n>.json)",
                path.display()
            ))
        };
        let n = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .ok_or_else(not_a_record)?
            .parse::<u64>()
            .map_err(|_| not_a_record())?;
        records.push((n, path));
    }
    records.sort_unstable();
    Ok(records)
}

/// [`arm`] for `config_dir`, then return a new bus that holds every condition
/// raised since and receives every later one, with the records of earlier
/// runs shown on it. The session must run on this bus. The last call in a
/// process is the one whose bus shows panics.
pub fn install(config_dir: &Path) -> Arc<ConditionBus> {
    install_with(config_dir, spawn_forwarder)
}

type SpawnForwarder = fn(mpsc::Receiver<Condition>, Arc<ConditionBus>) -> std::io::Result<()>;

fn install_with(config_dir: &Path, spawn: SpawnForwarder) -> Arc<ConditionBus> {
    hook_once();
    let conditions = Arc::new(ConditionBus::new());
    let (raised, forward) = mpsc::channel();
    let (before, unnoted) = {
        let mut target = target();
        let previous = target.take();
        if !previous
            .as_ref()
            .is_some_and(|t| t.armed_before_its_bus(config_dir))
        {
            start_run(config_dir, &raised);
        }
        let before = previous.map(|t| t.delivery);
        // Under the lock, so no later panic's record is overwritten.
        let unnoted = match &before {
            Some(Delivery::Queued {
                noted: Some((dir, record)),
                ..
            }) => match record.write_to(dir) {
                Ok(()) => None,
                Err(e) => Some((dir.clone(), e)),
            },
            Some(Delivery::Queued { noted: None, .. } | Delivery::Forwarded) | None => None,
        };
        *target = Some(Target {
            record_dir: config_dir.to_path_buf(),
            raised,
            delivery: Delivery::Forwarded,
        });
        (before, unnoted)
    };
    if let Some(Delivery::Queued { waiting, .. }) = before {
        for condition in waiting.try_iter() {
            conditions.emit(condition);
        }
    }
    for condition in forward.try_iter() {
        conditions.emit(condition);
    }
    if let Some((dir, e)) = unnoted {
        eprintln!(
            "panic record: cannot rewrite {} in {} now that it is shown: {e}",
            RECORD_FILE,
            dir.display()
        );
        conditions.emit(record_unwritable(
            &dir,
            format!("its record still says it was not shown: {e}"),
        ));
    }
    // A dir that is missing has already been disclosed by `start_run`.
    if config_dir.is_dir() {
        show_unshown(config_dir, &conditions);
    }
    if let Err(e) = spawn(forward, conditions.clone()) {
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

/// Emit every record in [`UNSHOWN_DIR`] on `conditions`, then mark it seen.
fn show_unshown(record_dir: &Path, conditions: &ConditionBus) {
    let records = match unshown_records(record_dir) {
        Ok(records) => records,
        Err(e) => {
            eprintln!(
                "panic record: cannot list the records in {}: {e}",
                record_dir.join(UNSHOWN_DIR).display()
            );
            conditions.emit(record_unwritable(
                &record_dir.join(UNSHOWN_DIR),
                format!("the records of earlier runs cannot be listed: {e}"),
            ));
            return;
        }
    };
    for (_, path) in records {
        let record = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<PanicRecord>(&bytes)
                .unwrap_or_else(|e| unreadable_record(&path, &e.to_string())),
            Err(e) => unreadable_record(&path, &e.to_string()),
        };
        conditions.emit(record.previous_run_panicked());
        if let Err(e) = std::fs::rename(&path, record_dir.join(SEEN_RECORD_FILE)) {
            eprintln!("panic record: cannot mark {} as seen: {e}", path.display());
            conditions.emit(record_unwritable(
                &path,
                format!("it cannot be marked as seen, so the next start shows it again: {e}"),
            ));
        }
    }
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
    let record = PanicRecord::from_hook(info);
    let mut target = target();
    let Some(target) = target.as_mut() else {
        eprintln!(
            "panic record: no target installed for the panic at {}",
            record.location
        );
        return;
    };
    let sent = target.raised.send(record.task_panicked()).is_ok();
    let written = match &mut target.delivery {
        Delivery::Queued { noted, .. } => {
            *noted = Some((target.record_dir.clone(), record.clone()));
            record.not_shown()
        }
        // Its forwarder never started, or `ConditionBus::emit` panicked in it.
        Delivery::Forwarded if !sent => {
            eprintln!(
                "panic record: the panic at {} reaches no bus; the next start shows it",
                record.location
            );
            record.not_shown()
        }
        Delivery::Forwarded => record,
    };
    if let Err(e) = written.write_to(&target.record_dir) {
        eprintln!(
            "panic record: cannot write {} in {}: {e}",
            RECORD_FILE,
            target.record_dir.display()
        );
        if target
            .raised
            .send(record_unwritable(&target.record_dir, e.to_string()))
            .is_err()
        {
            eprintln!("panic record: that the record is missing reaches no bus either");
        }
    }
}

/// Write this run's record for a death that runs no panic hook, such as
/// `std::process::exit`. A later start discloses it as it does a panic's.
#[track_caller]
pub fn record_exit(message: String) -> std::io::Result<()> {
    let location = Location::caller();
    let record = PanicRecord {
        message,
        location: format!(
            "{}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        thread: current_thread_name(),
    };
    let target = target();
    let target = target
        .as_ref()
        .ok_or_else(|| std::io::Error::other("no record dir: `arm` was not called"))?;
    record.write_to(&target.record_dir)
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

    /// The hook and its target are process-wide: each test starts a process
    /// that has not armed yet.
    fn fresh_process() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        *target() = None;
        serial
    }

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
        let _serial = fresh_process();
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
        let _serial = fresh_process();
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
        let _serial = fresh_process();
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
        let _serial = fresh_process();
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
        assert_eq!(
            recorded(dir.path()).message,
            format!("panic before the bus exists ({NOT_SHOWN_NOTE})"),
            "no bus has shown it yet"
        );
        let bus = install(dir.path());
        assert_eq!(
            recorded(dir.path()).message,
            "panic before the bus exists",
            "install has shown it"
        );

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

    #[test]
    fn a_panic_after_an_arm_that_follows_install_still_reaches_the_bus() {
        let _serial = fresh_process();
        let installed = tempfile::tempdir().expect("temp config dir");
        let armed = tempfile::tempdir().expect("another config dir");
        let bus = install(installed.path());

        arm(armed.path());
        panic_on_a_thread("panic after a later arm");
        assert!(
            eventually_shows_panic(&bus, "panic after a later arm"),
            "the installed bus must still show panics; it holds {:?}",
            kinds(&bus)
        );
        assert_eq!(recorded(armed.path()).message, "panic after a later arm");
    }
}
