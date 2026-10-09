//! Every panic leaves an indication: a [`ConditionKind::TaskPanicked`] on the
//! core bus now, and a record in the config dir that a later start discloses
//! as [`ConditionKind::PreviousRunPanicked`]. The record is written on the
//! panicking thread before anything else, so a panic that still aborts (one
//! that crosses `extern "C"`) leaves it too.
//!
//! A record leaves [`UNSHOWN_DIR`] only once a frontend has drawn the bus that
//! shows it ([`seen_on`]), so a run that dies before that keeps the records of
//! the runs before it.

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
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use chrono::Utc;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::DroppedPanics;
use holon_api::EarlierPanics;
use holon_api::condition_bus::PANIC_CONDITIONS_SUBJECT;
use serde::Deserialize;
use serde::Serialize;

/// The record of this run's last panic, in the config dir.
pub const RECORD_FILE: &str = "last-panic.json";

/// Records of earlier runs that no bus has shown yet, as `<n>.json` in the
/// order the runs ended.
pub const UNSHOWN_DIR: &str = "unshown-panics";

/// How many records [`UNSHOWN_DIR`] keeps. A frontend that never draws the bus
/// never marks them seen; the sites of the newest ten show a crash loop's
/// pattern, and [`DROPPED_FILE`] counts the rest.
pub const KEPT_UNSHOWN: usize = 10;

