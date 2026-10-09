//! Every panic leaves an indication: a [`ConditionKind::TaskPanicked`] on the
//! core bus now, and a record in the config dir that a later start discloses
//! as [`ConditionKind::PreviousRunPanicked`]. The record is written on the
//! panicking thread before anything else, so a panic that still aborts (one
//! that crosses `extern "C"`) leaves it too. A panic [`catch_disclosed`]
//! catches leaves no record once the bus has it.
//!
//! A record leaves [`UNSHOWN_DIR`] only once a frontend has drawn the bus that
//! shows it ([`seen_on`]), so a run that dies before that keeps the records of
//! the runs before it. It then moves to [`SEEN_DIR`], which keeps its full
//! message: no file Holon reads a crash from is deleted, only counted once a
//! bound is reached.

use std::cell::RefCell;
use std::collections::BTreeMap;
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
use holon_api::EarlierPanic;
use holon_api::EarlierPanics;
use holon_api::condition_bus::PANIC_CONDITIONS_SUBJECT;
use serde::Deserialize;
use serde::Serialize;

/// The record of this run's last panic, in the config dir.
pub const RECORD_FILE: &str = "last-panic.json";

/// Records of earlier runs that no bus has shown yet, as `<n>.json` in the
/// order the runs ended.
pub const UNSHOWN_DIR: &str = "unshown-panics";

/// How many readable records [`UNSHOWN_DIR`] keeps. A frontend that never
/// draws the bus never marks them seen; the condition lists the newest ten,
/// and the [`DROPPED_FILE`] summary counts the rest. A file that cannot be
/// read is not a record: the bound neither removes nor counts it.
pub const KEPT_UNSHOWN: usize = 10;

/// The one summary of the records dropped from [`UNSHOWN_DIR`] unshown. While
/// it cannot be read, it stays and the count goes to `dropped-panics-2.json`,
/// or the first later number whose file can be read or is missing.
pub const DROPPED_FILE: &str = "dropped-panics.json";

/// Records a frontend has shown, as `<n>.json` in the order the runs ended:
/// they hold the full messages the condition shortens.
pub const SEEN_DIR: &str = "seen-panics";

/// How many records [`SEEN_DIR`] keeps; [`SEEN_DROPPED_FILE`] counts the rest.
pub const KEPT_SEEN: usize = 20;

/// The one summary of the records dropped from [`SEEN_DIR`], with the same
/// overflow names [`DROPPED_FILE`] has.
pub const SEEN_DROPPED_FILE: &str = "seen-dropped-panics.json";

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
        let message = payload_message(info.payload());
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

fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with a non-text payload".to_string())
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

thread_local! {
    /// `Some` while [`catch_disclosed`] runs on this thread; the hook puts the
    /// panic's record in it.
    static CATCHING: RefCell<Option<Option<PanicRecord>>> = const { RefCell::new(None) };
}

/// Run `f`, and return the record of a panic in it instead of unwinding
/// further. The panic shows on the bus as any panic does, but leaves no record
/// for the next start once the bus has it: this run survives it.
///
/// `RenderInterpreter::interpret` is its one caller; a catch
/// anywhere else would hide panics the run cannot recover from.
pub fn catch_disclosed<R>(f: impl FnOnce() -> R) -> Result<R, PanicRecord> {
    hook_once();
    let outer = CATCHING.with(|slot| slot.replace(Some(None)));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    let caught = CATCHING.with(|slot| slot.replace(outer)).flatten();
    result.map_err(|payload| {
        caught.unwrap_or_else(|| PanicRecord {
            message: payload_message(payload.as_ref()),
            location: "an unknown location: a panic hook installed after Holon's replaced it"
                .to_string(),
            thread: current_thread_name(),
        })
    })
}

/// Hand `record` to the [`catch_disclosed`] running on this thread, if one is.
fn caught_here(record: &PanicRecord) -> bool {
    CATCHING
        .try_with(|slot| match slot.try_borrow_mut().as_deref_mut() {
            Ok(Some(caught)) => {
                *caught = Some(record.clone());
                true
            }
            Ok(None) | Err(_) => false,
        })
        .unwrap_or(false)
}

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

/// The dir this process's panics are recorded in, once [`arm`] or [`install`]
/// named one.
pub fn record_dir() -> Option<PathBuf> {
    target().as_ref().map(|t| t.record_dir.clone())
}

fn hook_once() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| match record(info) {
            Report::Full => previous(info),
            Report::Repeat(line) => eprintln!("{line}"),
        }));
    });
}

fn start_run(record_dir: &Path, raised: &mpsc::Sender<Condition>) {
    let prepared = std::fs::create_dir_all(record_dir)
        .map_err(|e| {
            format!("cannot create it, so a crash is not disclosed at the next start: {e}")
        })
        .and_then(|()| keep_unshown(record_dir));
    if let Err(reason) = prepared {
        eprintln!("panic record: {}: {reason}", record_dir.display());
        if raised.send(record_unwritable(record_dir, reason)).is_err() {
            eprintln!("panic record: that reaches no bus either");
        }
    }
}

/// Move the previous run's record into [`UNSHOWN_DIR`], out of the way of
/// this run's panics.
fn keep_unshown(record_dir: &Path) -> Result<(), String> {
    let record = record_dir.join(RECORD_FILE);
    let unshown_dir = record_dir.join(UNSHOWN_DIR);
    let keep = || -> std::io::Result<bool> {
        match std::fs::symlink_metadata(&record) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        std::fs::create_dir_all(&unshown_dir)?;
        let next = list_records(&unshown_dir)?.next;
        std::fs::rename(&record, unshown_dir.join(format!("{next}.json")))?;
        Ok(true)
    };
    let kept = keep()
        .map_err(|e| format!("cannot keep the previous run's record for the next bus: {e}"))?;
    if !kept {
        return Ok(());
    }
    drop_beyond(&unshown_dir, record_dir, DROPPED_FILE, KEPT_UNSHOWN).map_err(|e| {
        format!(
            "cannot bound the records of earlier runs to the newest {KEPT_UNSHOWN}, so all of \
             them stay: {e}"
        )
    })
}

/// The summary of dropped records at `path`, if there is one.
fn read_summary(path: &Path) -> std::io::Result<Option<DroppedPanics>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| std::io::Error::other(format!("{} does not parse: {e}", path.display())))
}

/// Where the summary named `file` in `dir` is read and written: `file`, or,
/// while it cannot be read, the first of `<stem>-2.json`, `<stem>-3.json`, …
/// whose file can be read or is missing.
pub(crate) struct SummarySlot {
    pub(crate) path: PathBuf,
    pub(crate) summary: Option<DroppedPanics>,
    /// The summaries before `path` that cannot be read; they stay where they
    /// are.
    pub(crate) unreadable: Vec<(PathBuf, std::io::Error)>,
}