/// The one summary of the records dropped from [`UNSHOWN_DIR`] unshown.
pub const DROPPED_FILE: &str = "dropped-panics.json";

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

    fn previous_run_panicked(&self, earlier: EarlierPanics) -> Condition {
        Condition {
            subject: self.location.clone(),
            reason: ConditionKind::PreviousRunPanicked {
                message: self.message.clone(),
                thread: self.thread.clone(),
                earlier,
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

fn record_unreadable(subject: &Path, reason: String) -> Condition {
    Condition {
        subject: subject.display().to_string(),
        reason: ConditionKind::PanicRecordUnreadable { reason },
    }
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
        .records
        .last()
        .map_or(1, |(n, _)| n + 1);
    std::fs::rename(&record, unshown_dir.join(format!("{next}.json")))?;
    drop_beyond_kept(record_dir)
}

/// The panics of runs whose records were dropped from [`UNSHOWN_DIR`].
fn read_dropped(record_dir: &Path) -> std::io::Result<Option<DroppedPanics>> {
    let path = record_dir.join(DROPPED_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| std::io::Error::other(format!("{} does not parse: {e}", path.display())))
}

/// When the run that left the record at `path` ended: the file's mtime.
fn ended_at(path: &Path) -> std::io::Result<DateTime<Utc>> {
    let since_epoch = std::fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))?;
    let beyond = || {
        std::io::Error::other(format!(
            "{} has an mtime of {since_epoch:?} after the epoch, beyond what a date can hold",
            path.display()
        ))
    };
    let secs = i64::try_from(since_epoch.as_secs()).map_err(|_| beyond())?;
    DateTime::from_timestamp(secs, since_epoch.subsec_nanos()).ok_or_else(beyond)
}

/// Fold the oldest records beyond [`KEPT_UNSHOWN`] into [`DROPPED_FILE`],
/// written before any of them is removed.
fn drop_beyond_kept(record_dir: &Path) -> std::io::Result<()> {
    let records = unshown_records(record_dir)?.records;
    let excess = records.len().saturating_sub(KEPT_UNSHOWN);
    if excess == 0 {
        return Ok(());
    }
    let mut dropped = read_dropped(record_dir)?;
    for (_, path) in &records[..excess] {
        let ended = ended_at(path)?;
        dropped = Some(match dropped {
            Some(d) => d.and(ended),
            None => DroppedPanics {
                count: 1,
                first_ended: ended,
                last_ended: ended,
            },
        });
    }
    let bytes = serde_json::to_vec_pretty(&dropped).map_err(std::io::Error::other)?;
    std::fs::write(record_dir.join(DROPPED_FILE), bytes)?;
    for (_, path) in &records[..excess] {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// What [`UNSHOWN_DIR`] holds.
struct Unshown {
    /// The records, oldest first.
    records: Vec<(u64, PathBuf)>,
    /// Entries not named `<n>.json`, which Holon did not write.
    strays: Vec<PathBuf>,
}

fn unshown_records(record_dir: &Path) -> std::io::Result<Unshown> {
    let mut unshown = Unshown {
        records: Vec::new(),
        strays: Vec::new(),
    };
    let entries = match std::fs::read_dir(record_dir.join(UNSHOWN_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(unshown),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        let n = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .map(str::parse::<u64>);
        match n {
            Some(Ok(n)) => unshown.records.push((n, path)),
            Some(Err(_)) | None => unshown.strays.push(path),
        }
    }
    unshown.records.sort_unstable();
    unshown.strays.sort_unstable();
    Ok(unshown)
}

/// [`arm`] for `config_dir`, then return a new bus that holds every condition
/// raised since and receives every later one, with the records of earlier
/// runs shown on it; they count as seen once a frontend calls [`seen_on`]. The
/// session must run on this bus. The last call in a process is the one whose
/// bus shows panics.
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
    let records = if config_dir.is_dir() {
        show_unshown(config_dir, &conditions)
    } else {
        Vec::new()
    };
    *shown() = Some(Shown {
        conditions: Arc::downgrade(&conditions),
        record_dir: config_dir.to_path_buf(),
        records,
    });
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

/// The records the last [`install`] showed, until a frontend has drawn them.
struct Shown {
    conditions: std::sync::Weak<ConditionBus>,
    record_dir: PathBuf,
    records: Vec<PathBuf>,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

fn shown() -> MutexGuard<'static, Option<Shown>> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Emit the records of earlier runs on `conditions` as one condition: the
/// newest record in [`UNSHOWN_DIR`], with the others and the [`DROPPED_FILE`]
/// summary as its earlier runs. Return the paths it stands for. An entry that
/// cannot be read is disclosed on its own and hides no record.
fn show_unshown(record_dir: &Path, conditions: &ConditionBus) -> Vec<PathBuf> {
    let dropped_path = record_dir.join(DROPPED_FILE);
    let mut shown = Vec::new();
    let dropped = match read_dropped(record_dir) {
        Ok(None) => None,
        Ok(Some(dropped)) => {
            shown.push(dropped_path.clone());
            Some(dropped)
        }
        Err(e) => {
            conditions.emit(record_unreadable(&dropped_path, e.to_string()));
            shown.push(dropped_path.clone());
            None
        }
    };
    let unshown = match unshown_records(record_dir) {
        Ok(unshown) => unshown,
        Err(e) => {
            eprintln!(
                "panic record: cannot list the records in {}: {e}",
                record_dir.join(UNSHOWN_DIR).display()
            );
            conditions.emit(record_unwritable(
                &record_dir.join(UNSHOWN_DIR),
                format!("the records of earlier runs cannot be listed: {e}"),
            ));
            Unshown {
                records: Vec::new(),
                strays: Vec::new(),
            }
        }
    };
    for stray in &unshown.strays {
        conditions.emit(record_unreadable(
            stray,
            "it is not a record Holon wrote (<n>.json), so it stays where it is".to_string(),
        ));
    }
    let mut records: Vec<PanicRecord> = unshown
        .records
        .iter()
        .map(|(_, path)| read_record(path))
        .collect();
    shown.extend(unshown.records.into_iter().map(|(_, path)| path));
    match (records.pop(), dropped) {
        (Some(newest), dropped) => {
            let mut sites: Vec<(String, usize)> = Vec::new();
            for earlier in records.iter().rev() {
                match sites.iter_mut().find(|(site, _)| *site == earlier.location) {
                    Some((_, runs)) => *runs += 1,
                    None => sites.push((earlier.location.clone(), 1)),
                }
            }
            conditions.emit(newest.previous_run_panicked(EarlierPanics { sites, dropped }));
        }
        (None, Some(dropped)) => conditions.emit(only_dropped(&dropped, &dropped_path)),
        (None, None) => {}
    }
    shown
}

fn read_record(path: &Path) -> PanicRecord {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<PanicRecord>(&bytes)
            .unwrap_or_else(|e| unreadable_record(path, &e.to_string())),
        Err(e) => unreadable_record(path, &e.to_string()),
    }
}

/// The [`DROPPED_FILE`] summary at `path`, when no record it summarizes the
/// runs before is left.
fn only_dropped(dropped: &DroppedPanics, path: &Path) -> Condition {
    let at = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M:%S UTC");
    PanicRecord {
        message: format!(
            "{} earlier panic records were dropped unshown, to keep only the newest {KEPT_UNSHOWN}; those panics happened between {} and {}",
            dropped.count,
            at(dropped.first_ended),
            at(dropped.last_ended)
        ),
        location: path.display().to_string(),
        thread: "an unknown thread".to_string(),
    }
    .previous_run_panicked(EarlierPanics::default())
}

/// A frontend has drawn `conditions` where the user sees it: the records of
/// earlier runs that [`install`] showed on it are seen, and the next start
/// does not show them again. A bus that is not the last one installed showed
/// records the last one shows too, so this does nothing for it.
pub fn seen_on(conditions: &ConditionBus) {
    let mut shown = shown();
    let Some(on_this_bus) = shown.take_if(|s| std::ptr::eq(s.conditions.as_ptr(), conditions))
    else {
        return;
    };
    drop(shown);
    for path in on_this_bus.records {
        if let Err(e) = std::fs::rename(&path, on_this_bus.record_dir.join(SEEN_RECORD_FILE)) {
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
        *shown() = None;
        serial
    }

    fn seed_previous_run(dir: &Path) {
        PanicRecord {
            message: "the previous run".to_string(),
            location: "prior.rs:1:1".to_string(),
            thread: "main".to_string(),
        }
        .write_to(dir)
        .expect("seed a previous record");
    }

    #[test]
    fn a_record_is_seen_only_once_its_own_bus_was_drawn() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_previous_run(dir.path());

        let undrawn = install(dir.path());
        assert_eq!(kinds(&undrawn), vec![ConditionKind::PREVIOUS_RUN_PANICKED]);
        let drawn = install(dir.path());
        assert_eq!(
            kinds(&drawn),
            vec![ConditionKind::PREVIOUS_RUN_PANICKED],
            "no frontend drew the first bus"
        );

        seen_on(&undrawn);
        assert!(
            dir.path().join(UNSHOWN_DIR).join("1.json").exists(),
            "the first bus is not the last one installed"
        );
        seen_on(&drawn);
        assert!(!dir.path().join(UNSHOWN_DIR).join("1.json").exists());
        assert!(kinds(&install(dir.path())).is_empty());
    }

    fn previous_run_messages(bus: &ConditionBus) -> Vec<String> {
        previous_runs(bus)
            .into_iter()
            .map(|(_, message, _)| message)
            .collect()
    }

    /// Subject, message and earlier runs of every previous-run condition.
    fn previous_runs(bus: &ConditionBus) -> Vec<(String, String, EarlierPanics)> {
        bus.current()
            .into_iter()
            .filter_map(|c| match c.reason {
                ConditionKind::PreviousRunPanicked {
                    message, earlier, ..
                } => Some((c.subject, message, earlier)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_crash_loop_is_one_condition_naming_every_earlier_site() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let unshown_dir = dir.path().join(UNSHOWN_DIR);
        std::fs::create_dir_all(&unshown_dir).expect("create the unshown dir");
        for (n, line) in [(1, 7), (2, 9), (3, 7), (4, 8)] {
            let record = PanicRecord {
                message: format!("run {n}"),
                location: format!("boot.rs:{line}:1"),
                thread: "main".to_string(),
            };
            std::fs::write(
                unshown_dir.join(format!("{n}.json")),
                serde_json::to_vec(&record).expect("serialize a record"),
            )
            .expect("seed a record");
        }

        let shown = previous_runs(&install(dir.path()));
        let site = |s: &str, runs| (s.to_string(), runs);
        assert_eq!(
            shown,
            vec![(
                "boot.rs:8:1".to_string(),
                "run 4".to_string(),
                EarlierPanics {
                    sites: vec![site("boot.rs:7:1", 2), site("boot.rs:9:1", 1)],
                    dropped: None,
                }
            )]
        );
    }

    #[test]
    fn unshown_records_keep_the_newest_and_count_the_dropped_ones() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let dropped = 3;
        let start = |run: usize| {
            PanicRecord {
                message: format!("run {run}"),
                location: format!("prior.rs:{run}:1"),
                thread: "main".to_string(),
            }
            .write_to(dir.path())
            .expect("seed a previous record");
            *target() = None;
            arm(dir.path());
        };
        for run in 1..=KEPT_UNSHOWN + dropped {
            start(run);
        }

        let unshown = unshown_records(dir.path()).expect("list the unshown records");
        assert_eq!(unshown.records.len(), KEPT_UNSHOWN);
        let shown = previous_runs(&install(dir.path()));
        assert_eq!(shown.len(), 1, "one condition for every run: {shown:#?}");
        let (subject, message, earlier) = &shown[0];
        let newest = KEPT_UNSHOWN + dropped;
        assert_eq!(
            (subject.as_str(), message.as_str()),
            (
                format!("prior.rs:{newest}:1").as_str(),
                format!("run {newest}").as_str()
            )
        );
        let kept: Vec<(String, usize)> = (dropped + 1..newest)
            .rev()
            .map(|run| (format!("prior.rs:{run}:1"), 1))
            .collect();
        assert_eq!(earlier.sites, kept);
        assert_eq!(
            earlier.dropped.as_ref().map(|d| d.count),
            Some(dropped),
            "the summary must say how many were dropped: {earlier:#?}"
        );
        assert_eq!(earlier.runs(), newest - 1);

        start(newest + 1);
        let bus = install(dir.path());
        let shown = previous_runs(&bus);
        assert_eq!(
            shown[0].2.dropped.as_ref().map(|d| d.count),
            Some(dropped + 1),
            "a later drop adds to the one summary: {shown:#?}"
        );
        assert_eq!(shown.len(), 1, "{shown:#?}");

        seen_on(&bus);
        assert_eq!(
            previous_run_messages(&install(dir.path())),
            Vec::<String>::new(),
            "the summary is seen with the records it summarizes"
        );
    }

    #[test]
    fn a_stray_file_among_the_records_is_disclosed_and_hides_none_of_them() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let stray = dir.path().join(UNSHOWN_DIR).join(".DS_Store");
        std::fs::create_dir_all(dir.path().join(UNSHOWN_DIR)).expect("create the unshown dir");
        std::fs::write(&stray, b"Finder").expect("write a stray file");
        seed_previous_run(dir.path());

        let bus = install(dir.path());
        assert_eq!(
            previous_run_messages(&bus),
            vec!["the previous run".to_string()],
            "the stray file must not withhold the record beside it"
        );
        let disclosed: Vec<(String, &'static str)> = bus
            .current()
            .into_iter()
            .filter(|c| c.reason.condition_kind() != ConditionKind::PREVIOUS_RUN_PANICKED)
            .map(|c| (c.subject, c.reason.condition_kind()))
            .collect();
        assert_eq!(
            disclosed,
            vec![(
                stray.display().to_string(),
                ConditionKind::PANIC_RECORD_UNREADABLE
            )],
            "the stray file is named once, as what it is"
        );
        seen_on(&bus);
        assert!(stray.exists(), "Holon removes no file it did not write");
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