pub(crate) fn summary_slot(dir: &Path, file: &str) -> std::io::Result<SummarySlot> {
    let stem = file
        .strip_suffix(".json")
        .expect("a summary file is named <stem>.json");
    let mut unreadable = Vec::new();
    for n in 1u64.. {
        let path = match n {
            1 => dir.join(file),
            n => dir.join(format!("{stem}-{n}.json")),
        };
        match read_summary(&path) {
            Ok(summary) => {
                return Ok(SummarySlot {
                    path,
                    summary,
                    unreadable,
                });
            }
            // Only a file that exists is skipped, so the names end with the
            // files.
            Err(e) if std::fs::symlink_metadata(&path).is_ok() => {
                eprintln!(
                    "panic record: {} cannot be read, so it stays and the count goes on: {e}",
                    path.display()
                );
                unreadable.push((path, e));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("a dir holds fewer than u64::MAX summaries")
}

fn write_summary(path: &Path, summary: &DroppedPanics) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(summary).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

/// When the run that left the record at `path` ended: the file's mtime.
pub(crate) fn ended_at(path: &Path) -> std::io::Result<DateTime<Utc>> {
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

/// Fold the readable records of `records_dir` beyond the newest `keep`, by
/// record number, into the summary named `summary_file` in `summary_dir`,
/// written before any of them is removed. A record whose end time cannot be
/// read counts as undated.
fn drop_beyond(
    records_dir: &Path,
    summary_dir: &Path,
    summary_file: &str,
    keep: usize,
) -> std::io::Result<()> {
    let records = list_records(records_dir)?.records;
    let excess = records.len().saturating_sub(keep);
    if excess == 0 {
        return Ok(());
    }
    let slot = summary_slot(summary_dir, summary_file)?;
    let mut dropped = slot.summary;
    for (_, path, _) in &records[..excess] {
        let ended = ended_at(path)
            .inspect_err(|e| {
                eprintln!(
                    "panic record: {} is counted at an unknown time: {e}",
                    path.display()
                );
            })
            .ok(); // ALLOW(ok): the summary counts it as undated, which the condition shows
        let one = DroppedPanics::one(ended);
        dropped = Some(match dropped {
            Some(d) => d.and(one),
            None => one,
        });
    }
    let dropped = dropped.expect("at least one record was folded in");
    write_summary(&slot.path, &dropped)?;
    for (_, path, _) in &records[..excess] {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// What a records dir holds.
pub(crate) struct Records {
    /// The files named `<n>.json` that read as records, oldest first.
    pub(crate) records: Vec<(u64, PathBuf, PanicRecord)>,
    /// Every other entry, with why it is not a record. Holon removes none.
    pub(crate) strays: Vec<(PathBuf, String)>,
    /// Above the `<n>` of every entry named `<n>.json`, read or not, so a new
    /// record replaces none.
    next: u64,
}

const NOT_A_RECORD_FILE: &str = "it is not a record file Holon wrote (a file named <n>.json)";

pub(crate) fn list_records(dir: &Path) -> std::io::Result<Records> {
    let mut listed = Records {
        records: Vec::new(),
        strays: Vec::new(),
        next: 1,
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(listed),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let n = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            // ALLOW(ok): a name that is no record number makes the entry a stray, disclosed as one
            .and_then(|name| name.parse::<u64>().ok())
            .filter(|n| *n < u64::MAX);
        let Some(n) = n else {
            listed.strays.push((path, NOT_A_RECORD_FILE.to_string()));
            continue;
        };
        listed.next = listed.next.max(n + 1);
        if !entry.file_type()?.is_file() {
            listed.strays.push((path, NOT_A_RECORD_FILE.to_string()));
            continue;
        }
        match read_record(&path) {
            Ok(record) => listed.records.push((n, path, record)),
            Err(reason) => listed.strays.push((path, reason)),
        }
    }
    listed.records.sort_unstable_by_key(|(n, ..)| *n);
    listed.strays.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
    Ok(listed)
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
    let files = if config_dir.is_dir() {
        show_unshown(config_dir, &conditions)
    } else {
        ShownFiles::default()
    };
    *shown() = Some(Shown {
        conditions: Arc::downgrade(&conditions),
        record_dir: config_dir.to_path_buf(),
        files,
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
    files: ShownFiles,
}

/// The files one previous-run condition stands for.
#[derive(Default)]
struct ShownFiles {
    /// Oldest first.
    records: Vec<PathBuf>,
    /// The [`DROPPED_FILE`] summary that was read and shown.
    summary: Option<PathBuf>,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

fn shown() -> MutexGuard<'static, Option<Shown>> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Emit the records of earlier runs on `conditions` as one condition: the
/// newest readable record in [`UNSHOWN_DIR`], with the other readable ones
/// and the [`DROPPED_FILE`] summary as its earlier runs. Every entry that
/// cannot be read is disclosed on its own, hides no record, and is not among
/// the files returned.
fn show_unshown(record_dir: &Path, conditions: &ConditionBus) -> ShownFiles {
    let unshown_dir = record_dir.join(UNSHOWN_DIR);
    let (summary_path, dropped) = match summary_slot(record_dir, DROPPED_FILE) {
        Ok(slot) => {
            for (path, e) in &slot.unreadable {
                conditions.emit(record_unreadable(path, e.to_string()));
            }
            (slot.path, slot.summary)
        }
        Err(e) => {
            let path = record_dir.join(DROPPED_FILE);
            conditions.emit(record_unreadable(&path, e.to_string()));
            (path, None)
        }
    };
    let listed = list_records(&unshown_dir).unwrap_or_else(|e| {
        eprintln!(
            "panic record: cannot list the records in {}: {e}",
            unshown_dir.display()
        );
        conditions.emit(record_unwritable(
            &unshown_dir,
            format!("the records of earlier runs cannot be listed: {e}"),
        ));
        Records {
            records: Vec::new(),
            strays: Vec::new(),
            next: 1,
        }
    });
    for (stray, reason) in listed.strays {
        conditions.emit(record_unreadable(&stray, reason));
    }
    let records: Vec<(PathBuf, PanicRecord)> = listed
        .records
        .into_iter()
        .map(|(_, path, record)| (path, record))
        .collect();
    let summary = dropped.is_some().then(|| summary_path.clone());
    match (records.last(), dropped) {
        (Some((_, newest)), dropped) => {
            let panics = records
                .iter()
                .rev()
                .skip(1)
                .map(|(_, r)| EarlierPanic {
                    location: r.location.clone(),
                    message: r.message.clone(),
                })
                .collect();
            conditions.emit(newest.previous_run_panicked(EarlierPanics { panics, dropped }));
        }
        (None, Some(dropped)) => conditions.emit(only_dropped(&dropped, &summary_path)),
        (None, None) => {}
    }
    ShownFiles {
        records: records.into_iter().map(|(path, _)| path).collect(),
        summary,
    }
}

pub(crate) fn read_record(path: &Path) -> Result<PanicRecord, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("it cannot be read: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("it does not parse as a panic record: {e}"))
}

/// The [`DROPPED_FILE`] summary at `path`, when no readable record it
/// summarizes the runs before is left.
fn only_dropped(dropped: &DroppedPanics, path: &Path) -> Condition {
    PanicRecord {
        message: format!(
            "{} earlier panic records were dropped unshown, to keep only the newest \
             {KEPT_UNSHOWN}",
            dropped.count(),
        ),
        location: path.display().to_string(),
        thread: "an unknown thread".to_string(),
    }
    .previous_run_panicked(EarlierPanics {
        panics: Vec::new(),
        dropped: Some(dropped.clone()),
    })
}

/// A frontend has drawn `conditions` where the user sees it: the records of
/// earlier runs that [`install`] showed on it move to [`SEEN_DIR`], and the
/// next start does not show them again. A bus that is not the last one
/// installed showed records the last one shows too, so this does nothing for
/// it.
pub fn seen_on(conditions: &ConditionBus) {
    let mut shown = shown();
    let Some(on_this_bus) = shown.take_if(|s| std::ptr::eq(s.conditions.as_ptr(), conditions))
    else {
        return;
    };
    drop(shown);
    if let Err((subject, e)) = move_to_history(&on_this_bus.record_dir, &on_this_bus.files) {
        eprintln!(
            "panic record: cannot mark {} as seen: {e}",
            subject.display()
        );
        conditions.emit(record_unwritable(
            &subject,
            format!("it cannot be marked as seen, so the next start shows it again: {e}"),
        ));
    }
}

/// Move `files` into the bounded history, oldest first. On an error, the
/// files not yet moved stay where they are; the error names the one it
/// stopped at.
fn move_to_history(record_dir: &Path, files: &ShownFiles) -> Result<(), (PathBuf, std::io::Error)> {
    let seen_dir = record_dir.join(SEEN_DIR);
    std::fs::create_dir_all(&seen_dir).map_err(|e| (seen_dir.clone(), e))?;
    let mut next = list_records(&seen_dir)
        .map_err(|e| (seen_dir.clone(), e))?
        .next;
    for path in &files.records {
        std::fs::rename(path, seen_dir.join(format!("{next}.json")))
            .map_err(|e| (path.clone(), e))?;
        next += 1;
    }
    if let Some(summary) = &files.summary {
        let unshown = read_summary(summary)
            .map_err(|e| (summary.clone(), e))?
            .ok_or_else(|| (summary.clone(), std::io::Error::from(ErrorKind::NotFound)))?;
        let seen = summary_slot(record_dir, SEEN_DROPPED_FILE)
            .map_err(|e| (record_dir.join(SEEN_DROPPED_FILE), e))?;
        let merged = match seen.summary {
            Some(seen) => seen.and(unshown),
            None => unshown,
        };
        write_summary(&seen.path, &merged).map_err(|e| (seen.path.clone(), e))?;
        std::fs::remove_file(summary).map_err(|e| (summary.clone(), e))?;
    }
    drop_beyond(&seen_dir, record_dir, SEEN_DROPPED_FILE, KEPT_SEEN).map_err(|e| (seen_dir, e))
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

/// How the hooks chained before Holon's report a panic.
enum Report {
    Full,
    /// One line in their place.
    Repeat(String),
}

/// A caught panic shown on the bus is reported in full the first time its
/// location and message occur in this process, and as one counting line after.
fn caught_report(record: &PanicRecord) -> Report {
    static OCCURRENCES: Mutex<BTreeMap<(String, String), u64>> = Mutex::new(BTreeMap::new());
    let mut occurrences = OCCURRENCES.lock().unwrap_or_else(PoisonError::into_inner);
    let times = occurrences
        .entry((record.location.clone(), record.message.clone()))
        .or_insert(0);
    *times += 1;
    match *times {
        1 => Report::Full,
        n => Report::Repeat(format!(
            "caught panic at {} again on thread {}, {n} times in this process; reported in full at its first: {}",
            record.location,
            record.thread,
            record.message.lines().next().unwrap_or_default()
        )),
    }
}

/// Runs inside the panic hook, so it must not panic: a panic here aborts the
/// process. Every failure goes to stderr.
fn record(info: &std::panic::PanicHookInfo<'_>) -> Report {
    let record = PanicRecord::from_hook(info);
    let caught = caught_here(&record);
    let mut target = target();
    let Some(target) = target.as_mut() else {
        eprintln!(
            "panic record: no target installed for the panic at {}",
            record.location
        );
        return Report::Full;
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
        Delivery::Forwarded if caught => return caught_report(&record),
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
    Report::Full
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
    fn a_crash_loop_is_one_condition_naming_every_earlier_run() {
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
        let run = |location: &str, message: &str| EarlierPanic {
            location: location.to_string(),
            message: message.to_string(),
        };
        assert_eq!(
            shown,
            vec![(
                "boot.rs:8:1".to_string(),
                "run 4".to_string(),
                EarlierPanics {
                    panics: vec![
                        run("boot.rs:7:1", "run 3"),
                        run("boot.rs:9:1", "run 2"),
                        run("boot.rs:7:1", "run 1"),
                    ],
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

        let unshown =
            list_records(&dir.path().join(UNSHOWN_DIR)).expect("list the unshown records");
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
        let kept: Vec<String> = (dropped + 1..newest)
            .rev()
            .map(|run| format!("prior.rs:{run}:1"))
            .collect();
        assert_eq!(
            earlier
                .panics
                .iter()
                .map(|p| p.location.clone())
                .collect::<Vec<_>>(),
            kept
        );
        assert_eq!(
            earlier.dropped.as_ref().map(DroppedPanics::count),
            Some(dropped),
            "the summary must say how many were dropped: {earlier:#?}"
        );
        assert_eq!(earlier.runs(), newest - 1);

        start(newest + 1);
        let bus = install(dir.path());
        let shown = previous_runs(&bus);
        assert_eq!(
            shown[0].2.dropped.as_ref().map(DroppedPanics::count),
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
        assert!(!dir.path().join(DROPPED_FILE).exists());
        let seen: DroppedPanics = serde_json::from_slice(
            &std::fs::read(dir.path().join(SEEN_DROPPED_FILE)).expect("the history counts it"),
        )
        .expect("parse the history's summary");
        assert_eq!(seen.count(), dropped + 1, "the history keeps the count");
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

    fn seed_record(dir: &Path, n: u64, message: &str, location: &str) -> PathBuf {
        let unshown_dir = dir.join(UNSHOWN_DIR);
        std::fs::create_dir_all(&unshown_dir).expect("create the unshown dir");
        let path = unshown_dir.join(format!("{n}.json"));
        let record = PanicRecord {
            message: message.to_string(),
            location: location.to_string(),
            thread: "main".to_string(),
        };
        std::fs::write(&path, serde_json::to_vec(&record).expect("serialize")).expect("seed");
        path
    }

    /// Every headline and body line the bus's conditions put in front of the
    /// user, with the condition's kind.
    fn disclosed(bus: &ConditionBus) -> Vec<(&'static str, String, String)> {
        bus.current()
            .into_iter()
            .map(|c| {
                let detail = c.reason.detail(&c.subject);
                (
                    c.reason.condition_kind(),
                    c.subject.clone(),
                    std::iter::once(detail.headline)
                        .chain(detail.body)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            })
            .collect()
    }

    fn unreadable_subjects(bus: &ConditionBus) -> Vec<(String, String)> {
        disclosed(bus)
            .into_iter()
            .filter(|(kind, ..)| *kind == ConditionKind::PANIC_RECORD_UNREADABLE)
            .map(|(_, subject, text)| (subject, text))
            .collect()
    }

    fn unwritable(bus: &ConditionBus) -> Vec<String> {
        disclosed(bus)
            .into_iter()
            .filter(|(kind, ..)| *kind == ConditionKind::PANIC_RECORD_UNWRITABLE)
            .map(|(_, _, text)| text)
            .collect()
    }

    /// The one previous-run condition's headline, then every run it carries
    /// as `location: message`.
    fn previous_run_text(bus: &ConditionBus) -> String {
        let texts: Vec<String> = bus
            .current()
            .into_iter()
            .filter_map(|c| match &c.reason {
                ConditionKind::PreviousRunPanicked {
                    message, earlier, ..
                } => Some(
                    [
                        c.reason.detail(&c.subject).headline,
                        format!("{}: {message}", c.subject),
                    ]
                    .into_iter()
                    .chain(
                        earlier
                            .panics
                            .iter()
                            .map(|p| format!("{}: {}", p.location, p.message)),
                    )
                    .collect::<Vec<_>>()
                    .join("\n"),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 1, "one previous-run condition: {texts:#?}");
        texts.into_iter().next().expect("one")
    }

    /// The contents of every record in the history, oldest first.
    fn history(dir: &Path) -> Vec<Vec<u8>> {
        let mut seen: Vec<(u64, Vec<u8>)> = match std::fs::read_dir(dir.join(SEEN_DIR)) {
            Ok(entries) => entries
                .map(|e| {
                    let path = e.expect("a history entry").path();
                    let n: u64 = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_else(|| panic!("{} has no text name", path.display()))
                        .parse()
                        .unwrap_or_else(|e| panic!("{} is not <n>.json: {e}", path.display()));
                    (n, std::fs::read(&path).expect("read a history record"))
                })
                .collect(),
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("list the history: {e}"),
        };
        seen.sort();
        seen.into_iter().map(|(_, bytes)| bytes).collect()
    }

    #[test]
    fn a_corrupt_record_is_disclosed_with_its_error_and_kept() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        let corrupt = dir.path().join(UNSHOWN_DIR).join("2.json");
        std::fs::write(&corrupt, b"{not json").expect("write a corrupt record");
        seed_record(dir.path(), 3, "run 3 broke", "c.rs:3:3");
        let error = serde_json::from_slice::<PanicRecord>(b"{not json")
            .expect_err("it does not parse")
            .to_string();

        let bus = install(dir.path());
        let unreadable = unreadable_subjects(&bus);
        assert!(
            matches!(unreadable.as_slice(), [(subject, text)]
                if *subject == corrupt.display().to_string() && text.contains(&error)),
            "the corrupt record is disclosed once, with its error {error:?}: {unreadable:#?}"
        );
        let shown = previous_run_text(&bus);
        assert!(
            shown.contains("run 3 broke") && shown.contains("run 1 broke"),
            "the readable records are shown with their messages: {shown}"
        );
        assert!(
            !shown.contains("2.json"),
            "the corrupt record is no crash site: {shown}"
        );

        seen_on(&bus);
        assert_eq!(
            std::fs::read(&corrupt).expect("the corrupt record is still there"),
            b"{not json"
        );
    }

    #[test]
    fn an_unreadable_record_is_disclosed_with_its_error_and_kept() {
        use std::os::unix::fs::PermissionsExt;
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let locked = seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        seed_record(dir.path(), 2, "run 2 broke", "b.rs:2:2");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("lock the record");
        let error = std::fs::read(&locked)
            .expect_err("a mode-000 file cannot be read")
            .to_string();

        let bus = install(dir.path());
        let unreadable = unreadable_subjects(&bus);
        seen_on(&bus);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644))
            .expect("unlock the record");
        assert!(
            matches!(unreadable.as_slice(), [(subject, text)]
                if *subject == locked.display().to_string() && text.contains(&error)),
            "the unreadable record is disclosed once, with {error:?}: {unreadable:#?}"
        );
        assert!(locked.exists(), "the unreadable record stays where it is");
    }

    #[test]
    fn a_directory_named_like_a_record_is_a_stray_and_hides_none() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        let directory = dir.path().join(UNSHOWN_DIR).join("2.json");
        std::fs::create_dir(&directory).expect("create a directory named 2.json");

        let bus = install(dir.path());
        assert_eq!(
            previous_runs(&bus)
                .into_iter()
                .map(|(subject, message, _)| (subject, message))
                .collect::<Vec<_>>(),
            vec![("a.rs:1:1".to_string(), "run 1 broke".to_string())],
            "the newest record is the newest FILE"
        );
        assert_eq!(
            unreadable_subjects(&bus)
                .into_iter()
                .map(|(subject, _)| subject)
                .collect::<Vec<_>>(),
            vec![directory.display().to_string()]
        );
        seen_on(&bus);
        assert_eq!(unwritable(&bus), Vec::<String>::new(), "the ack succeeds");
        assert!(directory.is_dir(), "Holon removes nothing it did not write");
        assert!(
            kinds(&install(dir.path()))
                .iter()
                .all(|k| *k != ConditionKind::PREVIOUS_RUN_PANICKED),
            "the record was acknowledged"
        );
    }

    #[test]
    fn an_unreadable_dropped_summary_is_disclosed_and_kept_through_the_ack() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        let summary = dir.path().join(DROPPED_FILE);
        std::fs::write(&summary, b"garbage").expect("write a broken summary");

        let bus = install(dir.path());
        let unreadable = unreadable_subjects(&bus);
        assert_eq!(
            unreadable
                .iter()
                .map(|(subject, _)| subject.clone())
                .collect::<Vec<_>>(),
            vec![summary.display().to_string()]
        );
        seen_on(&bus);
        assert_eq!(
            std::fs::read(&summary).expect("the summary is still there"),
            b"garbage",
            "a summary nobody could read is not removed"
        );
    }

    /// A run that panicked at `prior.rs:<run>:1` ends, and the next one arms.
    fn a_run_crashed(dir: &Path, run: usize) {
        PanicRecord {
            message: format!("run {run}"),
            location: format!("prior.rs:{run}:1"),
            thread: "main".to_string(),
        }
        .write_to(dir)
        .expect("seed a previous record");
        *target() = None;
        arm(dir);
    }

    fn readable_unshown(dir: &Path) -> usize {
        std::fs::read_dir(dir.join(UNSHOWN_DIR))
            .expect("list the unshown dir")
            .filter(|e| read_record(&e.as_ref().expect("an entry").path()).is_ok())
            .count()
    }

    #[test]
    fn a_corrupt_record_is_neither_dropped_nor_counted_by_the_bound() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        std::fs::create_dir_all(dir.path().join(UNSHOWN_DIR)).expect("create the unshown dir");
        let corrupt = dir.path().join(UNSHOWN_DIR).join("1.json");
        std::fs::write(&corrupt, b"{not json").expect("write a corrupt record");
        for run in 1..=KEPT_UNSHOWN + 1 {
            a_run_crashed(dir.path(), run);
        }

        let bus = install(dir.path());
        assert_eq!(
            std::fs::read(&corrupt).map_err(|e| e.to_string()),
            Ok(b"{not json".to_vec()),
            "the bound removes no file it cannot read"
        );
        assert_eq!(readable_unshown(dir.path()), KEPT_UNSHOWN);
        let shown = previous_runs(&bus);
        assert_eq!(
            shown[0].2.dropped.as_ref().map(DroppedPanics::count),
            Some(1),
            "only the one readable record beyond the bound was dropped unshown: {shown:#?}"
        );
        seen_on(&bus);
        assert_eq!(
            std::fs::read(&corrupt).map_err(|e| e.to_string()),
            Ok(b"{not json".to_vec()),
            "the ack moves no file it cannot read"
        );
    }

    #[test]
    fn an_unreadable_dropped_summary_does_not_lift_the_bound() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let summary = dir.path().join(DROPPED_FILE);
        std::fs::write(&summary, b"garbage").expect("write a broken summary");
        let beyond = 3;
        for run in 1..=KEPT_UNSHOWN + beyond {
            a_run_crashed(dir.path(), run);
        }

        let bus = install(dir.path());
        assert_eq!(unwritable(&bus), Vec::<String>::new());
        assert_eq!(readable_unshown(dir.path()), KEPT_UNSHOWN);
        assert!(
            unreadable_subjects(&bus)
                .iter()
                .any(|(subject, _)| *subject == summary.display().to_string()),
            "the broken summary is disclosed: {:#?}",
            disclosed(&bus)
        );
        let shown = previous_runs(&bus);
        assert_eq!(
            shown[0].2.dropped.as_ref().map(DroppedPanics::count),
            Some(beyond),
            "{shown:#?}"
        );

        seen_on(&bus);
        assert_eq!(std::fs::read(&summary).expect("still there"), b"garbage");
        assert!(
            kinds(&install(dir.path()))
                .iter()
                .all(|k| *k != ConditionKind::PREVIOUS_RUN_PANICKED),
            "the records and the count were acknowledged"
        );
        let seen: DroppedPanics = serde_json::from_slice(
            &std::fs::read(dir.path().join(SEEN_DROPPED_FILE)).expect("the history counts them"),
        )
        .expect("parse the history's summary");
        assert_eq!(seen.count(), beyond);
    }

    #[test]
    fn a_stray_directory_is_disclosed_and_kept() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        let stray = dir.path().join(UNSHOWN_DIR).join("subdir");
        std::fs::create_dir(&stray).expect("create a stray directory");

        let bus = install(dir.path());
        assert_eq!(
            unreadable_subjects(&bus)
                .into_iter()
                .map(|(subject, _)| subject)
                .collect::<Vec<_>>(),
            vec![stray.display().to_string()]
        );
        seen_on(&bus);
        assert!(stray.is_dir());
    }

    #[test]
    fn an_acknowledged_record_keeps_its_full_message_in_the_history() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let long = format!("first line of run 2\n{}", "detail ".repeat(200));
        let seeded: Vec<Vec<u8>> = [
            ("run 1 broke", "boot.rs:1:1"),
            (long.as_str(), "boot.rs:2:1"),
            ("run 3 broke", "boot.rs:3:1"),
            ("run 4 broke", "boot.rs:4:1"),
        ]
        .into_iter()
        .zip(1..)
        .map(|((message, location), n)| {
            std::fs::read(seed_record(dir.path(), n, message, location)).expect("read back")
        })
        .collect();

        let bus = install(dir.path());
        let shown = previous_run_text(&bus);
        for line in [
            "run 1 broke",
            "first line of run 2",
            "run 3 broke",
            "run 4 broke",
        ] {
            assert!(shown.contains(line), "{line:?} is shown: {shown}");
        }
        seen_on(&bus);
        assert_eq!(
            history(dir.path()),
            seeded,
            "every record keeps its full bytes in the history"
        );
        assert_eq!(
            list_records(&dir.path().join(UNSHOWN_DIR))
                .expect("list the unshown")
                .records
                .len(),
            0
        );
    }

    #[test]
    fn the_history_keeps_the_newest_and_counts_the_rest() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        let beyond = 3;
        for run in 1..=KEPT_SEEN + beyond {
            PanicRecord {
                message: format!("run {run}"),
                location: format!("prior.rs:{run}:1"),
                thread: "main".to_string(),
            }
            .write_to(dir.path())
            .expect("seed a previous record");
            *target() = None;
            let bus = install(dir.path());
            assert_eq!(previous_runs(&bus).len(), 1);
            seen_on(&bus);
        }
        let kept = history(dir.path());
        assert_eq!(kept.len(), KEPT_SEEN);
        let newest: PanicRecord =
            serde_json::from_slice(kept.last().expect("a kept record")).expect("parse");
        assert_eq!(newest.message, format!("run {}", KEPT_SEEN + beyond));
        let summary: DroppedPanics = serde_json::from_slice(
            &std::fs::read(dir.path().join(SEEN_DROPPED_FILE))
                .expect("the history counts the rest"),
        )
        .expect("parse the summary");
        assert_eq!(summary.count(), beyond);
    }

    #[test]
    fn a_record_with_a_pre_epoch_mtime_is_bounded_like_any_other() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        for n in 1..=KEPT_UNSHOWN as u64 {
            seed_record(
                dir.path(),
                n,
                &format!("run {n}"),
                &format!("prior.rs:{n}:1"),
            );
        }
        std::fs::File::options()
            .write(true)
            .open(dir.path().join(UNSHOWN_DIR).join("1.json"))
            .expect("open the oldest record")
            .set_modified(UNIX_EPOCH - Duration::from_secs(10 * 365 * 24 * 3600))
            .expect("set a pre-epoch mtime");
        seed_previous_run(dir.path());

        let bus = install(dir.path());
        assert_eq!(unwritable(&bus), Vec::<String>::new());
        assert_eq!(
            list_records(&dir.path().join(UNSHOWN_DIR))
                .expect("list the unshown")
                .records
                .len(),
            KEPT_UNSHOWN,
            "the bound applies whatever the mtime"
        );
        let shown = previous_runs(&bus);
        assert_eq!(
            shown[0].2.dropped.as_ref().map(DroppedPanics::count),
            Some(1),
            "{shown:#?}"
        );
    }

    #[test]
    fn one_earlier_run_reads_in_the_singular() {
        let _serial = fresh_process();
        let dir = tempfile::tempdir().expect("temp config dir");
        seed_record(dir.path(), 1, "run 1 broke", "a.rs:1:1");
        seed_record(dir.path(), 2, "run 2 broke", "b.rs:2:2");

        let shown = previous_run_text(&install(dir.path()));
        assert!(!shown.contains(" 1 runs"), "{shown}");
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
